//! Module Manifest & Capabilities specification.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[inline]
pub(crate) fn current_uid() -> u32 {
    // SAFETY: `getuid` has no preconditions and returns the current real UID.
    unsafe { libc::getuid() }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ModuleLevel {
    #[default]
    Wasm,
    Native,
    Lua,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CompanionConfig {
    pub daemon: String,
    pub socket: String,
    #[serde(default)]
    pub provides: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReconnectConfig {
    #[serde(default = "default_reconnect_mode")]
    pub mode: String,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_backoff_cap_sec")]
    pub backoff_cap_sec: u64,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            mode: default_reconnect_mode(),
            max_attempts: default_max_attempts(),
            backoff_cap_sec: default_backoff_cap_sec(),
        }
    }
}

fn default_reconnect_mode() -> String {
    "dormant".to_string()
}

fn default_max_attempts() -> u32 {
    10
}

fn default_backoff_cap_sec() -> u64 {
    30
}

pub fn is_plain_file_name(name: &str) -> bool {
    if name.is_empty() || name.contains(['/', '\\']) || name.chars().any(char::is_control) {
        return false;
    }

    let path = Path::new(name);
    path.components().count() == 1
        && matches!(
            path.components().next(),
            Some(std::path::Component::Normal(_))
        )
        && path.file_name().and_then(|file_name| file_name.to_str()) == Some(name)
}

fn application_data_dirs(data_dirs: &str, user_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = data_dirs
        .split(':')
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .collect();
    if let Some(user_dir) = user_dir.filter(|dir| dir.is_absolute()) {
        dirs.push(user_dir.to_path_buf());
    }
    dirs
}

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
    pub daemon: Option<String>,
    #[serde(default)]
    pub daemon_socket: Option<String>,
    #[serde(default)]
    pub companion: Option<CompanionConfig>,
    #[serde(default)]
    pub reconnect: Option<ReconnectConfig>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

fn canonicalize_path_or_parent(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut existing = path;
    let mut suffix = Vec::new();
    while std::fs::symlink_metadata(existing).is_err() {
        suffix.push(existing.file_name()?.to_os_string());
        existing = existing.parent()?;
    }
    let mut resolved = existing.canonicalize().ok()?;
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    Some(resolved)
}

fn path_is_within(candidate: &Path, prefix: &Path) -> bool {
    let Some(candidate) = canonicalize_path_or_parent(candidate) else {
        return false;
    };
    let Some(prefix) = canonicalize_path_or_parent(prefix) else {
        return false;
    };
    candidate.starts_with(prefix)
}

fn default_version() -> String {
    "1.0.0".to_string()
}

impl ModuleManifest {
    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap || c == "*")
    }

    pub fn can_watch_path(&self, path: &str) -> bool {
        let candidate_path = Path::new(path);
        if candidate_path.as_os_str().is_empty() || candidate_path == Path::new("/") {
            return false;
        }

        if candidate_path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return false;
        }

        // Sensitive system paths that can never be watched by sandboxed modules
        let sensitive_prefixes = [
            Path::new("/etc"),
            Path::new("/proc"),
            Path::new("/sys"),
            Path::new("/root"),
            Path::new("/boot"),
            Path::new("/dev"),
        ];
        if sensitive_prefixes
            .iter()
            .any(|p| candidate_path.starts_with(p))
        {
            return false;
        }

        let runtime_dirs: Vec<PathBuf> = {
            let mut dirs = Vec::new();
            if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
                let p = PathBuf::from(rt);
                if p.is_absolute() {
                    if let Ok(c) = p.canonicalize() {
                        dirs.push(c);
                    }
                    dirs.push(p);
                }
            }
            let uid = current_uid();
            let run_user = PathBuf::from(format!("/run/user/{}", uid));
            if let Ok(c) = run_user.canonicalize() {
                dirs.push(c);
            }
            dirs.push(run_user);
            dirs
        };

        // Canonicalization check to defend against symlink escapes
        let resolved_canonical = canonicalize_path_or_parent(candidate_path);

        if let Some(ref canon) = resolved_canonical {
            if canon == Path::new("/") {
                return false;
            }
            if sensitive_prefixes.iter().any(|p| canon.starts_with(p)) {
                return false;
            }
        }

        let tmp_dirs: Vec<PathBuf> = {
            let mut dirs = vec![PathBuf::from("/tmp"), PathBuf::from("/var/tmp")];
            if let Ok(c) = PathBuf::from("/tmp").canonicalize() {
                dirs.push(c);
            }
            if let Ok(c) = PathBuf::from("/var/tmp").canonicalize() {
                dirs.push(c);
            }
            dirs
        };

        self.capabilities.iter().any(|c| {
            if c == "fs:watch:runtime" || c == "fs:watch" {
                runtime_dirs
                    .iter()
                    .any(|dir| path_is_within(candidate_path, dir))
            } else if c == "fs:watch:tmp" {
                tmp_dirs
                    .iter()
                    .any(|dir| path_is_within(candidate_path, dir))
            } else if let Some(prefix) = c.strip_prefix("fs:watch:") {
                let prefix_path = Path::new(prefix);
                if !prefix_path.is_absolute() {
                    return false;
                }
                if prefix_path
                    .components()
                    .any(|comp| matches!(comp, std::path::Component::ParentDir))
                {
                    return false;
                }
                if prefix_path == Path::new("/")
                    || sensitive_prefixes
                        .iter()
                        .any(|p| prefix_path.starts_with(p))
                {
                    return false;
                }
                path_is_within(candidate_path, prefix_path)
            } else {
                false
            }
        })
    }

    pub fn can_spawn_binary(&self, binary: &str) -> bool {
        if !is_plain_file_name(binary) {
            return false;
        }
        let expected = format!("process:spawn:{}", binary);
        self.capabilities
            .iter()
            .any(|c| c == &expected || c == "process:spawn:*" || c == "*")
    }

    pub fn can_read_path(&self, path: &str) -> bool {
        let candidate_path = Path::new(path);
        if candidate_path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return false;
        }

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
                let all_dirs =
                    application_data_dirs(&data_dirs, user_dir.as_deref().map(Path::new));
                all_dirs
                    .iter()
                    .any(|dir| path_is_within(candidate_path, dir))
            } else if let Some(prefix) = c.strip_prefix("fs:read:") {
                let prefix_path = Path::new(prefix);
                if prefix_path
                    .components()
                    .any(|comp| matches!(comp, std::path::Component::ParentDir))
                {
                    return false;
                }
                path_is_within(candidate_path, prefix_path)
            } else {
                false
            }
        })
    }

    pub fn can_write_path(&self, path: &str) -> bool {
        let candidate_path = Path::new(path);
        if candidate_path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return false;
        }

        self.capabilities.iter().any(|c| {
            if c == "fs:write" || c == "fs:write:*" {
                true
            } else if let Some(prefix) = c.strip_prefix("fs:write:") {
                let prefix_path = Path::new(prefix);
                if prefix_path
                    .components()
                    .any(|comp| matches!(comp, std::path::Component::ParentDir))
                {
                    return false;
                }
                path_is_within(candidate_path, prefix_path)
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

    pub fn can_network_fetch(&self, url: &str) -> bool {
        let lower = url.to_ascii_lowercase();
        let authority = lower
            .strip_prefix("https://")
            .or_else(|| lower.strip_prefix("http://"))
            .and_then(|rest| rest.split(['/', '?', '#']).next())
            .unwrap_or("");
        let host_port = authority.rsplit('@').next().unwrap_or("");
        let host = if let Some(bracketed) = host_port.strip_prefix('[') {
            bracketed.split(']').next().unwrap_or("")
        } else {
            host_port.split(':').next().unwrap_or("")
        };
        if host.is_empty() {
            return false;
        }
        self.capabilities.iter().any(|capability| {
            capability == "*"
                || capability == "network:fetch"
                || capability == "network:fetch:*"
                || capability == "net:*"
                || capability == &format!("network:fetch:{host}")
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
        self.publishes.iter().any(|p| {
            p == topic || p == "*" || (p.ends_with('*') && topic.starts_with(&p[..p.len() - 1]))
        })
    }

    pub fn can_subscribe(&self, topic: &str) -> bool {
        self.subscribes.iter().any(|s| {
            s == topic || s == "*" || (s.ends_with('*') && topic.starts_with(&s[..s.len() - 1]))
        })
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
        let can_match_file_name = is_plain_file_name(socket_path);

        let expected_path = format!("socket:connect:{}", socket_path);
        let expected_file = format!("socket:connect:{}", file_name);

        self.capabilities.iter().any(|c| {
            c == &expected_path
                || (can_match_file_name && c == &expected_file)
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
                | "TERM"
                | "TERMINAL"
                | "LANG"
                | "LC_ALL"
        ) || key.starts_with("WYRD_")
    }

    pub fn from_toml_str(content: &str) -> Result<Self> {
        let mut manifest: Self =
            toml::from_str(content).context("failed to parse TOML module manifest")?;
        let unsupported_writes: Vec<_> = manifest
            .capabilities
            .iter()
            .filter(|c| c.starts_with("fs:write"))
            .cloned()
            .collect();
        for capability in &unsupported_writes {
            log::warn!(
                "manifest for module '{}' declares capability '{}' which is not supported in wyrd-wasm 0.1.0; ignoring it and continuing",
                manifest.name,
                capability
            );
        }
        manifest.capabilities.retain(|c| !c.starts_with("fs:write"));
        for capability in &manifest.capabilities {
            if capability == "*"
                || capability == "process:spawn:*"
                || capability
                    .strip_prefix("process:spawn:")
                    .is_some_and(|binary| {
                        matches!(
                            binary,
                            "sh" | "bash" | "python" | "python3" | "env" | "perl" | "ruby" | "node"
                        ) || binary == "*"
                    })
            {
                log::warn!(
                    "manifest for module '{}' grants '{}', which is equivalent to arbitrary code execution; only grant this capability with explicit user consent",
                    manifest.name,
                    capability
                );
            }
        }
        if manifest.capabilities.iter().any(|c| c == "fs:watch") {
            log::warn!(
                "manifest for module '{}' declares bare 'fs:watch' capability, which is deprecated. Treating as 'fs:watch:runtime'. Please update manifest to 'fs:watch:runtime'",
                manifest.name
            );
        }
        Ok(manifest)
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("failed to read module manifest at {:?}", path.as_ref()))?;
        Self::from_toml_str(&content)
    }
}

fn manifest_candidates(name: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return candidates;
    }
    if let Ok(dir) = std::env::var("WYRD_MODULE_MANIFEST_DIR") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            candidates.push(dir.join(name).join("manifest.toml"));
            candidates.push(dir.join(name).join("module.toml"));
            candidates.push(dir.join(format!("{}.toml", name)));
        } else {
            log::warn!("ignoring relative WYRD_MODULE_MANIFEST_DIR={:?}", dir);
        }
    }

    let config_home = absolute_env_dir("XDG_CONFIG_HOME")
        .or_else(|| absolute_env_dir("HOME").map(|home| home.join(".config")));

    if let Some(cfg) = &config_home {
        candidates.push(cfg.join("wyrd/modules").join(name).join("manifest.toml"));
        candidates.push(cfg.join("wyrd/modules").join(name).join("module.toml"));
    }

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
    candidates
}

fn fallback_manifest(name: &str, fallback_entry: &str) -> ModuleManifest {
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
        daemon: None,
        daemon_socket: None,
        companion: None,
        reconnect: None,
        config: None,
    }
}

