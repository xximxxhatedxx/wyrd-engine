//! Seat / input device discovery (L8 foundation for L5)

use wayland_client::{
    protocol::wl_seat::{self, WlSeat},
    Connection, Dispatch, QueueHandle,
};

use crate::BarState;

pub struct SeatState {
    pub seat: Option<WlSeat>,
    pub has_keyboard: bool,
    pub has_pointer: bool,
    pub has_touch: bool,
}

impl Default for SeatState {
    fn default() -> Self {
        Self::new()
    }
}

impl SeatState {
    pub fn new() -> Self {
        // Placeholder until seat capabilities event arrives
        Self {
            seat: None,
            has_keyboard: false,
            has_pointer: false,
            has_touch: false,
        }
    }
}

impl Dispatch<WlSeat, ()> for BarState {
    fn event(
        state: &mut Self,
        proxy: &WlSeat,
        event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_seat::Event::Capabilities { capabilities } => {
                let Ok(capabilities) = capabilities.into_result() else {
                    return;
                };
                let mut input = match state.input_state.try_write() {
                    Ok(input) => input,
                    Err(_) => return,
                };
                input.keyboard_available = capabilities.contains(wl_seat::Capability::Keyboard);
                input.pointer_available = capabilities.contains(wl_seat::Capability::Pointer);
                input.touch_available = capabilities.contains(wl_seat::Capability::Touch);
                let _ = proxy;
            }
            wl_seat::Event::Name { name } => {
                log::info!("Seat name: {}", name);
            }
            _ => {}
        }
    }
}
