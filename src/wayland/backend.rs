//! Raw Wayland client backend

use anyhow::{Context, Result};
use log::info;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::Arc;
use tokio::io::unix::AsyncFd;
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_compositor, wl_output, wl_seat, wl_shm},
    Connection, Dispatch, EventQueue, QueueHandle,
};

use super::{output::OutputState, seat::SeatState, surface::SurfaceManagerState, ProtocolSupport};
use crate::BarState;
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
    wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;

#[derive(Debug)]
pub struct WaylandFd(pub RawFd);
impl AsRawFd for WaylandFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

/// Main Wayland connection handle.
pub struct WaylandBackend {
    pub conn: Connection,
    pub event_queue: EventQueue<BarState>,
    pub qh: QueueHandle<BarState>,
    pub compositor: wl_compositor::WlCompositor,
    pub shm: wl_shm::WlShm,
    pub seat: wl_seat::WlSeat,
    pub pointer: wayland_client::protocol::wl_pointer::WlPointer,
    pub cursor_shape_device: Option<WpCursorShapeDeviceV1>,
    pub keyboard: wayland_client::protocol::wl_keyboard::WlKeyboard,
    pub xdg_output_manager: Option<ZxdgOutputManagerV1>,
    pub protocol: ProtocolSupport,
    pub outputs: Vec<OutputState>,
    pub seat_state: SeatState,
    pub surface_manager: SurfaceManagerState,
    pub async_fd: AsyncFd<WaylandFd>,
    pub text_input_manager: Option<wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    pub text_input: Option<wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::ZwpTextInputV3>,
}

