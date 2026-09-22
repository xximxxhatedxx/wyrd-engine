use super::types::{deduce_short_layout_name, KeyboardInfo, ToplevelInfo, WorkspaceInfo};
use super::CompositorIntegration;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_keyboard, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use xkbcommon::xkb;

pub fn parse_keymap_layouts_from_string(keymap_str: &str) -> Vec<String> {
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let keymap = match xkb::Keymap::new_from_string(
        &context,
        keymap_str.to_string(),
        xkb::KEYMAP_FORMAT_TEXT_V1,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    ) {
        Some(km) => km,
        None => return Vec::new(),
    };

    let num = keymap.num_layouts();
    let mut names = Vec::new();
    for i in 0..num {
        let name = keymap.layout_get_name(i);
        if !name.is_empty() {
            names.push(name.to_string());
        }
    }
    names
}

pub fn build_keyboard_info(layouts: &[String], group: u32) -> KeyboardInfo {
    let index = group;
    let layout = layouts
        .get(index as usize)
        .cloned()
        .unwrap_or_else(|| "English (US)".to_string());
    let short_layouts: Vec<String> = layouts
        .iter()
        .map(|l| deduce_short_layout_name(l))
        .collect();
    let short_name = short_layouts
        .get(index as usize)
        .cloned()
        .unwrap_or_else(|| deduce_short_layout_name(&layout));

    KeyboardInfo {
        layout,
        short_name,
        variant: String::new(),
        index,
        layouts: layouts.to_vec(),
        short_layouts,
        device_name: "wayland".to_string(),
    }
}

