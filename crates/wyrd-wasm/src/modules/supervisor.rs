//! Module Supervisor: lifecycle management and task tracking for WASM modules.

use anyhow::Result;
use log::{error, info, warn};
use std::collections::HashMap;
use tokio::sync::mpsc;

use super::manifest::{self, ModuleLevel};
use super::wasm::{WasmEngine, WasmModuleRunner};
use super::{CoreMessage, ModuleMessage};
use std::sync::{Arc, Mutex};

pub struct Supervisor {
    command_routes: HashMap<String, mpsc::Sender<CoreMessage>>,
    wasm_engine: WasmEngine,
    wasm_tasks: Vec<(String, tokio::task::JoinHandle<()>)>,
    updates: mpsc::Sender<(String, ModuleMessage)>,
    emitted_down: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.kill_all();
    }
}

/// Deliver a Down notification. Tries non-blocking try_send first, falling back to
/// spawning a task that awaits send if the channel is currently full.
pub fn emit_down(sender: &mpsc::Sender<(String, ModuleMessage)>, module: &str, reason: &str) {
    let msg = (
        module.to_string(),
        ModuleMessage::Down {
            module: module.to_string(),
            reason: reason.to_string(),
        },
    );
    match sender.try_send(msg) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(payload)) => {
            let s = sender.clone();
            tokio::spawn(async move {
                if let Err(e) = s.send(payload).await {
                    warn!("Failed to deliver Down notification: {}", e);
                }
            });
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            // supervisor or receiver shutting down; nothing to deliver
        }
    }
}

impl Supervisor {
    pub fn new(updates: mpsc::Sender<(String, ModuleMessage)>) -> Self {
        Self {
            command_routes: HashMap::new(),
            wasm_engine: WasmEngine::default(),
            wasm_tasks: Vec::new(),
            updates,
            emitted_down: Arc::new(Mutex::new(std::collections::HashSet::new())),
        }
    }

    pub fn kill_all(&mut self) {
        for (_, task) in &self.wasm_tasks {
            task.abort();
        }
        self.wasm_tasks.clear();
        self.command_routes.clear();
    }

    pub fn command_sender(&self, name: &str) -> Option<mpsc::Sender<CoreMessage>> {
        self.command_routes.get(name).cloned()
    }

    pub fn list_running(&self) -> Vec<String> {
        self.wasm_tasks
            .iter()
            .filter(|(_, task)| !task.is_finished())
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub async fn stop_module(&mut self, name: &str) -> Result<bool> {
        let Some(index) = self
            .wasm_tasks
            .iter()
            .position(|(task_name, _)| task_name == name)
        else {
            self.command_routes.remove(name);
            self.emitted_down
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(name);
            return Ok(false);
        };

        if let Some(sender) = self.command_routes.get(name) {
            if let Err(error) = sender.try_send(CoreMessage::Shutdown) {
                warn!("Failed to send Shutdown to module {name}: {error}");
            }
        }

        let (_, mut task) = self.wasm_tasks.remove(index);
        match tokio::time::timeout(std::time::Duration::from_millis(250), &mut task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warn!("Module '{name}' task ended with an error: {error}"),
            Err(_) => {
                task.abort();
                if let Err(error) = task.await {
                    if !error.is_cancelled() {
                        warn!("Failed to abort module '{name}' task: {error}");
                    }
                }
            }
        }
        self.emitted_down
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(name);
        self.command_routes.remove(name);
        info!("Stopped module: {name}");
        Ok(true)
    }

    async fn stop_existing_module(&mut self, name: &str) -> Result<()> {
        if self.command_routes.contains_key(name)
            || self
                .wasm_tasks
                .iter()
                .any(|(task_name, _)| task_name == name)
        {
            info!("Restarting module '{name}'");
            self.stop_module(name).await?;
        }
        Ok(())
    }

    pub fn reap_finished(&mut self) {
        let mut terminated = Vec::new();
        let routes = &mut self.command_routes;
        self.wasm_tasks.retain(|(name, task)| {
            if task.is_finished() {
                warn!("WASM module task '{}' terminated", name);
                terminated.push((name.clone(), "task terminated".to_string()));
                routes.remove(name);
                false
            } else {
                true
            }
        });

        for (mod_name, reason) in terminated {
            if !self
                .emitted_down
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&mod_name)
            {
                emit_down(&self.updates, &mod_name, &reason);
            }
        }
    }

    pub async fn spawn_module(
        &mut self,
        name: &str,
        command: &str,
        config: serde_json::Value,
        updates: mpsc::Sender<(String, ModuleMessage)>,
    ) -> Result<()> {
        self.stop_existing_module(name).await?;
        let manifest = manifest::try_find_manifest(name, command)?;
        let mut final_config = match &manifest.config {
            Some(serde_json::Value::Object(map)) => serde_json::Value::Object(map.clone()),
            _ => serde_json::json!({}),
        };
        if let serde_json::Value::Object(user_opts) = config {
            if let serde_json::Value::Object(ref mut base) = final_config {
                for (k, v) in user_opts {
                    base.insert(k, v);
                }
            }
        }

        match manifest.level {
            ModuleLevel::Wasm => {
                self.spawn_wasm_module(name, manifest, final_config, updates)
                    .await
            }
            ModuleLevel::Native | ModuleLevel::Lua => {
                log::debug!(
                    "Module {} is Native/Lua; skipping supervisor child spawn",
                    name
                );
                Ok(())
            }
        }
    }

