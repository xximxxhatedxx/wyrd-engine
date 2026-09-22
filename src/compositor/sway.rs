use super::types::{deduce_short_layout_name, KeyboardInfo, ToplevelInfo, WorkspaceInfo};
use super::CompositorIntegration;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path as StdPath;
use std::time::Duration;

pub fn sway_socket_path() -> Option<String> {
    if let Ok(sock) = std::env::var("SWAYSOCK") {
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
            if name.starts_with("sway-ipc.") && name.ends_with(".sock") {
                return Some(entry.path().to_string_lossy().to_string());
            }
        }
    }
    None
}

#[derive(Default, Debug)]
pub struct SwayIntegration;

impl SwayIntegration {
    pub fn new() -> Self {
        Self
    }

    pub fn send_ipc_message(&self, msg_type: u32, payload: &[u8]) -> Option<Vec<u8>> {
        let sock = sway_socket_path()?;
        let mut stream = UnixStream::connect(sock).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_millis(250)))
            .ok();
        stream
            .set_write_timeout(Some(Duration::from_millis(250)))
            .ok();

        let mut header = Vec::with_capacity(14);
        header.extend_from_slice(b"i3-ipc");
        header.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
        header.extend_from_slice(&msg_type.to_ne_bytes());
        stream.write_all(&header).ok()?;
        if !payload.is_empty() {
            stream.write_all(payload).ok()?;
        }

        let mut resp_header = [0u8; 14];
        stream.read_exact(&mut resp_header).ok()?;
        let payload_len = u32::from_ne_bytes([
            resp_header[6],
            resp_header[7],
            resp_header[8],
            resp_header[9],
        ]) as usize;
        let mut buf = vec![0u8; payload_len];
        stream.read_exact(&mut buf).ok()?;
        Some(buf)
    }

    fn query_json(&self, msg_type: u32, payload: &[u8]) -> Option<serde_json::Value> {
        let buf = self.send_ipc_message(msg_type, payload)?;
        serde_json::from_slice(&buf).ok()
    }
}

