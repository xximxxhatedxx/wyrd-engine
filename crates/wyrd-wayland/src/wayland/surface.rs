//! Layer-shell surface management

use anyhow::Context;
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
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
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

/// Identifies the active Wayland surface backend protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SurfaceBackendKind {
    ExtLayerShell,
    ZwlrLayerShell,
    XdgShell,
    None,
}

impl std::fmt::Display for SurfaceBackendKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExtLayerShell => write!(f, "ExtLayerShell"),
            Self::ZwlrLayerShell => write!(f, "ZwlrLayerShell"),
            Self::XdgShell => write!(f, "XdgShell"),
            Self::None => write!(f, "None"),
        }
    }
}

/// Capabilities supported by the active Wayland surface backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceCapabilities {
    pub exclusive_zone: bool,
    pub anchoring: bool,
    pub layer_ordering: bool,
    pub keyboard_interactivity: bool,
    pub set_size: bool,
    pub popup_positioning: bool,
}

impl SurfaceCapabilities {
    /// Returns the capabilities intrinsic to the given surface backend kind.
    #[must_use]
    pub fn for_kind(kind: SurfaceBackendKind) -> Self {
        match kind {
            SurfaceBackendKind::ExtLayerShell | SurfaceBackendKind::ZwlrLayerShell => Self {
                exclusive_zone: true,
                anchoring: true,
                layer_ordering: true,
                keyboard_interactivity: true,
                set_size: true,
                popup_positioning: true,
            },
            SurfaceBackendKind::XdgShell => Self {
                exclusive_zone: false,
                anchoring: false,
                layer_ordering: false,
                keyboard_interactivity: false,
                set_size: true,
                popup_positioning: false,
            },
            SurfaceBackendKind::None => Self {
                exclusive_zone: false,
                anchoring: false,
                layer_ordering: false,
                keyboard_interactivity: false,
                set_size: false,
                popup_positioning: false,
            },
        }
    }
}

/// Errors occurring during surface backend protocol negotiation, configuration, or commits.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SurfaceBackendError {
    ProtocolUnavailable(String),
    ConfigureFailed(String),
    CommitFailed(String),
}

impl std::fmt::Display for SurfaceBackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProtocolUnavailable(msg) => write!(f, "protocol unavailable: {msg}"),
            Self::ConfigureFailed(msg) => write!(f, "configure failed: {msg}"),
            Self::CommitFailed(msg) => write!(f, "commit failed: {msg}"),
        }
    }
}

impl std::error::Error for SurfaceBackendError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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

/// Concrete backend using wlroots zwlr-layer-shell-v1.
#[derive(Debug, Clone)]
pub struct ZwlrLayerShellBackend {
    pub layer_surface: ZwlrLayerSurfaceV1,
}

impl ZwlrLayerShellBackend {
    pub fn new(layer_surface: ZwlrLayerSurfaceV1) -> Self {
        Self { layer_surface }
    }

    pub fn disable_keyboard_interactivity(&self) {
        self.layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
    }

    pub fn set_anchor(&self, anchor: zwlr_layer_surface_v1::Anchor) {
        self.layer_surface.set_anchor(anchor);
    }

    pub fn set_exclusive_zone(&self, exclusive_zone: i32) {
        self.layer_surface.set_exclusive_zone(exclusive_zone);
    }

    pub fn set_keyboard_interactivity(&self, mode: zwlr_layer_surface_v1::KeyboardInteractivity) {
        self.layer_surface.set_keyboard_interactivity(mode);
    }

    pub fn set_layer(
        &self,
        layer: wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer,
    ) {
        self.layer_surface.set_layer(layer);
    }

    pub fn resize(&self, width: u32, height: u32) {
        self.layer_surface.set_size(width, height);
    }

    pub fn configure(&self, surface: &WlSurface, cfg: &LayerSurfaceConfig) {
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
        self.layer_surface.set_size(layer_width, layer_height);
        self.layer_surface.set_anchor(anchor);
        self.layer_surface
            .set_margin(cfg.margin.0, cfg.margin.1, cfg.margin.2, cfg.margin.3);
        self.layer_surface.set_exclusive_zone(cfg.exclusive_zone);
        let keyboard_mode = match cfg.keyboard.as_str() {
            "exclusive" => zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
            "on_demand" => zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand,
            _ => zwlr_layer_surface_v1::KeyboardInteractivity::None,
        };
        self.layer_surface.set_keyboard_interactivity(keyboard_mode);
        surface.commit();
    }
}

/// Fallback backend using xdg-shell (xdg_wm_base + xdg_toplevel) for GNOME, KDE Plasma, Weston, etc.
#[derive(Debug, Clone)]
pub struct XdgShellBackend {
    pub xdg_surface: XdgSurface,
    pub toplevel: XdgToplevel,
    pub warned_exclusive_zone: bool,
    pub warned_anchors: bool,
    pub warned_layer: bool,
}

