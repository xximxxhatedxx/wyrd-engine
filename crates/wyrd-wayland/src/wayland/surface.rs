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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SurfaceHandle(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceType {
    Bar,
    Panel,
    Popup,
    Background,
}

#[derive(Debug, Clone)]
pub struct LayerSurfaceConfig {
    pub ty: SurfaceType,
    pub layer_name: String,
    pub anchors: Vec<String>,
    pub margin: (i32, i32, i32, i32),
    pub width: u32,
    pub height: u32,
    pub exclusive_zone: i32,
    pub keyboard: String,
}

pub struct LayerShellSurface {
    pub config_name: String,
    pub dynamic: bool,
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
    pub closing: bool,
}

pub type ShellSurface = LayerShellSurface;
pub type BarSurface = LayerShellSurface;

impl LayerShellSurface {
    pub fn disable_keyboard_interactivity(&self) {
        if let Some(ls) = &self.layer_surface {
            ls.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        }
    }

    pub fn configure(&mut self, cfg: &LayerSurfaceConfig) {
        self.ty = cfg.ty;
        self.width = cfg.width;
        self.height = cfg.height;
        self.margin = cfg.margin;
        self.anchors = cfg.anchors.clone();
        if let Some(ls) = &self.layer_surface {
            let mut anchor = zwlr_layer_surface_v1::Anchor::empty();
            for value in &cfg.anchors {
                match value.as_str() {
                    "top" => anchor.insert(zwlr_layer_surface_v1::Anchor::Top),
                    "right" => anchor.insert(zwlr_layer_surface_v1::Anchor::Right),
                    "bottom" => anchor.insert(zwlr_layer_surface_v1::Anchor::Bottom),
                    "left" => anchor.insert(zwlr_layer_surface_v1::Anchor::Left),
                    _ => {}
                }
            }
            let is_horizontal_span = anchor.contains(zwlr_layer_surface_v1::Anchor::Left)
                && anchor.contains(zwlr_layer_surface_v1::Anchor::Right);
            let is_vertical_span = anchor.contains(zwlr_layer_surface_v1::Anchor::Top)
                && anchor.contains(zwlr_layer_surface_v1::Anchor::Bottom);
            let layer_width = if is_horizontal_span && cfg.width == 0 {
                0
            } else {
                cfg.width
            };
            let layer_height = if is_vertical_span && cfg.height == 0 {
                0
            } else {
                cfg.height
            };
            ls.set_size(layer_width, layer_height);
            ls.set_anchor(anchor);
            ls.set_margin(cfg.margin.0, cfg.margin.1, cfg.margin.2, cfg.margin.3);
            ls.set_exclusive_zone(cfg.exclusive_zone);
            let keyboard_mode = match cfg.keyboard.as_str() {
                "exclusive" => zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
                "on_demand" => zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand,
                _ => zwlr_layer_surface_v1::KeyboardInteractivity::None,
            };
            ls.set_keyboard_interactivity(keyboard_mode);
            self.surface.commit();
        }
        self.dirty = true;
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        if let Some(layer_surface) = &self.layer_surface {
            layer_surface.set_size(self.width, self.height);
        }
        self.dirty = true;
    }

    pub fn submit_frame<C: 'static, T: 'static>(
        &mut self,
        shm: &wayland_client::protocol::wl_shm::WlShm,
        qh: &QueueHandle<ShellState<C, T>>,
        surface_index: usize,
        pixmap: &Pixmap,
        damage: &[crate::render::damage::DamageRect],
        released_buffers: &mut HashSet<wayland_client::backend::ObjectId>,
    ) -> anyhow::Result<()> {
        let width = (pixmap.width() as i32).max(1);
        let height = (pixmap.height() as i32).max(1);
        let (buffer, mmap_ptr, buf_size) = ShmBufferPool::acquire_or_create(
            &mut self.shm_buffers,
            shm,
            qh,
            surface_index,
            width,
            height,
            released_buffers,
        )?;

        // SAFETY: `mmap_ptr` is a valid non-null `ShmMapping` pointing to `buf_size` bytes of
        // exclusively accessed shared memory (`&mut self`).
        let pixels = unsafe { std::slice::from_raw_parts_mut(mmap_ptr.as_ptr(), buf_size) };
        let pixmap_bytes = pixmap.data();

        // Convert RGBA -> ARGB8888 (BGRA in little-endian) using u32 word swap
        for (dst_chunk, src_chunk) in pixels
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(pixmap_bytes.as_chunks::<4>().0.iter())
        {
            let rgba = u32::from_le_bytes([src_chunk[0], src_chunk[1], src_chunk[2], src_chunk[3]]);
            let r = rgba & 0x000000FF;
            let b = (rgba & 0x00FF0000) >> 16;
            let ga = rgba & 0xFF00FF00;
            let bgra = ga | (r << 16) | b;
            dst_chunk.copy_from_slice(&bgra.to_le_bytes());
        }

        if let Some(viewport) = &self.viewport {
            viewport.set_destination(self.width as i32, self.height as i32);
        }
        self.surface.set_buffer_scale(1);
        self.frame_callback = Some(self.surface.frame(qh, ()));
        self.pending_frame = true;
        self.surface.attach(Some(&buffer), 0, 0);
        if damage.is_empty() {
            self.surface.damage_buffer(0, 0, width, height);
        } else {
            for rect in damage {
                self.surface.damage_buffer(
                    rect.x().max(0),
                    rect.y().max(0),
                    (rect.width() as i32).max(1),
                    (rect.height() as i32).max(1),
                );
            }
        }
        self.surface.commit();
        released_buffers.remove(&buffer.id());
        self.buffer = Some(buffer);
        Ok(())
    }
}

/// Reusable shared-memory buffer pool helper for Wayland surfaces.
pub struct ShmBufferPool;

impl ShmBufferPool {
    pub fn acquire_or_create<C: 'static, T: 'static>(
        buffers: &mut Vec<ShmBuffer>,
        shm: &wayland_client::protocol::wl_shm::WlShm,
        qh: &QueueHandle<ShellState<C, T>>,
        surface_index: usize,
        width: i32,
        height: i32,
        released_buffers: &mut HashSet<wayland_client::backend::ObjectId>,
    ) -> anyhow::Result<(
        wayland_client::protocol::wl_buffer::WlBuffer,
        ShmMapping,
        usize,
    )> {
        let stride = width * 4;
        let size = (stride * height) as usize;

        let mut matching_released = 0usize;
        buffers.retain(|candidate| {
            if released_buffers.contains(&candidate.buffer.id()) {
                if candidate.width != width || candidate.height != height {
                    released_buffers.remove(&candidate.buffer.id());
                    return false;
                }
                matching_released += 1;
                if matching_released > 2 {
                    released_buffers.remove(&candidate.buffer.id());
                    return false;
                }
            }
            true
        });

        let reusable = buffers.iter_mut().find(|candidate| {
            candidate.width == width
                && candidate.height == height
                && released_buffers.contains(&candidate.buffer.id())
        });

        if let Some(candidate) = reusable {
            Ok((candidate.buffer.clone(), candidate.mmap_ptr, candidate.size))
        } else {
            let name = CString::new(env!("CARGO_PKG_NAME")).unwrap();
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
            let Some(non_null_ptr) = std::ptr::NonNull::new(mapped.cast::<u8>()) else {
                return Err(std::io::Error::other("mmap returned null pointer").into());
            };
            let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
            let buffer = pool.create_buffer(
                0,
                width,
                height,
                stride,
                wl_shm::Format::Argb8888,
                qh,
                surface_index,
            );
            let shm_buf = ShmBuffer {
                fd,
                pool,
                buffer: buffer.clone(),
                mmap_ptr: ShmMapping::new(non_null_ptr),
                size,
                width,
                height,
            };
            let mmap_ptr = shm_buf.mmap_ptr;
            let buf_size = shm_buf.size;
            buffers.push(shm_buf);
            Ok((buffer, mmap_ptr, buf_size))
        }
    }
}

/// Type-safe wrapper around an exclusively owned `mmap` region.
///
/// Implements [`Send`] so the buffer can be moved across threads, while intentionally
/// omitting [`Sync`] because concurrent reads/writes to the mapped byte slice without
/// external synchronization would cause data races. Access is restricted to the single
/// owner thread holding `&mut ShmBuffer`.
#[derive(Debug, Clone, Copy)]
pub struct ShmMapping(std::ptr::NonNull<u8>);

// SAFETY: `ShmMapping` wraps an exclusively owned `mmap` region backed by a private
// anonymous `memfd`. Moving ownership between threads is safe; `Sync` is intentionally
// not implemented so multiple threads cannot read/write the mapping concurrently.
unsafe impl Send for ShmMapping {}

impl ShmMapping {
    /// Wraps a non-null `mmap` pointer.
    #[inline]
    pub fn new(ptr: std::ptr::NonNull<u8>) -> Self {
        Self(ptr)
    }

    /// Returns the raw mutable pointer to the mapped memory region.
    #[inline]
    pub fn as_ptr(self) -> *mut u8 {
        self.0.as_ptr()
    }
}

pub struct ShmBuffer {
    pub fd: OwnedFd,
    pub pool: wayland_client::protocol::wl_shm_pool::WlShmPool,
    pub buffer: wayland_client::protocol::wl_buffer::WlBuffer,
    pub mmap_ptr: ShmMapping,
    pub size: usize,
    pub width: i32,
    pub height: i32,
}

// SAFETY: `ShmBuffer` exclusively owns its anonymous `OwnedFd`, Wayland protocol objects,
// and `ShmMapping` (`Send`, `!Sync`). Implementing `Send` (and NOT `Sync`) allows moving
// `ShmBuffer` across threads while statically preventing concurrent access from multiple
// threads without a `Mutex`.
unsafe impl Send for ShmBuffer {}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
        if self.size > 0 {
            // SAFETY: `self.mmap_ptr` was created by a successful `libc::mmap` call of length
            // `self.size` and is exclusively owned by this `ShmBuffer` instance.
            unsafe {
                libc::munmap(self.mmap_ptr.as_ptr().cast(), self.size);
            }
        }
    }
}

