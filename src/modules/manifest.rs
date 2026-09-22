//! Module Manifest & Capabilities specification.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ModuleLevel {
    #[default]
    Wasm,
    Process,
    Native,
    Lua,
}

/// Official TOML Module Manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleManifest {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub level: ModuleLevel,
    #[serde(default)]
    pub entry: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub publishes: Vec<String>,
    #[serde(default)]
    pub subscribes: Vec<String>,
    #[serde(default)]
    pub interval_ms: Option<u64>,
    #[serde(default)]
    pub align_to_minute: Option<bool>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

impl ModuleManifest {
    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap || c == "*")
    }

    pub fn can_spawn_binary(&self, binary: &str) -> bool {
        let binary_name = std::path::Path::new(binary)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(binary);
        let expected = format!("process:spawn:{}", binary_name);
        self.capabilities
            .iter()
            .any(|c| c == &expected || c == "process:spawn:*" || c == "*")
    }

    pub fn can_read_path(&self, path: &str) -> bool {
        self.capabilities.iter().any(|c| {
            if c == "fs:read" || c == "fs:read:*" || c == "*" {
                true
            } else if c == "fs:read:applications" {
                let data_dirs = std::env::var("XDG_DATA_DIRS")
                    .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
                let user_dir = std::env::var("XDG_DATA_HOME").ok().or_else(|| {
                    std::env::var("HOME")
                        .ok()
                        .map(|h| format!("{}/.local/share", h))
                });
                let mut all_dirs: Vec<String> =
                    data_dirs.split(':').map(|s| s.to_string()).collect();
                if let Some(u) = user_dir {
                    all_dirs.push(u);
                }
                all_dirs.iter().any(|d| path.starts_with(d))
            } else if let Some(prefix) = c.strip_prefix("fs:read:") {
                let clean_prefix = prefix.trim_end_matches('/');
                let clean_path = path.trim_end_matches('/');
                clean_path == clean_prefix
                    || path.starts_with(prefix)
                    || clean_path.starts_with(&format!("{}/", clean_prefix))
            } else {
                false
            }
        })
    }

    pub fn can_dbus_call(&self, bus: &str, service: &str) -> bool {
        let expected = format!("dbus:call:{}:{}", bus, service);
        let bus_wildcard = format!("dbus:call:{}:*", bus);
        self.capabilities.iter().any(|c| {
            c == &expected
                || c == &bus_wildcard
                || c == "dbus:call:*"
                || c == "dbus:call"
                || c == "*"
        })
    }

    pub fn can_dbus_subscribe(&self, bus: &str, service: &str, interface: &str) -> bool {
        let expected = format!("dbus:subscribe:{}:{}:{}", bus, service, interface);
        let service_wildcard = format!("dbus:subscribe:{}:{}:*", bus, service);
        let bus_wildcard = format!("dbus:subscribe:{}:*", bus);
        self.capabilities.iter().any(|c| {
            c == &expected
                || c == &service_wildcard
                || c == &bus_wildcard
                || c == "dbus:subscribe:*"
                || c == "dbus:subscribe"
                || c == "*"
        })
    }

    pub fn can_publish(&self, topic: &str) -> bool {
        self.publishes.iter().any(|p| p == topic || p == "*")
    }

    pub fn can_subscribe(&self, topic: &str) -> bool {
        self.subscribes.iter().any(|s| s == topic || s == "*")
    }

    pub fn can_read_clipboard(&self) -> bool {
        self.capabilities
            .iter()
            .any(|c| c == "clipboard:read" || c == "clipboard:*" || c == "clipboard" || c == "*")
    }

    pub fn can_write_clipboard(&self) -> bool {
        self.capabilities
            .iter()
            .any(|c| c == "clipboard:write" || c == "clipboard:*" || c == "clipboard" || c == "*")
    }

    pub fn can_unix_socket_connect(&self, socket_path: &str) -> bool {
        let path_obj = std::path::Path::new(socket_path);
        let file_name = path_obj.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if (file_name == "wyrd-clipboard.sock" || socket_path.ends_with("wyrd-clipboard.sock"))
            && (self.can_read_clipboard() || self.can_write_clipboard())
        {
            return true;
        }

        let expected_path = format!("socket:connect:{}", socket_path);
        let expected_file = format!("socket:connect:{}", file_name);

        self.capabilities.iter().any(|c| {
            c == &expected_path
                || c == &expected_file
                || c == "socket:connect:*"
                || c == "socket:connect"
                || c == "socket:*"
                || c == "socket"
                || c == "*"
        })
    }

    pub fn can_request_focus(&self) -> bool {
        self.capabilities.iter().any(|c| {
            c == "focus:request" || c == "focus:*" || c == "focus" || c == "popups" || c == "*"
        })
    }

    pub fn can_read_env(&self, key: &str) -> bool {
        if self.capabilities.iter().any(|c| {
            c == "env:read"
                || c == "env:read:*"
                || c == "*"
                || c == &format!("env:read:{}", key)
                || c == &format!("env:read:{}", key.to_uppercase())
        }) {
            return true;
        }

        matches!(
            key,
            "HOME"
                | "XDG_DATA_HOME"
                | "XDG_DATA_DIRS"
                | "XDG_CONFIG_HOME"
                | "XDG_RUNTIME_DIR"
                | "TERMINAL"
                | "LANG"
                | "LC_ALL"
        ) || key.starts_with("WYRD_")
    }

    pub fn from_toml_str(content: &str) -> Result<Self> {
        let manifest: Self =
            toml::from_str(content).context("failed to parse TOML module manifest")?;
        Ok(manifest)
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("failed to read module manifest at {:?}", path.as_ref()))?;
        Self::from_toml_str(&content)
    }
}

