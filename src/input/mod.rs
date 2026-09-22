//! Input System: pointer, keyboard, focus, text-input, cursor-shape.

pub mod focus;
pub mod keyboard;
pub mod pointer;

use crate::widgets::WidgetId;

/// Global input state.
pub struct InputState {
    pub pointer: pointer::PointerState,
    pub keyboard: keyboard::KeyboardState,
    pub focus: focus::FocusManager,
    pub pointer_available: bool,
    pub keyboard_available: bool,
    pub touch_available: bool,
    pub focused_surface_id: Option<wayland_client::backend::ObjectId>,
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

impl InputState {
    pub fn new() -> Self {
        Self {
            pointer: pointer::PointerState::new(),
            keyboard: keyboard::KeyboardState::new(),
            focus: focus::FocusManager::new(),
            pointer_available: false,
            keyboard_available: false,
            touch_available: false,
            focused_surface_id: None,
        }
    }
}

/// Unified input event for widget dispatch.
#[derive(Debug, Clone)]
pub enum InputEvent {
    PointerMove {
        x: f64,
        y: f64,
    },
    PointerEnter {
        widget: WidgetId,
    },
    PointerLeave {
        surface_id: wayland_client::backend::ObjectId,
        widget: Option<WidgetId>,
    },
    PointerButton {
        surface_id: wayland_client::backend::ObjectId,
        widget: WidgetId,
        x: f64,
        y: f64,
        button: u32,
        pressed: bool,
    },
    PointerScroll {
        surface_id: wayland_client::backend::ObjectId,
        widget: WidgetId,
        axis_x: f64,
        axis_y: f64,
    },
    KeyPress {
        keysym: u32,
        utf8: Option<String>,
    },
    KeyRelease {
        keysym: u32,
    },
}
