//! WASM Sandbox Runtime Engine.

pub mod engine;
pub mod guest_abi;
pub mod host_api;

use anyhow::{Context, Result};
use log::{error, info, warn};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use wasmtime::{Linker, Module, Store};

const INITIAL_FUEL: u64 = 1_000_000_000;
const SHUTDOWN_FUEL: u64 = 500_000;
const STORE_MEMORY_LIMIT: usize = 64 << 20;
const EPOCH_TICK: Duration = Duration::from_millis(100);
const EPOCH_DEADLINE_TICKS: u64 = 5;

struct EpochTicker {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl EpochTicker {
    fn start(engine: wasmtime::Engine) -> Self {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(std::sync::atomic::Ordering::Acquire) {
                std::thread::sleep(EPOCH_TICK);
                engine.increment_epoch();
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn set_guest_budget(store: &mut Store<ModuleHostState>, fuel: u64) -> Result<()> {
    store.set_fuel(fuel)?;
    store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);
    Ok(())
}

use crate::modules::manifest::ModuleManifest;
use crate::modules::{CoreMessage, ModuleMessage};
pub use engine::WasmEngine;
use guest_abi::GuestExports;
use host_api::{register_host_functions, ModuleHostState, SocketEvent};

pub struct WasmModuleRunner {
    name: String,
    manifest: ModuleManifest,
    wasm_bytes: Vec<u8>,
    wasm_path: Option<PathBuf>,
    emitted_down: Option<Arc<Mutex<std::collections::HashSet<String>>>>,
}

impl WasmModuleRunner {
    pub fn from_file<P: AsRef<Path>>(manifest: ModuleManifest, path: P) -> Result<Self> {
        let p = path.as_ref().to_path_buf();
        if !p.exists() {
            anyhow::bail!("WASM module binary not found at {:?}", p);
        }
        Ok(Self {
            name: manifest.name.clone(),
            manifest,
            wasm_bytes: Vec::new(),
            wasm_path: Some(p),
            emitted_down: None,
        })
    }

    pub fn from_bytes(manifest: ModuleManifest, bytes: Vec<u8>) -> Self {
        Self {
            name: manifest.name.clone(),
            manifest,
            wasm_bytes: bytes,
            wasm_path: None,
            emitted_down: None,
        }
    }

    pub fn with_emitted_down(
        mut self,
        emitted_down: Arc<Mutex<std::collections::HashSet<String>>>,
    ) -> Self {
        self.emitted_down = Some(emitted_down);
        self
    }

    pub async fn run(
        mut self,
        engine: WasmEngine,
        config: serde_json::Value,
        mut command_rx: mpsc::Receiver<CoreMessage>,
        updates_tx: mpsc::Sender<(String, ModuleMessage)>,
    ) -> Result<()> {
        let name = self.name.clone();
        info!("Initializing WASM module '{}' in secure sandbox", name);

        if self.wasm_bytes.is_empty() {
            if let Some(wasm_path) = &self.wasm_path {
                self.wasm_bytes = std::fs::read(wasm_path).with_context(|| {
                    format!("failed to read WASM module binary at {:?}", wasm_path)
                })?;
            }
        }
        let module = Module::new(engine.engine(), &self.wasm_bytes)
            .map_err(|e| anyhow::anyhow!("failed to compile WASM module '{}': {e}", name))?;
        self.wasm_bytes.clear();
        self.wasm_bytes.shrink_to_fit();

        let mut linker = Linker::new(engine.engine());
        register_host_functions(&mut linker)?;

        let (dbus_signal_tx, mut dbus_signal_rx) =
            mpsc::channel::<(String, String, String, String)>(64);
        let (socket_event_tx, mut socket_event_rx) = mpsc::channel::<SocketEvent>(64);
        let (timer_event_tx, mut timer_event_rx) = mpsc::channel::<u32>(64);
        let (path_event_tx, mut path_event_rx) = mpsc::channel::<u32>(64);

        let host_state = ModuleHostState {
            name: name.clone(),
            manifest: self.manifest.clone(),
            updates_tx: updates_tx.clone(),
            dbus_signal_tx,
            socket_event_tx,
            timer_event_tx,
            path_event_tx,
            subscribed_signals: std::collections::HashSet::new(),
            last_updates: std::collections::HashMap::new(),
            active_timers: std::collections::HashSet::new(),
            active_watches: std::collections::HashSet::new(),
            active_sockets: std::collections::HashMap::new(),
            limits: wasmtime::StoreLimitsBuilder::new()
                .memory_size(STORE_MEMORY_LIMIT)
                .instances(1)
                .tables(1)
                .build(),
        };

        let mut store = Store::new(engine.engine(), host_state);
        store.limiter(|state| &mut state.limits);
        // Per-module ticker; a shared ticker on WasmEngine is a 0.1.1 optimization.
        let _epoch_ticker = EpochTicker::start(engine.engine().clone());
        set_guest_budget(&mut store, INITIAL_FUEL)?;

        info!(
            "Loading WASM module '{}' with ABI version 0.1.0 (no fs:write)",
            name
        );
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| anyhow::anyhow!("failed to instantiate WASM module '{}': {e}", name))?;

        let guest = GuestExports::extract(&mut store, &instance)?;

        // Call init if present
        if let Some(init_fn) = &guest.init {
            let mut init_config = config.clone();
            if let Some(obj) = init_config.as_object_mut() {
                if !obj.contains_key("home") {
                    if let Ok(home) = std::env::var("HOME") {
                        obj.insert("home".to_string(), serde_json::Value::String(home));
                    }
                }
                if !obj.contains_key("xdg_data_home") {
                    if let Ok(xdg_data_home) = std::env::var("XDG_DATA_HOME") {
                        obj.insert(
                            "xdg_data_home".to_string(),
                            serde_json::Value::String(xdg_data_home),
                        );
                    }
                }
                if !obj.contains_key("xdg_data_dirs") {
                    if let Ok(xdg_data_dirs) = std::env::var("XDG_DATA_DIRS") {
                        obj.insert(
                            "xdg_data_dirs".to_string(),
                            serde_json::Value::String(xdg_data_dirs),
                        );
                    }
                }
                if let Some(ref reconnect) = self.manifest.reconnect {
                    if !obj.contains_key("reconnect") {
                        if let Ok(rec_val) = serde_json::to_value(reconnect) {
                            obj.insert("reconnect".to_string(), rec_val);
                        }
                    }
                }
            }
            let config_json =
                serde_json::to_string(&init_config).unwrap_or_else(|_| "{}".to_string());
            let (ptr, len) = guest.write_bytes(&mut store, config_json.as_bytes())?;
            set_guest_budget(&mut store, INITIAL_FUEL)?;
            let res = init_fn.call(&mut store, (ptr, len));
            let _ = guest.free_bytes(&mut store, ptr, len);

            match res {
                Ok(0) => {
                    info!("WASM module '{}' initialized successfully", name);
                    let _ = updates_tx.send((name.clone(), ModuleMessage::Ready)).await;
                }
                Ok(code) => {
                    warn!("WASM module '{}' init returned error code {}", name, code);
                }
                Err(e) => {
                    error!("WASM module '{}' init trapped: {}", name, e);
                    return Err(e.into());
                }
            }
        } else {
            let _ = updates_tx.send((name.clone(), ModuleMessage::Ready)).await;
        }

        let tick_ms_opt = config
            .get("interval_ms")
            .and_then(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_f64().map(|number| number as u64))
            })
            .filter(|milliseconds| *milliseconds > 0)
            .or_else(|| {
                config
                    .get("interval")
                    .and_then(|value| {
                        value
                            .as_u64()
                            .or_else(|| value.as_f64().map(|number| number as u64))
                    })
                    .map(|seconds| seconds.max(1).saturating_mul(1000))
            })
            .or(self.manifest.interval_ms);
        let has_tick = guest.on_tick.is_some() && tick_ms_opt.is_some();
        let tick_ms = tick_ms_opt.unwrap_or(1000).max(50);
        let align_to_minute = config
            .get("align_to_minute")
            .and_then(|v| v.as_bool())
            .or(self.manifest.align_to_minute)
            .unwrap_or(false);