pub(crate) fn try_find_manifest(name: &str, fallback_entry: &str) -> Result<ModuleManifest> {
    for path in manifest_candidates(name) {
        if path.is_file() {
            return ModuleManifest::from_file(&path).map_err(|error| {
                anyhow::anyhow!(
                    "failed to load manifest for module '{}' at '{}': {error:#}",
                    name,
                    path.display()
                )
            });
        }
    }

    Ok(fallback_manifest(name, fallback_entry))
}

pub fn find_manifest(name: &str, fallback_entry: &str) -> ModuleManifest {
    match try_find_manifest(name, fallback_entry) {
        Ok(manifest) => manifest,
        Err(error) => {
            log::warn!("{error}; using fallback manifest");
            fallback_manifest(name, fallback_entry)
        }
    }
}

impl ModuleManifest {
    pub fn companion_provides(&self) -> Option<&str> {
        self.companion
            .as_ref()
            .and_then(|c| c.provides.as_deref())
            .filter(|s| !s.trim().is_empty())
    }

    pub fn companion_description(&self) -> Option<&str> {
        self.companion
            .as_ref()
            .and_then(|c| c.description.as_deref())
            .filter(|s| !s.trim().is_empty())
    }

    pub fn companion_daemon(&self) -> Option<(&str, &str)> {
        if let Some(comp) = &self.companion {
            if !comp.daemon.is_empty() && !comp.socket.is_empty() {
                return Some((comp.daemon.as_str(), comp.socket.as_str()));
            }
        }
        if let (Some(bin), Some(sock)) = (self.daemon.as_deref(), self.daemon_socket.as_deref()) {
            if !bin.is_empty() && !sock.is_empty() {
                return Some((bin, sock));
            }
        }
        if let Some(cfg) = &self.config {
            if let (Some(bin), Some(sock)) = (
                cfg.get("daemon").and_then(|v| v.as_str()),
                cfg.get("daemon_socket").and_then(|v| v.as_str()),
            ) {
                if !bin.is_empty() && !sock.is_empty() {
                    return Some((bin, sock));
                }
            }
        }
        for cap in &self.capabilities {
            if let Some(sock) = cap.strip_prefix("socket:connect:") {
                if let Some(bin) = sock.strip_suffix(".sock") {
                    if !bin.is_empty() {
                        return Some((bin, sock));
                    }
                }
            }
        }
        None
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
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        }
    }
}

