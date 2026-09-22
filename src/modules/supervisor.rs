//! Module Supervisor: spawn, event loop, and crash recovery for WASM modules.

use anyhow::Result;
use log::{error, info};
use std::collections::HashMap;
use tokio::sync::mpsc;

use super::manifest::{self, ModuleLevel};
use super::wasm::{WasmEngine, WasmModuleRunner};
use super::{CoreMessage, ModuleMessage};

pub struct Supervisor {
    command_routes: HashMap<String, mpsc::Sender<CoreMessage>>,
    wasm_engine: WasmEngine,
    wasm_tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.kill_all();
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Supervisor {
    pub fn new() -> Self {
        Self {
            command_routes: HashMap::new(),
            wasm_engine: WasmEngine::default(),
            wasm_tasks: Vec::new(),
        }
    }

    pub fn kill_all(&mut self) {
        for task in &self.wasm_tasks {
            task.abort();
        }
        self.wasm_tasks.clear();
        self.command_routes.clear();
    }

    pub fn command_sender(&self, name: &str) -> Option<mpsc::Sender<CoreMessage>> {
        self.command_routes.get(name).cloned()
    }

    pub async fn spawn_module(
        &mut self,
        name: &str,
        command: &str,
        config: serde_json::Value,
        updates: mpsc::Sender<(String, ModuleMessage)>,
    ) -> Result<()> {
        let manifest = manifest::find_manifest(name, command);
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
            ModuleLevel::Wasm | ModuleLevel::Process => {
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
        info!("Spawning WASM module: {}", name);
        let binary_path = manifest::find_module_binary(name, &manifest)
            .ok_or_else(|| anyhow::anyhow!("Could not find WASM binary for module '{}'", name))?;

        info!("Loading WASM module '{}' from {:?}", name, binary_path);
        let runner = WasmModuleRunner::from_file(manifest, binary_path)?;

        let (command_tx, command_rx) = mpsc::channel(256);
        let engine = self.wasm_engine.clone();
        let module_name = name.to_string();

        let task = tokio::spawn(async move {
            if let Err(e) = runner.run(engine, config, command_rx, updates).await {
                error!("WASM module '{}' execution error: {:#}", module_name, e);
            }
        });

        self.wasm_tasks.push(task);
        self.command_routes.insert(name.to_string(), command_tx);

        Ok(())
    }

    pub async fn run_supervision(&mut self) {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            ticker.tick().await;
            self.wasm_tasks.retain(|task| !task.is_finished());
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
}
