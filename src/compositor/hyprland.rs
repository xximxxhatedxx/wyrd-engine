use super::types::{
    deduce_short_layout_name, KeyboardInfo, ToplevelInfo, WorkspaceApp, WorkspaceInfo,
};
use super::CompositorIntegration;
use std::io::{BufRead, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub fn hyprland_socket_path() -> Option<String> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        // SAFETY: `libc::getuid()` is a POSIX system call that reads the real UID of the calling
        // process. It takes no pointers, mutates no memory, and is always safe to invoke.
        let uid = unsafe { libc::getuid() };
        format!("/run/user/{}", uid)
    });
    if let Ok(sig) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        let path = format!("{}/hypr/{}/.socket.sock", runtime, sig);
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }
    }
    let p = std::path::Path::new(&runtime).join("hypr");
    if let Ok(entries) = std::fs::read_dir(p) {
        for entry in entries.flatten() {
            let sock = entry.path().join(".socket.sock");
            if sock.exists() {
                return Some(sock.to_string_lossy().to_string());
            }
        }
    }
    None
}

pub fn hyprland_event_socket_path() -> Option<String> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        // SAFETY: `libc::getuid()` is a POSIX system call that reads the real UID of the calling
        // process. It takes no pointers, mutates no memory, and is always safe to invoke.
        let uid = unsafe { libc::getuid() };
        format!("/run/user/{}", uid)
    });
    if let Ok(sig) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        let path = format!("{}/hypr/{}/.socket2.sock", runtime, sig);
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }
    }
    let p = std::path::Path::new(&runtime).join("hypr");
    if let Ok(entries) = std::fs::read_dir(p) {
        for entry in entries.flatten() {
            let sock = entry.path().join(".socket2.sock");
            if sock.exists() {
                return Some(sock.to_string_lossy().to_string());
            }
        }
    }
    None
}

#[derive(Default, Debug)]
pub struct HyprlandIntegration;

impl HyprlandIntegration {
    pub fn new() -> Self {
        Self
    }

    pub fn query_ipc(&self, cmd: &str) -> Option<Vec<u8>> {
        let path = hyprland_socket_path()?;
        let mut stream = UnixStream::connect(path).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_millis(250)))
            .ok();
        stream
            .set_write_timeout(Some(Duration::from_millis(250)))
            .ok();
        stream.write_all(cmd.as_bytes()).ok()?;
        let _ = stream.shutdown(std::net::Shutdown::Write);
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).ok()?;
        Some(resp)
    }

    fn query_json(&self, cmd: &str) -> Option<serde_json::Value> {
        let resp = self.query_ipc(cmd)?;
        serde_json::from_slice(&resp).ok()
    }
}

