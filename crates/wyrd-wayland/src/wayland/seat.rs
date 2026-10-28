//! Seat / input device discovery (L8 foundation for L5)

use wayland_client::{
    protocol::wl_seat::{self, WlSeat},
    Connection, Dispatch, QueueHandle,
};

fn sync_proxy<T>(
    proxy: &mut Option<T>,
    available: bool,
    create: impl FnOnce() -> T,
    release: impl FnOnce(T),
) {
    if available && proxy.is_none() {
        *proxy = Some(create());
    } else if !available {
        if let Some(proxy) = proxy.take() {
            release(proxy);
        }
    }
}

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

impl<C: 'static, T: 'static> Dispatch<WlSeat, ()> for crate::ShellState<C, T> {
    fn event(
        state: &mut Self,
        proxy: &WlSeat,
        event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_seat::Event::Capabilities { capabilities } => {
                let Ok(capabilities) = capabilities.into_result() else {
                    return;
                };
                let mut input = wyrd_sync::write_unpoisoned(&state.input_state);
                input.keyboard_available = capabilities.contains(wl_seat::Capability::Keyboard);
                input.pointer_available = capabilities.contains(wl_seat::Capability::Pointer);
                input.touch_available = capabilities.contains(wl_seat::Capability::Touch);
                let pointer_available = input.pointer_available;
                let keyboard_available = input.keyboard_available;
                sync_proxy(
                    &mut input.wayland_pointer,
                    pointer_available,
                    || proxy.get_pointer(qhandle, ()),
                    |pointer| pointer.release(),
                );
                sync_proxy(
                    &mut input.wayland_keyboard,
                    keyboard_available,
                    || proxy.get_keyboard(qhandle, ()),
                    |keyboard| keyboard.release(),
                );
                let pointer = input.wayland_pointer.clone();
                let cursor_shape_manager = input.cursor_shape_manager.clone();
                sync_proxy(
                    &mut input.cursor_shape_device,
                    pointer.is_some() && cursor_shape_manager.is_some(),
                    || {
                        cursor_shape_manager
                            .as_ref()
                            .expect("manager must exist when creating cursor shape device")
                            .get_pointer(
                                pointer
                                    .as_ref()
                                    .expect("pointer must exist when creating cursor shape device"),
                                qhandle,
                                (),
                            )
                    },
                    |device| device.destroy(),
                );
                if !pointer_available {
                    input.pointer.pressed_buttons.clear();
                    input.pointer.hovered_widget = None;
                    state.pointer_surface_id = None;
                }
                if !keyboard_available {
                    input.keyboard.repeating_key = None;
                }
            }
            wl_seat::Event::Name { name } => {
                log::info!("Seat name: {}", name);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sync_proxy;

    #[test]
    fn seat_proxy_tracks_capability_removal_and_addition() {
        let mut keyboard = None;
        let mut created = 0;
        let mut released = 0;

        sync_proxy(&mut keyboard, false, || 1, |_| released += 1);
        assert!(keyboard.is_none());
        sync_proxy(
            &mut keyboard,
            true,
            || {
                created += 1;
                7
            },
            |_| released += 1,
        );
        assert_eq!(keyboard, Some(7));
        sync_proxy(&mut keyboard, true, || 8, |_| released += 1);
        assert_eq!(created, 1);
        assert_eq!(keyboard, Some(7));
        sync_proxy(&mut keyboard, false, || 9, |proxy| released += proxy);
        assert!(keyboard.is_none());
        assert_eq!(released, 7);
    }

    #[test]
    fn cursor_shape_device_tracks_pointer_availability() {
        let mut device = None;
        let mut manager_present = true;
        let mut pointer_present = false;
        let mut created = 0;
        let sync_device = |device: &mut Option<()>,
                           manager_present: bool,
                           pointer_present: bool,
                           created: &mut usize| {
            sync_proxy(
                device,
                manager_present && pointer_present,
                || {
                    *created += 1;
                },
                drop,
            );
        };

        sync_device(&mut device, manager_present, pointer_present, &mut created);
        assert!(device.is_none());
        pointer_present = true;
        sync_device(&mut device, manager_present, pointer_present, &mut created);
        assert_eq!(device, Some(()));
        assert_eq!(created, 1);
        manager_present = false;
        sync_device(&mut device, manager_present, pointer_present, &mut created);
        assert!(device.is_none());
    }
}
