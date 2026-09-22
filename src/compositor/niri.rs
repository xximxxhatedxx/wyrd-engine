use super::types::{deduce_short_layout_name, KeyboardInfo, ToplevelInfo, WorkspaceInfo};
use super::CompositorIntegration;
use std::io::{BufRead, Write};
use std::os::unix::net::UnixStream;
use std::path::Path as StdPath;
use std::time::Duration;

pub fn niri_socket_path() -> Option<String> {
    if let Ok(sock) = std::env::var("NIRI_SOCKET") {
        if StdPath::new(&sock).exists() {
            return Some(sock);
        }
    }
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        // SAFETY: `libc::getuid()` is a POSIX system call that reads the real UID of the calling
        // process. It takes no pointers, mutates no memory, and is always safe to invoke.
        let uid = unsafe { libc::getuid() };
        format!("/run/user/{uid}")
    });
    if let Ok(entries) = std::fs::read_dir(&runtime) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("niri") && name.ends_with(".sock") {
                return Some(entry.path().to_string_lossy().to_string());
            }
        }
    }
    None
}

#[derive(Default, Debug)]
pub struct NiriIntegration;

impl NiriIntegration {
    pub fn new() -> Self {
        Self
    }

    pub fn query_ipc(&self, req: &str) -> Option<serde_json::Value> {
        let sock_path = niri_socket_path()?;
        let mut stream = UnixStream::connect(sock_path).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_millis(250)))
            .ok();
        stream
            .set_write_timeout(Some(Duration::from_millis(250)))
            .ok();
        stream.write_all(req.as_bytes()).ok()?;
        stream.write_all(b"\n").ok()?;
        let mut reader = std::io::BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        serde_json::from_str(&line).ok()
    }
}

impl CompositorIntegration for NiriIntegration {
    fn name(&self) -> &'static str {
        "niri"
    }

    fn register_keybind(&self, _mods: &str, _key: &str, _action: &str) -> anyhow::Result<()> {
        log::info!(
            "Runtime keybind registration unsupported on Niri. Add to niri config: binds {{ ... }}"
        );
        Ok(())
    }

    fn cursor_position(&self) -> Option<(i32, i32)> {
        None
    }

    fn focused_monitor(&self) -> Option<String> {
        let val = self.query_ipc("\"Outputs\"")?;
        if let Some(map) = val.get("Ok").and_then(|v| v.as_object()) {
            for (name, output) in map {
                if output.get("is_focused").and_then(|v| v.as_bool()) == Some(true) {
                    return Some(name.clone());
                }
            }
        }
        None
    }

    fn switch_workspace(&self, id: &str) -> anyhow::Result<()> {
        let cmd = format!(
            "{{\"Action\": {{\"FocusWorkspace\": {{\"reference\": {{\"Name\": \"{}\"}}}}}}}}",
            id
        );
        if let Some(resp) = self.query_ipc(&cmd) {
            if resp.get("Ok").is_some() {
                return Ok(());
            }
        }
        log::debug!("Niri workspace switch attempted for {}", id);
        Ok(())
    }

    fn list_windows(&self) -> Vec<ToplevelInfo> {
        let mut res = Vec::new();
        if let Some(resp) = self.query_ipc("\"Windows\"") {
            if let Some(arr) = resp.get("Ok").and_then(serde_json::Value::as_array) {
                for win in arr {
                    let title = win
                        .get("title")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let app_id = win
                        .get("app_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let is_focused = win
                        .get("is_focused")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    res.push(ToplevelInfo {
                        title,
                        app_id,
                        active: is_focused,
                        maximized: false,
                        minimized: false,
                        fullscreen: false,
                    });
                }
            }
        }
        res
    }

    fn active_window(&self) -> Option<ToplevelInfo> {
        self.list_windows().into_iter().find(|w| w.active)
    }

    fn list_workspaces(&self) -> Vec<WorkspaceInfo> {
        let mut res = Vec::new();
        if let Some(resp) = self.query_ipc("\"Workspaces\"") {
            if let Some(arr) = resp.get("Ok").and_then(serde_json::Value::as_array) {
                for ws in arr {
                    let id = ws
                        .get("id")
                        .and_then(serde_json::Value::as_i64)
                        .map(|i| i.to_string())
                        .unwrap_or_default();
                    let name = ws
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&id)
                        .to_string();
                    let monitor = ws
                        .get("output")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let active = ws
                        .get("is_active")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    res.push(WorkspaceInfo {
                        id,
                        name,
                        monitor,
                        active,
                        apps: Vec::new(),
                    });
                }
            }
        }
        res
    }

    fn active_workspace(&self) -> Option<WorkspaceInfo> {
        self.list_workspaces().into_iter().find(|ws| ws.active)
    }

    fn keyboard_layout(&self) -> Option<KeyboardInfo> {
        let resp = self.query_ipc("\"KeyboardLayouts\"")?;
        let kl = resp.get("Ok")?;
        let names: Vec<String> = kl
            .get("names")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let index = kl
            .get("current_idx")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let layout = names
            .get(index as usize)
            .cloned()
            .unwrap_or_else(|| "English (US)".to_string());
        let short_layouts: Vec<String> =
            names.iter().map(|n| deduce_short_layout_name(n)).collect();
        let short_name = short_layouts
            .get(index as usize)
            .cloned()
            .unwrap_or_else(|| deduce_short_layout_name(&layout));
        Some(KeyboardInfo {
            layout,
            short_name,
            variant: String::new(),
            index,
            layouts: names,
            short_layouts,
            device_name: "niri".to_string(),
        })
    }

    fn switch_keyboard_layout(&self, target: &str) -> anyhow::Result<()> {
        let cmd = if target == "next" {
            "{\"Action\": \"SwitchLayoutNext\"}".to_string()
        } else if let Ok(idx) = target.parse::<u32>() {
            format!(
                "{{\"Action\": {{\"SwitchLayout\": {{\"Index\": {}}}}}}}",
                idx
            )
        } else {
            format!(
                "{{\"Action\": {{\"SwitchLayout\": {{\"Name\": \"{}\"}}}}}}",
                target
            )
        };
        if let Some(resp) = self.query_ipc(&cmd) {
            if resp.get("Ok").is_some() {
                return Ok(());
            }
        }
        anyhow::bail!("failed to switch Niri layout to {}", target)
    }

    fn exit(&self) -> anyhow::Result<()> {
        let cmd = "{\"Action\": {\"Quit\": {\"skip_confirmation\": true}}}";
        if let Some(resp) = self.query_ipc(cmd) {
            if resp.get("Ok").is_some() {
                return Ok(());
            }
        }
        if let Some(resp) = self.query_ipc("{\"Action\": \"Quit\"}") {
            if resp.get("Ok").is_some() {
                return Ok(());
            }
        }
        anyhow::bail!("failed to exit Niri via IPC")
    }

    fn run_event_loop(&self, on_change: &dyn Fn()) {
        loop {
            std::thread::sleep(Duration::from_millis(1000));
            on_change();
        }
    }
}