impl XdgShellBackend {
    pub fn new(xdg_surface: XdgSurface, toplevel: XdgToplevel) -> Self {
        Self {
            xdg_surface,
            toplevel,
            warned_exclusive_zone: false,
            warned_anchors: false,
            warned_layer: false,
        }
    }

    pub fn disable_keyboard_interactivity(&self) {
        // xdg-shell relies on compositor focus management; no grab disable request
    }

    pub fn set_anchor(&mut self, surface_name: &str, anchors: &[String]) {
        if !anchors.is_empty() && !self.warned_anchors {
            log::warn!(
                "Surface '{}': anchors {:?} requested but not supported on XdgShell backend",
                surface_name,
                anchors
            );
            self.warned_anchors = true;
        }
    }

    pub fn set_exclusive_zone(&mut self, surface_name: &str, exclusive_zone: i32) {
        if exclusive_zone != 0 && !self.warned_exclusive_zone {
            log::info!(
                "Surface '{}': exclusive zone ({}) requested but not supported on XdgShell backend; degrading gracefully",
                surface_name,
                exclusive_zone
            );
            self.warned_exclusive_zone = true;
        }
    }

    pub fn set_keyboard_interactivity(&self, _mode: &str) {
        // xdg-shell relies on standard compositor window focus
    }

    pub fn set_layer(&mut self, surface_name: &str, layer: &str) {
        if !self.warned_layer {
            log::warn!(
                "Surface '{}': layer '{}' requested but not supported on XdgShell backend",
                surface_name,
                layer
            );
            self.warned_layer = true;
        }
    }

    pub fn resize(&self, width: u32, height: u32) {
        let w = width.max(1) as i32;
        let h = height.max(1) as i32;
        self.toplevel.set_min_size(w, h);
        self.toplevel.set_max_size(w, h);
    }

    pub fn configure(&mut self, surface: &WlSurface, cfg: &LayerSurfaceConfig) {
        self.set_exclusive_zone(&cfg.layer_name, cfg.exclusive_zone);
        self.set_anchor(&cfg.layer_name, &cfg.anchors);
        self.set_layer(&cfg.layer_name, &cfg.layer_name);

        if cfg.height > 0 {
            let min_w = if cfg.width > 0 { cfg.width as i32 } else { 0 };
            let max_w = if cfg.width > 0 { cfg.width as i32 } else { 0 };
            self.toplevel.set_min_size(min_w, cfg.height as i32);
            self.toplevel.set_max_size(max_w, cfg.height as i32);
        }
        surface.commit();
    }
}

/// Stub implementation for upstream ext-layer-shell-v1 protocol.
///
/// TODO(upstream): `wayland-protocols 0.32.13` does not yet ship staging `ext-layer-shell-v1`.
/// When an updated protocol crate is available, this stub will be replaced with concrete
/// bindings to `ext_layer_shell_v1::ExtLayerShellV1`.
#[derive(Debug, Clone)]
pub struct ExtLayerShellBackend;

impl ExtLayerShellBackend {
    /// Returns `None` because `ext-layer-shell-v1` is unavailable in 0.1.0.
    pub fn probe() -> Option<Self> {
        None
    }
}

// TODO(upstream): wayland-protocols does not yet ship stable
// ext-layer-shell-v1 as of 0.32.x. Re-check when upgrading to
// wayland-protocols >= 0.33; if the interface is stabilized,
// replace this stub with concrete bindings.
pub fn detect_ext_layer_shell_v1() -> Option<ExtLayerShellBackend> {
    ExtLayerShellBackend::probe()
}

/// Protocol-agnostic Wayland surface backend.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SurfaceBackend {
    ZwlrLayerShell(ZwlrLayerShellBackend),
    XdgShell(XdgShellBackend),
    ExtLayerShell(ExtLayerShellBackend),
    None,
}

impl SurfaceBackend {
    #[must_use]
    pub fn kind(&self) -> SurfaceBackendKind {
        match self {
            Self::ZwlrLayerShell(_) => SurfaceBackendKind::ZwlrLayerShell,
            Self::XdgShell(_) => SurfaceBackendKind::XdgShell,
            Self::ExtLayerShell(_) => SurfaceBackendKind::ExtLayerShell,
            Self::None => SurfaceBackendKind::None,
        }
    }

    #[must_use]
    pub fn capabilities(&self) -> SurfaceCapabilities {
        SurfaceCapabilities::for_kind(self.kind())
    }

