//! Raw Wayland client backend

use anyhow::{Context, Result};
use log::info;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::Arc;
use tokio::io::unix::AsyncFd;
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_compositor, wl_output, wl_seat, wl_shm},
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
};

use super::{
    output::OutputState,
    seat::SeatState,
    surface::{SurfaceBackendKind, SurfaceManagerState},
    ProtocolSupport,
};
use crate::BarState;
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1::Shape, wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;

#[derive(Debug)]
pub struct WaylandFd(pub RawFd);
impl AsRawFd for WaylandFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

pub struct WaylandBackend<C = (), T = ()> {
    pub conn: Connection,
    pub event_queue: EventQueue<BarState<C, T>>,
    pub qh: QueueHandle<BarState<C, T>>,
    pub compositor: wl_compositor::WlCompositor,
    pub shm: wl_shm::WlShm,
    pub seat: wl_seat::WlSeat,
    #[doc(hidden)]
    pub pointer: Option<wayland_client::protocol::wl_pointer::WlPointer>,
    #[doc(hidden)]
    pub keyboard: Option<wayland_client::protocol::wl_keyboard::WlKeyboard>,
    pub xdg_output_manager: Option<ZxdgOutputManagerV1>,
    pub protocol: ProtocolSupport,
    pub outputs: Vec<OutputState>,
    pub seat_state: SeatState,
    pub surface_manager: SurfaceManagerState,
    pub async_fd: AsyncFd<WaylandFd>,
    pub state: Arc<std::sync::Mutex<BarState<C, T>>>,
    pub text_input_manager: Option<wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    pub text_input: Option<wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::ZwpTextInputV3>,
}

impl<C: Default + 'static, T: Default + 'static> WaylandBackend<C, T> {
    pub async fn roundtrip_async(&mut self, state: &mut BarState<C, T>) -> Result<()> {
        let callback = self.conn.display().sync(&self.qh, ());
        let callback_id = callback.id();
        state.pending_roundtrip = Some(callback_id.clone());

        let result = async {
            while state.pending_roundtrip.as_ref() == Some(&callback_id) {
                loop {
                    match self.conn.flush() {
                        Ok(()) => break,
                        Err(wayland_backend::client::WaylandError::Io(error))
                            if error.kind() == std::io::ErrorKind::WouldBlock =>
                        {
                            let mut writable = self.async_fd.writable().await?;
                            writable.clear_ready();
                        }
                        Err(error) => return Err(error.into()),
                    }
                }

                if self.event_queue.dispatch_pending(state)? > 0 {
                    continue;
                }

                let Some(guard) = self.conn.prepare_read() else {
                    self.event_queue.dispatch_pending(state)?;
                    continue;
                };
                let mut readable = self.async_fd.readable().await?;
                match guard.read() {
                    Ok(_) => readable.clear_ready(),
                    Err(wayland_backend::client::WaylandError::Io(error))
                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        readable.clear_ready();
                    }
                    Err(error) => {
                        readable.clear_ready();
                        return Err(error.into());
                    }
                }
                self.event_queue.dispatch_pending(state)?;
            }
            Ok(())
        }
        .await;

