//! Keyboard handling via xkbcommon

use wayland_client::{Connection, Dispatch, QueueHandle};
use xkbcommon::xkb;

#[derive(Debug, Clone)]
pub struct RepeatInfo {
    pub key: u32,
    pub keysym: u32,
    pub utf8: Option<String>,
    pub next_repeat: std::time::Instant,
    pub interval: std::time::Duration,
}

pub struct KeyboardState {
    pub context: xkb::Context,
    pub keymap: Option<xkb::Keymap>,
    pub state: Option<xkb::State>,
    pub modifiers: Modifiers,
    pub repeat_rate: u32,
    pub repeat_delay: u32,
    pub repeating_key: Option<RepeatInfo>,
}

fn is_modifier_keysym(keysym: u32) -> bool {
    matches!(
        keysym,
        0xffe1
            | 0xffe2
            | 0xffe3
            | 0xffe4
            | 0xffe5
            | 0xffe7
            | 0xffe8
            | 0xffe9
            | 0xffea
            | 0xffeb
            | 0xffec
            | 0xffed
            | 0xffee
            | 0xfe03
    )
}

impl<C: 'static, T: 'static> Dispatch<wayland_client::protocol::wl_keyboard::WlKeyboard, ()>
    for crate::ShellState<C, T>
{
    fn event(
        state: &mut Self,
        _proxy: &wayland_client::protocol::wl_keyboard::WlKeyboard,
        event: wayland_client::protocol::wl_keyboard::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let mut input = wyrd_sync::write_unpoisoned(&state.input_state);
        match event {
            wayland_client::protocol::wl_keyboard::Event::Enter { surface, .. } => {
                use wayland_client::Proxy;
                input.focused_surface_id = Some(surface.id());
                state.frame_ready = true;
            }
            wayland_client::protocol::wl_keyboard::Event::Leave { surface, .. } => {
                use wayland_client::Proxy;
                let sid = surface.id();
                if input.focused_surface_id.as_ref() == Some(&sid) {
                    input.focused_surface_id = None;
                }
                input.keyboard.repeating_key = None;
                state
                    .pending_input_events
                    .push(super::InputEvent::KeyboardLeave { surface_id: sid });
                state.frame_ready = true;
            }
            wayland_client::protocol::wl_keyboard::Event::RepeatInfo { rate, delay } => {
                input.keyboard.repeat_rate = rate.max(0) as u32;
                input.keyboard.repeat_delay = delay.max(0) as u32;
            }
            wayland_client::protocol::wl_keyboard::Event::Keymap { format, fd, size } => {
                if let Err(error) = input.keyboard.load_keymap(u32::from(format), fd, size) {
                    log::warn!("Failed to load keyboard keymap: {error}");
                }
            }
            wayland_client::protocol::wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                input
                    .keyboard
                    .update_modifiers(mods_depressed, mods_latched, mods_locked, group);
            }
            wayland_client::protocol::wl_keyboard::Event::Key {
                key,
                state: key_state,
                ..
            } => {
                let pressed = key_state
                    == wayland_client::WEnum::Value(
                        wayland_client::protocol::wl_keyboard::KeyState::Pressed,
                    );
                if let Some((keysym, utf8)) = input.keyboard.process_key(key, pressed) {
                    input.focus.dispatch(keysym, utf8.clone(), pressed);
                    let input_evt = if pressed {
                        if input.keyboard.repeat_rate > 0 && !is_modifier_keysym(keysym) {
                            let interval_micros = (1_000_000
                                / (input.keyboard.repeat_rate.max(1) as u64))
                                .max(10_000);
                            input.keyboard.repeating_key = Some(RepeatInfo {
                                key,
                                keysym,
                                utf8: (!utf8.is_empty()).then_some(utf8.clone()),
                                next_repeat: std::time::Instant::now()
                                    + std::time::Duration::from_millis(
                                        input.keyboard.repeat_delay as u64,
                                    ),
                                interval: std::time::Duration::from_micros(interval_micros),
                            });
                        }
                        crate::input::InputEvent::KeyPress {
                            keysym,
                            utf8: (!utf8.is_empty()).then_some(utf8),
                        }
                    } else {
                        if let Some(ref rep) = input.keyboard.repeating_key {
                            if rep.key == key {
                                input.keyboard.repeating_key = None;
                            }
                        }
                        crate::input::InputEvent::KeyRelease { keysym }
                    };
                    state.pending_input_events.push(input_evt);
                    state.frame_ready = true;
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Default for KeyboardState {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyboardState {
    pub fn new() -> Self {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        Self {
            context,
            keymap: None,
            state: None,
            modifiers: Modifiers::default(),
            repeat_rate: 25,
            repeat_delay: 400,
            repeating_key: None,
        }
    }

    pub fn load_keymap(
        &mut self,
        format: u32,
        fd: std::os::fd::OwnedFd,
        size: u32,
    ) -> Result<(), String> {
        if format != xkb::KEYMAP_FORMAT_TEXT_V1 {
            return Err(format!("unsupported keymap format: {format}"));
        }

        // SAFETY: `fd` is a valid `OwnedFd` passed from the Wayland compositor via `wl_keyboard.keymap`.
        // `xkb::Keymap::new_from_fd` takes ownership and safely mmaps the keymap file descriptor within `size` bytes.
        let map = unsafe {
            xkb::Keymap::new_from_fd(
                &self.context,
                fd,
                size as usize,
                xkb::KEYMAP_FORMAT_TEXT_V1,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
        };
        match map {
            Ok(Some(km)) => {
                self.state = Some(xkb::State::new(&km));
                self.keymap = Some(km);
                Ok(())
            }
            Ok(None) => Err("keymap parser returned no keymap".to_string()),
            Err(error) => Err(format!("failed to parse keymap: {error:?}")),
        }
    }

    pub fn process_key(&mut self, keycode: u32, pressed: bool) -> Option<(u32, String)> {
        let state = self.state.as_mut()?;
        let keycode = keycode.saturating_add(8).into(); // wl_keyboard keycode offset

        let keysym = state.key_get_one_sym(keycode);
        let utf8 = state.key_get_utf8(keycode);
        state.update_key(
            keycode,
            if pressed {
                xkb::KeyDirection::Down
            } else {
                xkb::KeyDirection::Up
            },
        );
        Some((keysym.raw(), utf8))
    }

    pub fn update_modifiers(
        &mut self,
        mods_depressed: u32,
        mods_latched: u32,
        mods_locked: u32,
        group: u32,
    ) {
        if let Some(state) = self.state.as_mut() {
            state.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
            let names = [
                ("Shift", &mut self.modifiers.shift),
                ("Control", &mut self.modifiers.ctrl),
                ("Mod1", &mut self.modifiers.alt),
                ("Mod4", &mut self.modifiers.logo),
            ];
            for (name, active) in names {
                *active = state.mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE);
            }
        }
    }

    pub fn poll_repeat(&mut self, now: std::time::Instant) -> Option<(u32, Option<String>)> {
        if let Some(ref mut rep) = self.repeating_key {
            if now >= rep.next_repeat {
                rep.next_repeat += rep.interval;
                if rep.next_repeat < now {
                    rep.next_repeat = now + rep.interval;
                }
                return Some((rep.keysym, rep.utf8.clone()));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::fd::{FromRawFd, IntoRawFd};
    use std::os::unix::io::OwnedFd;

    use super::KeyboardState;
    use xkbcommon::xkb;

    fn keymap_fd() -> (OwnedFd, usize) {
        let text = r#"xkb_keymap {
  xkb_keycodes {
    minimum = 8;
    maximum = 255;
    <A> = 30;
  };
  xkb_types {
    type "complete" {};
  };
  xkb_compat {};
  xkb_symbols { key <A> { [ a ] }; };
};"#;
        // SAFETY: `memfd_create` is called with a valid C-string literal and raw_fd is verified non-negative before wrapping into `OwnedFd`.
        let raw_fd = unsafe { libc::memfd_create(c"wyrd-keymap-test".as_ptr(), 0) };
        assert!(raw_fd >= 0);
        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        let mut file = std::fs::File::from(fd);
        file.write_all(text.as_bytes())
            .expect("valid keymap should be written");
        let size = text.len();
        // SAFETY: `into_raw_fd` transfers ownership of a valid descriptor to `OwnedFd::from_raw_fd`.
        (unsafe { OwnedFd::from_raw_fd(file.into_raw_fd()) }, size)
    }

    #[test]
    fn rejects_unsupported_keymap_format() {
        let mut keyboard = KeyboardState::new();
        // SAFETY: `memfd_create` is called with a valid C-string literal and invalid fd is checked.
        let invalid = unsafe { libc::memfd_create(c"wyrd-invalid-keymap".as_ptr(), 0) };
        assert!(invalid >= 0);
        let fd = unsafe { OwnedFd::from_raw_fd(invalid) };

        let error = keyboard
            .load_keymap(xkb::KEYMAP_FORMAT_TEXT_V1 + 1, fd, 0)
            .expect_err("unsupported format must be rejected");
        assert!(error.contains("unsupported keymap format"));
    }

    #[test]
    #[ignore = "requires full xkb-data; minimal keymap rejected by some xkbcommon builds on CI"]
    fn loads_valid_keymap_and_initializes_state() {
        let mut keyboard = KeyboardState::new();
        let (fd, size) = keymap_fd();

        keyboard
            .load_keymap(xkb::KEYMAP_FORMAT_TEXT_V1, fd, size as u32)
            .expect("valid keymap should load");

        assert!(keyboard.keymap.is_some());
        assert!(keyboard.state.is_some());
    }
}