    pub fn disable_keyboard_interactivity(&self) {
        match self {
            Self::ZwlrLayerShell(b) => b.disable_keyboard_interactivity(),
            Self::XdgShell(b) => b.disable_keyboard_interactivity(),
            Self::ExtLayerShell(_) | Self::None => {}
        }
    }

    pub fn resize(&self, width: u32, height: u32) {
        match self {
            Self::ZwlrLayerShell(b) => b.resize(width, height),
            Self::XdgShell(b) => b.resize(width, height),
            Self::ExtLayerShell(_) | Self::None => {}
        }
    }

    pub fn configure(&mut self, surface: &WlSurface, cfg: &LayerSurfaceConfig) {
        match self {
            Self::ZwlrLayerShell(b) => b.configure(surface, cfg),
            Self::XdgShell(b) => b.configure(surface, cfg),
            Self::ExtLayerShell(_) | Self::None => {
                surface.commit();
            }
        }
    }

    pub fn set_anchor(&mut self, surface_name: &str, anchors: &[String]) {
        match self {
            Self::ZwlrLayerShell(b) => {
                let mut anchor = zwlr_layer_surface_v1::Anchor::empty();
                for value in anchors {
                    match value.as_str() {
                        "top" => anchor.insert(zwlr_layer_surface_v1::Anchor::Top),
                        "right" => anchor.insert(zwlr_layer_surface_v1::Anchor::Right),
                        "bottom" => anchor.insert(zwlr_layer_surface_v1::Anchor::Bottom),
                        "left" => anchor.insert(zwlr_layer_surface_v1::Anchor::Left),
                        _ => {}
                    }
                }
                b.set_anchor(anchor);
            }
            Self::XdgShell(b) => b.set_anchor(surface_name, anchors),
            Self::ExtLayerShell(_) | Self::None => {}
        }
    }

    pub fn set_exclusive_zone(&mut self, surface_name: &str, zone: i32) {
        match self {
            Self::ZwlrLayerShell(b) => b.set_exclusive_zone(zone),
            Self::XdgShell(b) => b.set_exclusive_zone(surface_name, zone),
            Self::ExtLayerShell(_) | Self::None => {}
        }
    }

    pub fn set_keyboard_interactivity(&self, mode: &str) {
        match self {
            Self::ZwlrLayerShell(b) => {
                let km = match mode {
                    "exclusive" => zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
                    "on_demand" => zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand,
                    _ => zwlr_layer_surface_v1::KeyboardInteractivity::None,
                };
                b.set_keyboard_interactivity(km);
            }
            Self::XdgShell(b) => b.set_keyboard_interactivity(mode),
            Self::ExtLayerShell(_) | Self::None => {}
        }
    }

    pub fn set_layer(&mut self, surface_name: &str, layer: &str) {
        match self {
            Self::ZwlrLayerShell(b) => {
                let l = match layer {
                    "overlay" => {
                        wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer::Overlay
                    }
                    "background" => {
                        wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer::Background
                    }
                    "bottom" => {
                        wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer::Bottom
                    }
                    _ => {
                        wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer::Top
                    }
                };
                b.set_layer(l);
            }
            Self::XdgShell(b) => b.set_layer(surface_name, layer),
            Self::ExtLayerShell(_) | Self::None => {}
        }
    }

    pub fn destroy(&self) {
        match self {
            Self::ZwlrLayerShell(b) => b.layer_surface.destroy(),
            Self::XdgShell(b) => {
                b.toplevel.destroy();
                b.xdg_surface.destroy();
            }
            Self::ExtLayerShell(_) | Self::None => {}
        }
    }
}

pub struct LayerShellSurface {
    pub config_name: String,
    pub dynamic: bool,
    #[cfg(feature = "gpu-render")]
    gpu_surface: Option<super::gpu_surface::WaylandGpuSurface>,
    pub surface: WlSurface,
    pub layer_surface: Option<ZwlrLayerSurfaceV1>,
    pub backend: SurfaceBackend,
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
    #[cfg(feature = "gpu-render")]
    pub async fn initialize_gpu_renderer(
        &mut self,
        connection: &wayland_client::Connection,
    ) -> anyhow::Result<wyrd_graphics::render::gpu::GpuRenderer> {
        anyhow::ensure!(
            std::env::var_os("WYRD_FORCE_CPU").is_none_or(|value| value != "1"),
            "GPU rendering disabled by WYRD_FORCE_CPU=1"
        );
        let width = (self.width as f64 * self.scale).round().max(1.0) as u32;
        let height = (self.height as f64 * self.scale).round().max(1.0) as u32;
        let (renderer, surface) = super::gpu_surface::WaylandGpuSurface::new_renderer(
            connection,
            &self.surface,
            width,
            height,
        )
        .await?;
        self.gpu_surface = Some(surface);
        Ok(renderer)
    }