        state.pending_roundtrip = None;
        result
    }

    pub fn ensure_xdg_outputs(&self, state: &Arc<std::sync::Mutex<BarState<C, T>>>) {
        let Some(manager) = self.xdg_output_manager.as_ref() else {
            return;
        };
        let outputs = {
            let mut state = wyrd_sync::lock_unpoisoned(state);
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

    /// Logs a formatted diagnostics table of compositor protocol support and active backend.
    pub fn print_protocol_support(&self) {
        let yes_no = |v: bool| if v { "yes" } else { "no" };
        log::info!(
            "Wayland protocol support:\n  ext-layer-shell-v1   : not compiled\n  zwlr-layer-shell-v1  : {}\n  xdg-shell            : {}\n  fractional-scale-v1  : {}\n  viewporter           : {}\n  cursor-shape-v1      : {}\n  text-input-v3        : {}\n  xdg-activation-v1    : {}\nSelected surface backend: {}",
            yes_no(self.protocol.zwlr_layer_shell),
            yes_no(self.protocol.xdg_shell),
            yes_no(self.protocol.fractional_scale),
            yes_no(self.protocol.viewporter),
            yes_no(self.protocol.cursor_shape),
            yes_no(self.protocol.text_input_v3),
            yes_no(self.protocol.xdg_activation),
            self.surface_manager.active_backend,
        );
    }

    /// Connects to the Wayland compositor and initializes the global registry and event queue.
    #[allow(clippy::arc_with_non_send_sync)]
    pub async fn connect() -> Result<(Self, Arc<std::sync::Mutex<BarState<C, T>>>)> {
        let conn = Connection::connect_to_env()
            .context("WAYLAND_DISPLAY not set; cannot connect to compositor")?;

        let (globals, mut event_queue) = registry_queue_init::<BarState<C, T>>(&conn)
            .context("failed to init Wayland registry")?;

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
            wyrd_sync::lock_unpoisoned(&state).outputs.insert(
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
        let mut state_guard = wyrd_sync::lock_unpoisoned(&state);
        event_queue.roundtrip(&mut *state_guard)?;
        drop(state_guard);

        let mut protocol = ProtocolSupport::default();

        let ext_layer_shell = crate::wayland::surface::detect_ext_layer_shell_v1();
        protocol.ext_layer_shell = ext_layer_shell.is_some();

        let zwlr_layer_shell: Option<
            wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1,
        > = globals.bind(&qh, 1..=4, ()).ok();
        protocol.zwlr_layer_shell = zwlr_layer_shell.is_some();

        let xdg_wm_base: Option<wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase> =
            globals.bind(&qh, 1..=6, ()).ok();
        protocol.xdg_shell = xdg_wm_base.is_some();

        let force_env = std::env::var("WYRD_FORCE_BACKEND").ok();
        let selected_backend = select_backend(&protocol, force_env.as_deref())?;
        log::info!("Selected surface backend: {}", selected_backend);
        if selected_backend == SurfaceBackendKind::XdgShell && !protocol.zwlr_layer_shell {
            log::warn!("zwlr-layer-shell-v1 not available; falling back to XdgShell backend");
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
        {
            let state_guard = wyrd_sync::lock_unpoisoned(&state);
            let mut input_state = wyrd_sync::write_unpoisoned(&state_guard.input_state);
            input_state.cursor_shape_manager = cursor_shape_manager;
            if let (Some(manager), Some(pointer)) = (
                input_state.cursor_shape_manager.as_ref(),
                input_state.wayland_pointer.as_ref(),
            ) {
                input_state.cursor_shape_device = Some(manager.get_pointer(pointer, &qh, ()));
            }
        }
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
            pointer: None,
            keyboard: None,
            xdg_output_manager,
            protocol,
            outputs: Vec::new(),
            seat_state: SeatState::new(),
            surface_manager: SurfaceManagerState::with_xdg_shell(
                zwlr_layer_shell,
                xdg_wm_base,
                fractional_scale_manager,
                viewporter,
                selected_backend,
            ),
            async_fd,
            state: state.clone(),
            text_input_manager,
            text_input,
        };

        backend.print_protocol_support();
        backend.ensure_xdg_outputs(&state);
        let _ = backend.conn.flush();
        {
            let mut s = wyrd_sync::lock_unpoisoned(&state);
            let _ = backend.event_queue.roundtrip(&mut *s);
        }

        Ok((backend, state))
    }

    pub fn set_cursor_shape(&self, serial: u32, shape: crate::input::pointer::CursorShape) {
        let state = wyrd_sync::lock_unpoisoned(&self.state);
        let input = wyrd_sync::read_unpoisoned(&state.input_state);
        let Some(device) = input.cursor_shape_device.as_ref() else {
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
        state: Arc<std::sync::Mutex<BarState<C, T>>>,
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
                    let mut state = wyrd_sync::lock_unpoisoned(&state);
                    self.event_queue.dispatch_pending(&mut *state)?;
                }
            }
        }
    }

    pub async fn dispatch_once(
        &mut self,
        state: Arc<std::sync::Mutex<BarState<C, T>>>,
    ) -> Result<()> {
        if let Err(error) = self.conn.flush() {
            match error {
                wayland_backend::client::WaylandError::Io(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock => {}
                other => return Err(other.into()),
            }
        }

        {
            let mut s = wyrd_sync::lock_unpoisoned(&state);
            let count = self.event_queue.dispatch_pending(&mut *s)?;
            if count > 0 {
                return Ok(());
            }
        }

        let Some(guard) = self.conn.prepare_read() else {
            let mut s = wyrd_sync::lock_unpoisoned(&state);
            self.event_queue.dispatch_pending(&mut *s)?;
            return Ok(());
        };

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

        let mut s = wyrd_sync::lock_unpoisoned(&state);
        self.event_queue.dispatch_pending(&mut *s)?;
        drop(s);
        self.ensure_xdg_outputs(&state);
        Ok(())
    }
}

impl<C: 'static, T: 'static> Dispatch<wl_compositor::WlCompositor, ()> for BarState<C, T> {
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

impl<C: 'static, T: 'static> Dispatch<wl_shm::WlShm, ()> for BarState<C, T> {
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

/// Selects the appropriate surface backend based on detected protocol support and `WYRD_FORCE_BACKEND`.
pub fn select_backend(
    protocol: &ProtocolSupport,
    force_env: Option<&str>,
) -> anyhow::Result<SurfaceBackendKind> {
    let mode = force_env
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("auto");

    match mode {
        "auto" => {
            if protocol.zwlr_layer_shell {
                Ok(SurfaceBackendKind::ZwlrLayerShell)
            } else if protocol.xdg_shell {
                Ok(SurfaceBackendKind::XdgShell)
            } else {
                anyhow::bail!(
                    "neither layer-shell nor xdg-shell available from compositor; cannot start surface runtime"
                )
            }
        }

        "ext-layer-shell" | "ext_layer_shell" => {
            anyhow::bail!(
                "ext-layer-shell-v1 is not available in 0.1.0; WYRD_FORCE_BACKEND=ext-layer-shell cannot be used"
            )
        }

        "zwlr-layer-shell" | "zwlr_layer_shell" | "layer-shell" | "layer_shell" => {
            if protocol.zwlr_layer_shell {
                Ok(SurfaceBackendKind::ZwlrLayerShell)
            } else {
                anyhow::bail!(
                    "WYRD_FORCE_BACKEND=zwlr-layer-shell specified but zwlr-layer-shell-v1 is not supported by compositor"
                )
            }
        }
        "xdg-shell" | "xdg_shell" => {
            if protocol.xdg_shell {
                Ok(SurfaceBackendKind::XdgShell)
            } else {
                anyhow::bail!(
                    "WYRD_FORCE_BACKEND=xdg-shell specified but xdg-shell is not supported by compositor"
                )
            }
        }
        other => {
            anyhow::bail!(
                "unknown WYRD_FORCE_BACKEND value: '{}' (valid: auto, ext-layer-shell, zwlr-layer-shell, xdg-shell)",
                other
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn roundtrip_async_does_not_block_tokio_runtime() {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return;
        }

        let (mut first, first_state) = WaylandBackend::<(), ()>::connect()
            .await
            .expect("first Wayland connection should initialize");
        let (mut second, second_state) = WaylandBackend::<(), ()>::connect()
            .await
            .expect("second Wayland connection should initialize");
        let mut first_state = wyrd_sync::lock_unpoisoned(&first_state).clone();
        let mut second_state = wyrd_sync::lock_unpoisoned(&second_state).clone();

        let (first_result, second_result, _) = tokio::join!(
            first.roundtrip_async(&mut first_state),
            second.roundtrip_async(&mut second_state),
            tokio::time::sleep(std::time::Duration::from_millis(1)),
        );
        first_result.expect("first async roundtrip should complete");
        second_result.expect("second async roundtrip should complete");
    }
}