pub fn find_module_binary(name: &str, manifest: &ModuleManifest) -> Option<PathBuf> {
    let home = absolute_env_dir("HOME");
    let data_home = absolute_env_dir("XDG_DATA_HOME")
        .or_else(|| home.as_ref().map(|path| path.join(".local/share")));
    let config_home =
        absolute_env_dir("XDG_CONFIG_HOME").or_else(|| home.map(|path| path.join(".config")));
    let dev_dir = absolute_env_dir("WYRD_MODULES_DIR");
    let search_dirs = module_search_dirs(name, dev_dir, data_home, config_home);
    find_module_binary_in_dirs(name, manifest, &search_dirs)
}

fn absolute_env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn module_search_dirs(
    name: &str,
    dev_dir: Option<PathBuf>,
    data_home: Option<PathBuf>,
    config_home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut search_dirs = Vec::new();

    if let Some(dir) = dev_dir.filter(|path| path.is_absolute()) {
        search_dirs.push(dir.join(name));
        search_dirs.push(dir);
    }
    if let Some(dir) = data_home.filter(|path| path.is_absolute()) {
        let modules = dir.join("wyrd/modules");
        search_dirs.push(modules.join(name));
        search_dirs.push(modules);
    }
    if let Some(dir) = config_home.filter(|path| path.is_absolute()) {
        let modules = dir.join("wyrd/modules");
        search_dirs.push(modules.join(name));
        search_dirs.push(modules);
    }
    search_dirs.push(PathBuf::from("/usr/share/wyrd/modules").join(name));
    search_dirs.push(PathBuf::from("/usr/share/wyrd/modules"));
    search_dirs.push(PathBuf::from("/usr/lib/wyrd/modules").join(name));
    search_dirs.push(PathBuf::from("/usr/lib/wyrd/modules"));
    search_dirs
}

