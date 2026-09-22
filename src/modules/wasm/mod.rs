//! WASM Sandbox Runtime Engine.

pub mod engine;
pub mod guest_abi;
pub mod host_api;

use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use wasmtime::{Linker, Module, Store};

use crate::modules::manifest::ModuleManifest;
use crate::modules::{CoreMessage, ModuleMessage};
pub use engine::WasmEngine;
use guest_abi::GuestExports;
use host_api::{register_host_functions, ModuleHostState};

pub struct WasmModuleRunner {
    name: String,
    manifest: ModuleManifest,
    wasm_bytes: Vec<u8>,
    wasm_path: Option<PathBuf>,
}

impl WasmModuleRunner {
    pub fn from_file<P: AsRef<Path>>(manifest: ModuleManifest, path: P) -> Result<Self> {
        let p = path.as_ref().to_path_buf();
        let bytes = std::fs::read(&p)
            .with_context(|| format!("failed to read WASM module binary at {:?}", p))?;
        Ok(Self {
            name: manifest.name.clone(),
            manifest,
            wasm_bytes: bytes,
            wasm_path: Some(p),
        })
    }

    pub fn from_bytes(manifest: ModuleManifest, bytes: Vec<u8>) -> Self {
        Self {
            name: manifest.name.clone(),
            manifest,
            wasm_bytes: bytes,
            wasm_path: None,
        }
    }

    pub async fn run(
        self,
        engine: WasmEngine,
        config: serde_json::Value,
        mut command_rx: mpsc::Receiver<CoreMessage>,
        updates_tx: mpsc::Sender<(String, ModuleMessage)>,
    ) -> Result<()> {
        let name = self.name.clone();
        info!("Initializing WASM module '{}' in secure sandbox", name);

        // Load precompiled AOT module (.cwasm) or compile via JIT with disk caching
        let module = {
            let mut loaded = None;
            if let Some(wasm_path) = &self.wasm_path {
                let cwasm_path = wasm_path.with_extension("cwasm");
                if cwasm_path.exists() {
                    if let (Ok(w_meta), Ok(c_meta)) =
                        (std::fs::metadata(wasm_path), std::fs::metadata(&cwasm_path))
                    {
                        if let (Ok(w_mtime), Ok(c_mtime)) = (w_meta.modified(), c_meta.modified()) {
                            if c_mtime >= w_mtime {
                                // SAFETY: `Module::deserialize_file` assumes the serialized file was generated
                                // by the same wasmtime version and engine configuration. We verify the file exists
                                // and its modification time matches the source wasm binary before attempting load.
                                match unsafe {
                                    Module::deserialize_file(engine.engine(), &cwasm_path)
                                } {
                                    Ok(m) => {
                                        debug!(
                                            "Loaded precompiled AOT module for '{}' from {:?}",
                                            name, cwasm_path
                                        );
                                        loaded = Some(m);
                                    }
                                    Err(e) => {
                                        warn!("Precompiled module {:?} invalid ({}), will recompile...", cwasm_path, e);
                                    }
                                }
                            }
                        }
                    }
                }

                if loaded.is_none() {
                    match engine.engine().precompile_module(&self.wasm_bytes) {
                        Ok(cwasm_bytes) => {
                            if let Err(e) = std::fs::write(&cwasm_path, &cwasm_bytes) {
                                debug!(
                                    "Could not cache precompiled module to {:?}: {}",
                                    cwasm_path, e
                                );
                            } else {
                                debug!(
                                    "Saved precompiled AOT module for '{}' to {:?}",
                                    name, cwasm_path
                                );
                            }
                            // SAFETY: `Module::deserialize` is called directly on bytes just produced by
                            // `engine.precompile_module` in the current process, guaranteeing matching compiler artifacts.
                            match unsafe { Module::deserialize(engine.engine(), &cwasm_bytes) } {
                                Ok(m) => loaded = Some(m),
                                Err(e) => warn!(
                                    "Failed to deserialize freshly precompiled module '{}': {}",
                                    name, e
                                ),
                            }
                        }
                        Err(e) => {
                            warn!("Failed to precompile module '{}': {}", name, e);
                        }
                    }
                }
            }

            match loaded {
                Some(m) => m,
                None => Module::new(engine.engine(), &self.wasm_bytes)
                    .with_context(|| format!("failed to compile WASM module '{}'", name))?,
            }
        };

        let mut linker = Linker::new(engine.engine());
        register_host_functions(&mut linker)?;

        let (dbus_signal_tx, mut dbus_signal_rx) = mpsc::channel::<(String, String, String)>(64);
        let (socket_event_tx, mut socket_event_rx) = mpsc::channel::<String>(64);

        let host_state = ModuleHostState {
            name: name.clone(),
            manifest: self.manifest.clone(),
            updates_tx: updates_tx.clone(),
            dbus_signal_tx,
            socket_event_tx,
        };

        let mut store = Store::new(engine.engine(), host_state);
        // Grant initial fuel
        let _ = store.set_fuel(1_000_000_000);

        let instance = linker
            .instantiate(&mut store, &module)
            .with_context(|| format!("failed to instantiate WASM module '{}'", name))?;

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
            }
            let config_json =
                serde_json::to_string(&init_config).unwrap_or_else(|_| "{}".to_string());
            let (ptr, len) = guest.write_bytes(&mut store, config_json.as_bytes())?;
            let _ = store.set_fuel(1_000_000_000);
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
                    return Err(e);
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
                    .map(|seconds| seconds.max(1) * 1000)
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

