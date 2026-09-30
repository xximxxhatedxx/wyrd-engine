use super::types::{
    deduce_short_layout_name, KeyboardInfo, ToplevelInfo, WorkspaceApp, WorkspaceInfo,
};
use super::CompositorIntegration;
use std::io::{BufRead, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub fn hyprland_socket_path() -> Option<String> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        // SAFETY: getuid is a stateless POSIX syscall.
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
        // SAFETY: getuid is a stateless POSIX syscall.
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
        let instance = Self;
        let ns = std::env::var("WYRD_NAMESPACE").unwrap_or_else(|_| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
                .unwrap_or_else(|| env!("CARGO_PKG_NAME").to_string())
        });
        let rule = format!(
            "eval hl.layer_rule({{ name = \"no-anim-{ns}\", match = {{ namespace = \"{ns}\" }}, no_anim = true }})"
        );
        let _ = instance.query_ipc(&rule);
        instance
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
        let mut target_mask: u32 = 0;
        let mut mod_parts = Vec::new();
        for part in mods
            .split(['+', ' ', ','])
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match part.to_ascii_uppercase().as_str() {
                "SHIFT" => target_mask |= 1,
                "CTRL" | "CONTROL" => target_mask |= 4,
                "ALT" | "MOD1" => target_mask |= 8,
                "SUPER" | "WIN" | "LOGO" | "MOD4" => target_mask |= 64,
                _ => {}
            }
            mod_parts.push(part);
        }
        let clean_key = key.trim();
        if let Some(serde_json::Value::Array(existing_binds)) = self.query_json("j/binds") {
            let already_bound = existing_binds.iter().any(|b| {
                let b_mask = b
                    .get("modmask")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(u64::MAX) as u32;
                let b_key = b.get("key").and_then(|v| v.as_str()).unwrap_or("");
                b_mask == target_mask && b_key.eq_ignore_ascii_case(clean_key)
            });
            if already_bound {
                log::info!(
                    "Hyprland keybind {} + {} already active, skipping duplicate registration",
                    mods,
                    clean_key
                );
                return Ok(());
            }
        }

        let normalized_mods = mod_parts.join(" + ");
        let bind_combo = if normalized_mods.is_empty() {
            clean_key.to_string()
        } else {
            format!("{} + {}", normalized_mods, clean_key)
        };
        let bin_path = std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(String::from))
            .unwrap_or_else(|| env!("CARGO_PKG_NAME").to_string());
        let escaped_combo = bind_combo.replace('\\', "\\\\").replace('"', "\\\"");
        let escaped_bin = bin_path.replace('\\', "\\\\").replace('"', "\\\"");
        let escaped_action_sq = action.replace('\'', "'\\''");
        let escaped_action_eval = escaped_action_sq.replace('\\', "\\\\").replace('"', "\\\"");
        let eval_cmd = format!(
            "eval hl.bind(\"{}\", hl.dsp.exec_cmd(\"{} --action '{}'\"))",
            escaped_combo, escaped_bin, escaped_action_eval
        );
        if let Some(resp) = self.query_ipc(&eval_cmd) {
            let text = String::from_utf8_lossy(&resp);
            if text.trim() == "ok" || (!text.contains("error") && !text.contains("unknown")) {
                log::info!(
                    "Registered Hyprland keybind via eval: {} -> {}",
                    bind_combo,
                    action
                );
                return Ok(());
            }
        }
        let legacy_mods = normalized_mods.replace(" + ", " ");
        let cmd = format!(
            "[[BATCH]]keyword bind {},{},exec,{} --action '{}'",
            legacy_mods, clean_key, bin_path, escaped_action_sq
        );
        if let Some(resp) = self.query_ipc(&cmd) {
            let text = String::from_utf8_lossy(&resp);
            if !text.contains("can't work") && !text.contains("error") {
                log::info!(
                    "Registered Hyprland keybind: {} + {} -> {}",
                    legacy_mods,
                    key,
                    action
                );
                return Ok(());
            }
        }
        anyhow::bail!(
            "Failed to register Hyprland keybind via IPC: {} + {}",
            mods,
            key
        )
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
                        .get("fullscreen")
                        .and_then(serde_json::Value::as_i64)
                        .is_some_and(|v| v == 1 || v == 3)
                        || client
                            .get("grouped")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|v| !v.is_empty()),
                    minimized: false,
                    fullscreen: client
                        .get("fullscreen")
                        .map(|v| v.as_bool().unwrap_or(false) || v.as_i64().is_some_and(|n| n >= 2))
                        .unwrap_or(false)
                        || client
                            .get("fullscreenClient")
                            .and_then(serde_json::Value::as_i64)
                            .is_some_and(|n| n >= 2),
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

        let cmd_current = format!("switchxkblayout current {}", cmd_arg);
        if let Some(resp) = self.query_ipc(&cmd_current) {
            let resp_str = String::from_utf8_lossy(&resp);
            let trimmed = resp_str.trim();
            if !trimmed.starts_with("invalid") && !trimmed.starts_with("error") {
                success = true;
            }
        }

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