fn find_module_binary_in_dirs(
    name: &str,
    manifest: &ModuleManifest,
    search_dirs: &[PathBuf],
) -> Option<PathBuf> {
    if !manifest.entry.is_empty() && !is_plain_file_name(&manifest.entry) {
        log::warn!(
            "ignoring invalid module binary entry {:?} for '{}'",
            manifest.entry,
            name
        );
        return None;
    }

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

    for dir in search_dirs.iter().filter(|dir| dir.is_absolute()) {
        let Ok(canonical_dir) = dir.canonicalize() else {
            continue;
        };
        for fname in &filenames {
            if !is_plain_file_name(fname) {
                log::warn!(
                    "ignoring invalid module binary entry {:?} for '{}'",
                    fname,
                    name
                );
                continue;
            }
            let candidate = dir.join(fname);
            if let Ok(canonical_candidate) = candidate.canonicalize() {
                if canonical_candidate.starts_with(&canonical_dir) && canonical_candidate.is_file()
                {
                    return Some(canonical_candidate);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn wasm_manifest(name: &str) -> ModuleManifest {
        ModuleManifest {
            name: name.to_string(),
            entry: "evil.wasm".to_string(),
            ..ModuleManifest::default()
        }
    }

    #[test]
    fn find_module_binary_rejects_relative_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let relative_dir = PathBuf::from("relative-modules");
        std::fs::create_dir_all(temp_dir.path().join(&relative_dir)).unwrap();
        std::fs::write(
            temp_dir.path().join(&relative_dir).join("evil.wasm"),
            b"not a wasm module",
        )
        .unwrap();

        assert!(
            find_module_binary_in_dirs("example", &wasm_manifest("example"), &[relative_dir])
                .is_none()
        );
    }

    #[test]
    fn find_module_binary_respects_xdg_data_home() {
        let temp_dir = tempfile::tempdir().unwrap();
        let data_home = temp_dir.path().join("data");
        let module_dir = data_home.join("wyrd/modules/example");
        std::fs::create_dir_all(&module_dir).unwrap();
        let binary = module_dir.join("evil.wasm");
        std::fs::write(&binary, b"test module").unwrap();

        let search_dirs = module_search_dirs("example", None, Some(data_home), None);
        assert_eq!(
            find_module_binary_in_dirs("example", &wasm_manifest("example"), &search_dirs),
            Some(binary)
        );
    }

    #[test]
    fn find_module_binary_rejects_cwd_relative() {
        let temp_dir = tempfile::tempdir().unwrap();
        let module_name = "cwd-relative-binary-lookup-probe";
        let relative_module_dir = temp_dir.path().join("wyrd-modules").join(module_name);
        std::fs::create_dir_all(&relative_module_dir).unwrap();
        std::fs::write(relative_module_dir.join("evil.wasm"), b"shadow module").unwrap();
        let target_dir = temp_dir
            .path()
            .join("target/wasm32-unknown-unknown/release");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(target_dir.join("evil.wasm"), b"shadow module").unwrap();

        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "modules::manifest::tests::cwd_relative_lookup_probe",
            ])
            .current_dir(temp_dir.path())
            .env("WYRD_CWD_LOOKUP_PROBE", "1")
            .env("HOME", temp_dir.path())
            .env_remove("WYRD_MODULES_DIR")
            .env_remove("XDG_DATA_HOME")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "probe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn cwd_relative_lookup_probe() {
        if std::env::var_os("WYRD_CWD_LOOKUP_PROBE").is_none() {
            return;
        }
        let module_name = "cwd-relative-binary-lookup-probe";
        assert!(find_module_binary(module_name, &wasm_manifest(module_name)).is_none());
    }

    #[test]
    fn find_manifest_rejects_cwd_relative() {
        let temp_dir = tempfile::tempdir().unwrap();
        let module_name = "cwd-relative-manifest-lookup-probe";
        let module_dir = temp_dir.path().join("wyrd-modules").join(module_name);
        std::fs::create_dir_all(&module_dir).unwrap();
        std::fs::write(
            module_dir.join("manifest.toml"),
            format!("name = \"{module_name}\"\ncapabilities = [\"*\"]\n"),
        )
        .unwrap();

        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "modules::manifest::tests::cwd_relative_manifest_lookup_probe",
            ])
            .current_dir(temp_dir.path())
            .env("WYRD_CWD_MANIFEST_LOOKUP_PROBE", "1")
            .env("HOME", temp_dir.path())
            .env_remove("WYRD_MODULE_MANIFEST_DIR")
            .env_remove("WYRD_MODULES_DIR")
            .env_remove("XDG_DATA_HOME")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "probe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn cwd_relative_manifest_lookup_probe() {
        if std::env::var_os("WYRD_CWD_MANIFEST_LOOKUP_PROBE").is_none() {
            return;
        }
        let manifest = find_manifest("cwd-relative-manifest-lookup-probe", "module.wasm");
        assert!(manifest.capabilities.is_empty());
        assert_eq!(manifest.entry, "module.wasm");
    }

    #[test]
    fn module_binary_entry_must_be_a_plain_file_name() {
        let temp_dir = tempfile::tempdir().unwrap();
        let module_dir = temp_dir.path().join("modules");
        std::fs::create_dir_all(&module_dir).unwrap();
        std::fs::write(module_dir.join("module.wasm"), b"wasm").unwrap();

        for entry in ["/etc/passwd", "../module.wasm"] {
            let manifest = ModuleManifest {
                entry: entry.to_string(),
                ..wasm_manifest("example")
            };
            assert!(
                find_module_binary_in_dirs(
                    "different-name",
                    &manifest,
                    std::slice::from_ref(&module_dir)
                )
                .is_none(),
                "accepted invalid entry {entry:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn find_module_binary_rejects_symlink_escape() {
        let temp_dir = tempfile::tempdir().unwrap();
        let module_dir = temp_dir.path().join("modules");
        std::fs::create_dir_all(&module_dir).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", module_dir.join("module.wasm")).unwrap();
        let manifest = ModuleManifest {
            entry: "module.wasm".to_string(),
            ..wasm_manifest("example")
        };
        assert!(
            find_module_binary_in_dirs("example", &manifest, &[module_dir]).is_none(),
            "binary symlink pointing outside its module directory must be rejected"
        );
    }

    #[test]
    fn application_data_capability_ignores_empty_and_relative_directories() {
        let dirs = application_data_dirs("/usr/share:", None);
        assert_eq!(dirs, vec![PathBuf::from("/usr/share")]);
        assert!(!dirs
            .iter()
            .any(|dir| path_is_within(Path::new("/etc/passwd"), dir)));

        assert!(application_data_dirs("", None).is_empty());
        assert!(application_data_dirs("nonexistent-rel", None).is_empty());
    }

    #[test]
    fn scoped_read_capability_rejects_symlink_escape() {
        let temp_dir = tempfile::tempdir().unwrap();
        let allowed = temp_dir.path().join("allowed");
        std::fs::create_dir_all(&allowed).unwrap();
        let link = allowed.join("link");
        std::os::unix::fs::symlink("/etc/passwd", &link).unwrap();
        let manifest = ModuleManifest {
            capabilities: vec![format!("fs:read:{}", allowed.display())],
            ..ModuleManifest::default()
        };
        assert!(!manifest.can_read_path(link.to_str().unwrap()));
    }

    #[test]
    fn network_fetch_capability_matches_exact_host_and_wildcard() {
        let exact = ModuleManifest {
            capabilities: vec!["network:fetch:example.com".to_string()],
            ..ModuleManifest::default()
        };
        assert!(exact.can_network_fetch("https://example.com/image.png"));
        assert!(exact.can_network_fetch("HTTP://EXAMPLE.COM:8080/path"));
        assert!(!exact.can_network_fetch("https://other.com/image.png"));
        assert!(!ModuleManifest::default().can_network_fetch("http://127.0.0.1:1/x.png"));
        let wildcard = ModuleManifest {
            capabilities: vec!["network:fetch:*".to_string()],
            ..ModuleManifest::default()
        };
        assert!(wildcard.can_network_fetch("https://other.com"));
    }

    #[test]
    fn process_spawn_capability_rejects_paths() {
        let manifest = ModuleManifest {
            capabilities: vec!["process:spawn:echo".to_string()],
            ..ModuleManifest::default()
        };
        assert!(manifest.can_spawn_binary("echo"));
        assert!(!manifest.can_spawn_binary("/tmp/evil/echo"));
        assert!(!manifest.can_spawn_binary("../echo"));
        assert!(!manifest.can_spawn_binary(r"..\echo"));
    }

    #[test]
    fn test_parse_manifest_toml() {
        let toml_str = r#"
name = "network"
version = "1.2.0"
level = "wasm"
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
        assert_eq!(manifest.level, ModuleLevel::Wasm);
        assert_eq!(manifest.entry, "wyrd-module-network");
        assert!(manifest.has_capability("popups"));
        assert!(manifest.has_capability("dbus:system:org.freedesktop.NetworkManager"));
        assert!(!manifest.has_capability("process:spawn"));
        assert!(manifest.can_publish("network.status"));
        assert!(manifest.can_subscribe("theme.accent"));
    }

    #[test]
    fn process_module_level_is_rejected() {
        let toml_str = r#"
name = "network"
level = "process"
"#;
        assert!(ModuleManifest::from_toml_str(toml_str).is_err());
    }

    #[test]
    fn socket_connect_capabilities_grant_only_the_declared_socket() {
        let clipboard = ModuleManifest::from_toml_str(
            r#"
name = "launcher"
capabilities = ["socket:connect:wyrd-clipboard.sock"]
"#,
        )
        .unwrap();
        assert!(clipboard.can_unix_socket_connect("wyrd-clipboard.sock"));
        assert!(!clipboard.can_unix_socket_connect("/tmp/wyrd-clipboard.sock"));
        assert!(!clipboard.can_unix_socket_connect("../wyrd-clipboard.sock"));
        assert!(!clipboard.can_unix_socket_connect("other.sock"));

        let other = ModuleManifest::from_toml_str(
            r#"
name = "example"
capabilities = ["socket:connect:other.sock"]
"#,
        )
        .unwrap();
        assert!(other.can_unix_socket_connect("other.sock"));
        assert!(!other.can_unix_socket_connect("wyrd-clipboard.sock"));
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
            daemon: None,
            daemon_socket: None,
            companion: None,
            reconnect: None,
            config: None,
        };

        // Publishes and Subscribes are deny-by-default when empty
        assert!(!manifest.can_publish("network.status"));
        assert!(!manifest.can_subscribe("theme.accent"));

        // Standard safe env variables are permitted
        assert!(manifest.can_read_env("HOME"));
        assert!(manifest.can_read_env("XDG_DATA_DIRS"));
        assert!(manifest.can_read_env("TERM"));
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

    #[test]
    fn test_manifest_ignores_fs_write() {
        for capability in ["fs:write", "fs:write:*", "fs:write:/tmp/foo"] {
            let toml_str =
                format!("name = \"test\"\ncapabilities = [{capability:?}, \"fs:read:/tmp\"]\n");
            let manifest = ModuleManifest::from_toml_str(&toml_str).unwrap();
            assert!(
                !manifest
                    .capabilities
                    .iter()
                    .any(|c| c.starts_with("fs:write")),
                "fs:write capabilities should be stripped from manifest: {:?}",
                manifest.capabilities
            );
            assert_eq!(manifest.capabilities, vec!["fs:read:/tmp".to_string()]);
            assert!(!manifest.can_write_path("/tmp/foo"));
        }
    }

    #[test]
    fn test_manifest_wildcard_does_not_grant_write() {
        let manifest = ModuleManifest {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test".to_string(),
            capabilities: vec!["*".to_string()],
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

        // Wildcard '*' must grant read capabilities, but NEVER file write capabilities
        assert!(manifest.can_read_path("/tmp/allowed/file.txt"));
        assert!(!manifest.can_write_path("/tmp/allowed/file.txt"));
        assert!(!manifest.can_write_path("/etc/passwd"));
    }

    #[test]
    fn test_manifest_accepts_read_capabilities() {
        for capability in ["fs:read:/tmp", "fs:read:applications"] {
            let toml_str = format!("name = \"test\"\ncapabilities = [{capability:?}]\n");
            let manifest = ModuleManifest::from_toml_str(&toml_str).unwrap();
            assert_eq!(manifest.capabilities, vec![capability.to_string()]);
        }
    }

    #[test]
    fn test_wasm_path_traversal_blocked() {
        let manifest = ModuleManifest {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            level: ModuleLevel::Wasm,
            entry: "test.wasm".to_string(),
            description: "Test".to_string(),
            capabilities: vec!["fs:read:/tmp/allowed".to_string()],
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

        assert!(manifest.can_read_path("/tmp/allowed/file.txt"));
        assert!(!manifest.can_read_path("/tmp/allowed/../etc/passwd"));
        assert!(!manifest.can_read_path("/tmp/allowed/../../root/.ssh/id_rsa"));
        assert!(!manifest.can_read_path("/tmp/allowed/sub/../../etc/shadow"));
        assert!(!manifest.can_read_path("/etc/passwd"));

        assert!(manifest_candidates("../evil").is_empty());
        assert!(manifest_candidates("valid_module-1").len() >= 4);
    }

    #[test]
    fn test_manifest_with_companion_section() {
        let toml_str = r#"
name = "audio"
version = "1.0.0"
level = "wasm"
entry = "module.wasm"
description = "PipeWire/PulseAudio volume controller"

[companion]
daemon = "wyrd-audio"
socket = "wyrd-audio.sock"
provides = "audio"
description = "PipeWire/Pulse volume, sinks, mute state"
"#;
        let manifest = ModuleManifest::from_toml_str(toml_str).unwrap();
        assert_eq!(manifest.name, "audio");
        assert_eq!(manifest.companion_provides(), Some("audio"));
        assert_eq!(
            manifest.companion_description(),
            Some("PipeWire/Pulse volume, sinks, mute state")
        );
        assert_eq!(
            manifest.companion_daemon(),
            Some(("wyrd-audio", "wyrd-audio.sock"))
        );
    }

    #[test]
    fn test_manifest_without_companion_section() {
        let toml_str = r#"
name = "clock"
version = "1.0.0"
level = "wasm"
entry = "clock.wasm"
description = "System clock and calendar"
"#;
        let manifest = ModuleManifest::from_toml_str(toml_str).unwrap();
        assert_eq!(manifest.companion_provides(), None);
        assert_eq!(manifest.companion_description(), None);
        assert_eq!(manifest.companion_daemon(), None);
    }

    #[test]
    fn test_manifest_companion_partial_fields() {
        let toml_str = r#"
name = "clipboard"
version = "1.0.0"
level = "wasm"
entry = "module.wasm"

[companion]
daemon = "wyrd-clipboard"
socket = "wyrd-clipboard.sock"
provides = "clipboard"
"#;
        let manifest = ModuleManifest::from_toml_str(toml_str).unwrap();
        assert_eq!(manifest.companion_provides(), Some("clipboard"));
        assert_eq!(manifest.companion_description(), None);
        assert_eq!(
            manifest.companion_daemon(),
            Some(("wyrd-clipboard", "wyrd-clipboard.sock"))
        );
    }

    #[test]
    fn test_manifest_companion_empty_strings_normalized() {
        let toml_str = r#"
name = "test_empty"
version = "1.0.0"
level = "wasm"
entry = "module.wasm"

[companion]
daemon = ""
socket = ""
provides = "   "
description = ""
"#;
        let manifest = ModuleManifest::from_toml_str(toml_str).unwrap();
        assert_eq!(manifest.companion_provides(), None);
        assert_eq!(manifest.companion_description(), None);
        assert_eq!(manifest.companion_daemon(), None);
    }

    #[test]
    fn test_manifest_companion_missing_daemon_fails() {
        let toml_str = r#"
name = "audio"
version = "1.0.0"
level = "wasm"
entry = "module.wasm"

[companion]
socket = "wyrd-audio.sock"
"#;
        assert!(ModuleManifest::from_toml_str(toml_str).is_err());
    }

    #[test]
    fn test_manifest_companion_missing_socket_fails() {
        let toml_str = r#"
name = "audio"
version = "1.0.0"
level = "wasm"
entry = "module.wasm"

[companion]
daemon = "wyrd-audio"
"#;
        assert!(ModuleManifest::from_toml_str(toml_str).is_err());
    }

    #[test]
    fn test_manifest_serde_roundtrip() {
        let toml_str = r#"
name = "tray"
version = "2.0.0"
level = "wasm"
entry = "tray.wasm"
description = "System tray manager"

[companion]
daemon = "wyrd-tray"
socket = "wyrd-tray.sock"
provides = "tray"
description = "StatusNotifierItem SNI watcher"
"#;
        let manifest = ModuleManifest::from_toml_str(toml_str).unwrap();
        let serialized_json = serde_json::to_string(&manifest).unwrap();
        let deserialized: ModuleManifest = serde_json::from_str(&serialized_json).unwrap();
        assert_eq!(manifest, deserialized);
        assert_eq!(deserialized.companion_provides(), Some("tray"));
        assert_eq!(
            deserialized.companion_description(),
            Some("StatusNotifierItem SNI watcher")
        );
        assert_eq!(
            deserialized.companion_daemon(),
            Some(("wyrd-tray", "wyrd-tray.sock"))
        );
    }

    #[test]
    fn test_reconnect_config_from_manifest() {
        let toml_str = r#"
name = "workspaces"
version = "1.0.0"
level = "wasm"
entry = "workspaces.wasm"

[reconnect]
mode = "infinite"
backoff_cap_sec = 30
"#;
        let manifest = ModuleManifest::from_toml_str(toml_str).unwrap();
        assert!(manifest.reconnect.is_some());
        let reconnect = manifest.reconnect.unwrap();
        assert_eq!(reconnect.mode, "infinite");
        assert_eq!(reconnect.backoff_cap_sec, 30);
    }

    #[test]
    fn test_reconnect_config_override_max_attempts() {
        let toml_str = r#"
name = "tray"
version = "1.0.0"
level = "wasm"
entry = "tray.wasm"

[reconnect]
mode = "dormant"
max_attempts = 3
backoff_cap_sec = 15
"#;
        let manifest = ModuleManifest::from_toml_str(toml_str).unwrap();
        let reconnect = manifest.reconnect.unwrap();
        assert_eq!(reconnect.mode, "dormant");
        assert_eq!(reconnect.max_attempts, 3);
        assert_eq!(reconnect.backoff_cap_sec, 15);
    }

    #[test]
    fn test_fs_watch_bare_deprecated_resolves_to_runtime() {
        let toml_str = r#"
name = "legacy"
version = "1.0.0"
level = "wasm"
entry = "legacy.wasm"
capabilities = ["fs:watch"]
"#;
        let manifest = ModuleManifest::from_toml_str(toml_str).unwrap();
        let rt = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| format!("/run/user/{}", current_uid()));
        let runtime_path = format!("{}/test.sock", rt);
        assert!(
            manifest.can_watch_path(&runtime_path),
            "bare fs:watch must allow runtime dir paths"
        );
        assert!(
            !manifest.can_watch_path("/tmp/test.sock"),
            "bare fs:watch must reject /tmp"
        );
        assert!(
            !manifest.can_watch_path("/etc/shadow"),
            "bare fs:watch must reject /etc"
        );
    }

    #[test]
    fn test_watch_path_canonicalizes_existing_parent_only() {
        let temp_dir = tempfile::tempdir().unwrap();
        let valid_parent = temp_dir.path().join("valid_parent");
        std::fs::create_dir(&valid_parent).unwrap();

        // 1. Target file does not exist, but existing parent is within temp_dir
        let non_existent_target = valid_parent.join("daemon.sock");
        let manifest = ModuleManifest {
            capabilities: vec!["fs:watch:tmp".to_string()],
            ..Default::default()
        };
        assert!(
            manifest.can_watch_path(non_existent_target.to_str().unwrap()),
            "non-existent target under valid existing parent must be permitted"
        );

        // 2. Existing parent is a symlink pointing to /etc
        let symlink_parent = temp_dir.path().join("symlink_to_etc");
        let _ = std::os::unix::fs::symlink("/etc", &symlink_parent);
        let escape_target = symlink_parent.join("non_existent.sock");
        assert!(
            !manifest.can_watch_path(escape_target.to_str().unwrap()),
            "non-existent target whose existing ancestor symlinks to /etc must be rejected"
        );
    }
}
