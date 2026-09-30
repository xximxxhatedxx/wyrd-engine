//! Filesystem hot-reload watcher for Lua and TOML configuration files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use log::info;
use notify_debouncer_mini::new_debouncer;

use super::{LuaRuntime, LuaRuntimeAdapter};

fn scan_mtimes(dir: &Path, mtimes: &mut HashMap<PathBuf, SystemTime>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) != Some("modules") {
                    scan_mtimes(&path, mtimes);
                }
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("lua")
                || path.extension().and_then(|ext| ext.to_str()) == Some("toml")
            {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(mtime) = meta.modified() {
                        mtimes.insert(path, mtime);
                    }
                }
            }
        }
    }
}

impl<A: LuaRuntimeAdapter> LuaRuntime<A> {
    pub async fn watch_config(&self, tx: tokio::sync::mpsc::Sender<A::Config>) -> Result<()> {
        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel(10);
        let mut debouncer = new_debouncer(Duration::from_millis(250), move |res| {
            let _ = notify_tx.blocking_send(res);
        })?;

        let watch_dir = self.config_path.parent().unwrap_or_else(|| Path::new("."));
        let _ = debouncer
            .watcher()
            .watch(watch_dir, notify::RecursiveMode::Recursive);

        let mut last_mtimes: HashMap<PathBuf, SystemTime> = HashMap::new();
        scan_mtimes(watch_dir, &mut last_mtimes);
        if let Ok(meta) = std::fs::metadata(&self.config_path) {
            if let Ok(mtime) = meta.modified() {
                last_mtimes.insert(self.config_path.clone(), mtime);
            }
        }

        while let Some(events) = notify_rx.recv().await {
            match events {
                Ok(events) => {
                    let mut has_config_change = false;
                    for e in events {
                        let path = &e.path;
                        if path.components().any(|c| c.as_os_str() == "modules") {
                            continue;
                        }
                        let is_candidate = path == &self.config_path
                            || path.extension().and_then(|ext| ext.to_str()) == Some("lua")
                            || path.extension().and_then(|ext| ext.to_str()) == Some("toml");
                        if !is_candidate {
                            continue;
                        }
                        if let Ok(meta) = std::fs::metadata(path) {
                            if let Ok(mtime) = meta.modified() {
                                match last_mtimes.get(path) {
                                    Some(&prev_mtime) if prev_mtime == mtime => {}
                                    _ => {
                                        last_mtimes.insert(path.clone(), mtime);
                                        has_config_change = true;
                                    }
                                }
                            }
                        } else if last_mtimes.remove(path).is_some() {
                            has_config_change = true;
                        }
                    }
                    if has_config_change {
                        info!(
                            "Config/theme files modified ({:?}), hot-reloading...",
                            self.config_path
                        );
                        match self.load_config().await {
                            Ok(new_cfg) => {
                                if tx.send(new_cfg).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                log::error!("Config hot-reload parse error: {}", e);
                            }
                        }
                    }
                }
                Err(error) => {
                    log::warn!("Config debouncer error: {:?}", error);
                }
            }
        }

        Ok(())
    }
}