    pub async fn spawn_wasm_module(
        &mut self,
        name: &str,
        manifest: manifest::ModuleManifest,
        config: serde_json::Value,
        updates: mpsc::Sender<(String, ModuleMessage)>,
    ) -> Result<()> {
        self.stop_existing_module(name).await?;
        info!("Spawning WASM module: {}", name);
        let binary_path = manifest::find_module_binary(name, &manifest)
            .ok_or_else(|| anyhow::anyhow!("Could not find WASM binary for module '{}'", name))?;

        self.spawn_wasm_module_from_binary(name, manifest, config, updates, binary_path)
            .await
    }

    async fn spawn_wasm_module_from_binary(
        &mut self,
        name: &str,
        manifest: manifest::ModuleManifest,
        config: serde_json::Value,
        updates: mpsc::Sender<(String, ModuleMessage)>,
        binary_path: std::path::PathBuf,
    ) -> Result<()> {
        self.stop_existing_module(name).await?;
        info!("Loading WASM module '{}' from {:?}", name, binary_path);
        let runner = WasmModuleRunner::from_file(manifest, binary_path)?
            .with_emitted_down(self.emitted_down.clone());

        let (command_tx, command_rx) = mpsc::channel(256);
        let engine = self.wasm_engine.clone();
        let module_name = name.to_string();

        let task = tokio::spawn(async move {
            if let Err(e) = runner.run(engine, config, command_rx, updates).await {
                error!("WASM module '{}' execution error: {:#}", module_name, e);
            }
        });

        self.wasm_tasks.push((name.to_string(), task));
        self.command_routes.insert(name.to_string(), command_tx);

        Ok(())
    }

    pub async fn run_supervision(&mut self) {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            ticker.tick().await;
            self.reap_finished();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_manifest_uses_compatible_defaults() {
        std::env::remove_var("WYRD_MODULE_MANIFEST_DIR");
        let manifest = manifest::find_manifest("module-that-does-not-exist", "fallback");
        assert_eq!(manifest.entry, "fallback");
        assert_eq!(manifest.level, ModuleLevel::Wasm);
    }

    #[tokio::test]
    async fn test_emit_down_ok() {
        let (tx, mut rx) = mpsc::channel(10);
        emit_down(&tx, "brightness", "task terminated");
        let (name, msg) = rx.recv().await.expect("message received");
        assert_eq!(name, "brightness");
        match msg {
            ModuleMessage::Down { module, reason } => {
                assert_eq!(module, "brightness");
                assert_eq!(reason, "task terminated");
            }
            _ => panic!("unexpected message"),
        }
    }

    #[tokio::test]
    async fn test_supervisor_stop_module_single() {
        let (updates, _updates_rx) = mpsc::channel(10);
        let mut supervisor = Supervisor::new(updates);
        for name in ["clock", "audio"] {
            let (command_tx, mut command_rx) = mpsc::channel(1);
            let task = tokio::spawn(async move {
                match command_rx.recv().await {
                    Some(CoreMessage::Shutdown) | None => {}
                    _ => panic!("expected shutdown message"),
                }
            });
            supervisor
                .command_routes
                .insert(name.to_string(), command_tx);
            supervisor.wasm_tasks.push((name.to_string(), task));
        }

        assert_eq!(
            supervisor.list_running(),
            vec!["clock".to_string(), "audio".to_string()]
        );

        assert!(supervisor
            .stop_module("clock")
            .await
            .expect("stop succeeds"));
        assert!(supervisor.command_sender("clock").is_none());
        assert!(supervisor.command_sender("audio").is_some());
        assert_eq!(supervisor.list_running(), vec!["audio".to_string()]);
        assert!(!supervisor
            .stop_module("clock")
            .await
            .expect("absent is safe"));
        assert!(supervisor
            .stop_module("audio")
            .await
            .expect("second module stops"));
        assert!(supervisor.list_running().is_empty());
    }

    #[tokio::test]
    async fn test_emit_down_on_full_channel_spawns_and_delivers() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(("filler".to_string(), ModuleMessage::Ping))
            .expect("filler sent");

        emit_down(&tx, "audio", "panic occurred");

        let (first_name, _) = rx.recv().await.expect("first message received");
        assert_eq!(first_name, "filler");

        let (down_name, msg) =
            tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
                .await
                .expect("timeout waiting for Down message")
                .expect("message received");

        assert_eq!(down_name, "audio");
        match msg {
            ModuleMessage::Down { module, reason } => {
                assert_eq!(module, "audio");
                assert_eq!(reason, "panic occurred");
            }
            _ => panic!("unexpected message"),
        }
    }

    #[tokio::test]
    async fn test_supervisor_emits_down_on_task_termination() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut supervisor = Supervisor::new(tx);