pub struct SurfaceRegistry {
    pub layer_shell: Option<ZwlrLayerShellV1>,
    pub fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
    pub viewporter: Option<WpViewporter>,
    pub surfaces: Vec<LayerShellSurface>,
}

pub type SurfaceManagerState = SurfaceRegistry;

impl SurfaceRegistry {
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

    pub fn get(&self, handle: SurfaceHandle) -> Option<&LayerShellSurface> {
        self.surfaces.get(handle.0)
    }

    pub fn get_mut(&mut self, handle: SurfaceHandle) -> Option<&mut LayerShellSurface> {
        self.surfaces.get_mut(handle.0)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_surface<C: 'static, T: 'static>(
        &mut self,
        compositor: &wayland_client::protocol::wl_compositor::WlCompositor,
        qh: &QueueHandle<ShellState<C, T>>,
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
    ) -> SurfaceHandle {
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

            let namespace = std::env::var("WYRD_NAMESPACE").unwrap_or_else(|_| {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
                    .unwrap_or_else(|| env!("CARGO_PKG_NAME").to_string())
            });
            shell.get_layer_surface(&surface, output, layer, namespace, qh, ())
        });

        let mut shell_surface = LayerShellSurface {
            config_name: config_name.to_owned(),
            dynamic,
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
            closing: false,
        };

