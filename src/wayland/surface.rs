//! Layer-shell surface management

use std::collections::HashSet;
use std::ffi::CString;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use wayland_client::{
    protocol::{wl_output::WlOutput, wl_shm, wl_surface::WlSurface},
    Connection, Proxy, QueueHandle,
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::WpFractionalScaleV1,
    },
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::ZwlrLayerShellV1,
    zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
};

use crate::render::damage::DamageTracker;
use crate::ShellState;
use tiny_skia::Pixmap;

/// Surface type per §6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceType {
    Bar,
    Panel,
    Popup,
    Background,
}

/// A managed shell surface.
pub struct ShellSurface {
    pub config_name: String,
    pub dynamic: bool,
    pub id: Option<crate::widgets::WidgetId>,
    pub surface: WlSurface,
    pub layer_surface: Option<ZwlrLayerSurfaceV1>,
    pub ty: SurfaceType,
    pub output: Option<WlOutput>,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub dirty: bool,
    pub pending_frame: bool,
    pub buffer: Option<wayland_client::protocol::wl_buffer::WlBuffer>,
    pub frame_callback: Option<wayland_client::protocol::wl_callback::WlCallback>,
    pub shm_buffers: Vec<ShmBuffer>,
    pub fractional_scale: Option<WpFractionalScaleV1>,
    pub viewport: Option<WpViewport>,
    pub margin: (i32, i32, i32, i32),
    pub anchors: Vec<String>,
    pub shadow_insets: (f32, f32, f32, f32),
}

pub type BarSurface = ShellSurface;

pub struct ShmBuffer {
    pub fd: OwnedFd,
    pub pool: wayland_client::protocol::wl_shm_pool::WlShmPool,
    pub buffer: wayland_client::protocol::wl_buffer::WlBuffer,
    pub mmap_ptr: *mut u8,
    pub size: usize,
    pub width: i32,
    pub height: i32,
}

// SAFETY: `ShmBuffer` encapsulates an exclusively owned shared memory buffer. The underlying
// `mmap_ptr` and file descriptors are not shared across threads without proper synchronization,
// making it safe to transfer across thread boundaries.
unsafe impl Send for ShmBuffer {}

// SAFETY: Synchronization of internal mutations is guarded by caller lifecycle rules and Wayland
// buffer release callbacks ensuring non-concurrent access.
unsafe impl Sync for ShmBuffer {}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        if !self.mmap_ptr.is_null() && self.size > 0 {
            // SAFETY: `self.mmap_ptr` was allocated by `libc::mmap` with size `self.size`,
            // is non-null, and is exclusively owned by this `ShmBuffer` instance.
            unsafe {
                libc::munmap(self.mmap_ptr.cast(), self.size);
            }
        }
    }
}

/// State holder for all surfaces.
pub struct SurfaceManagerState {
    pub layer_shell: Option<ZwlrLayerShellV1>,
    pub fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
    pub viewporter: Option<WpViewporter>,
    pub surfaces: Vec<ShellSurface>,
}