        let tick_ms = tick_ms.max(50);
        let mut tick_interval = if align_to_minute {
            let now_epoch_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let rem_ms = (60_000 - (now_epoch_ms % 60_000)).max(100);
            let start = tokio::time::Instant::now() + Duration::from_millis(rem_ms);
            tokio::time::interval_at(start, Duration::from_millis(tick_ms.max(60_000)))
        } else {
            tokio::time::interval(Duration::from_millis(tick_ms))
        };
        tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut trap_reason = None;
        loop {
            tokio::select! {
                biased;

                msg = command_rx.recv() => {
                    match msg {
                        Some(CoreMessage::Event { widget_id, event }) => {
                            if let Some(on_event_fn) = &guest.on_event {
                                let (w_ptr, w_len) = guest.write_bytes(&mut store, widget_id.as_bytes())?;
                                let (e_ptr, e_len) = guest.write_bytes(&mut store, event.as_bytes())?;
                                set_guest_budget(&mut store, INITIAL_FUEL)?;
                                let res = on_event_fn.call(&mut store, (w_ptr, w_len, e_ptr, e_len));
                                let _ = guest.free_bytes(&mut store, w_ptr, w_len);
                                let _ = guest.free_bytes(&mut store, e_ptr, e_len);
                                if let Err(e) = res {
                                    error!("WASM module '{}' on_event trapped: {}", name, e);
                                    trap_reason = Some(e.into());
                                    break;
                                }
                            }
                        }
                        Some(CoreMessage::PopupEvent { popup_id, event }) => {
                            if let Some(on_event_fn) = &guest.on_event {
                                let (w_ptr, w_len) = guest.write_bytes(&mut store, popup_id.as_bytes())?;
                                let (e_ptr, e_len) = guest.write_bytes(&mut store, event.as_bytes())?;
                                set_guest_budget(&mut store, INITIAL_FUEL)?;
                                let res = on_event_fn.call(&mut store, (w_ptr, w_len, e_ptr, e_len));
                                let _ = guest.free_bytes(&mut store, w_ptr, w_len);
                                let _ = guest.free_bytes(&mut store, e_ptr, e_len);
                                if let Err(e) = res {
                                    error!("WASM module '{}' popup event trapped: {}", name, e);
                                    trap_reason = Some(e.into());
                                    break;
                                }
                            }
                        }
                        Some(CoreMessage::TopicEvent { topic, value }) => {
                            if store.data().manifest.can_subscribe(&topic) {
                                if let Some(on_topic_fn) = &guest.on_topic {
                                    let val_json = serde_json::to_string(&value).unwrap_or_else(|_| "null".to_string());
                                    let (t_ptr, t_len) = guest.write_bytes(&mut store, topic.as_bytes())?;
                                    let (v_ptr, v_len) = guest.write_bytes(&mut store, val_json.as_bytes())?;
                                    set_guest_budget(&mut store, INITIAL_FUEL)?;
                                    let res = on_topic_fn.call(&mut store, (t_ptr, t_len, v_ptr, v_len));
                                    let _ = guest.free_bytes(&mut store, t_ptr, t_len);
                                    let _ = guest.free_bytes(&mut store, v_ptr, v_len);
                                    if let Err(e) = res {
                                        error!("WASM module '{}' on_topic trapped: {}", name, e);
                                        trap_reason = Some(e.into());
                                        break;
                                    }
                                }
                            }
                        }
                        Some(CoreMessage::Init { .. }) => {}
                        Some(CoreMessage::Shutdown) | None => {
                            if let Some(shutdown_fn) = &guest.shutdown {
                                set_guest_budget(&mut store, SHUTDOWN_FUEL)?;
                                let _ = shutdown_fn.call(&mut store, ());
                            }
                            break;
                        }
                    }
                }

                Some(event) = socket_event_rx.recv() => {
                    let mut events = vec![event];
                    while let Ok(next) = socket_event_rx.try_recv() {
                        events.push(next);
                        if events.len() >= 32 {
                            break;
                        }
                    }
                    events.dedup_by(|a, b| {
                        match (a, b) {
                            (SocketEvent::Data { handle: h1, payload: p1 }, SocketEvent::Data { handle: h2, payload: p2 }) => {
                                h1 == h2 && p1 == p2
                            }
                            _ => false,
                        }
                    });

                    let mut trapped = false;
                    for ev in events {
                        match ev {
                            SocketEvent::Data { handle, payload } => {
                                if let Some(on_data_fn) = &guest.on_socket_data {
                                    let (p_ptr, p_len) = guest.write_bytes(&mut store, &payload)?;
                                    set_guest_budget(&mut store, INITIAL_FUEL)?;
                                    let res = on_data_fn.call(&mut store, (handle, p_ptr, p_len));
                                    let _ = guest.free_bytes(&mut store, p_ptr, p_len);
                                    if let Err(error) = res {
                                        error!("WASM module '{}' on_socket_data trapped: {}", name, error);
                                        trap_reason = Some(error.into());
                                        trapped = true;
                                        break;
                                    }
                                } else if let Some(on_socket_event_fn) = &guest.on_socket_event {
                                    let (p_ptr, p_len) = guest.write_bytes(&mut store, &payload)?;
                                    set_guest_budget(&mut store, INITIAL_FUEL)?;
                                    let res = on_socket_event_fn.call(&mut store, (handle, p_ptr, p_len));
                                    let _ = guest.free_bytes(&mut store, p_ptr, p_len);
                                    if let Err(error) = res {
                                        error!("WASM module '{}' on_socket_event trapped: {}", name, error);
                                        trap_reason = Some(error.into());
                                        trapped = true;
                                        break;
                                    }
                                } else if let Some(on_event_fn) = &guest.on_event {
                                    warn!("WASM module '{}' uses deprecated on_event fallback for socket data", name);
                                    let widget_id = format!("unix_socket:{}", handle);
                                    let payload_str = String::from_utf8_lossy(&payload);
                                    let (w_ptr, w_len) = guest.write_bytes(&mut store, widget_id.as_bytes())?;
                                    let (e_ptr, e_len) = guest.write_bytes(&mut store, payload_str.as_bytes())?;
                                    set_guest_budget(&mut store, INITIAL_FUEL)?;
                                    let res = on_event_fn.call(&mut store, (w_ptr, w_len, e_ptr, e_len));
                                    let _ = guest.free_bytes(&mut store, w_ptr, w_len);
                                    let _ = guest.free_bytes(&mut store, e_ptr, e_len);
                                    if let Err(error) = res {
                                        error!("WASM module '{}' socket event fallback trapped: {}", name, error);
                                        trap_reason = Some(error.into());
                                        trapped = true;
                                        break;
                                    }
                                }
                            }
                            SocketEvent::Closed { handle, reason } => {
                                store.data_mut().active_sockets.remove(&handle);
                                let reason_code = match reason {
                                    host_api::CloseReason::PeerClosed => 0u32,
                                    host_api::CloseReason::ReadError(_) => 1u32,
                                    host_api::CloseReason::Shutdown => 2u32,
                                };
                                if let Some(on_closed_fn) = &guest.on_socket_closed {
                                    set_guest_budget(&mut store, INITIAL_FUEL)?;
                                    let res = on_closed_fn.call(&mut store, (handle, reason_code));
                                    if let Err(error) = res {
                                        error!("WASM module '{}' on_socket_closed trapped: {}", name, error);
                                        trap_reason = Some(error.into());
                                        trapped = true;
                                        break;
                                    }
                                } else {
                                    warn!("WASM module '{}' socket handle {} closed ({:?}), but on_socket_closed not exported", name, handle, reason);
                                }
                            }
                            SocketEvent::Error { handle, code } => {
                                if let Some(on_error_fn) = &guest.on_socket_error {
                                    set_guest_budget(&mut store, INITIAL_FUEL)?;
                                    let res = on_error_fn.call(&mut store, (handle, code));
                                    if let Err(error) = res {
                                        error!("WASM module '{}' on_socket_error trapped: {}", name, error);
                                        trap_reason = Some(error.into());
                                        trapped = true;
                                        break;
                                    }
                                } else {
                                    warn!("WASM module '{}' socket handle {} error ({}), but on_socket_error not exported", name, handle, code);
                                }
                            }
                        }
                    }
                    if trapped {
                        break;
                    }
                }

                Some((path, iface, member, body_json)) = dbus_signal_rx.recv() => {
                    if guest.on_dbus_signal_with_path.is_some() || guest.on_dbus_signal.is_some() {
                        let (p_ptr, p_len) = guest.write_bytes(&mut store, path.as_bytes())?;
                        let (i_ptr, i_len) = guest.write_bytes(&mut store, iface.as_bytes())?;
                        let (m_ptr, m_len) = guest.write_bytes(&mut store, member.as_bytes())?;
                        let (b_ptr, b_len) = guest.write_bytes(&mut store, body_json.as_bytes())?;
                        set_guest_budget(&mut store, INITIAL_FUEL)?;
                        let res = if let Some(on_dbus_fn) = &guest.on_dbus_signal_with_path {
                            on_dbus_fn.call(
                                &mut store,
                                (p_ptr, p_len, i_ptr, i_len, m_ptr, m_len, b_ptr, b_len),
                            )
                        } else if let Some(on_dbus_fn) = &guest.on_dbus_signal {
                            on_dbus_fn.call(&mut store, (i_ptr, i_len, m_ptr, m_len, b_ptr, b_len))
                        } else {
                            Ok(())
                        };
                        let _ = guest.free_bytes(&mut store, p_ptr, p_len);
                        let _ = guest.free_bytes(&mut store, i_ptr, i_len);
                        let _ = guest.free_bytes(&mut store, m_ptr, m_len);
                        let _ = guest.free_bytes(&mut store, b_ptr, b_len);
                        if let Err(e) = res {
                            error!("WASM module '{}' on_dbus_signal trapped: {}", name, e);
                            trap_reason = Some(e.into());
                            break;
                        }
                    }
                }

                Some(timer_id) = timer_event_rx.recv() => {
                    host_api::unregister_timer(timer_id);
                    store.data_mut().active_timers.remove(&timer_id);
                    if let Some(on_timer_fn) = &guest.on_timer {
                        set_guest_budget(&mut store, INITIAL_FUEL)?;
                        if let Err(e) = on_timer_fn.call(&mut store, timer_id) {
                            error!("WASM module '{}' on_timer trapped: {}", name, e);
                            trap_reason = Some(e.into());
                            break;
                        }
                    }
                }

                Some(watch_id) = path_event_rx.recv() => {
                    if let Some(on_path_fn) = &guest.on_path_appeared {
                        set_guest_budget(&mut store, INITIAL_FUEL)?;
                        if let Err(e) = on_path_fn.call(&mut store, watch_id) {
                            error!("WASM module '{}' on_path_appeared trapped: {}", name, e);
                            trap_reason = Some(e.into());
                            break;
                        }
                    }
                }

                _ = tick_interval.tick(), if has_tick => {
                    if let Some(tick_fn) = &guest.on_tick {
                        set_guest_budget(&mut store, INITIAL_FUEL)?;
                        if let Err(e) = tick_fn.call(&mut store, ()) {
                            error!("WASM module '{}' on_tick trapped: {}", name, e);
                            trap_reason = Some(e.into());
                            break;
                        }
                    }
                }
            }
        }

