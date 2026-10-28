//! Input System: pointer, keyboard, focus, text-input, cursor-shape.

pub mod focus;
pub mod keyboard;
pub mod pointer;

pub use wyrd_graphics::render::scene::WidgetId;

pub struct InputState {
    pub pointer: pointer::PointerState,
    pub keyboard: keyboard::KeyboardState,
    pub wayland_pointer: Option<wayland_client::protocol::wl_pointer::WlPointer>,
    pub wayland_keyboard: Option<wayland_client::protocol::wl_keyboard::WlKeyboard>,
    pub cursor_shape_manager: Option<
        wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
    >,
    pub cursor_shape_device: Option<
        wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1,
    >,
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
            wayland_pointer: None,
            wayland_keyboard: None,
            cursor_shape_manager: None,
            cursor_shape_device: None,
            focus: focus::FocusManager::new(),
            pointer_available: false,
            keyboard_available: false,
            touch_available: false,
            focused_surface_id: None,
        }
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
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
    KeyboardLeave {
        surface_id: wayland_client::backend::ObjectId,
    },
}