struct GenericSeatState {
    keyboard: Option<wl_keyboard::WlKeyboard>,
    layouts: Vec<String>,
    group: u32,
    keyboard_info: Arc<Mutex<Option<KeyboardInfo>>>,
    changed: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for GenericSeatState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for GenericSeatState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities } = event {
            if let Ok(caps) = capabilities.into_result() {
                if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                    state.keyboard = Some(seat.get_keyboard(qh, ()));
                }
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for GenericSeatState {
    fn event(
        state: &mut Self,
        _keyboard: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap {
                format: _,
                fd,
                size,
            } => {
                use std::io::Read;
                let mut file = std::fs::File::from(fd);
                let mut buf = vec![0u8; size as usize];
                if file.read_exact(&mut buf).is_ok() {
                    if let Ok(s) = std::str::from_utf8(&buf) {
                        let trimmed = s.trim_matches(char::from(0));
                        let layouts = parse_keymap_layouts_from_string(trimmed);
                        if !layouts.is_empty() {
                            state.layouts = layouts;
                            let kb = build_keyboard_info(&state.layouts, state.group);
                            *state.keyboard_info.lock().unwrap() = Some(kb);
                            state.changed = true;
                        }
                    }
                }
            }
            wl_keyboard::Event::Modifiers { group, .. } => {
                state.group = group;
                if !state.layouts.is_empty() {
                    let kb = build_keyboard_info(&state.layouts, state.group);
                    *state.keyboard_info.lock().unwrap() = Some(kb);
                    state.changed = true;
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug)]
pub struct NoneIntegration {
    keyboard_info: Arc<Mutex<Option<KeyboardInfo>>>,
}

impl Default for NoneIntegration {
    fn default() -> Self {
        Self::new()
    }
}

impl NoneIntegration {
    pub fn new() -> Self {
        let keyboard_info = Arc::new(Mutex::new(None));
        // Try initial connection to read current keymap if possible
        if let Ok(conn) = Connection::connect_to_env() {
            if let Ok((globals, mut queue)) = registry_queue_init::<GenericSeatState>(&conn) {
                let qh = queue.handle();
                if let Ok(_seat) = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=5, ()) {
                    let mut state = GenericSeatState {
                        keyboard: None,
                        layouts: Vec::new(),
                        group: 0,
                        keyboard_info: keyboard_info.clone(),
                        changed: false,
                    };
                    let _ = queue.roundtrip(&mut state);
                    let _ = queue.roundtrip(&mut state);
                }
            }
        }
        Self { keyboard_info }
    }
}

impl CompositorIntegration for NoneIntegration {
    fn name(&self) -> &'static str {
        "none"
    }

    fn register_keybind(&self, mods: &str, key: &str, action: &str) -> anyhow::Result<()> {
        log::info!(
            "Keybind defined in config: {} + {} -> {} (bind this in your compositor using 'wyrd-shell --action \"{}\"')",
            mods, key, action, action
        );
        Ok(())
    }

    fn cursor_position(&self) -> Option<(i32, i32)> {
        None
    }

    fn focused_monitor(&self) -> Option<String> {
        None
    }

    fn switch_workspace(&self, _id: &str) -> anyhow::Result<()> {
        anyhow::bail!("unsupported by generic compositor backend")
    }

    fn list_windows(&self) -> Vec<ToplevelInfo> {
        Vec::new()
    }

    fn active_window(&self) -> Option<ToplevelInfo> {
        None
    }

    fn list_workspaces(&self) -> Vec<WorkspaceInfo> {
        Vec::new()
    }

    fn active_workspace(&self) -> Option<WorkspaceInfo> {
        None
    }

    fn keyboard_layout(&self) -> Option<KeyboardInfo> {
        self.keyboard_info.lock().ok()?.clone()
    }

    fn switch_keyboard_layout(&self, _target: &str) -> anyhow::Result<()> {
        anyhow::bail!("unsupported by generic compositor backend")
    }

    fn run_event_loop(&self, on_change: &dyn Fn()) {
        loop {
            let conn = match Connection::connect_to_env() {
                Ok(c) => c,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(1000));
                    on_change();
                    continue;
                }
            };
            let (globals, mut queue) = match registry_queue_init::<GenericSeatState>(&conn) {
                Ok(res) => res,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(1000));
                    on_change();
                    continue;
                }
            };
            let qh = queue.handle();
            let _seat = match globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=5, ()) {
                Ok(s) => s,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(1000));
                    on_change();
                    continue;
                }
            };
            let mut state = GenericSeatState {
                keyboard: None,
                layouts: Vec::new(),
                group: 0,
                keyboard_info: self.keyboard_info.clone(),
                changed: false,
            };

            while queue.blocking_dispatch(&mut state).is_ok() {
                if state.changed {
                    state.changed = false;
                    on_change();
                }
            }
            std::thread::sleep(Duration::from_millis(1000));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_keyboard_info_translation() {
        let layouts = vec!["English (US)".to_string(), "Russian".to_string()];

        // Group 0 -> US
        let kb0 = build_keyboard_info(&layouts, 0);
        assert_eq!(kb0.layout, "English (US)");
        assert_eq!(kb0.short_name, "US");
        assert_eq!(kb0.index, 0);
        assert_eq!(kb0.short_layouts, vec!["US", "RU"]);
        assert_eq!(kb0.device_name, "wayland");

        // Group 1 -> RU
        let kb1 = build_keyboard_info(&layouts, 1);
        assert_eq!(kb1.layout, "Russian");
        assert_eq!(kb1.short_name, "RU");
        assert_eq!(kb1.index, 1);
        assert_eq!(kb1.short_layouts, vec!["US", "RU"]);
        assert_eq!(kb1.device_name, "wayland");
    }

    #[test]
    fn test_parse_keymap_from_xkb_names() {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            "us,ru",
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        );
        if let Some(km) = keymap {
            let keymap_str = km.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
            let parsed = parse_keymap_layouts_from_string(&keymap_str);
            assert_eq!(parsed.len(), 2);
            assert!(
                parsed[0].contains("English")
                    || parsed[0].contains("US")
                    || parsed[0].to_lowercase().contains("us")
            );
            assert!(
                parsed[1].contains("Russian")
                    || parsed[1].contains("RU")
                    || parsed[1].to_lowercase().contains("ru")
            );

            // Verify building KeyboardInfo with group 0 and group 1
            let kb0 = build_keyboard_info(&parsed, 0);
            assert_eq!(kb0.index, 0);
            assert_eq!(kb0.short_name, "US");

            let kb1 = build_keyboard_info(&parsed, 1);
            assert_eq!(kb1.index, 1);
            assert_eq!(kb1.short_name, "RU");
        }
    }
}