/// Discover module manifest searching standard directories.
pub fn find_manifest(name: &str, fallback_entry: &str) -> ModuleManifest {
    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("WYRD_MODULE_MANIFEST_DIR") {
        let dir = PathBuf::from(dir);
        candidates.push(dir.join(name).join("manifest.toml"));
        candidates.push(dir.join(name).join("module.toml"));
        candidates.push(dir.join(format!("{}.toml", name)));
    }

    let config_home = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".config")));

    if let Ok(cfg) = &config_home {
        candidates.push(cfg.join("wyrd/modules").join(name).join("manifest.toml"));
        candidates.push(cfg.join("wyrd/modules").join(name).join("module.toml"));
    }

    candidates.push(
        PathBuf::from("wyrd-modules")
            .join(name)
            .join("manifest.toml"),
    );
    candidates.push(
        PathBuf::from("../wyrd-modules")
            .join(name)
            .join("manifest.toml"),
    );
    candidates.push(
        PathBuf::from("/usr/lib/wyrd/modules")
            .join(name)
            .join("manifest.toml"),
    );
    candidates.push(
        PathBuf::from("/usr/share/wyrd/modules")
            .join(name)
            .join("manifest.toml"),
    );

    for path in candidates {
        if path.is_file() {
            if let Ok(manifest) = ModuleManifest::from_file(&path) {
                return manifest;
            }
        }
    }

    ModuleManifest {
        name: name.to_string(),
        version: "1.0.0".to_string(),
        level: ModuleLevel::Wasm,
        entry: fallback_entry.to_string(),
        description: format!("Module {}", name),
        capabilities: Vec::new(),
        publishes: Vec::new(),
        subscribes: Vec::new(),
        interval_ms: None,
        align_to_minute: None,
        config: None,
    }
}

impl Default for ModuleManifest {
    fn default() -> Self {
        Self {
            name: String::new(),
            version: default_version(),
            level: ModuleLevel::Wasm,
            entry: String::new(),
            description: String::new(),
            capabilities: Vec::new(),
            publishes: Vec::new(),
            subscribes: Vec::new(),
            interval_ms: None,
            align_to_minute: None,
            config: None,
        }
    }
}