        store.data_mut().cleanup_timers();
        store.data_mut().cleanup_watches();
        if let Some(error) = trap_reason {
            let reason = format!("{error:#}");
            if let Some(emitted_down) = &self.emitted_down {
                emitted_down
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(name.clone());
            }
            let _ = updates_tx
                .send((
                    name.clone(),
                    ModuleMessage::Down {
                        module: name.clone(),
                        reason,
                    },
                ))
                .await;
            return Err(error);
        }
        info!("WASM module '{}' sandbox terminated cleanly", name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::manifest::ModuleLevel;

    #[tokio::test]
    async fn colocated_cwasm_is_ignored_and_wasm_source_is_compiled() {
        let temp_dir = tempfile::tempdir().unwrap();
        let wasm_path = temp_dir.path().join("module.wasm");
        let source_wasm = wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
            )"#,
        )
        .unwrap();
        let sidecar_wasm = wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func $start unreachable)
                (start $start)
            )"#,
        )
        .unwrap();
        std::fs::write(&wasm_path, source_wasm).unwrap();
        let engine = WasmEngine::new().unwrap();
        let sidecar = engine.engine().precompile_module(&sidecar_wasm).unwrap();
        std::fs::write(wasm_path.with_extension("cwasm"), sidecar).unwrap();

        let manifest = ModuleManifest {
            name: "sidecar_probe".to_string(),
            ..ModuleManifest::default()
        };
        let runner = WasmModuleRunner::from_file(manifest, wasm_path).unwrap();
        let (command_tx, command_rx) = mpsc::channel(1);
        let (updates_tx, mut updates_rx) = mpsc::channel(2);
        let task = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), command_rx, updates_tx)
                .await
        });

        let received = updates_rx.recv().await;
        if received.is_none() {
            panic!("runner ended unexpectedly: {:?}", task.await.unwrap());
        }
        let (module_name, ready) = received.unwrap();
        assert_eq!(module_name, "sidecar_probe");
        assert!(matches!(ready, ModuleMessage::Ready));
        command_tx.send(CoreMessage::Shutdown).await.unwrap();
        assert!(task.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn guest_memory_growth_is_limited_by_store_limits() {
        let wat = r#"(module
            (memory (export "memory") 1 2049)
            (func (export "grow") (result i32)
                (i32.const 2048)
                memory.grow)
        )"#;
        let engine = WasmEngine::new().unwrap();
        let module = Module::new(engine.engine(), wat::parse_str(wat).unwrap()).unwrap();
        let mut store = Store::new(
            engine.engine(),
            wasmtime::StoreLimitsBuilder::new()
                .memory_size(STORE_MEMORY_LIMIT)
                .instances(1)
                .tables(1)
                .build(),
        );
        store.limiter(|limits| limits);
        store.set_fuel(INITIAL_FUEL).unwrap();
        store.set_epoch_deadline(1_000_000);
        let instance = Linker::new(engine.engine())
            .instantiate(&mut store, &module)
            .unwrap();
        let grow = instance
            .get_typed_func::<(), i32>(&mut store, "grow")
            .unwrap();
        assert_eq!(grow.call(&mut store, ()).unwrap(), -1);
    }

    #[tokio::test]
    async fn infinite_guest_initialization_is_interrupted_by_epoch_deadline() {
        let wat = r#"(module
            (memory (export "memory") 1)
            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (loop $forever
                    br $forever
                )
                (i32.const 0))
        )"#;
        let manifest = ModuleManifest {
            name: "epoch_limit_probe".to_string(),
            ..ModuleManifest::default()
        };
        let runner = WasmModuleRunner::from_bytes(manifest, wat::parse_str(wat).unwrap());
        let engine = WasmEngine::new().unwrap();
        let (_command_tx, command_rx) = mpsc::channel(1);
        let (updates_tx, _updates_rx) = mpsc::channel(1);
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            runner.run(engine, serde_json::json!({}), command_rx, updates_tx),
        )
        .await
        .expect("guest initialization must be interrupted")
        .unwrap_err();
        let error = format!("{result:#}");
        assert!(
            error.contains("fuel") || error.contains("interrupt") || error.contains("epoch"),
            "unexpected trap reason: {error}"
        );
    }

    #[test]
    fn test_unregistered_host_import_fails_to_instantiate() {
        let engine = WasmEngine::new().unwrap();
        let wat = r#"
            (module
                (import "wyrd_host" "non_existent_function"
                    (func (param i32) (result i32)))
            )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let module = Module::new(engine.engine(), &wasm_bytes).unwrap();
        let mut linker = Linker::new(engine.engine());
        register_host_functions(&mut linker).unwrap();

        let (updates_tx, _) = mpsc::channel(1);
        let (dbus_signal_tx, _) = mpsc::channel(1);
        let (socket_event_tx, _) = mpsc::channel(1);
        let (timer_event_tx, _) = mpsc::channel(1);
        let (path_event_tx, _) = mpsc::channel(1);
        let host_state = ModuleHostState {
            name: "unregistered_import_test".to_string(),
            manifest: ModuleManifest::default(),
            updates_tx,
            dbus_signal_tx,
            socket_event_tx,
            timer_event_tx,
            path_event_tx,
            subscribed_signals: std::collections::HashSet::new(),
            last_updates: std::collections::HashMap::new(),
            active_timers: std::collections::HashSet::new(),
            active_watches: std::collections::HashSet::new(),
            active_sockets: std::collections::HashMap::new(),
            limits: wasmtime::StoreLimitsBuilder::new()
                .memory_size(STORE_MEMORY_LIMIT)
                .instances(1)
                .tables(1)
                .build(),
        };
        let mut store = Store::new(engine.engine(), host_state);

        let error = linker
            .instantiate(&mut store, &module)
            .expect_err("module with unknown import must fail to instantiate");
        assert!(
            error.to_string().contains("non_existent_function"),
            "unexpected linker error: {error}"
        );
    }

    #[tokio::test]
    async fn test_wasm_module_lifecycle_and_updates() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_send_update" (func $host_send_update (param i32 i32 i32 i32) (result i32)))
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "clock_chip")
            (data (i32.const 16) "{\"text\":\"12:00\"}")
            (data (i32.const 40) "clock.tick")
            (data (i32.const 60) "\"12:00\"")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))

            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_send_update (i32.const 0) (i32.const 10) (i32.const 16) (i32.const 16))
                drop
                (call $host_publish (i32.const 40) (i32.const 10) (i32.const 60) (i32.const 7))
                drop
                (i32.const 0)
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_clock".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test WASM Module".to_string(),
            capabilities: vec!["popups".to_string()],
            publishes: vec!["clock.tick".to_string()],
            subscribes: vec!["theme.accent".to_string()],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let (name, msg1) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "test_clock");
        match msg1 {
            ModuleMessage::Update { widget_id, payload } => {
                assert_eq!(widget_id, "clock_chip");
                assert_eq!(payload["text"], "12:00");
            }
            other => panic!("expected Update, got {:?}", other),
        }

        let (name, msg2) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "test_clock");
        match msg2 {
            ModuleMessage::Publish { topic, value } => {
                assert_eq!(topic, "clock.tick");
                assert_eq!(value, "12:00");
            }
            other => panic!("expected Publish, got {:?}", other),
        }

        let (name, msg3) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "test_clock");
        assert!(matches!(msg3, ModuleMessage::Ready));

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_wasm_schedule_timer_and_cancel() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_schedule_timer" (func $host_schedule_timer (param i64) (result i32)))
            (import "wyrd_host" "host_cancel_timer" (func $host_cancel_timer (param i32) (result i32)))
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "timer.fired")
            (data (i32.const 16) "true")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))

            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_schedule_timer (i64.const 20))
                drop
                (i32.const 0)
            )

            (func (export "wyrd_on_timer") (param i32)
                (call $host_publish (i32.const 0) (i32.const 11) (i32.const 16) (i32.const 4))
                drop
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_timer".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Timer".to_string(),
            capabilities: vec![],
            publishes: vec!["timer.fired".to_string()],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        let timer_published =
            tokio::time::timeout(std::time::Duration::from_millis(1000), upd_rx.recv())
                .await
                .expect("timer should fire within 1000ms")
                .unwrap();
        match timer_published.1 {
            ModuleMessage::Publish { topic, value } => {
                assert_eq!(topic, "timer.fired");
                assert_eq!(value, serde_json::Value::Bool(true));
            }
            other => panic!("expected publish, got {:?}", other),
        }

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_timers_cleaned_on_module_reload() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_schedule_timer" (func $host_schedule_timer (param i64) (result i32)))
            (memory (export "memory") 1)
            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_schedule_timer (i64.const 60000))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_timer") (param i32))
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_reload_timer".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Reload Timer".to_string(),
            capabilities: vec![],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        assert!(host_api::active_timers_for_module("test_reload_timer") > 0);

        cmd_tx.send(CoreMessage::Shutdown).await.unwrap();
        let _ = handle.await.unwrap();

        assert_eq!(host_api::active_timers_for_module("test_reload_timer"), 0);
    }

    #[tokio::test]
    async fn test_timer_id_removed_even_on_guest_trap() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_schedule_timer" (func $host_schedule_timer (param i64) (result i32)))
            (memory (export "memory") 1)
            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_schedule_timer (i64.const 20))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_timer") (param i32)
                unreachable
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_trap_timer".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Trap Timer".to_string(),
            capabilities: vec![],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (_cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        let _ = handle.await.unwrap();

        assert_eq!(host_api::active_timers_for_module("test_trap_timer"), 0);
    }

    #[tokio::test]
    async fn test_schedule_timer_returns_none_after_shutdown() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_schedule_timer" (func $host_schedule_timer (param i64) (result i32)))
            (memory (export "memory") 1)
            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32) (i32.const 0))
            (func (export "call_schedule") (result i32)
                (call $host_schedule_timer (i64.const 100))
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let engine = WasmEngine::new().unwrap();
        let module = Module::new(engine.engine(), &wasm_bytes).unwrap();
        let mut linker = Linker::new(engine.engine());
        register_host_functions(&mut linker).unwrap();

        let (updates_tx, _) = mpsc::channel(1);
        let (dbus_signal_tx, _) = mpsc::channel(1);
        let (socket_event_tx, _) = mpsc::channel(1);
        let (timer_event_tx, timer_event_rx) = mpsc::channel(1);
        let (path_event_tx, _) = mpsc::channel(1);
        let host_state = ModuleHostState {
            name: "test_shutdown_timer".to_string(),
            manifest: ModuleManifest::default(),
            updates_tx,
            dbus_signal_tx,
            socket_event_tx,
            timer_event_tx,
            path_event_tx,
            subscribed_signals: std::collections::HashSet::new(),
            last_updates: std::collections::HashMap::new(),
            active_timers: std::collections::HashSet::new(),
            active_watches: std::collections::HashSet::new(),
            active_sockets: std::collections::HashMap::new(),
            limits: wasmtime::StoreLimitsBuilder::new()
                .memory_size(STORE_MEMORY_LIMIT)
                .instances(1)
                .tables(1)
                .build(),
        };
        let mut store = Store::new(engine.engine(), host_state);
        let _ = store.set_fuel(1_000_000_000);
        store.set_epoch_deadline(1_000_000);
        let instance = linker.instantiate(&mut store, &module).unwrap();

        drop(timer_event_rx);

        let call_schedule = instance
            .get_typed_func::<(), i32>(&mut store, "call_schedule")
            .unwrap();
        let result = call_schedule.call(&mut store, ()).unwrap();
        assert_eq!(result, 0);
        assert_eq!(host_api::active_timers_for_module("test_shutdown_timer"), 0);
    }

    #[tokio::test]
    async fn test_wasm_unauthorized_publish_blocked() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "secret.topic")
            (data (i32.const 20) "\"data\"")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))

            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_publish (i32.const 0) (i32.const 12) (i32.const 20) (i32.const 6))
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "malicious_module".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Unauthorized Module".to_string(),
            capabilities: vec![],
            publishes: vec!["safe.topic".to_string()],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        // The unauthorized publish must not generate a ModuleMessage::Publish
        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await.unwrap();

        while let Ok((_name, msg)) = upd_rx.try_recv() {
            if matches!(msg, ModuleMessage::Publish { .. }) {
                panic!("Unauthorized publish should have been blocked!");
            }
        }
    }

    #[tokio::test]
    async fn test_compiled_wasm_counter_module() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_send_update" (func $host_send_update (param i32 i32 i32 i32) (result i32)))
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "counter_chip")
            (data (i32.const 16) "{\"count\":10,\"text\":\"Count: 10\"}")
            (data (i32.const 64) "{\"count\":15,\"text\":\"Count: 15\"}")
            (data (i32.const 112) "counter.value")
            (data (i32.const 128) "15")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))

            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_send_update (i32.const 0) (i32.const 12) (i32.const 16) (i32.const 31))
                drop
                (i32.const 0)
            )

            (func (export "wyrd_on_event") (param i32 i32 i32 i32)
                (call $host_send_update (i32.const 0) (i32.const 12) (i32.const 64) (i32.const 31))
                drop
                (call $host_publish (i32.const 112) (i32.const 13) (i32.const 128) (i32.const 2))
                drop
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();

        let manifest = ModuleManifest {
            name: "counter".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "wyrd_module_counter.wasm".to_string(),
            description: "Pluggable WASM Counter".to_string(),
            capabilities: vec!["popups".to_string()],
            publishes: vec!["counter.value".to_string()],
            subscribes: vec!["theme.accent".to_string()],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(
                    engine,
                    serde_json::json!({ "initial": 10, "step": 5 }),
                    cmd_rx,
                    upd_tx,
                )
                .await
        });

        let (name, msg1) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "counter");
        match msg1 {
            ModuleMessage::Update { widget_id, payload } => {
                assert_eq!(widget_id, "counter_chip");
                assert_eq!(payload["count"], 10);
                assert_eq!(payload["text"], "Count: 10");
            }
            other => panic!("expected initial Update, got {:?}", other),
        }

        let (name, msg2) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "counter");
        assert!(matches!(msg2, ModuleMessage::Ready));

        cmd_tx
            .send(CoreMessage::Event {
                widget_id: "counter_chip".to_string(),
                event: "click".to_string(),
            })
            .await
            .unwrap();

        let (name, msg3) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "counter");
        match msg3 {
            ModuleMessage::Update { widget_id, payload } => {
                assert_eq!(widget_id, "counter_chip");
                assert_eq!(payload["count"], 15);
                assert_eq!(payload["text"], "Count: 15");
            }
            other => panic!("expected Update after click, got {:?}", other),
        }

        let (name, msg4) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "counter");
        match msg4 {
            ModuleMessage::Publish { topic, value } => {
                assert_eq!(topic, "counter.value");
                assert_eq!(value, 15);
            }
            other => panic!("expected Publish after click, got {:?}", other),
        }

        cmd_tx.send(CoreMessage::Shutdown).await.unwrap();
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn consecutive_events_are_all_delivered_to_guest() {
        let wat = r#"(module
            (import "wyrd_host" "host_publish"
                (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (global $count (mut i32) (i32.const 0))
            (data (i32.const 0) "events.count")
            (data (i32.const 32) "3")
            (func (export "wyrd_on_event") (param i32 i32 i32 i32)
                global.get $count
                i32.const 1
                i32.add
                global.set $count
                global.get $count
                i32.const 3
                i32.eq
                if
                    i32.const 0
                    i32.const 12
                    i32.const 32
                    i32.const 1
                    call $host_publish
                    drop
                end)
        )"#;
        let manifest = ModuleManifest {
            name: "event_count_probe".to_string(),
            publishes: vec!["events.count".to_string()],
            ..ModuleManifest::default()
        };
        let runner = WasmModuleRunner::from_bytes(manifest, wat::parse_str(wat).unwrap());
        let engine = WasmEngine::new().unwrap();
        let (command_tx, command_rx) = mpsc::channel(8);
        let (updates_tx, mut updates_rx) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), command_rx, updates_tx)
                .await
        });

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), updates_rx.recv())
                .await
                .expect("runner should become ready")
                .expect("ready notification should arrive")
                .1,
            ModuleMessage::Ready
        ));
        for _ in 0..3 {
            command_tx
                .send(CoreMessage::Event {
                    widget_id: "same-widget".to_string(),
                    event: "activate".to_string(),
                })
                .await
                .unwrap();
        }

        let (_, message) = tokio::time::timeout(Duration::from_secs(2), updates_rx.recv())
            .await
            .expect("guest should publish after processing all three events")
            .expect("publish notification should arrive");
        match message {
            ModuleMessage::Publish { topic, value } => {
                assert_eq!(topic, "events.count");
                assert_eq!(value, 3);
            }
            other => panic!("expected event count publish, got {other:?}"),
        }

        command_tx.send(CoreMessage::Shutdown).await.unwrap();
        assert!(task.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn test_compiled_wasm_clock_module() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_send_update" (func $host_send_update (param i32 i32 i32 i32) (result i32)))
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "clock")
            (data (i32.const 16) "{\"text\":\"12:00\",\"raw_time\":1700000000}")
            (data (i32.const 64) "clock.tick")
            (data (i32.const 80) "\"12:00\"")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))

            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_send_update (i32.const 0) (i32.const 5) (i32.const 16) (i32.const 38))
                drop
                (call $host_publish (i32.const 64) (i32.const 10) (i32.const 80) (i32.const 7))
                drop
                (i32.const 0)
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();

        let manifest = ModuleManifest {
            name: "clock".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "wyrd_module_clock.wasm".to_string(),
            description: "WASM Clock Module".to_string(),
            capabilities: vec!["popups".to_string()],
            publishes: vec!["clock.tick".to_string()],
            subscribes: vec!["theme.accent".to_string()],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(
                    engine,
                    serde_json::json!({"format": "%H:%M"}),
                    cmd_rx,
                    upd_tx,
                )
                .await
        });

        let (name, msg1) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "clock");
        match msg1 {
            ModuleMessage::Update { widget_id, payload } => {
                assert_eq!(widget_id, "clock");
                assert!(payload.get("text").is_some());
                assert!(payload.get("raw_time").is_some());
            }
            other => panic!("expected initial Update, got {:?}", other),
        }

        let (name, msg2) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "clock");
        assert!(matches!(msg2, ModuleMessage::Publish { ref topic, .. } if topic == "clock.tick"));

        let (name, msg3) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "clock");
        assert!(matches!(msg3, ModuleMessage::Ready));

        cmd_tx.send(CoreMessage::Shutdown).await.unwrap();
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_compiled_wasm_network_dbus_module() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_send_update" (func $host_send_update (param i32 i32 i32 i32) (result i32)))
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (import "wyrd_host" "host_request_surface" (func $host_request_surface (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (global $step (mut i32) (i32.const 0))

            (data (i32.const 0) "network")
            (data (i32.const 16) "{\"text\":\"Wi-Fi\",\"connected\":true}")
            (data (i32.const 64) "network.status")
            (data (i32.const 80) "\"online\"")
            (data (i32.const 96) "popup:wifi-auth")
            (data (i32.const 128) "{\"target_ssid\":\"UnsavedSecuredWifi\",\"children\":[]}")
            (data (i32.const 192) "open")
            (data (i32.const 200) "close")
            (data (i32.const 208) "{\"name\":\"wifi-auth\"}")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))

            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_send_update (i32.const 0) (i32.const 7) (i32.const 16) (i32.const 33))
                drop
                (call $host_publish (i32.const 64) (i32.const 14) (i32.const 80) (i32.const 8))
                drop
                (i32.const 0)
            )

            (func (export "wyrd_on_event") (param i32 i32 i32 i32)
                (if (i32.eq (global.get $step) (i32.const 0))
                    (then
                        (global.set $step (i32.const 1))
                        (call $host_send_update (i32.const 96) (i32.const 15) (i32.const 128) (i32.const 50))
                        drop
                        (call $host_request_surface (i32.const 192) (i32.const 4) (i32.const 208) (i32.const 20))
                        drop
                    )
                    (else
                        (call $host_request_surface (i32.const 200) (i32.const 5) (i32.const 208) (i32.const 20))
                        drop
                    )
                )
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();

        let manifest = ModuleManifest {
            name: "network".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "wyrd_module_network.wasm".to_string(),
            description: "WASM Network Module via D-Bus".to_string(),
            capabilities: vec![
                "dbus:call:system:org.freedesktop.NetworkManager".to_string(),
                "dbus:subscribe:system:org.freedesktop.NetworkManager:org.freedesktop.NetworkManager".to_string(),
                "dbus:subscribe:system:org.freedesktop.NetworkManager:org.freedesktop.DBus.Properties".to_string(),
                "dbus:subscribe:system:org.freedesktop.NetworkManager:org.freedesktop.NetworkManager.Device.Wireless".to_string(),
                "process:spawn:nmcli".to_string(),
                "popups".to_string(),
            ],
            publishes: vec!["network.status".to_string()],
            subscribes: vec!["theme.accent".to_string()],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let (name, msg1) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "network");
        match msg1 {
            ModuleMessage::Update { widget_id, payload } => {
                assert_eq!(widget_id, "network");
                assert!(payload.get("text").is_some());
                assert!(payload.get("connected").is_some());
            }
            other => panic!("expected initial Update, got {:?}", other),
        }

        let (name, msg2) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "network");
        assert!(
            matches!(msg2, ModuleMessage::Publish { ref topic, .. } if topic == "network.status")
        );

        let (name, msg3) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "network");
        assert!(matches!(msg3, ModuleMessage::Ready));

        cmd_tx
            .send(CoreMessage::Event {
                widget_id: "connect_UnsavedSecuredWifi".to_string(),
                event: "click".to_string(),
            })
            .await
            .unwrap();

        let (name, msg4) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "network");
        match msg4 {
            ModuleMessage::Update { widget_id, payload } => {
                assert_eq!(widget_id, "popup:wifi-auth");
                assert_eq!(
                    payload.get("target_ssid").and_then(|v| v.as_str()),
                    Some("UnsavedSecuredWifi")
                );
                assert!(payload.get("children").is_some());
            }
            other => panic!("expected popup:wifi-auth Update, got {:?}", other),
        }

        let (name, msg5) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "network");
        match msg5 {
            ModuleMessage::Surface { action, params } => {
                assert_eq!(action, "open");
                assert_eq!(
                    params.get("name").and_then(|v| v.as_str()),
                    Some("wifi-auth")
                );
            }
            other => panic!("expected Surface open, got {:?}", other),
        }

        cmd_tx
            .send(CoreMessage::Event {
                widget_id: "wifi_auth_cancel".to_string(),
                event: "click".to_string(),
            })
            .await
            .unwrap();

        let (name, msg6) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "network");
        match msg6 {
            ModuleMessage::Surface { action, params } => {
                assert_eq!(action, "close");
                assert_eq!(
                    params.get("name").and_then(|v| v.as_str()),
                    Some("wifi-auth")
                );
            }
            other => panic!("expected Surface close, got {:?}", other),
        }

        cmd_tx.send(CoreMessage::Shutdown).await.unwrap();
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_wasm_process_spawn_capability_denial() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_exec_process" (func $host_exec_process (param i32 i32 i32 i32 i32 i32) (result i32)))
            (import "wyrd_host" "host_send_update" (func $host_send_update (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "bluetoothctl")
            (data (i32.const 20) "[\"show\"]")
            (data (i32.const 40) "test_widget")
            (data (i32.const 60) "{\"status\":\"denied\"}")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))

            (func (export "wyrd_init") (param i32 i32) (result i32)
                ;; Try to execute bluetoothctl (12 bytes) with args (8 bytes), buffer at 200 (max 256)
                (call $host_exec_process (i32.const 0) (i32.const 12) (i32.const 20) (i32.const 8) (i32.const 200) (i32.const 256))
                ;; If result is -403, send update
                (i32.const -403)
                i32.eq
                if
                    (call $host_send_update (i32.const 40) (i32.const 11) (i32.const 60) (i32.const 19))
                    drop
                end
                (i32.const 0)
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "restricted_network".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Restricted Module".to_string(),
            // Only nmcli capability granted, bluetoothctl is NOT allowed!
            capabilities: vec!["process:spawn:nmcli".to_string()],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        // Verify update confirming denial was sent
        let (name, msg1) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "restricted_network");
        match msg1 {
            ModuleMessage::Update { widget_id, payload } => {
                assert_eq!(widget_id, "test_widget");
                assert_eq!(payload["status"], "denied");
            }
            other => panic!("expected Update with denial confirmation, got {:?}", other),
        }

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_wasm_process_spawn_safe_argv_injection_immune() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_exec_process" (func $host_exec_process (param i32 i32 i32 i32 i32 i32) (result i32)))
            (import "wyrd_host" "host_send_update" (func $host_send_update (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "echo")
            ;; Argv json containing dangerous shell metacharacters: ["; touch /tmp/injection_pwned"]
            (data (i32.const 10) "[\"; touch /tmp/injection_pwned\"]")
            (data (i32.const 50) "safe_test")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))

            (func (export "wyrd_init") (param i32 i32) (result i32)
                ;; Call echo (4 bytes) with args (33 bytes), write output to 200
                (call $host_exec_process (i32.const 0) (i32.const 4) (i32.const 10) (i32.const 33) (i32.const 200) (i32.const 256))
                drop
                ;; Send payload containing the raw echoed output string back to host
                (call $host_send_update (i32.const 50) (i32.const 9) (i32.const 10) (i32.const 33))
                drop
                (i32.const 0)
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "safe_echo_module".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Safe Echo Module".to_string(),
            capabilities: vec!["process:spawn:echo".to_string()],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        // Ensure canary file does not exist before
        let _ = std::fs::remove_file("/tmp/injection_pwned");

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let (name, _msg1) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "safe_echo_module");

        // Verify that /tmp/injection_pwned was NOT created (no shell execution occurred)
        assert!(!std::path::Path::new("/tmp/injection_pwned").exists());

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_wasm_dbus_call_capability_denial() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_dbus_call" (func $host_dbus_call (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
            (import "wyrd_host" "host_send_update" (func $host_send_update (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "system")
            (data (i32.const 10) "org.freedesktop.NetworkManager")
            (data (i32.const 50) "/org/freedesktop/NetworkManager")
            (data (i32.const 90) "org.freedesktop.NetworkManager")
            (data (i32.const 130) "GetDevices")
            (data (i32.const 150) "[]")
            (data (i32.const 160) "dbus_test")
            (data (i32.const 180) "{\"status\":\"dbus_denied\"}")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))

            (func (export "wyrd_init") (param i32 i32) (result i32)
                (local $res i32)
                ;; Call dbus without capability
                (call $host_dbus_call
                    (i32.const 0) (i32.const 6)
                    (i32.const 10) (i32.const 30)
                    (i32.const 50) (i32.const 31)
                    (i32.const 90) (i32.const 30)
                    (i32.const 130) (i32.const 10)
                    (i32.const 150) (i32.const 2)
                    (i32.const 200) (i32.const 256)
                )
                ;; If result is -403, send update
                (i32.const -403)
                i32.eq
                if
                    (call $host_send_update (i32.const 160) (i32.const 9) (i32.const 180) (i32.const 24))
                    drop
                end
                (i32.const 0)
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "unauthorized_dbus_module".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Unauthorized D-Bus Module".to_string(),
            // No dbus capabilities granted
            capabilities: vec![],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let (name, msg1) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "unauthorized_dbus_module");
        match msg1 {
            ModuleMessage::Update { widget_id, payload } => {
                assert_eq!(widget_id, "dbus_test");
                assert_eq!(payload["status"], "dbus_denied");
            }
            other => panic!("expected Update confirming D-Bus denial, got {:?}", other),
        }

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_wasm_dbus_signal_dispatch() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_send_update" (func $host_send_update (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "signal_widget")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))

            (func (export "wyrd_init") (param i32 i32) (result i32)
                (i32.const 0)
            )

            ;; Exported signal handler
            (func (export "wyrd_on_dbus_signal") (param i32 i32 i32 i32 i32 i32)
                ;; Send update using the received body bytes (param 4 and 5)
                (call $host_send_update (i32.const 0) (i32.const 13) (local.get 4) (local.get 5))
                drop
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "signal_test_module".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Signal Module".to_string(),
            capabilities: vec![],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        // Wait for ready
        let (name, msg1) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "signal_test_module");
        assert!(matches!(msg1, ModuleMessage::Ready));

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_wasm_dbus_subscribe_capability_denial() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_dbus_subscribe" (func $host_dbus_subscribe (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
            (import "wyrd_host" "host_send_update" (func $host_send_update (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "system")
            (data (i32.const 10) "org.freedesktop.NetworkManager")
            (data (i32.const 50) "/org/freedesktop/NetworkManager")
            (data (i32.const 90) "org.freedesktop.NetworkManager")
            (data (i32.const 130) "StateChanged")
            (data (i32.const 160) "subscribe_test")
            (data (i32.const 180) "{\"status\":\"subscribe_denied\"}")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))

            (func (export "wyrd_init") (param i32 i32) (result i32)
                ;; Call dbus subscribe without capability
                (call $host_dbus_subscribe
                    (i32.const 0) (i32.const 6)
                    (i32.const 10) (i32.const 30)
                    (i32.const 50) (i32.const 31)
                    (i32.const 90) (i32.const 30)
                    (i32.const 130) (i32.const 12)
                )
                ;; If result is -403, send update
                (i32.const -403)
                i32.eq
                if
                    (call $host_send_update (i32.const 160) (i32.const 14) (i32.const 180) (i32.const 29))
                    drop
                end
                (i32.const 0)
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "unauthorized_subscribe_module".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Unauthorized Subscribe Module".to_string(),
            capabilities: vec![],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let (name, msg1) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "unauthorized_subscribe_module");
        match msg1 {
            ModuleMessage::Update { widget_id, payload } => {
                assert_eq!(widget_id, "subscribe_test");
                assert_eq!(payload["status"], "subscribe_denied");
            }
            other => panic!(
                "expected Update confirming subscribe denial, got {:?}",
                other
            ),
        }

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_wasm_file_exists_capability_denial() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_file_exists" (func $host_file_exists (param i32 i32) (result i32)))
            (import "wyrd_host" "host_send_update" (func $host_send_update (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "/etc/shadow")
            (data (i32.const 20) "file_test")
            (data (i32.const 40) "{\"status\":\"file_denied\"}")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))

            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_file_exists (i32.const 0) (i32.const 11))
                ;; Must return -1 on permission denial
                (i32.const -1)
                i32.eq
                if
                    (call $host_send_update (i32.const 20) (i32.const 9) (i32.const 40) (i32.const 24))
                    drop
                end
                (i32.const 0)
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "unauthorized_file_exists_module".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Unauthorized File Exists Module".to_string(),
            capabilities: vec![], // zero fs:read capability
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let mut found_update = false;
        while let Some((name, msg)) = upd_rx.recv().await {
            assert_eq!(name, "unauthorized_file_exists_module");
            match msg {
                ModuleMessage::Update { widget_id, payload } => {
                    assert_eq!(widget_id, "file_test");
                    assert_eq!(payload["status"], "file_denied");
                    found_update = true;
                    break;
                }
                ModuleMessage::Ready => {}
                other => panic!("unexpected message: {:?}", other),
            }
        }
        assert!(found_update, "expected Update confirming file denial");

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_wasm_list_dir_capability_denial() {
        let wat = r#"
        (module
            (import "wyrd_host" "host_list_dir" (func $host_list_dir (param i32 i32 i32 i32) (result i32)))
            (import "wyrd_host" "host_send_update" (func $host_send_update (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "/root")
            (data (i32.const 20) "dir_test")
            (data (i32.const 40) "{\"status\":\"dir_denied\"}")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))

            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_list_dir (i32.const 0) (i32.const 5) (i32.const 200) (i32.const 256))
                ;; Must return -403 on permission denial
                (i32.const -403)
                i32.eq
                if
                    (call $host_send_update (i32.const 20) (i32.const 8) (i32.const 40) (i32.const 23))
                    drop
                end
                (i32.const 0)
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "unauthorized_list_dir_module".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Unauthorized List Dir Module".to_string(),
            capabilities: vec![], // zero fs:read capability
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let mut found_update = false;
        while let Some((name, msg)) = upd_rx.recv().await {
            assert_eq!(name, "unauthorized_list_dir_module");
            match msg {
                ModuleMessage::Update { widget_id, payload } => {
                    assert_eq!(widget_id, "dir_test");
                    assert_eq!(payload["status"], "dir_denied");
                    found_update = true;
                    break;
                }
                ModuleMessage::Ready => {}
                other => panic!("unexpected message: {:?}", other),
            }
        }
        assert!(found_update, "expected Update confirming dir denial");

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_watch_path_fires_on_file_creation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target_file = temp_dir.path().join("service.sock");
        let path_str = target_file.to_str().unwrap();

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{path_str}")
            (data (i32.const 500) "path.appeared")
            (data (i32.const 600) "true")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_path_appeared") (param i32)
                (call $host_publish (i32.const 500) (i32.const 13) (i32.const 600) (i32.const 4))
                drop
            )
        )
        "#,
            path_str = path_str,
            path_len = path_str.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_watch_path".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Watch Path".to_string(),
            capabilities: vec!["fs:watch:tmp".to_string()],
            publishes: vec!["path.appeared".to_string()],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        std::fs::File::create(&target_file).unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(2), upd_rx.recv())
            .await
            .expect("path appeared should trigger within 2s")
            .unwrap();

        match event.1 {
            ModuleMessage::Publish { topic, value } => {
                assert_eq!(topic, "path.appeared");
                assert_eq!(value, serde_json::Value::Bool(true));
            }
            other => panic!("expected path.appeared publish, got {:?}", other),
        }

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_watch_path_cleaned_on_module_unload() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target_file = temp_dir.path().join("service.sock");
        let path_str = target_file.to_str().unwrap();

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{path_str}")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_path_appeared") (param i32))
        )
        "#,
            path_str = path_str,
            path_len = path_str.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_unload_watch".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Watch Cleanup".to_string(),
            capabilities: vec!["fs:watch:tmp".to_string()],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        assert_eq!(host_api::active_watches_for_module("test_unload_watch"), 1);

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;

        assert_eq!(host_api::active_watches_for_module("test_unload_watch"), 0);
    }

    #[tokio::test]
    async fn test_watch_path_no_event_on_existing_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target_file = temp_dir.path().join("existing.sock");
        std::fs::File::create(&target_file).unwrap();
        let path_str = target_file.to_str().unwrap();

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{path_str}")
            (data (i32.const 500) "path.appeared")
            (data (i32.const 600) "true")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_path_appeared") (param i32)
                (call $host_publish (i32.const 500) (i32.const 13) (i32.const 600) (i32.const 4))
                drop
            )
        )
        "#,
            path_str = path_str,
            path_len = path_str.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_existing_watch".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Watch Existing".to_string(),
            capabilities: vec!["fs:watch".to_string()],
            publishes: vec!["path.appeared".to_string()],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        // Wait 150ms - no path.appeared event should be received
        let event =
            tokio::time::timeout(std::time::Duration::from_millis(150), upd_rx.recv()).await;
        assert!(
            event.is_err(),
            "expected no event for already existing file"
        );

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_watch_path_denied_without_capability() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target_file = temp_dir.path().join("service.sock");
        let path_str = target_file.to_str().unwrap();

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{path_str}")
            (data (i32.const 500) "path.appeared")
            (data (i32.const 600) "true")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                ;; Call host_watch_path without having fs:watch capability
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_path_appeared") (param i32)
                (call $host_publish (i32.const 500) (i32.const 13) (i32.const 600) (i32.const 4))
                drop
            )
        )
        "#,
            path_str = path_str,
            path_len = path_str.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_denied_watch".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Watch Denied".to_string(),
            capabilities: vec![], // No fs:watch!
            publishes: vec!["path.appeared".to_string()],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        // Capability denial should result in 0 active watches registered
        assert_eq!(host_api::active_watches_for_module("test_denied_watch"), 0);

        // Creating file should NOT trigger any notification
        std::fs::File::create(&target_file).unwrap();
        let event =
            tokio::time::timeout(std::time::Duration::from_millis(150), upd_rx.recv()).await;
        assert!(
            event.is_err(),
            "watch event must not fire when capability was denied"
        );

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_watches_cleaned_on_module_task_drop() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target_file = temp_dir.path().join("reload.sock");
        let path_str = target_file.to_str().unwrap();

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{path_str}")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_path_appeared") (param i32))
        )
        "#,
            path_str = path_str,
            path_len = path_str.len()
        );

        let wasm_bytes = wat::parse_str(&wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_task_drop_watch".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Watch Task Drop".to_string(),
            capabilities: vec!["fs:watch:tmp".to_string()],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        // Run Instance 1
        let runner1 = WasmModuleRunner::from_bytes(manifest.clone(), wasm_bytes.clone());
        let engine = WasmEngine::new().unwrap();
        let (_cmd_tx1, cmd_rx1) = mpsc::channel(10);
        let (upd_tx1, mut upd_rx1) = mpsc::channel(10);

        let handle1 = tokio::spawn({
            let engine = engine.clone();
            async move {
                runner1
                    .run(engine, serde_json::json!({}), cmd_rx1, upd_tx1)
                    .await
            }
        });

        let ready1 = upd_rx1.recv().await.unwrap();
        assert!(matches!(ready1.1, ModuleMessage::Ready));
        assert_eq!(
            host_api::active_watches_for_module("test_task_drop_watch"),
            1
        );

        // Abort Instance 1 task (simulating instance drop / runner cancellation)
        handle1.abort();
        let _ = handle1.await;

        // Give the task abortion drop a moment to execute
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            host_api::active_watches_for_module("test_task_drop_watch"),
            0,
            "watches must be cleaned up on module task drop / abortion"
        );

        // Run Instance 2 (subsequent generation)
        let runner2 = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let (cmd_tx2, cmd_rx2) = mpsc::channel(10);
        let (upd_tx2, mut upd_rx2) = mpsc::channel(10);

        let handle2 = tokio::spawn({
            let engine = engine.clone();
            async move {
                runner2
                    .run(engine, serde_json::json!({}), cmd_rx2, upd_tx2)
                    .await
            }
        });

        let ready2 = upd_rx2.recv().await.unwrap();
        assert!(matches!(ready2.1, ModuleMessage::Ready));
        assert_eq!(
            host_api::active_watches_for_module("test_task_drop_watch"),
            1,
            "new instance registers its own watch cleanly"
        );

        let _ = cmd_tx2.send(CoreMessage::Shutdown).await;
        let _ = handle2.await;
        assert_eq!(
            host_api::active_watches_for_module("test_task_drop_watch"),
            0
        );
    }

    #[tokio::test]
    async fn test_watch_path_fails_gracefully_on_missing_parent() {
        let missing_path = "/nonexistent_parent_dir_wyrd_test_99999/daemon.sock";

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{missing_path}")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_path_appeared") (param i32))
        )
        "#,
            missing_path = missing_path,
            path_len = missing_path.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_missing_parent".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Missing Parent".to_string(),
            capabilities: vec!["fs:watch".to_string()],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        // When parent dir does not exist, watch_path must fail gracefully (return 0, 0 active watches)
        assert_eq!(
            host_api::active_watches_for_module("test_missing_parent"),
            0
        );

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_watch_path_rejected_for_sensitive_path() {
        let sensitive_path = "/etc/passwd";

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{sensitive_path}")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_path_appeared") (param i32))
        )
        "#,
            sensitive_path = sensitive_path,
            path_len = sensitive_path.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_sensitive_watch".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Sensitive Watch".to_string(),
            capabilities: vec!["fs:watch:runtime".to_string(), "fs:watch:/etc".to_string()],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        // Attempting to watch /etc/passwd MUST be rejected by can_watch_path -> returns 0
        assert_eq!(
            host_api::active_watches_for_module("test_sensitive_watch"),
            0
        );

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_watch_path_walks_up_to_existing_parent() {
        let temp_dir = tempfile::tempdir().unwrap();
        let deep_dir = temp_dir.path().join("a").join("b").join("c");
        let target_file = deep_dir.join("file.sock");
        let path_str = target_file.to_str().unwrap();

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{path_str}")
            (data (i32.const 500) "path.appeared")
            (data (i32.const 600) "true")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_path_appeared") (param i32)
                (call $host_publish (i32.const 500) (i32.const 13) (i32.const 600) (i32.const 4))
                drop
            )
        )
        "#,
            path_str = path_str,
            path_len = path_str.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_walkup_watch".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Walkup Watch".to_string(),
            capabilities: vec!["fs:watch:/tmp".to_string()],
            publishes: vec!["path.appeared".to_string()],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        // Parent /a/b/c does not exist, so watcher successfully walked up to temp_dir and attached
        assert_eq!(host_api::active_watches_for_module("test_walkup_watch"), 1);

        // Create intermediate directory 'a'
        std::fs::create_dir(temp_dir.path().join("a")).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Now create intermediate directories and the target file
        std::fs::create_dir_all(&deep_dir).unwrap();
        std::fs::write(&target_file, b"test").unwrap();

        // The escalated watch must detect target file creation and notify guest
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), upd_rx.recv())
            .await
            .expect("should receive event within 2s")
            .unwrap();

        match msg.1 {
            ModuleMessage::Publish { topic, .. } => {
                assert_eq!(topic, "path.appeared");
            }
            other => panic!("expected Publish, got {:?}", other),
        }

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_watch_path_rejected_for_tmp_without_capability() {
        let manifest_runtime = ModuleManifest {
            capabilities: vec!["fs:watch:runtime".to_string()],
            ..Default::default()
        };
        assert!(!manifest_runtime.can_watch_path("/tmp/test.sock"));
        assert!(!manifest_runtime.can_watch_path("/var/tmp/test.sock"));

        let manifest_bare = ModuleManifest {
            capabilities: vec!["fs:watch".to_string()],
            ..Default::default()
        };
        assert!(!manifest_bare.can_watch_path("/tmp/test.sock"));

        let manifest_tmp = ModuleManifest {
            capabilities: vec!["fs:watch:tmp".to_string()],
            ..Default::default()
        };
        assert!(manifest_tmp.can_watch_path("/tmp/test.sock"));
    }

    #[tokio::test]
    async fn test_watch_path_allowed_for_runtime_dir() {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| format!("/run/user/{}", crate::modules::manifest::current_uid()));
        let socket_path = format!("{}/test_allowed.sock", runtime_dir);

        let manifest_runtime = ModuleManifest {
            capabilities: vec!["fs:watch:runtime".to_string()],
            ..Default::default()
        };
        assert!(manifest_runtime.can_watch_path(&socket_path));

        let manifest_bare = ModuleManifest {
            capabilities: vec!["fs:watch".to_string()],
            ..Default::default()
        };
        assert!(manifest_bare.can_watch_path(&socket_path));
    }

    #[tokio::test]
    #[ignore = "requires real inotify events; flaky on CI overlayfs/tmpfs"]
    async fn test_watch_path_walkup_beyond_10_levels() {
        let temp_dir = tempfile::tempdir().unwrap();
        let deep_dir = temp_dir.path().join("1/2/3/4/5/6/7/8/9/10/11/12/13/14/15");
        let target_file = deep_dir.join("deep.sock");
        let path_str = target_file.to_str().unwrap().to_string();

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{path_str}")
            (data (i32.const 500) "path.appeared")
            (data (i32.const 600) "true")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_path_appeared") (param i32)
                (call $host_publish (i32.const 500) (i32.const 13) (i32.const 600) (i32.const 4))
                drop
            )
        )
        "#,
            path_str = path_str,
            path_len = path_str.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_walkup_15_levels".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Walkup 15 Levels".to_string(),
            capabilities: vec!["fs:watch:tmp".to_string()],
            publishes: vec!["path.appeared".to_string()],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        // Walk-up should walk up 15 levels to temp_dir and succeed
        assert_eq!(
            host_api::active_watches_for_module("test_walkup_15_levels"),
            1
        );

        // 1. Create first intermediate directory '1' and let watcher escalate
        std::fs::create_dir(temp_dir.path().join("1")).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 2. Now create the entire deep hierarchy and target file
        std::fs::create_dir_all(&deep_dir).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        std::fs::write(&target_file, b"deep").unwrap();

        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), upd_rx.recv())
            .await
            .expect("should receive event within 2s")
            .unwrap();

        match msg.1 {
            ModuleMessage::Publish { topic, .. } => {
                assert_eq!(topic, "path.appeared");
            }
            other => panic!("expected Publish, got {:?}", other),
        }

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_watch_path_refuses_root_watch() {
        let manifest = ModuleManifest {
            capabilities: vec!["fs:watch:*".to_string()],
            ..Default::default()
        };
        // Walking up non-existent /wyrd_missing_root_test_12345/sub/test.sock will reach /
        let path = "/wyrd_missing_root_test_12345/sub/test.sock";

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{path}")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
        )
        "#,
            path = path,
            path_len = path.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let mut full_manifest = manifest;
        full_manifest.name = "test_root_refused".to_string();
        full_manifest.capabilities = vec!["fs:watch:tmp".to_string()];

        let runner = WasmModuleRunner::from_bytes(full_manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        // Host must have refused to attach to /
        assert_eq!(host_api::active_watches_for_module("test_root_refused"), 0);

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_watch_path_rejects_symlink_escape() {
        let temp_dir = tempfile::tempdir().unwrap();

        // 1. Symlink directly to /etc/passwd
        let symlink_file = temp_dir.path().join("link_to_passwd");
        let _ = std::os::unix::fs::symlink("/etc/passwd", &symlink_file);

        let manifest = ModuleManifest {
            capabilities: vec!["fs:watch:tmp".to_string()],
            ..Default::default()
        };
        assert!(
            !manifest.can_watch_path(symlink_file.to_str().unwrap()),
            "symlink pointing to /etc/passwd must be rejected by can_watch_path"
        );

        // 2. Symlink to /etc directory
        let symlink_dir = temp_dir.path().join("link_to_etc");
        let _ = std::os::unix::fs::symlink("/etc", &symlink_dir);
        let escape_subpath = symlink_dir.join("shadow");
        assert!(
            !manifest.can_watch_path(escape_subpath.to_str().unwrap()),
            "path through symlink into /etc must be rejected by can_watch_path"
        );
    }

    #[tokio::test]
    async fn test_watch_escalation_survives_target_disappearing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let deep_dir = temp_dir.path().join("sub/target");
        let target_file = deep_dir.join("resilient.sock");
        let path_str = target_file.to_str().unwrap().to_string();

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{path_str}")
            (data (i32.const 500) "path.appeared")
            (data (i32.const 600) "true")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_path_appeared") (param i32)
                (call $host_publish (i32.const 500) (i32.const 13) (i32.const 600) (i32.const 4))
                drop
            )
        )
        "#,
            path_str = path_str,
            path_len = path_str.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_resilient_watch".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Resilient Watch".to_string(),
            capabilities: vec!["fs:watch:tmp".to_string()],
            publishes: vec!["path.appeared".to_string()],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));
        assert_eq!(
            host_api::active_watches_for_module("test_resilient_watch"),
            1
        );

        // 1. Create sub, then immediately remove it to simulate a race during escalation
        let sub = temp_dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::remove_dir(&sub).unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Watcher must still be alive and active
        assert_eq!(
            host_api::active_watches_for_module("test_resilient_watch"),
            1
        );

        // 2. Now recreate properly and create target file
        std::fs::create_dir_all(&deep_dir).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        std::fs::write(&target_file, b"resilient").unwrap();

        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), upd_rx.recv())
            .await
            .expect("should receive event after re-creation")
            .unwrap();

        match msg.1 {
            ModuleMessage::Publish { topic, .. } => {
                assert_eq!(topic, "path.appeared");
            }
            other => panic!("expected Publish, got {:?}", other),
        }

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_walkup_task_cancelled_on_module_unload() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target_file = temp_dir.path().join("sub/unload_test.sock");
        let path_str = target_file.to_str().unwrap().to_string();

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{path_str}")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
        )
        "#,
            path_str = path_str,
            path_len = path_str.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_walkup_cancel".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Walkup Cancel".to_string(),
            capabilities: vec!["fs:watch:tmp".to_string()],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        assert_eq!(host_api::active_watches_for_module("test_walkup_cancel"), 1);

        // Shut down module
        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;

        // Active watches must be immediately 0 and token cancelled
        assert_eq!(host_api::active_watches_for_module("test_walkup_cancel"), 0);

        // Creating the file afterwards must not cause any errors or re-awaken watches
        std::fs::create_dir_all(temp_dir.path().join("sub")).unwrap();
        std::fs::write(&target_file, b"data").unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(host_api::active_watches_for_module("test_walkup_cancel"), 0);
    }

    #[tokio::test]
    async fn test_walkup_cancel_actually_stops_thread() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target_file = temp_dir.path().join("a/b/c/d/test.sock");
        let path_str = target_file.to_str().unwrap().to_string();

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{path_str}")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
        )
        "#,
            path_str = path_str,
            path_len = path_str.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_walkup_cancel_thread".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Walkup Cancel Thread".to_string(),
            capabilities: vec!["fs:watch:tmp".to_string()],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        // Create directory "a" to trigger escalation
        std::fs::create_dir_all(temp_dir.path().join("a")).unwrap();

        // Immediately cancel via shutdown
        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            host_api::active_escalations_count(),
            0,
            "escalation threads must terminate without leaking"
        );
        assert_eq!(
            host_api::active_watches_for_module("test_walkup_cancel_thread"),
            0
        );
    }

    #[tokio::test]
    async fn test_walkup_escalation_failure_preserves_previous_watch() {
        let temp_dir = tempfile::tempdir().unwrap();
        let deep_dir = temp_dir.path().join("parent/sub");
        let target_file = deep_dir.join("test.sock");
        let path_str = target_file.to_str().unwrap().to_string();

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{path_str}")
            (data (i32.const 500) "path.appeared")
            (data (i32.const 600) "true")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_path_appeared") (param i32)
                (call $host_publish (i32.const 500) (i32.const 13) (i32.const 600) (i32.const 4))
                drop
            )
        )
        "#,
            path_str = path_str,
            path_len = path_str.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_escalation_preserves_watch".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test Escalation Preserves Watch".to_string(),
            capabilities: vec!["fs:watch:tmp".to_string()],
            publishes: vec!["path.appeared".to_string()],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        // Create parent directory
        let parent = temp_dir.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        // Even if sub is created and target file appears immediately, watch on parent or sub delivers event
        std::fs::create_dir(&deep_dir).unwrap();
        std::fs::write(&target_file, b"ready").unwrap();

        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), upd_rx.recv())
            .await
            .expect("should receive path.appeared")
            .unwrap();
        assert!(matches!(msg.1, ModuleMessage::Publish { topic, .. } if topic == "path.appeared"));

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_walkup_no_duplicate_events_after_escalation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let target_file = temp_dir.path().join("step1/step2/final.sock");
        let path_str = target_file.to_str().unwrap().to_string();

        let wat = format!(
            r#"
        (module
            (import "wyrd_host" "host_watch_path" (func $host_watch_path (param i32 i32) (result i32)))
            (import "wyrd_host" "host_publish" (func $host_publish (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "{path_str}")
            (data (i32.const 500) "path.appeared")
            (data (i32.const 600) "1")

            (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
            (func (export "wyrd_dealloc") (param i32 i32))
            (func (export "wyrd_init") (param i32 i32) (result i32)
                (call $host_watch_path (i32.const 0) (i32.const {path_len}))
                drop
                (i32.const 0)
            )
            (func (export "wyrd_on_path_appeared") (param i32)
                (call $host_publish (i32.const 500) (i32.const 13) (i32.const 600) (i32.const 1))
                drop
            )
        )
        "#,
            path_str = path_str,
            path_len = path_str.len()
        );

        let wasm_bytes = wat::parse_str(wat).unwrap();
        let manifest = ModuleManifest {
            name: "test_no_duplicates".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test No Duplicates".to_string(),
            capabilities: vec!["fs:watch:tmp".to_string()],
            publishes: vec!["path.appeared".to_string()],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(10);
        let (upd_tx, mut upd_rx) = mpsc::channel(10);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        let ready = upd_rx.recv().await.unwrap();
        assert!(matches!(ready.1, ModuleMessage::Ready));

        // Step 1: create step1
        std::fs::create_dir(temp_dir.path().join("step1")).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Step 2: create step2
        std::fs::create_dir_all(temp_dir.path().join("step1/step2")).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Step 3: create target file
        std::fs::write(&target_file, b"final").unwrap();

        let first_msg = tokio::time::timeout(std::time::Duration::from_secs(2), upd_rx.recv())
            .await
            .expect("should receive exactly 1 path.appeared event")
            .unwrap();
        assert!(
            matches!(first_msg.1, ModuleMessage::Publish { topic, .. } if topic == "path.appeared")
        );

        // Ensure no duplicate event arrives
        let dup = tokio::time::timeout(std::time::Duration::from_millis(200), upd_rx.recv()).await;
        assert!(
            dup.is_err(),
            "no duplicate path.appeared events must be delivered"
        );

        let _ = cmd_tx.send(CoreMessage::Shutdown).await;
        let _ = handle.await;
    }
}