        loop {
            tokio::select! {
                _ = tick_interval.tick(), if has_tick => {
                    if let Some(tick_fn) = &guest.on_tick {
                        let _ = store.set_fuel(1_000_000_000);
                        if let Err(e) = tick_fn.call(&mut store, ()) {
                            error!("WASM module '{}' on_tick trapped: {}", name, e);
                            break;
                        }
                    }
                }

                Some(event) = socket_event_rx.recv() => {
                    if let Some(on_event_fn) = &guest.on_event {
                        let (w_ptr, w_len) = guest.write_bytes(&mut store, event.as_bytes())?;
                        let (e_ptr, e_len) = guest.write_bytes(&mut store, b"changed")?;
                        let _ = store.set_fuel(1_000_000_000);
                        let res = on_event_fn.call(&mut store, (w_ptr, w_len, e_ptr, e_len));
                        let _ = guest.free_bytes(&mut store, w_ptr, w_len);
                        let _ = guest.free_bytes(&mut store, e_ptr, e_len);
                        if let Err(error) = res {
                            error!("WASM module '{}' socket event trapped: {}", name, error);
                            break;
                        }
                    }
                }

                Some((iface, member, body_json)) = dbus_signal_rx.recv() => {
                    if let Some(on_dbus_fn) = &guest.on_dbus_signal {
                        let (i_ptr, i_len) = guest.write_bytes(&mut store, iface.as_bytes())?;
                        let (m_ptr, m_len) = guest.write_bytes(&mut store, member.as_bytes())?;
                        let (b_ptr, b_len) = guest.write_bytes(&mut store, body_json.as_bytes())?;
                        let _ = store.set_fuel(1_000_000_000);
                        let res = on_dbus_fn.call(&mut store, (i_ptr, i_len, m_ptr, m_len, b_ptr, b_len));
                        let _ = guest.free_bytes(&mut store, i_ptr, i_len);
                        let _ = guest.free_bytes(&mut store, m_ptr, m_len);
                        let _ = guest.free_bytes(&mut store, b_ptr, b_len);
                        if let Err(e) = res {
                            error!("WASM module '{}' on_dbus_signal trapped: {}", name, e);
                            break;
                        }
                    }
                }

                msg = command_rx.recv() => {
                    match msg {
                        Some(CoreMessage::Event { widget_id, event }) => {
                            if let Some(on_event_fn) = &guest.on_event {
                                let (w_ptr, w_len) = guest.write_bytes(&mut store, widget_id.as_bytes())?;
                                let (e_ptr, e_len) = guest.write_bytes(&mut store, event.as_bytes())?;
                                let _ = store.set_fuel(1_000_000_000);
                                let res = on_event_fn.call(&mut store, (w_ptr, w_len, e_ptr, e_len));
                                let _ = guest.free_bytes(&mut store, w_ptr, w_len);
                                let _ = guest.free_bytes(&mut store, e_ptr, e_len);
                                if let Err(e) = res {
                                    error!("WASM module '{}' on_event trapped: {}", name, e);
                                    break;
                                }
                            }
                        }
                        Some(CoreMessage::PopupEvent { popup_id, event }) => {
                            if let Some(on_event_fn) = &guest.on_event {
                                let (w_ptr, w_len) = guest.write_bytes(&mut store, popup_id.as_bytes())?;
                                let (e_ptr, e_len) = guest.write_bytes(&mut store, event.as_bytes())?;
                                let _ = store.set_fuel(1_000_000_000);
                                let res = on_event_fn.call(&mut store, (w_ptr, w_len, e_ptr, e_len));
                                let _ = guest.free_bytes(&mut store, w_ptr, w_len);
                                let _ = guest.free_bytes(&mut store, e_ptr, e_len);
                                if let Err(e) = res {
                                    error!("WASM module '{}' popup event trapped: {}", name, e);
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
                                    let _ = store.set_fuel(1_000_000_000);
                                    let res = on_topic_fn.call(&mut store, (t_ptr, t_len, v_ptr, v_len));
                                    let _ = guest.free_bytes(&mut store, t_ptr, t_len);
                                    let _ = guest.free_bytes(&mut store, v_ptr, v_len);
                                    if let Err(e) = res {
                                        error!("WASM module '{}' on_topic trapped: {}", name, e);
                                        break;
                                    }
                                }
                            }
                        }
                        Some(CoreMessage::Init { .. }) => {}
                        Some(CoreMessage::Shutdown) | None => {
                            if let Some(shutdown_fn) = &guest.shutdown {
                                let _ = store.set_fuel(500_000);
                                let _ = shutdown_fn.call(&mut store, ());
                            }
                            break;
                        }
                    }
                }
            }
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
        let wasm_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/wasm32-unknown-unknown/release/wyrd_module_counter.wasm");

        if !wasm_path.exists() {
            return;
        }

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
            config: None,
        };