impl SurfaceManagerState {
    pub fn new(
        layer_shell: Option<ZwlrLayerShellV1>,
        fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
        viewporter: Option<WpViewporter>,
    ) -> Self {
        Self {
            layer_shell,
            fractional_scale_manager,
            viewporter,
            surfaces: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_surface(
        &mut self,
        compositor: &wayland_client::protocol::wl_compositor::WlCompositor,
        qh: &QueueHandle<ShellState>,
        ty: SurfaceType,
        config_name: &str,
        dynamic: bool,
        output: Option<&WlOutput>,
        width: u32,
        height: u32,
        layer_name: &str,
        anchors: &[String],
        margin: (i32, i32, i32, i32),
        exclusive_zone: i32,
        keyboard: &str,
        scale: f64,
    ) -> Option<crate::widgets::WidgetId> {
        log::debug!(
            "Creating {} surface '{}' output_bound={} size={}x{} layer={} exclusive_zone={}",
            if dynamic { "dynamic" } else { "configured" },
            config_name,
            output.is_some(),
            width,
            height,
            layer_name,
            exclusive_zone,
        );
        let surface = compositor.create_surface(qh, ());
        let fractional_scale = self
            .fractional_scale_manager
            .as_ref()
            .map(|manager| manager.get_fractional_scale(&surface, qh, surface.id()));
        let viewport = self
            .viewporter
            .as_ref()
            .map(|manager| manager.get_viewport(&surface, qh, ()));
        let layer_surface = self.layer_shell.as_ref().map(|shell| {
            let layer = match layer_name {
                "overlay" => wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer::Overlay,
                "background" => wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer::Background,
                "bottom" => wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer::Bottom,
                _ => wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer::Top,
            };

            let ls = shell.get_layer_surface(
                &surface,
                output,
                layer,
                "wyrd-shell".to_string(),
                qh,
                (),
            );

            let mut anchor = zwlr_layer_surface_v1::Anchor::empty();
            for value in anchors {
                match value.as_str() {
                    "top" => anchor.insert(zwlr_layer_surface_v1::Anchor::Top),
                    "right" => anchor.insert(zwlr_layer_surface_v1::Anchor::Right),
                    "bottom" => anchor.insert(zwlr_layer_surface_v1::Anchor::Bottom),
                    "left" => anchor.insert(zwlr_layer_surface_v1::Anchor::Left),
                    _ => {},
                };
            }
            let is_horizontal_span = anchor.contains(zwlr_layer_surface_v1::Anchor::Left)
                && anchor.contains(zwlr_layer_surface_v1::Anchor::Right);
            let is_vertical_span = anchor.contains(zwlr_layer_surface_v1::Anchor::Top)
                && anchor.contains(zwlr_layer_surface_v1::Anchor::Bottom);
            let layer_width = if is_horizontal_span && width == 0 { 0 } else { width };
            let layer_height = if is_vertical_span && height == 0 { 0 } else { height };
            ls.set_size(layer_width, layer_height);
            ls.set_anchor(anchor);
            ls.set_margin(margin.0, margin.1, margin.2, margin.3);

            ls.set_exclusive_zone(exclusive_zone);

            let keyboard_mode = match keyboard {
                "exclusive" => zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
                "on_demand" => zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand,
                _ => zwlr_layer_surface_v1::KeyboardInteractivity::None,
            };
            ls.set_keyboard_interactivity(keyboard_mode);

            surface.commit();
            ls
        });
        self.surfaces.push(ShellSurface {
            config_name: config_name.to_owned(),
            dynamic,
            id: None,
            surface,
            layer_surface,
            ty,
            output: output.cloned(),
            width,
            height,
            scale: scale.max(1.0),
            dirty: true,
            pending_frame: false,
            buffer: None,
            frame_callback: None,
            shm_buffers: Vec::new(),
            fractional_scale,
            viewport,
            margin,
            anchors: anchors.to_vec(),
            shadow_insets: (0.0, 0.0, 0.0, 0.0),
        });

        None
    }

    pub fn attach_solid_buffers(
        &mut self,
        shm: &wayland_client::protocol::wl_shm::WlShm,
        qh: &QueueHandle<ShellState>,
        color: [u8; 4],
    ) -> anyhow::Result<()> {
        for surface in &mut self.surfaces {
            let width = (surface.width as f64 * surface.scale).round().max(1.0) as i32;
            let height = (surface.height as f64 * surface.scale).round().max(1.0) as i32;
            let stride = width * 4;
            let size = stride * height;
            let name = CString::new("wyrd-shell").unwrap();
            // SAFETY: `name` is a valid null-terminated C string. `libc::memfd_create` creates a
            // new anonymous memory-backed file descriptor in kernel space.
            let raw_fd = unsafe { libc::memfd_create(name.as_ptr(), 0) };
            if raw_fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            // SAFETY: `raw_fd` is a valid, newly created non-negative file descriptor. Taking ownership into `OwnedFd`.
            let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
            // SAFETY: `fd` is a valid open file descriptor; resizing memory-backed fd to `size`.
            if unsafe { libc::ftruncate(fd.as_raw_fd(), size as i64) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let pool = shm.create_pool(fd.as_fd(), size, qh, ());
            let buffer =
                pool.create_buffer(0, width, height, stride, wl_shm::Format::Xrgb8888, qh, 0);
            // SAFETY: Memory mapping the valid anonymous file descriptor for read/write access with valid bounds.
            let mapped = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    size as usize,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd.as_raw_fd(),
                    0,
                )
            };
            if mapped == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error().into());
            }
            // SAFETY: `mapped` is non-null and points to `size` bytes of allocated, valid shared memory.
            let pixels =
                unsafe { std::slice::from_raw_parts_mut(mapped.cast::<u8>(), size as usize) };
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.copy_from_slice(&color);
            }
            // SAFETY: Unmapping the temporary `mapped` memory pointer of size `size`.
            unsafe {
                libc::munmap(mapped, size as usize);
            }
            if let Some(viewport) = &surface.viewport {
                viewport.set_destination(width, height);
            }
            surface.surface.set_buffer_scale(1);
            surface.frame_callback = Some(surface.surface.frame(qh, ()));
            surface.pending_frame = true;
            surface.surface.attach(Some(&buffer), 0, 0);
            surface.surface.damage_buffer(0, 0, width, height);
            surface.surface.commit();
            surface.buffer = Some(buffer);
            log::debug!(
                "Committed rendered surface '{}' size={}x{}",
                surface.config_name,
                width,
                height
            );
        }
        Ok(())
    }

    pub fn update_surface_size(
        &mut self,
        compositor: &wayland_client::protocol::wl_compositor::WlCompositor,
        qh: &QueueHandle<ShellState>,
        config_name: &str,
        width: u32,
        height: u32,
    ) -> bool {
        let Some(surface) = self
            .surfaces
            .iter_mut()
            .find(|surface| surface.dynamic && surface.config_name == config_name)
        else {
            return false;
        };
        surface.width = width.max(1);
        surface.height = height.max(1);
        if let Some(layer_surface) = &surface.layer_surface {
            layer_surface.set_size(surface.width, surface.height);
            let input_region = compositor.create_region(qh, ());
            input_region.add(0, 0, surface.width as i32, surface.height as i32);
            surface.surface.set_input_region(Some(&input_region));
            input_region.destroy();
            surface.surface.commit();
        }
        surface.dirty = true;
        true
    }

    pub fn destroy_configured_surfaces(&mut self, config_name: &str) -> usize {
        let mut removed = 0;
        self.surfaces.retain(|surface| {
            if !surface.dynamic && surface.config_name == config_name {
                if let Some(layer_surface) = &surface.layer_surface {
                    layer_surface.destroy();
                }
                surface.surface.destroy();
                removed += 1;
                false
            } else {
                true
            }
        });
        removed
    }

    pub fn attach_pixmap_buffers(
        &mut self,
        shm: &wayland_client::protocol::wl_shm::WlShm,
        qh: &QueueHandle<ShellState>,
        pixmaps: &[(usize, Pixmap, DamageTracker)],
        released_buffers: &mut HashSet<wayland_client::backend::ObjectId>,
    ) -> anyhow::Result<()> {
        for (surface_index, pixmap, _damage) in pixmaps {
            let Some(surface) = self.surfaces.get_mut(*surface_index) else {
                continue;
            };
            let width = (pixmap.width() as i32).max(1);
            let height = (pixmap.height() as i32).max(1);
            let stride = width * 4;
            let size = (stride * height) as usize;

            let reusable = surface.shm_buffers.iter_mut().find(|candidate| {
                candidate.width == width
                    && candidate.height == height
                    && released_buffers.contains(&candidate.buffer.id())
            });

            let (buffer, mmap_ptr, buf_size) = if let Some(candidate) = reusable {
                (candidate.buffer.clone(), candidate.mmap_ptr, candidate.size)
            } else {
                let name = CString::new("wyrd-shell").unwrap();
                // SAFETY: `name` is a valid null-terminated C string. `libc::memfd_create` creates a
                // new anonymous memory-backed file descriptor in kernel space.
                let raw_fd = unsafe { libc::memfd_create(name.as_ptr(), 0) };
                if raw_fd < 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                // SAFETY: `raw_fd` is a valid, newly created non-negative file descriptor. Taking ownership into `OwnedFd`.
                let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
                // SAFETY: `fd` is a valid open file descriptor; resizing memory-backed fd to `size`.
                if unsafe { libc::ftruncate(fd.as_raw_fd(), size as i64) } != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                // SAFETY: Memory mapping the valid anonymous file descriptor for read/write access with valid bounds.
                let mapped = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        size,
                        libc::PROT_READ | libc::PROT_WRITE,
                        libc::MAP_SHARED,
                        fd.as_raw_fd(),
                        0,
                    )
                };
                if mapped == libc::MAP_FAILED {
                    return Err(std::io::Error::last_os_error().into());
                }
                let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
                let buffer = pool.create_buffer(
                    0,
                    width,
                    height,
                    stride,
                    wl_shm::Format::Argb8888,
                    qh,
                    *surface_index,
                );
                let shm_buf = ShmBuffer {
                    fd,
                    pool,
                    buffer: buffer.clone(),
                    mmap_ptr: mapped.cast::<u8>(),
                    size,
                    width,
                    height,
                };
                let mmap_ptr = shm_buf.mmap_ptr;
                let buf_size = shm_buf.size;
                surface.shm_buffers.push(shm_buf);
                (buffer, mmap_ptr, buf_size)
            };

            // SAFETY: `mmap_ptr` is non-null and points to `buf_size` bytes of valid shared memory allocated above.
            let pixels = unsafe { std::slice::from_raw_parts_mut(mmap_ptr, buf_size) };
            let pixmap_bytes = pixmap.data();

            // Convert RGBA -> ARGB8888 (BGRA in little-endian) using u32 word swap
            for (dst_chunk, src_chunk) in
                pixels.chunks_exact_mut(4).zip(pixmap_bytes.chunks_exact(4))
            {
                let rgba =
                    u32::from_le_bytes([src_chunk[0], src_chunk[1], src_chunk[2], src_chunk[3]]);
                let r = rgba & 0x000000FF;
                let b = (rgba & 0x00FF0000) >> 16;
                let ga = rgba & 0xFF00FF00;
                let bgra = ga | (r << 16) | b;
                dst_chunk.copy_from_slice(&bgra.to_le_bytes());
            }

            log::debug!(
                "Submitting surface {}x{} scale={} pixmap_first={:?} shm_first={:?}",
                width,
                height,
                surface.scale,
                pixmap_bytes.get(..4),
                pixels.get(..4),
            );

            if let Some(viewport) = &surface.viewport {
                viewport.set_destination(surface.width as i32, surface.height as i32);
            }
            surface.surface.set_buffer_scale(1);
            surface.frame_callback = Some(surface.surface.frame(qh, ()));
            surface.pending_frame = true;
            surface.surface.attach(Some(&buffer), 0, 0);
            surface.surface.damage_buffer(0, 0, width, height);
            surface.surface.commit();
            released_buffers.remove(&buffer.id());
            surface.buffer = Some(buffer);
        }
        Ok(())
    }
}