    #[cfg(feature = "gpu-render")]
    pub fn initialize_gpu_surface(
        &mut self,
        connection: &wayland_client::Connection,
        renderer: &wyrd_graphics::render::gpu::GpuRenderer,
    ) -> bool {
        if std::env::var("WYRD_FORCE_CPU").is_ok_and(|value| value == "1") {
            self.gpu_surface = None;
            return false;
        }
        let width = (self.width as f64 * self.scale).round().max(1.0) as u32;
        let height = (self.height as f64 * self.scale).round().max(1.0) as u32;
        self.gpu_surface = match super::gpu_surface::WaylandGpuSurface::new(
            connection,
            &self.surface,
            renderer,
            width,
            height,
        ) {
            Ok(surface) => Some(surface),
            Err(error) => {
                log::warn!("Wayland GPU surface initialization failed; using wl_shm: {error:#}");
                None
            }
        };
        self.gpu_surface.is_some()
    }

    #[cfg(feature = "gpu-render")]
    pub fn submit_frame_gpu<C: 'static, T: 'static>(
        &mut self,
        qh: &QueueHandle<ShellState<C, T>>,
        queue: &wgpu::Queue,
        frame: wyrd_graphics::render::gpu::GpuPresented,
        damage: &[crate::render::damage::DamageRect],
    ) -> anyhow::Result<()> {
        let gpu_surface = self
            .gpu_surface
            .as_mut()
            .context("GPU surface is not initialized")?;
        let (buffer_width, buffer_height) = gpu_surface.dimensions();

        if let Some(viewport) = &self.viewport {
            viewport.set_destination(self.width as i32, self.height as i32);
        }
        self.surface.set_buffer_scale(1);
        self.frame_callback = Some(self.surface.frame(qh, ()));
        self.pending_frame = true;
        if damage.is_empty() {
            self.surface
                .damage_buffer(0, 0, buffer_width as i32, buffer_height as i32);
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
        gpu_surface.present(queue, frame);
        self.surface.commit();
        self.buffer = None;
        Ok(())
    }

    #[cfg(feature = "gpu-render")]
    pub fn gpu_surface(&mut self) -> Option<&mut super::gpu_surface::WaylandGpuSurface> {
        self.gpu_surface.as_mut()
    }

    pub fn kind(&self) -> SurfaceBackendKind {
        self.backend.kind()
    }

    pub fn capabilities(&self) -> SurfaceCapabilities {
        self.backend.capabilities()
    }

    pub fn disable_keyboard_interactivity(&self) {
        self.backend.disable_keyboard_interactivity();
    }

    pub fn configure(&mut self, cfg: &LayerSurfaceConfig) {
        self.ty = cfg.ty;
        self.width = cfg.width;
        self.height = cfg.height;
        self.margin = cfg.margin;
        self.anchors = cfg.anchors.clone();
        self.backend.configure(&self.surface, cfg);
        #[cfg(feature = "gpu-render")]
        self.sync_gpu_surface_size();
        self.dirty = true;
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.backend.resize(self.width, self.height);
        #[cfg(feature = "gpu-render")]
        self.sync_gpu_surface_size();
        self.dirty = true;
    }

    #[cfg(feature = "gpu-render")]
    pub fn sync_gpu_surface_size(&mut self) {
        if let Some(surface) = &mut self.gpu_surface {
            let width = (self.width as f64 * self.scale).round().max(1.0) as u32;
            let height = (self.height as f64 * self.scale).round().max(1.0) as u32;
            surface.resize(width, height);
        }
    }

    #[allow(clippy::mutable_key_type)]
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

        let pixmap_bytes = pixmap.data();
        if pixmap_bytes.len() != buf_size {
            anyhow::bail!(
                "pixmap buffer size {} does not match shared-memory buffer size {}",
                pixmap_bytes.len(),
                buf_size
            );
        }

        // SAFETY: `mmap_ptr` is a valid non-null `ShmMapping` pointing to `buf_size` bytes of
        // exclusively accessed shared memory (`&mut self`).
        let pixels = unsafe { std::slice::from_raw_parts_mut(mmap_ptr, buf_size) };

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

fn create_memfd(name: &str) -> anyhow::Result<OwnedFd> {
    if name.is_empty() {
        return Err(anyhow::anyhow!(
            "Wayland anonymous memory name must not be empty"
        ));
    }
    let name = CString::new(name).context("invalid Wayland anonymous memory name")?;
    // SAFETY: `memfd_create` is invoked with a valid null-terminated C string pointer.
    let raw_fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if raw_fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: `raw_fd` is a valid, open file descriptor returned by the kernel.
    Ok(unsafe { OwnedFd::from_raw_fd(raw_fd) })
}

fn calculate_shm_buffer_size(width: i32, height: i32) -> anyhow::Result<(usize, i32, i32)> {
    if width <= 0 || height <= 0 {
        return Err(anyhow::anyhow!(
            "Wayland shared-memory dimensions must be positive"
        ));
    }
    let stride = usize::try_from(
        i64::from(width)
            .checked_mul(4)
            .ok_or_else(|| anyhow::anyhow!("Wayland shared-memory stride overflow"))?,
    )
    .context("Wayland shared-memory stride is invalid")?;
    let size = stride
        .checked_mul(usize::try_from(height).context("invalid Wayland height")?)
        .context("Wayland shared-memory size overflow")?;
    let stride_i32 =
        i32::try_from(stride).context("Wayland shared-memory stride exceeds wl_shm limits")?;
    let size_i32 =
        i32::try_from(size).context("Wayland shared-memory size exceeds wl_shm limits")?;
    Ok((size, stride_i32, size_i32))
}

impl ShmBufferPool {
    #[allow(clippy::mutable_key_type)]
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
        *mut u8,
        usize,
    )> {
        let (size, stride_i32, size_i32) = calculate_shm_buffer_size(width, height)?;

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
            Ok((
                candidate.buffer.clone(),
                candidate.mmap_ptr.as_ptr(),
                candidate.size,
            ))
        } else {
            let fd = create_memfd(env!("CARGO_PKG_NAME"))?;
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
            let pool = shm.create_pool(fd.as_fd(), size_i32, qh, ());
            let buffer = pool.create_buffer(
                0,
                width,
                height,
                stride_i32,
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
            let mmap_ptr = shm_buf.mmap_ptr.as_ptr();
            let buf_size = shm_buf.size;
            buffers.push(shm_buf);
            Ok((buffer, mmap_ptr, buf_size))
        }
    }
}

/// Non-cloneable pointer handle for a mapped shared-memory region.
///
/// Implements [`Send`] so the buffer can be moved across threads, while intentionally
/// omitting [`Sync`] because concurrent reads/writes to the mapped byte slice without
/// external synchronization would cause data races. Access is restricted to the single
/// owner thread holding `&mut ShmBuffer`.
#[derive(Debug)]
pub struct ShmMapping(std::ptr::NonNull<u8>);

// SAFETY: Moving the raw pointer does not access the mapped memory. Dereferencing it is
// unsafe and remains the caller's responsibility to synchronize.
unsafe impl Send for ShmMapping {}

impl ShmMapping {
    /// Wraps a non-null `mmap` pointer.
    #[inline]
    pub fn new(ptr: std::ptr::NonNull<u8>) -> Self {
        Self(ptr)
    }