        shell_surface.configure(&LayerSurfaceConfig {
            ty,
            layer_name: layer_name.to_owned(),
            anchors: anchors.to_vec(),
            margin,
            width,
            height,
            exclusive_zone,
            keyboard: keyboard.to_owned(),
        });

        let idx = self.surfaces.len();
        self.surfaces.push(shell_surface);
        SurfaceHandle(idx)
    }

    pub fn attach_solid_buffers<C: 'static, T: 'static>(
        &mut self,
        shm: &wayland_client::protocol::wl_shm::WlShm,
        qh: &QueueHandle<ShellState<C, T>>,
        color: [u8; 4],
    ) -> anyhow::Result<()> {
        for surface in &mut self.surfaces {
            let width = (surface.width as f64 * surface.scale).round().max(1.0) as i32;
            let height = (surface.height as f64 * surface.scale).round().max(1.0) as i32;
            let stride = width * 4;
            let size = stride * height;
            let name = CString::new(env!("CARGO_PKG_NAME")).unwrap();
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
            for pixel in pixels.as_chunks_mut::<4>().0 {
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

    pub fn update_surface_size<C: 'static, T: 'static>(
        &mut self,
        compositor: &wayland_client::protocol::wl_compositor::WlCompositor,
        qh: &QueueHandle<ShellState<C, T>>,
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
        surface.resize(width, height);
        if surface.layer_surface.is_some() {
            let input_region = compositor.create_region(qh, ());
            input_region.add(0, 0, surface.width as i32, surface.height as i32);
            surface.surface.set_input_region(Some(&input_region));
            input_region.destroy();
            surface.surface.commit();
        }
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

    pub fn attach_pixmap_buffers<C: 'static, T: 'static>(
        &mut self,
        shm: &wayland_client::protocol::wl_shm::WlShm,
        qh: &QueueHandle<ShellState<C, T>>,
        pixmaps: &[(usize, Pixmap, DamageTracker)],
        released_buffers: &mut HashSet<wayland_client::backend::ObjectId>,
    ) -> anyhow::Result<()> {
        for (surface_index, pixmap, _damage) in pixmaps {
            let Some(surface) = self.surfaces.get_mut(*surface_index) else {
                continue;
            };
            surface.submit_frame(shm, qh, *surface_index, pixmap, &[], released_buffers)?;
        }
        Ok(())
    }
}

impl<C: 'static, T: 'static> wayland_client::Dispatch<ZwlrLayerSurfaceV1, ()> for ShellState<C, T> {
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

impl<C: 'static, T: 'static>
    wayland_client::Dispatch<wayland_client::protocol::wl_callback::WlCallback, ()>
    for ShellState<C, T>
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

impl<C: 'static, T: 'static>
    wayland_client::Dispatch<wayland_client::protocol::wl_buffer::WlBuffer, usize>
    for ShellState<C, T>
{
    fn event(
        state: &mut ShellState<C, T>,
        proxy: &wayland_client::protocol::wl_buffer::WlBuffer,
        event: wayland_client::protocol::wl_buffer::Event,
        _data: &usize,
        _conn: &Connection,
        _qhandle: &QueueHandle<ShellState<C, T>>,
    ) {
        if matches!(event, wayland_client::protocol::wl_buffer::Event::Release) {
            state.released_buffers.insert(proxy.id());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shm_buffer_alloc_write_and_drop() {
        let width: usize = 64;
        let height: usize = 32;
        let size = width * height * 4;
        let name = CString::new("wyrd_shm_test").unwrap();

        // SAFETY: `name` is a valid null-terminated CString; `memfd_create` creates an anonymous fd.
        let raw_fd = unsafe { libc::memfd_create(name.as_ptr(), 0) };
        assert!(raw_fd >= 0, "memfd_create must succeed");
        // SAFETY: `raw_fd` is a valid newly created file descriptor.
        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        // SAFETY: `fd` is valid; resizing anonymous memory file to `size`.
        assert_eq!(unsafe { libc::ftruncate(fd.as_raw_fd(), size as i64) }, 0);

        // SAFETY: mapping valid `fd` of `size` bytes with read/write permissions.
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
        assert_ne!(mapped, libc::MAP_FAILED);
        let non_null = std::ptr::NonNull::new(mapped.cast::<u8>()).expect("non-null mmap");
        let mapping = ShmMapping::new(non_null);

        // SAFETY: `mapping` points to `size` valid exclusively-owned bytes.
        let slice = unsafe { std::slice::from_raw_parts_mut(mapping.as_ptr(), size) };
        for chunk in slice.as_chunks_mut::<4>().0 {
            chunk.copy_from_slice(&0xFF112233u32.to_le_bytes());
        }
        assert_eq!(&slice[0..4], &0xFF112233u32.to_le_bytes());
        assert_eq!(&slice[size - 4..size], &0xFF112233u32.to_le_bytes());

        // SAFETY: unmapping the region created above with identical pointer and `size`.
        assert_eq!(
            unsafe { libc::munmap(mapping.as_ptr().cast(), size) },
            0,
            "munmap must succeed cleanly"
        );
    }
}