impl CompositorIntegration for SwayIntegration {
    fn name(&self) -> &'static str {
        "sway"
    }

    fn register_keybind(&self, _mods: &str, _key: &str, _action: &str) -> anyhow::Result<()> {
        log::info!(
            "Runtime keybind registration unsupported on Sway. Add to sway config: bindsym ..."
        );
        Ok(())
    }

    fn cursor_position(&self) -> Option<(i32, i32)> {
        None
    }

    fn focused_monitor(&self) -> Option<String> {
        let outputs = self.query_json(4, &[])?;
        for o in outputs.as_array()? {
            if o.get("focused").and_then(|v| v.as_bool()) == Some(true) {
                return o
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);
            }
        }
        None
    }

    fn switch_workspace(&self, id: &str) -> anyhow::Result<()> {
        let cmd = format!("workspace {}", id);
        let resp = self.send_ipc_message(0, cmd.as_bytes());
        if resp.is_some() {
            Ok(())
        } else {
            anyhow::bail!("Failed to switch Sway workspace to {}", id)
        }
    }

    fn list_windows(&self) -> Vec<ToplevelInfo> {
        let mut list = Vec::new();
        let tree = match self.query_json(4, &[]) {
            Some(t) => t,
            None => return list,
        };

        fn collect_windows(node: &serde_json::Value, list: &mut Vec<ToplevelInfo>) {
            let title = node
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let app_id = node
                .get("app_id")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    node.get("window_properties")
                        .and_then(|wp| wp.get("class"))
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or_default()
                .to_string();
            let is_focused = node
                .get("focused")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            let is_window = (!title.is_empty() || !app_id.is_empty())
                && (node.get("pid").is_some()
                    || node.get("app_id").is_some()
                    || node.get("window").is_some());
            if is_window {
                list.push(ToplevelInfo {
                    title,
                    app_id,
                    active: is_focused,
                    maximized: false,
                    minimized: false,
                    fullscreen: node
                        .get("fullscreen_mode")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0)
                        > 0,
                });
            }

            if let Some(nodes) = node.get("nodes").and_then(serde_json::Value::as_array) {
                for child in nodes {
                    collect_windows(child, list);
                }
            }
            if let Some(floating) = node
                .get("floating_nodes")
                .and_then(serde_json::Value::as_array)
            {
                for child in floating {
                    collect_windows(child, list);
                }
            }
        }

        collect_windows(&tree, &mut list);

        // Fallback: if no window had is_window flag set, check focused node directly
        if list.is_empty() {
            fn find_focused(node: &serde_json::Value) -> Option<ToplevelInfo> {
                if node.get("focused").and_then(serde_json::Value::as_bool) == Some(true) {
                    let title = node
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let app_id = node
                        .get("app_id")
                        .and_then(serde_json::Value::as_str)
                        .or_else(|| {
                            node.get("window_properties")
                                .and_then(|wp| wp.get("class"))
                                .and_then(serde_json::Value::as_str)
                        })
                        .unwrap_or_default()
                        .to_string();
                    if !title.is_empty() || !app_id.is_empty() {
                        return Some(ToplevelInfo {
                            title,
                            app_id,
                            active: true,
                            maximized: false,
                            minimized: false,
                            fullscreen: node
                                .get("fullscreen_mode")
                                .and_then(serde_json::Value::as_i64)
                                .unwrap_or(0)
                                > 0,
                        });
                    }
                }
                if let Some(nodes) = node.get("nodes").and_then(serde_json::Value::as_array) {
                    for child in nodes {
                        if let Some(f) = find_focused(child) {
                            return Some(f);
                        }
                    }
                }
                if let Some(floating) = node
                    .get("floating_nodes")
                    .and_then(serde_json::Value::as_array)
                {
                    for child in floating {
                        if let Some(f) = find_focused(child) {
                            return Some(f);
                        }
                    }
                }
                None
            }
            if let Some(focused) = find_focused(&tree) {
                list.push(focused);
            }
        }

        list
    }

    fn active_window(&self) -> Option<ToplevelInfo> {
        self.list_windows().into_iter().find(|w| w.active)
    }

    fn list_workspaces(&self) -> Vec<WorkspaceInfo> {
        let mut res = Vec::new();
        if let Some(serde_json::Value::Array(ws_list)) = self.query_json(1, &[]) {
            for ws in ws_list {
                let name = ws
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let num = ws
                    .get("num")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                let id = if num > 0 {
                    num.to_string()
                } else {
                    name.clone()
                };
                let monitor = ws
                    .get("output")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let active = ws
                    .get("focused")
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
        res
    }

    fn active_workspace(&self) -> Option<WorkspaceInfo> {
        self.list_workspaces().into_iter().find(|ws| ws.active)
    }

    fn keyboard_layout(&self) -> Option<KeyboardInfo> {
        let val = self.query_json(100, &[])?;
        let inputs = val.as_array()?;
        let target_kb = inputs.iter().find(|i| {
            i.get("type").and_then(serde_json::Value::as_str) == Some("keyboard")
                && i.get("xkb_active_layout_name").is_some()
        })?;

        let layout = target_kb
            .get("xkb_active_layout_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("English (US)")
            .to_string();
        let device_name = target_kb
            .get("identifier")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let index = target_kb
            .get("xkb_active_layout_index")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let layouts: Vec<String> = target_kb
            .get("xkb_layout_names")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_else(|| vec![layout.clone()]);
        let short_layouts: Vec<String> = layouts
            .iter()
            .map(|l| deduce_short_layout_name(l))
            .collect();
        let short_name = short_layouts
            .get(index as usize)
            .cloned()
            .unwrap_or_else(|| deduce_short_layout_name(&layout));

        Some(KeyboardInfo {
            layout,
            short_name,
            variant: String::new(),
            index,
            layouts,
            short_layouts,
            device_name,
        })
    }

    fn switch_keyboard_layout(&self, target: &str) -> anyhow::Result<()> {
        let cmd = if target == "next" {
            "input type:keyboard xkb_switch_layout next".to_string()
        } else {
            format!("input type:keyboard xkb_switch_layout {}", target)
        };
        if self.send_ipc_message(0, cmd.as_bytes()).is_some() {
            Ok(())
        } else {
            anyhow::bail!("failed to switch Sway layout to {}", target)
        }
    }

    fn exit(&self) -> anyhow::Result<()> {
        if self.send_ipc_message(0, b"exit").is_some() {
            Ok(())
        } else {
            anyhow::bail!("Failed to exit Sway via IPC")
        }
    }

    fn run_event_loop(&self, on_change: &dyn Fn()) {
        if let Some(sway_sock) = sway_socket_path() {
            loop {
                if let Ok(mut stream) = UnixStream::connect(&sway_sock) {
                    let sub_payload = b"[\"workspace\", \"window\", \"input\"]";
                    let mut header = Vec::with_capacity(14);
                    header.extend_from_slice(b"i3-ipc");
                    header.extend_from_slice(&(sub_payload.len() as u32).to_ne_bytes());
                    header.extend_from_slice(&2u32.to_ne_bytes());
                    if stream.write_all(&header).is_ok() && stream.write_all(sub_payload).is_ok() {
                        let mut resp_header = [0u8; 14];
                        while stream.read_exact(&mut resp_header).is_ok() {
                            let payload_len = u32::from_ne_bytes([
                                resp_header[6],
                                resp_header[7],
                                resp_header[8],
                                resp_header[9],
                            ]) as usize;
                            let mut buf = vec![0u8; payload_len];
                            if stream.read_exact(&mut buf).is_err() {
                                break;
                            }
                            on_change();
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        } else {
            loop {
                std::thread::sleep(Duration::from_millis(1000));
                on_change();
            }
        }
    }
}