    /// Returns the raw mutable pointer to the mapped memory region.
    #[inline]
    pub fn as_ptr(&self) -> *mut u8 {
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
    pub xdg_wm_base: Option<XdgWmBase>,
    pub fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
    pub viewporter: Option<WpViewporter>,
    pub surfaces: Vec<LayerShellSurface>,
    pub active_backend: SurfaceBackendKind,
}

pub type SurfaceManagerState = SurfaceRegistry;

impl SurfaceRegistry {
    pub fn new(
        layer_shell: Option<ZwlrLayerShellV1>,
        fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
        viewporter: Option<WpViewporter>,
    ) -> Self {
        let active_backend = if layer_shell.is_some() {
            SurfaceBackendKind::ZwlrLayerShell
        } else {
            SurfaceBackendKind::None
        };
        Self {
            layer_shell,
            xdg_wm_base: None,
            fractional_scale_manager,
            viewporter,
            surfaces: Vec::new(),
            active_backend,
        }
    }

    pub fn with_xdg_shell(
        layer_shell: Option<ZwlrLayerShellV1>,
        xdg_wm_base: Option<XdgWmBase>,
        fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
        viewporter: Option<WpViewporter>,
        active_backend: SurfaceBackendKind,
    ) -> Self {
        Self {
            layer_shell,
            xdg_wm_base,
            fractional_scale_manager,
            viewporter,
            surfaces: Vec::new(),
            active_backend,
        }
    }

    pub fn get(&self, handle: SurfaceHandle) -> Option<&LayerShellSurface> {
        self.surfaces.get(handle.0)
    }

