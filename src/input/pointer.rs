//! Pointer / mouse handling

use crate::input::InputEvent;
use crate::widgets::{WidgetId, WidgetTree};
use std::collections::HashSet;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

pub struct PointerState {
    pub x: f64,
    pub y: f64,
    pub pressed_buttons: HashSet<u32>,
    pub hovered_widget: Option<WidgetId>,
    pub cursor_shape: CursorShape,
}

impl Dispatch<wayland_client::protocol::wl_pointer::WlPointer, ()> for crate::BarState {
    fn event(
        state: &mut Self,
        _proxy: &wayland_client::protocol::wl_pointer::WlPointer,
        event: wayland_client::protocol::wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let Ok(mut input) = state.input_state.try_write() else {
            return;
        };
        match event {
            wayland_client::protocol::wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_surface_id = Some(surface.id());
                state.pointer_enter_serial = serial;
                input.pointer.x = surface_x;
                input.pointer.y = surface_y;
                state.pending_input_events.push(InputEvent::PointerMove {
                    x: surface_x,
                    y: surface_y,
                });
                state.frame_ready = true;
            }
            wayland_client::protocol::wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                input.pointer.x = surface_x;
                input.pointer.y = surface_y;
                state.pending_input_events.push(InputEvent::PointerMove {
                    x: surface_x,
                    y: surface_y,
                });
                state.frame_ready = true;
            }
            wayland_client::protocol::wl_pointer::Event::Button {
                button,
                state: button_state,
                ..
            } => {
                let pressed = button_state
                    == wayland_client::WEnum::Value(
                        wayland_client::protocol::wl_pointer::ButtonState::Pressed,
                    );
                if pressed {
                    input.pointer.pressed_buttons.insert(button);
                } else {
                    input.pointer.pressed_buttons.remove(&button);
                }
                let widget_id = input.pointer.hovered_widget.unwrap_or_default();
                if let Some(surface_id) = state.pointer_surface_id.clone() {
                    state.pending_input_events.push(InputEvent::PointerButton {
                        surface_id,
                        widget: widget_id,
                        x: input.pointer.x,
                        y: input.pointer.y,
                        button,
                        pressed,
                    });
                }
                log::debug!(
                    "Pointer button {} {} on widget {:?} at ({}, {})",
                    button,
                    if pressed { "pressed" } else { "released" },
                    widget_id,
                    input.pointer.x,
                    input.pointer.y
                );
            }
            wayland_client::protocol::wl_pointer::Event::Leave { surface, .. } => {
                state.pointer_surface_id = None;
                input.pointer.pressed_buttons.clear();
                state.pending_input_events.push(InputEvent::PointerLeave {
                    surface_id: surface.id(),
                    widget: None,
                });
                state.frame_ready = true;
            }
            wayland_client::protocol::wl_pointer::Event::Axis { value, .. } => {
                if let Some(widget) = input.pointer.hovered_widget {
                    if let Some(surface_id) = state.pointer_surface_id.clone() {
                        state.pending_input_events.push(InputEvent::PointerScroll {
                            surface_id,
                            widget,
                            axis_x: 0.0,
                            axis_y: value,
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Default,
    Pointer,
    Text,
    Grab,
}

impl Default for PointerState {
    fn default() -> Self {
        Self::new()
    }
}

impl PointerState {
    pub fn new() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            pressed_buttons: HashSet::new(),
            hovered_widget: None,
            cursor_shape: CursorShape::Default,
        }
    }

    pub fn hit_test(&self, tree: &WidgetTree) -> Option<WidgetId> {
        tree.hit_test(self.x, self.y)
    }

    pub fn dispatch_event(&mut self, tree: &WidgetTree, event: InputEvent) {
        match event {
            InputEvent::PointerMove { x, y } => {
                self.x = x;
                self.y = y;
                let new_hover = self.hit_test(tree);
                if new_hover != self.hovered_widget {
                    self.hovered_widget = new_hover;
                }
            }
            InputEvent::PointerButton {
                surface_id: _,
                widget: _,
                x: _,
                y: _,
                button,
                pressed,
            } => {
                if pressed {
                    self.pressed_buttons.insert(button);
                } else {
                    self.pressed_buttons.remove(&button);
                }
            }
            _ => {}
        }
    }
}