        let runner = WasmModuleRunner::from_file(manifest, &wasm_path).unwrap();
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

        // 1. Module init sends initial render
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

        // 2. Module sends Ready
        let (name, msg2) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "counter");
        assert!(matches!(msg2, ModuleMessage::Ready));

        // 3. Send click event to counter
        cmd_tx
            .send(CoreMessage::Event {
                widget_id: "counter_chip".to_string(),
                event: "click".to_string(),
            })
            .await
            .unwrap();

        // 4. Counter should update to count=15
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

        // 5. Counter should publish counter.value = 15
        let (name, msg4) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "counter");
        match msg4 {
            ModuleMessage::Publish { topic, value } => {
                assert_eq!(topic, "counter.value");
                assert_eq!(value, 15);
            }
            other => panic!("expected Publish after click, got {:?}", other),
        }

        // Shutdown cleanly
        cmd_tx.send(CoreMessage::Shutdown).await.unwrap();
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_compiled_wasm_clock_module() {
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
            config: None,
        };

        let wasm_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/wasm32-unknown-unknown/release/wyrd_module_clock.wasm");

        if !wasm_path.exists() {
            eprintln!("Skipping test_compiled_wasm_clock_module: wasm binary not compiled yet");
            return;
        }

        let wasm_bytes = std::fs::read(&wasm_path).unwrap();
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

        // 1. Initial update from init()
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

        // 2. Initial publish
        let (name, msg2) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "clock");
        assert!(matches!(msg2, ModuleMessage::Publish { ref topic, .. } if topic == "clock.tick"));

        // 3. Ready message
        let (name, msg3) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "clock");
        assert!(matches!(msg3, ModuleMessage::Ready));

        // Shutdown cleanly
        cmd_tx.send(CoreMessage::Shutdown).await.unwrap();
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_compiled_wasm_network_dbus_module() {
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
            config: None,
        };

        let wasm_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/wasm32-unknown-unknown/release/wyrd_module_network.wasm");

        if !wasm_path.exists() {
            eprintln!(
                "Skipping test_compiled_wasm_network_dbus_module: wasm binary not compiled yet"
            );
            return;
        }

        let wasm_bytes = std::fs::read(&wasm_path).unwrap();
        let runner = WasmModuleRunner::from_bytes(manifest, wasm_bytes);
        let engine = WasmEngine::new().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (upd_tx, mut upd_rx) = mpsc::channel(8);

        let handle = tokio::spawn(async move {
            runner
                .run(engine, serde_json::json!({}), cmd_rx, upd_tx)
                .await
        });

        // 1. Initial update from init() via D-Bus
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

        // 2. Initial publish
        let (name, msg2) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "network");
        assert!(
            matches!(msg2, ModuleMessage::Publish { ref topic, .. } if topic == "network.status")
        );

        // 3. Ready message
        let (name, msg3) = upd_rx.recv().await.unwrap();
        assert_eq!(name, "network");
        assert!(matches!(msg3, ModuleMessage::Ready));

        // 4. Test connect to an unknown/secured SSID -> triggers wifi-auth popup
        cmd_tx
            .send(CoreMessage::Event {
                widget_id: "connect_UnsavedSecuredWifi".to_string(),
                event: "click".to_string(),
            })
            .await
            .unwrap();

        // Expect Update for popup:wifi-auth
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

        // Expect Surface { action: "open", params: {"name": "wifi-auth", ...} }
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

        // 5. Test Cancel on wifi_auth_cancel
        cmd_tx
            .send(CoreMessage::Event {
                widget_id: "wifi_auth_cancel".to_string(),
                event: "click".to_string(),
            })
            .await
            .unwrap();

        // Expect Surface { action: "close", params: {"name": "wifi-auth", ...} }
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

        // Shutdown cleanly
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
}
