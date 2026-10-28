//! Filesystem hot-reload watcher for Lua and TOML configuration files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use log::info;
use notify_debouncer_mini::new_debouncer;

use super::{LuaRuntime, LuaRuntimeAdapter};

fn is_config_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("lua" | "toml")
    )
}

fn scan_mtimes(dir: &Path, mtimes: &mut HashMap<PathBuf, SystemTime>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) != Some("modules") {
                    scan_mtimes(&path, mtimes);
                }
            } else if is_config_file(&path) {
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
                        let is_candidate = is_config_file(path);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_mtimes_recurses_and_filters_extensions() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("wyrd-watch-test-{}-{unique}", std::process::id()));
        let nested = root.join("widgets");
        let modules = root.join("modules");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&modules).unwrap();
        let lua_file = nested.join("clock.lua");
        let toml_file = root.join("settings.toml");
        let ignored_file = nested.join("notes.txt");
        let ignored_module = modules.join("module.lua");
        for path in [&lua_file, &toml_file, &ignored_file, &ignored_module] {
            std::fs::write(path, "").unwrap();
        }

        let mut mtimes = HashMap::new();
        scan_mtimes(&root, &mut mtimes);
        assert!(mtimes.contains_key(&lua_file));
        assert!(mtimes.contains_key(&toml_file));
        assert!(!mtimes.contains_key(&ignored_file));
        assert!(!mtimes.contains_key(&ignored_module));
        assert!(!is_config_file(Path::new("backup.lua~")));

        std::fs::remove_dir_all(root).unwrap();
    }
}