impl wayland_client::Dispatch<ZwlrLayerSurfaceV1, ()> for ShellState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                proxy.ack_configure(serial);
                state
                    .layer_surface_sizes
                    .insert(proxy.id(), (width, height));
                state.frame_ready = true;
                log::debug!("Layer surface configured: {}x{}", width, height);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                log::info!("Layer surface closed by compositor");
            }
            _ => {}
        }
    }
}

impl wayland_client::Dispatch<wayland_client::protocol::wl_callback::WlCallback, ()>
    for ShellState
{
    fn event(
        state: &mut Self,
        _proxy: &wayland_client::protocol::wl_callback::WlCallback,
        event: wayland_client::protocol::wl_callback::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if matches!(
            event,
            wayland_client::protocol::wl_callback::Event::Done { .. }
        ) {
            state.frame_ready = true;
        }
    }
}

impl wayland_client::Dispatch<wayland_client::protocol::wl_buffer::WlBuffer, usize> for ShellState {
    fn event(
        state: &mut ShellState,
        proxy: &wayland_client::protocol::wl_buffer::WlBuffer,
        event: wayland_client::protocol::wl_buffer::Event,
        _data: &usize,
        _conn: &Connection,
        _qhandle: &QueueHandle<ShellState>,
    ) {
        if matches!(event, wayland_client::protocol::wl_buffer::Event::Release) {
            state.released_buffers.insert(proxy.id());
        }
    }
}