        let finished_task = tokio::spawn(async {});
        while !finished_task.is_finished() {
            tokio::task::yield_now().await;
        }
        supervisor
            .wasm_tasks
            .push(("test_mod".to_string(), finished_task));

        let supervisor_handle = tokio::spawn(async move {
            supervisor.run_supervision().await;
        });

        let received = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
            .await
            .expect("timeout waiting for Down from supervisor")
            .expect("channel not closed");

        supervisor_handle.abort();

        assert_eq!(received.0, "test_mod");
        match received.1 {
            ModuleMessage::Down { module, reason } => {
                assert_eq!(module, "test_mod");
                assert_eq!(reason, "task terminated");
            }
            _ => panic!("expected ModuleMessage::Down"),
        }
    }

    #[tokio::test]
    async fn guest_trap_emits_only_one_down_notification() {
        let (updates_tx, mut updates_rx) = mpsc::channel(16);
        let mut supervisor = Supervisor::new(updates_tx.clone());
        let manifest = manifest::ModuleManifest {
            name: "trap_probe".to_string(),
            ..manifest::ModuleManifest::default()
        };
        let wasm = wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
                (func (export "wyrd_dealloc") (param i32 i32))
                (func (export "wyrd_on_event") (param i32 i32 i32 i32)
                    unreachable)
            )"#,
        )
        .unwrap();
        let runner = WasmModuleRunner::from_bytes(manifest, wasm)
            .with_emitted_down(supervisor.emitted_down.clone());
        let engine = WasmEngine::new().unwrap();
        let (command_tx, command_rx) = mpsc::channel(8);
        let emitted_down = supervisor.emitted_down.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = runner
                .run(engine, serde_json::json!({}), command_rx, updates_tx)
                .await
            {
                error!("WASM module 'trap_probe' execution error: {error:#}");
            }
        });
        supervisor
            .command_routes
            .insert("trap_probe".to_string(), command_tx.clone());
        supervisor.wasm_tasks.push(("trap_probe".to_string(), task));

        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), updates_rx.recv())
                .await
                .expect("runner should become ready")
                .expect("ready notification should arrive")
                .1,
            ModuleMessage::Ready
        ));
        command_tx
            .send(CoreMessage::Event {
                widget_id: "probe".to_string(),
                event: "trigger".to_string(),
            })
            .await
            .unwrap();

        let (module, message) =
            tokio::time::timeout(std::time::Duration::from_secs(2), updates_rx.recv())
                .await
                .expect("runner should report its trap")
                .expect("Down notification should arrive");
        assert_eq!(module, "trap_probe");
        match message {
            ModuleMessage::Down { reason, .. } => assert!(reason.contains("unreachable")),
            other => panic!("expected Down after trap, got {other:?}"),
        }

        while !supervisor.wasm_tasks[0].1.is_finished() {
            tokio::task::yield_now().await;
        }
        supervisor.reap_finished();
        assert!(
            updates_rx.try_recv().is_err(),
            "reap_finished must not emit a duplicate Down"
        );
        assert!(emitted_down
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty());
    }

    #[tokio::test]
    async fn spawn_module_twice_replaces_old_task() {
        let temp_dir = tempfile::tempdir().unwrap();
        let wasm_path = temp_dir.path().join("counter.wasm");
        let wasm = wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func (export "wyrd_alloc") (param i32) (result i32) (i32.const 1024))
                (func (export "wyrd_dealloc") (param i32 i32))
                (func (export "wyrd_init") (param i32 i32) (result i32) (i32.const 0))
            )"#,
        )
        .unwrap();
        std::fs::write(&wasm_path, wasm).unwrap();

        let (updates_tx, mut updates_rx) = mpsc::channel(16);
        let mut supervisor = Supervisor::new(updates_tx.clone());
        let manifest = manifest::ModuleManifest {
            name: "counter".to_string(),
            entry: "counter.wasm".to_string(),
            ..manifest::ModuleManifest::default()
        };
        supervisor
            .spawn_wasm_module_from_binary(
                "counter",
                manifest.clone(),
                serde_json::json!({}),
                updates_tx.clone(),
                wasm_path.clone(),
            )
            .await
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), updates_rx.recv())
                .await
                .unwrap()
                .unwrap()
                .1,
            ModuleMessage::Ready
        ));
        let old_sender = supervisor.command_sender("counter").unwrap();

        supervisor
            .spawn_wasm_module_from_binary(
                "counter",
                manifest,
                serde_json::json!({}),
                updates_tx,
                wasm_path,
            )
            .await
            .unwrap();

        let new_sender = supervisor.command_sender("counter").unwrap();
        assert!(!old_sender.same_channel(&new_sender));
        assert!(new_sender.try_send(CoreMessage::Shutdown).is_ok());
        assert_eq!(supervisor.list_running(), vec!["counter"]);
        assert_eq!(supervisor.wasm_tasks.len(), 1);
        assert!(supervisor
            .emitted_down
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty());
        supervisor.stop_module("counter").await.unwrap();
    }
}