impl WaylandBackend {
    pub fn ensure_xdg_outputs(&self, state: &Arc<std::sync::Mutex<BarState>>) {
        let Some(manager) = self.xdg_output_manager.as_ref() else {
            return;
        };
        let outputs = {
            let mut state = state.lock().unwrap();
            let candidates = state
                .outputs
                .values()
                .map(|output| (output.global_name, output.output.clone()))
                .collect::<Vec<_>>();
            candidates
                .into_iter()
                .filter_map(|(global_name, output)| {
                    if state.xdg_output_bound.insert(global_name) {
                        Some((global_name, output))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        for (global_name, output) in outputs {
            manager.get_xdg_output(&output, &self.qh, crate::OutputUserData { global_name });
        }
    }

    #[allow(clippy::arc_with_non_send_sync)]
    pub async fn connect() -> Result<(Self, Arc<std::sync::Mutex<BarState>>)> {
        let conn = Connection::connect_to_env()
            .context("WAYLAND_DISPLAY not set; cannot connect to compositor")?;

        let (globals, event_queue) =
            registry_queue_init::<BarState>(&conn).context("failed to init Wayland registry")?;

        let qh = event_queue.handle();
        let state = Arc::new(std::sync::Mutex::new(BarState::new()));

        for global in globals
            .contents()
            .clone_list()
            .into_iter()
            .filter(|global| global.interface == "wl_output")
        {
            let output = globals.registry().bind::<wl_output::WlOutput, _, _>(
                global.name,
                global.version.min(4),
                &qh,
                crate::OutputUserData {
                    global_name: global.name,
                },
            );
            state.lock().unwrap().outputs.insert(
                global.name,
                crate::OutputRuntime {
                    global_name: global.name,
                    output,
                    name: format!("output-{}", global.name),
                    logical_x: 0,
                    logical_y: 0,
                    logical_width: 0,
                    logical_height: 0,
                    scale: 1,
                    refresh_rate_mhz: 0,
                    current_mode: None,
                },
            );
        }

        let compositor: wl_compositor::WlCompositor = globals
            .bind(&qh, 1..=6, ())
            .context("wl_compositor not available")?;
        let shm: wl_shm::WlShm = globals
            .bind(&qh, 1..=2, ())
            .context("wl_shm not available")?;
        let seat: wl_seat::WlSeat = globals
            .bind(&qh, 1..=9, ())
            .context("wl_seat not available")?;
        let pointer = seat.get_pointer(&qh, ());
        let keyboard = seat.get_keyboard(&qh, ());

        let mut protocol = ProtocolSupport::default();

        let zwlr_layer_shell: Option<
            wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1,
        > = globals.bind(&qh, 1..=4, ()).ok();
        protocol.zwlr_layer_shell = zwlr_layer_shell.is_some();

        if !protocol.zwlr_layer_shell {
            anyhow::bail!("zwlr-layer-shell-v1 not available from compositor; bar cannot start");
        }

        protocol.fractional_scale = globals.bind::<wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, _, _>(&qh, 1..=1, ()).is_ok();
        protocol.viewporter = globals
            .bind::<wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter, _, _>(
                &qh,
                1..=1,
                (),
            )
            .is_ok();
        let cursor_shape_manager = globals
            .bind::<WpCursorShapeManagerV1, _, _>(&qh, 1..=2, ())
            .ok();
        protocol.cursor_shape = cursor_shape_manager.is_some();
        let cursor_shape_device = cursor_shape_manager
            .as_ref()
            .map(|manager| manager.get_pointer(&pointer, &qh, ()));
        let text_input_manager: Option<wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3> = globals.bind(&qh, 1..=1, ()).ok();
        protocol.text_input_v3 = text_input_manager.is_some();
        let text_input = text_input_manager
            .as_ref()
            .map(|mgr| mgr.get_text_input(&seat, &qh, ()));

        protocol.xdg_activation = globals.bind::<wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1, _, _>(&qh, 1..=1, ()).is_ok();
        let xdg_output_manager = globals.bind(&qh, 1..=3, ()).ok();
        let fractional_scale_manager = globals
            .bind::<wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, _, _>(&qh, 1..=1, ())
            .ok();
        let viewporter = globals
            .bind::<wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter, _, _>(
                &qh,
                1..=1,
                (),
            )
            .ok();

        info!("Wayland protocols: {:?}", protocol);

        let fd = WaylandFd(conn.backend().poll_fd().as_raw_fd());
        let async_fd = AsyncFd::new(fd)?;

        let mut backend = Self {
            conn,
            event_queue,
            qh,
            compositor,
            shm,
            seat,
            pointer,
            keyboard,
            cursor_shape_device,
            xdg_output_manager,
            protocol,
            outputs: Vec::new(),
            seat_state: SeatState::new(),
            surface_manager: SurfaceManagerState::new(
                zwlr_layer_shell,
                fractional_scale_manager,
                viewporter,
            ),
            async_fd,
            text_input_manager,
            text_input,
        };

        backend.ensure_xdg_outputs(&state);
        let _ = backend.conn.flush();
        {
            let mut s = state.lock().unwrap();
            let _ = backend.event_queue.roundtrip(&mut *s);
        }

        Ok((backend, state))
    }

    pub fn set_cursor_shape(&self, serial: u32, shape: crate::input::pointer::CursorShape) {
        let Some(device) = self.cursor_shape_device.as_ref() else {
            return;
        };
        let shape = match shape {
            crate::input::pointer::CursorShape::Text => Shape::Text,
            crate::input::pointer::CursorShape::Pointer => Shape::Pointer,
            crate::input::pointer::CursorShape::Grab => Shape::Grab,
            crate::input::pointer::CursorShape::Default => Shape::Default,
        };
        device.set_shape(serial, shape);
    }

    pub fn enable_text_input(&self) {
        if let Some(ti) = &self.text_input {
            ti.enable();
            ti.commit();
        }
    }

    pub fn disable_text_input(&self) {
        if let Some(ti) = &self.text_input {
            ti.disable();
            ti.commit();
        }
    }

    pub async fn run_dispatch_loop(
        &mut self,
        state: Arc<std::sync::Mutex<BarState>>,
    ) -> Result<()> {
        loop {
            if let Err(e) = self.conn.flush() {
                match e {
                    wayland_backend::client::WaylandError::Io(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    other => return Err(other.into()),
                }
            }

            tokio::select! {
                readable = self.async_fd.readable() => {
                    let mut readable = readable?;
                    if let Some(guard) = self.conn.prepare_read() {
                        if let Err(e) = guard.read() {
                            readable.clear_ready();
                            return Err(e.into());
                        }
                    }
                    readable.clear_ready();
                    let mut state = state.lock().unwrap();
                    self.event_queue.dispatch_pending(&mut *state)?;
                }
            }
        }
    }

    pub async fn dispatch_once(&mut self, state: Arc<std::sync::Mutex<BarState>>) -> Result<()> {
        if let Err(error) = self.conn.flush() {
            match error {
                wayland_backend::client::WaylandError::Io(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock => {}
                other => return Err(other.into()),
            }
        }

        // 1. Dispatch any already pending events first
        {
            let mut s = state.lock().unwrap();
            let count = self.event_queue.dispatch_pending(&mut *s)?;
            if count > 0 {
                return Ok(());
            }
        }

        // 2. Prepare read from connection
        let Some(guard) = self.conn.prepare_read() else {
            let mut s = state.lock().unwrap();
            self.event_queue.dispatch_pending(&mut *s)?;
            return Ok(());
        };

        // 3. Await socket readability on persistent AsyncFd
        let mut readable = self.async_fd.readable().await?;
        match guard.read() {
            Ok(_) => {
                readable.clear_ready();
            }
            Err(wayland_backend::client::WaylandError::Io(err))
                if err.kind() == std::io::ErrorKind::WouldBlock =>
            {
                readable.clear_ready();
            }
            Err(err) => {
                readable.clear_ready();
                return Err(err.into());
            }
        }

        // 4. Dispatch the new events
        let mut s = state.lock().unwrap();
        self.event_queue.dispatch_pending(&mut *s)?;
        drop(s);
        self.ensure_xdg_outputs(&state);
        Ok(())
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for BarState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_compositor::WlCompositor,
        _event: wl_compositor::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm::WlShm, ()> for BarState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_shm::WlShm,
        _event: wl_shm::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}