    pub fn get_mut(&mut self, handle: SurfaceHandle) -> Option<&mut LayerShellSurface> {
        self.surfaces.get_mut(handle.0)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_surface_with_backend<C: 'static, T: 'static>(
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
        backend_override: Option<&str>,
    ) -> SurfaceHandle {
        let chosen_backend_kind = match backend_override {
            Some("layer_shell") | Some("zwlr-layer-shell") | Some("zwlr_layer_shell") => {
                if self.layer_shell.is_some() {
                    SurfaceBackendKind::ZwlrLayerShell
                } else if self.xdg_wm_base.is_some() {
                    log::warn!("Requested zwlr-layer-shell backend override unavailable; falling back to XdgShell");
                    SurfaceBackendKind::XdgShell
                } else {
                    SurfaceBackendKind::None
                }
            }
            Some("xdg_shell") | Some("xdg-shell") => {
                if self.xdg_wm_base.is_some() {
                    SurfaceBackendKind::XdgShell
                } else {
                    SurfaceBackendKind::None
                }
            }
            Some("ext_layer_shell") | Some("ext-layer-shell") => {
                log::warn!("ext-layer-shell backend not available; falling back to active backend");
                self.active_backend
            }
            _ => self.active_backend,
        };

        log::debug!(
            "Creating {} surface '{}' (backend={}) output_bound={} size={}x{} layer={} exclusive_zone={}",
            if dynamic { "dynamic" } else { "configured" },
            config_name,
            chosen_backend_kind,
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

        let namespace = std::env::var("WYRD_NAMESPACE").unwrap_or_else(|_| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
                .unwrap_or_else(|| env!("CARGO_PKG_NAME").to_string())
        });

        let (backend, layer_surface) = match chosen_backend_kind {
            SurfaceBackendKind::ZwlrLayerShell => {
                if let Some(shell) = self.layer_shell.as_ref() {
                    let layer = match layer_name {
                        "overlay" => {
                            wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer::Overlay
                        }
                        "background" => {
                            wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer::Background
                        }
                        "bottom" => {
                            wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer::Bottom
                        }
                        _ => {
                            wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer::Top
                        }
                    };
                    let ls = shell.get_layer_surface(&surface, output, layer, namespace, qh, ());
                    (
                        SurfaceBackend::ZwlrLayerShell(ZwlrLayerShellBackend::new(ls.clone())),
                        Some(ls),
                    )
                } else {
                    (SurfaceBackend::None, None)
                }
            }
            SurfaceBackendKind::XdgShell => {
                if let Some(wm_base) = self.xdg_wm_base.as_ref() {
                    let xdg_surf = wm_base.get_xdg_surface(&surface, qh, ());
                    let toplevel = xdg_surf.get_toplevel(qh, ());
                    toplevel.set_app_id(namespace);
                    toplevel.set_title(config_name.to_owned());
                    if ty == SurfaceType::Background {
                        toplevel.set_fullscreen(output);
                    }
                    (
                        SurfaceBackend::XdgShell(XdgShellBackend::new(xdg_surf, toplevel)),
                        None,
                    )
                } else {
                    (SurfaceBackend::None, None)
                }
            }
            SurfaceBackendKind::ExtLayerShell => {
                log::warn!("ext-layer-shell not supported in this build; falling back to None");
                (SurfaceBackend::None, None)
            }
            SurfaceBackendKind::None => (SurfaceBackend::None, None),
        };

        let mut shell_surface = LayerShellSurface {
            config_name: config_name.to_owned(),
            dynamic,
            #[cfg(feature = "gpu-render")]
            gpu_surface: None,
            surface,
            layer_surface,
            backend,
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
        self.create_surface_with_backend(
            compositor,
            qh,
            ty,
            config_name,
            dynamic,
            output,
            width,
            height,
            layer_name,
            anchors,
            margin,
            exclusive_zone,
            keyboard,
            scale,
            None,
        )
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
            let (size, stride, size_limit) = calculate_shm_buffer_size(width, height)?;
            let fd = create_memfd(env!("CARGO_PKG_NAME"))?;
            // SAFETY: `fd` is a valid open file descriptor; resizing memory-backed fd to `size`.
            if unsafe { libc::ftruncate(fd.as_raw_fd(), size as i64) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let pool = shm.create_pool(fd.as_fd(), size_limit, qh, ());
            let buffer =
                pool.create_buffer(0, width, height, stride, wl_shm::Format::Xrgb8888, qh, 0);
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
            // SAFETY: `mapped` is non-null and points to `size` bytes of allocated, valid shared memory.
            let pixels = unsafe { std::slice::from_raw_parts_mut(mapped.cast::<u8>(), size) };
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.copy_from_slice(&color);
            }
            // SAFETY: Unmapping the temporary `mapped` memory pointer of size `size`.
            unsafe {
                libc::munmap(mapped, size);
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
        if surface.layer_surface.is_some() || matches!(surface.backend, SurfaceBackend::XdgShell(_))
        {
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
                surface.backend.destroy();
                surface.surface.destroy();
                removed += 1;
                false
            } else {
                true
            }
        });
        removed
    }

    #[allow(clippy::mutable_key_type)]
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

impl<C: 'static, T: 'static> wayland_client::Dispatch<XdgWmBase, ()> for ShellState<C, T> {
    fn event(
        _state: &mut Self,
        proxy: &XdgWmBase,
        event: xdg_wm_base::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            proxy.pong(serial);
        }
    }
}

impl<C: 'static, T: 'static> wayland_client::Dispatch<XdgSurface, ()> for ShellState<C, T> {
    fn event(
        state: &mut Self,
        proxy: &XdgSurface,
        event: xdg_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            proxy.ack_configure(serial);
            state.frame_ready = true;
            log::debug!("XDG surface configured with serial {}", serial);
        }
    }
}

impl<C: 'static, T: 'static> wayland_client::Dispatch<XdgToplevel, ()> for ShellState<C, T> {
    fn event(
        state: &mut Self,
        proxy: &XdgToplevel,
        event: xdg_toplevel::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states: _,
            } => {
                let w = if width > 0 { width as u32 } else { 0 };
                let h = if height > 0 { height as u32 } else { 0 };
                state.layer_surface_sizes.insert(proxy.id(), (w, h));
                state.frame_ready = true;
                log::debug!("XDG toplevel configured: {}x{}", width, height);
            }
            xdg_toplevel::Event::Close => {
                log::info!("XDG toplevel closed by compositor");
            }
            _ => {}
        }
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
        proxy: &wayland_client::protocol::wl_callback::WlCallback,
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
            if state.pending_roundtrip.as_ref() == Some(&proxy.id()) {
                state.pending_roundtrip = None;
            }
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
    fn test_calculate_shm_buffer_size() {
        let (size, stride, size_limit) = calculate_shm_buffer_size(64, 32).unwrap();
        assert_eq!(size, 8192);
        assert_eq!(stride, 256);
        assert_eq!(size_limit, 8192);
    }