impl CompositorIntegration for HyprlandIntegration {
    fn name(&self) -> &'static str {
        "hyprland"
    }

    fn register_keybind(&self, mods: &str, key: &str, action: &str) -> anyhow::Result<()> {
        let cmd = format!(
            "[[BATCH]]keyword bind {},{},exec,wyrd-shell --action '{}'",
            mods, key, action
        );
        if self.query_ipc(&cmd).is_some() {
            log::info!(
                "Registered Hyprland keybind: {} + {} -> {}",
                mods,
                key,
                action
            );
            Ok(())
        } else {
            anyhow::bail!(
                "Failed to register Hyprland keybind via IPC: {} + {}",
                mods,
                key
            )
        }
    }

    fn cursor_position(&self) -> Option<(i32, i32)> {
        let resp = self.query_ipc("cursorpos")?;
        let s = std::str::from_utf8(&resp).ok()?;
        let (x, y) = s.trim().split_once(',')?;
        Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
    }

    fn focused_monitor(&self) -> Option<String> {
        let monitors = self.query_json("j/monitors")?;
        for m in monitors.as_array()? {
            if m.get("focused").and_then(|v| v.as_bool()) == Some(true) {
                return m
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);
            }
        }
        None
    }

    fn switch_workspace(&self, id: &str) -> anyhow::Result<()> {
        let cmd = format!("dispatch workspace {}", id);
        if self.query_ipc(&cmd).is_some() {
            Ok(())
        } else {
            anyhow::bail!("Failed to switch Hyprland workspace to {}", id)
        }
    }

    fn list_windows(&self) -> Vec<ToplevelInfo> {
        let mut list = Vec::new();
        if let Some(serde_json::Value::Array(clients)) = self.query_json("j/clients") {
            for client in clients {
                let info = ToplevelInfo {
                    title: client
                        .get("title")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    app_id: client
                        .get("class")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    active: client
                        .get("focusHistoryID")
                        .and_then(serde_json::Value::as_i64)
                        == Some(0),
                    maximized: client
                        .get("grouped")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|v| !v.is_empty()),
                    minimized: false,
                    fullscreen: client
                        .get("fullscreen")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                };
                list.push(info);
            }
        }
        list
    }

    fn active_window(&self) -> Option<ToplevelInfo> {
        self.list_windows().into_iter().find(|item| item.active)
    }

    fn list_workspaces(&self) -> Vec<WorkspaceInfo> {
        let mut apps_by_workspace: std::collections::HashMap<i64, Vec<WorkspaceApp>> =
            std::collections::HashMap::new();
        if let Some(serde_json::Value::Array(clients)) = self.query_json("j/clients") {
            for client in clients {
                if let Some(workspace_id) = client
                    .get("workspace")
                    .and_then(|w| w.get("id"))
                    .and_then(serde_json::Value::as_i64)
                {
                    apps_by_workspace
                        .entry(workspace_id)
                        .or_default()
                        .push(WorkspaceApp {
                            app_id: client
                                .get("class")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            title: client
                                .get("title")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        });
                }
            }
        }

        let active_id = self
            .query_json("j/activeworkspace")
            .and_then(|ws| ws.get("id").and_then(serde_json::Value::as_i64));

        let mut workspaces = Vec::new();
        if let Some(serde_json::Value::Array(ws_list)) = self.query_json("j/workspaces") {
            workspaces = ws_list
                .into_iter()
                .filter_map(|workspace| {
                    let numeric_id = workspace.get("id").and_then(serde_json::Value::as_i64)?;
                    let id = numeric_id.to_string();
                    Some(WorkspaceInfo {
                        id,
                        name: workspace
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        monitor: workspace
                            .get("monitor")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        active: active_id == Some(numeric_id),
                        apps: apps_by_workspace.remove(&numeric_id).unwrap_or_default(),
                    })
                })
                .collect();
            workspaces.sort_by(|left, right| {
                let left_id = left.id.parse::<i64>().unwrap_or(i64::MAX);
                let right_id = right.id.parse::<i64>().unwrap_or(i64::MAX);
                left_id.cmp(&right_id).then_with(|| left.id.cmp(&right.id))
            });
        }
        workspaces
    }

    fn active_workspace(&self) -> Option<WorkspaceInfo> {
        self.list_workspaces().into_iter().find(|ws| ws.active)
    }

    fn keyboard_layout(&self) -> Option<KeyboardInfo> {
        let devices = self.query_json("j/devices")?;
        let keyboards = devices.get("keyboards")?.as_array()?;

        let target_kb = keyboards
            .iter()
            .find(|k| k.get("main").and_then(serde_json::Value::as_bool) == Some(true))
            .or_else(|| {
                keyboards.iter().find(|k| {
                    k.get("layout")
                        .and_then(serde_json::Value::as_str)
                        .map(|s| !s.is_empty())
                        .unwrap_or(false)
                })
            })
            .or_else(|| keyboards.first())?;

        let layout = target_kb
            .get("active_keymap")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("English (US)")
            .to_string();
        let device_name = target_kb
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let index = target_kb
            .get("active_layout_index")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let variant = target_kb
            .get("variant")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let layout_rule = target_kb
            .get("layout")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();

        let raw_layouts: Vec<String> = if !layout_rule.is_empty() {
            layout_rule
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            vec![layout.clone()]
        };

        let short_layouts: Vec<String> = raw_layouts
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
            variant,
            index,
            layouts: raw_layouts,
            short_layouts,
            device_name,
        })
    }

    fn switch_keyboard_layout(&self, target: &str) -> anyhow::Result<()> {
        let kb_opt = self.keyboard_layout();
        let cmd_arg = if target == "next" || target == "prev" || target.parse::<u32>().is_ok() {
            target.to_string()
        } else if let Some(ref kb) = kb_opt {
            let target_lower = target.trim().to_lowercase();
            let target_short = deduce_short_layout_name(target);
            let idx = kb
                .layouts
                .iter()
                .position(|l| l.trim().to_lowercase() == target_lower)
                .or_else(|| {
                    kb.short_layouts.iter().position(|s| {
                        s.eq_ignore_ascii_case(&target_short) || s.eq_ignore_ascii_case(target)
                    })
                });
            if let Some(i) = idx {
                i.to_string()
            } else {
                target.to_string()
            }
        } else {
            target.to_string()
        };

        let mut success = false;

        // 1. Switch for 'current'
        let cmd_current = format!("switchxkblayout current {}", cmd_arg);
        if let Some(resp) = self.query_ipc(&cmd_current) {
            let resp_str = String::from_utf8_lossy(&resp);
            let trimmed = resp_str.trim();
            if !trimmed.starts_with("invalid") && !trimmed.starts_with("error") {
                success = true;
            }
        }

        // 2. Also switch specific physical device if detected (e.g. external USB/BT keyboard)
        if let Some(kb) = kb_opt {
            if !kb.device_name.is_empty() && kb.device_name != "current" {
                let cmd_dev = format!("switchxkblayout {} {}", kb.device_name, cmd_arg);
                if let Some(resp) = self.query_ipc(&cmd_dev) {
                    let resp_str = String::from_utf8_lossy(&resp);
                    let trimmed = resp_str.trim();
                    if !trimmed.starts_with("invalid") && !trimmed.starts_with("error") {
                        success = true;
                    }
                }
            }
        }

        if success {
            Ok(())
        } else {
            anyhow::bail!("failed to switch Hyprland layout to {}", target)
        }
    }

    fn exit(&self) -> anyhow::Result<()> {
        if self.query_ipc("dispatch exit").is_some() {
            Ok(())
        } else {
            anyhow::bail!("Failed to exit Hyprland via IPC")
        }
    }

    fn run_event_loop(&self, on_change: &dyn Fn()) {
        if let Some(event_socket) = hyprland_event_socket_path() {
            loop {
                match UnixStream::connect(&event_socket) {
                    Ok(stream) => {
                        let reader = std::io::BufReader::new(stream);
                        for line in reader.lines().map_while(Result::ok) {
                            if line.starts_with("workspace>>")
                                || line.starts_with("focusedmon>>")
                                || line.starts_with("openwindow>>")
                                || line.starts_with("closewindow>>")
                                || line.starts_with("movewindow>>")
                                || line.starts_with("activewindow>>")
                                || line.starts_with("activelayout>>")
                            {
                                on_change();
                            }
                        }
                    }
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
            }
        } else {
            loop {
                std::thread::sleep(Duration::from_millis(1000));
                on_change();
            }
        }
    }
}