/// Find compiled WASM binary or executable for a module.
pub fn find_module_binary(name: &str, manifest: &ModuleManifest) -> Option<PathBuf> {
    let mut search_dirs = Vec::new();

    if let Ok(dir) =
        std::env::var("WYRD_MODULE_DIR").or_else(|_| std::env::var("WYRD_MODULE_MANIFEST_DIR"))
    {
        let p = PathBuf::from(dir);
        search_dirs.push(p.join(name));
        search_dirs.push(p);
    }

    let config_home = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".config")));

    if let Ok(cfg) = &config_home {
        search_dirs.push(cfg.join("wyrd/modules").join(name));
        search_dirs.push(cfg.join("wyrd/modules"));
    }

    search_dirs.push(PathBuf::from("target/wasm32-unknown-unknown/release"));
    search_dirs.push(PathBuf::from("../target/wasm32-unknown-unknown/release"));
    search_dirs.push(PathBuf::from("target/wasm32-unknown-unknown/debug"));
    search_dirs.push(PathBuf::from("wyrd-modules").join(name));
    search_dirs.push(PathBuf::from("../wyrd-modules").join(name));

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            search_dirs.push(parent.to_path_buf());
            search_dirs.push(parent.join("modules").join(name));
            search_dirs.push(parent.join("wasm"));
            if let Some(grandparent) = parent.parent() {
                search_dirs.push(grandparent.join("wasm32-unknown-unknown/release"));
            }
        }
    }

    search_dirs.push(PathBuf::from("/usr/lib/wyrd/modules").join(name));
    search_dirs.push(PathBuf::from("/usr/share/wyrd/modules").join(name));

    let mut filenames = Vec::new();
    if !manifest.entry.is_empty() {
        filenames.push(manifest.entry.clone());
    }
    if manifest.level == ModuleLevel::Wasm {
        filenames.push(format!("{}.wasm", name));
        filenames.push(format!("wyrd_module_{}.wasm", name));
        filenames.push(format!("wyrd_module_{}.wasm", name.replace('-', "_")));
        filenames.push("plugin.wasm".to_string());
        filenames.push("module.wasm".to_string());
    } else {
        filenames.push(format!("wyrd-module-{}", name));
        filenames.push(name.to_string());
    }

    for dir in search_dirs {
        for fname in &filenames {
            let candidate = dir.join(fname);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_manifest_toml() {
        let toml_str = r#"
name = "network"
version = "1.2.0"
level = "process"
entry = "wyrd-module-network"
description = "NetworkManager status and popup"
capabilities = [
    "dbus:system:org.freedesktop.NetworkManager",
    "popups"
]
publishes = ["network.status", "network.active"]
subscribes = ["theme.accent"]
"#;
        let manifest = ModuleManifest::from_toml_str(toml_str).unwrap();
        assert_eq!(manifest.name, "network");
        assert_eq!(manifest.version, "1.2.0");
        assert_eq!(manifest.level, ModuleLevel::Process);
        assert_eq!(manifest.entry, "wyrd-module-network");
        assert!(manifest.has_capability("popups"));
        assert!(manifest.has_capability("dbus:system:org.freedesktop.NetworkManager"));
        assert!(!manifest.has_capability("process:spawn"));
        assert!(manifest.can_publish("network.status"));
        assert!(manifest.can_subscribe("theme.accent"));
    }

    #[test]
    fn test_manifest_env_capabilities() {
        let mut manifest = ModuleManifest {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "".to_string(),
            capabilities: vec!["env:read:CUSTOM_KEY".to_string()],
            publishes: vec![],
            subscribes: vec![],
            interval_ms: None,
            align_to_minute: None,
            config: None,
        };

        // Publishes and Subscribes are deny-by-default when empty
        assert!(!manifest.can_publish("network.status"));
        assert!(!manifest.can_subscribe("theme.accent"));

        // Standard safe env variables are permitted
        assert!(manifest.can_read_env("HOME"));
        assert!(manifest.can_read_env("XDG_DATA_DIRS"));
        assert!(manifest.can_read_env("WYRD_CONFIG_PATH"));

        // Granted capability
        assert!(manifest.can_read_env("CUSTOM_KEY"));

        // Sensitive / ungranted variables denied
        assert!(!manifest.can_read_env("SSH_AUTH_SOCK"));
        assert!(!manifest.can_read_env("AWS_SECRET_ACCESS_KEY"));
        assert!(!manifest.can_read_env("GITHUB_TOKEN"));

        // Wildcard capability
        manifest.capabilities.push("env:read".to_string());
        assert!(manifest.can_read_env("SSH_AUTH_SOCK"));
    }
}