    #[test]
    fn test_calculate_shm_buffer_rejects_invalid_dimensions() {
        assert!(calculate_shm_buffer_size(-1, 32).is_err());
        assert!(calculate_shm_buffer_size(0, 0).is_err());
        assert!(calculate_shm_buffer_size(i32::MAX, i32::MAX).is_err());
    }

    #[test]
    fn test_create_memfd_returns_owned_descriptor() {
        let fd = create_memfd("wyrd_integration_test").unwrap();
        assert!(fd.as_raw_fd() >= 0);
        // SAFETY: `fd` is an open descriptor and F_GETFD does not mutate it.
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0);
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
    }

    #[test]
    fn test_create_memfd_rejects_empty_name() {
        assert!(create_memfd("").is_err());
    }

    #[test]
    fn test_shm_buffer_alloc_write_and_drop() {
        let width: usize = 64;
        let height: usize = 32;
        let size = width * height * 4;
        let name = CString::new("wyrd_shm_test").unwrap();

        // SAFETY: `name` is a valid null-terminated CString; `memfd_create` creates an anonymous fd.
        let raw_fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
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

    #[test]
    fn backend_priority_order() {
        // ext-layer-shell-v1 is not compiled, so auto selects zwlr before xdg-shell.
        let proto_wlroots = crate::wayland::ProtocolSupport {
            ext_layer_shell: false,
            zwlr_layer_shell: true,
            xdg_shell: true,
            ..Default::default()
        };
        assert_eq!(
            crate::wayland::backend::select_backend(&proto_wlroots, None).unwrap(),
            SurfaceBackendKind::ZwlrLayerShell
        );

        let proto_xdg = crate::wayland::ProtocolSupport {
            ext_layer_shell: false,
            zwlr_layer_shell: false,
            xdg_shell: true,
            ..Default::default()
        };
        assert_eq!(
            crate::wayland::backend::select_backend(&proto_xdg, None).unwrap(),
            SurfaceBackendKind::XdgShell
        );

        let proto_none = crate::wayland::ProtocolSupport {
            ext_layer_shell: false,
            zwlr_layer_shell: false,
            xdg_shell: false,
            ..Default::default()
        };
        assert!(crate::wayland::backend::select_backend(&proto_none, None).is_err());
    }

    #[test]
    fn ext_layer_shell_is_not_compiled() {
        assert!(ExtLayerShellBackend::probe().is_none());
        let protocol = crate::wayland::ProtocolSupport {
            ext_layer_shell: true,
            zwlr_layer_shell: true,
            xdg_shell: true,
            ..Default::default()
        };
        assert_eq!(
            crate::wayland::backend::select_backend(&protocol, None).unwrap(),
            SurfaceBackendKind::ZwlrLayerShell
        );
        let error = crate::wayland::backend::select_backend(&protocol, Some("ext-layer-shell"))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "ext-layer-shell-v1 is not available in 0.1.0; WYRD_FORCE_BACKEND=ext-layer-shell cannot be used"
        );
    }

    #[test]
    fn capabilities_for_xdg_shell() {
        let caps = SurfaceCapabilities::for_kind(SurfaceBackendKind::XdgShell);
        assert!(!caps.exclusive_zone);
        assert!(!caps.anchoring);
        assert!(!caps.layer_ordering);
        assert!(!caps.keyboard_interactivity);
        assert!(caps.set_size);
        assert!(!caps.popup_positioning);
    }

    #[test]
    fn capabilities_for_zwlr_layer_shell() {
        let caps = SurfaceCapabilities::for_kind(SurfaceBackendKind::ZwlrLayerShell);
        assert!(caps.exclusive_zone);
        assert!(caps.anchoring);
        assert!(caps.layer_ordering);
        assert!(caps.keyboard_interactivity);
        assert!(caps.set_size);
        assert!(caps.popup_positioning);
    }

    #[test]
    fn test_mock_global_list_environments() {
        // 1. wlroots environment (has zwlr-layer-shell-v1)
        let wlroots_globals = [
            "wl_compositor",
            "wl_shm",
            "wl_seat",
            "wl_output",
            "zwlr_layer_shell_v1",
            "xdg_wm_base",
            "wp_fractional_scale_manager_v1",
            "wp_viewporter",
        ];
        let wlroots_proto = crate::wayland::ProtocolSupport {
            zwlr_layer_shell: wlroots_globals.contains(&"zwlr_layer_shell_v1"),
            xdg_shell: wlroots_globals.contains(&"xdg_wm_base"),
            fractional_scale: wlroots_globals.contains(&"wp_fractional_scale_manager_v1"),
            viewporter: wlroots_globals.contains(&"wp_viewporter"),
            ..Default::default()
        };
        assert_eq!(
            crate::wayland::backend::select_backend(&wlroots_proto, None).unwrap(),
            SurfaceBackendKind::ZwlrLayerShell,
            "wlroots environment with zwlr-layer-shell-v1 must select ZwlrLayerShell"
        );

        // 2. GNOME-like environment (has xdg_wm_base, no layer-shell)
        let gnome_globals = [
            "wl_compositor",
            "wl_shm",
            "wl_seat",
            "wl_output",
            "xdg_wm_base",
            "wp_fractional_scale_manager_v1",
            "wp_viewporter",
        ];
        let gnome_proto = crate::wayland::ProtocolSupport {
            zwlr_layer_shell: gnome_globals.contains(&"zwlr_layer_shell_v1"),
            xdg_shell: gnome_globals.contains(&"xdg_wm_base"),
            fractional_scale: gnome_globals.contains(&"wp_fractional_scale_manager_v1"),
            viewporter: gnome_globals.contains(&"wp_viewporter"),
            ..Default::default()
        };
        assert_eq!(
            crate::wayland::backend::select_backend(&gnome_proto, None).unwrap(),
            SurfaceBackendKind::XdgShell,
            "GNOME-like environment without layer-shell must select XdgShell"
        );

        // 3. Minimal environment (only wl_compositor, wl_shm, wl_seat, xdg_wm_base)
        let minimal_globals = ["wl_compositor", "wl_shm", "wl_seat", "xdg_wm_base"];
        let minimal_proto = crate::wayland::ProtocolSupport {
            zwlr_layer_shell: minimal_globals.contains(&"zwlr_layer_shell_v1"),
            xdg_shell: minimal_globals.contains(&"xdg_wm_base"),
            ..Default::default()
        };
        assert_eq!(
            crate::wayland::backend::select_backend(&minimal_proto, None).unwrap(),
            SurfaceBackendKind::XdgShell,
            "Minimal environment must select XdgShell fallback"
        );
    }

    #[test]
    fn test_force_backend_env_var() {
        let proto = crate::wayland::ProtocolSupport {
            zwlr_layer_shell: true,
            xdg_shell: true,
            ..Default::default()
        };

        assert_eq!(
            crate::wayland::backend::select_backend(&proto, Some("xdg-shell")).unwrap(),
            SurfaceBackendKind::XdgShell
        );
        assert_eq!(
            crate::wayland::backend::select_backend(&proto, Some("zwlr-layer-shell")).unwrap(),
            SurfaceBackendKind::ZwlrLayerShell
        );
        assert_eq!(
            crate::wayland::backend::select_backend(&proto, Some("auto")).unwrap(),
            SurfaceBackendKind::ZwlrLayerShell
        );
        assert!(crate::wayland::backend::select_backend(&proto, Some("invalid-backend")).is_err());
    }
}
