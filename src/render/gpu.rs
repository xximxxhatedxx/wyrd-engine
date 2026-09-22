//! GPU device initialization for the shell renderer.

#[cfg(feature = "gpu-render")]
use anyhow::Context;
use anyhow::Result;

#[cfg(feature = "gpu-render")]
pub struct GpuRenderer {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

#[cfg(feature = "gpu-render")]
impl GpuRenderer {
    pub async fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .context("no compatible GPU adapter found")?;
        let info = adapter.get_info();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .context("failed to create GPU device")?;
        log::info!(
            "GPU renderer initialized: {} ({:?})",
            info.name,
            info.backend
        );
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
        })
    }

    pub fn supports_storage_textures(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
    }

    /// Renders a SceneGraph to the target surface.
    ///
    /// # Why this is intentionally a no-op
    ///
    /// This stub is a deliberate architectural hold-point, not an oversight.
    ///
    /// The caller (`render_surface` in `mod.rs`) already falls through to
    /// `scene_to_pixmap` unconditionally after this call, so GPU mode produces
    /// correct output via the CPU path — there is no visual regression.
    ///
    /// An intermediate implementation that CPU-rasterizes the scene and round-
    /// trips it through a wgpu texture upload + GPU→CPU readback was evaluated
    /// and rejected: it would add real per-frame overhead (staging buffer alloc,
    /// PCIe round-trip, sync stall) while producing byte-identical output and
    /// laying no useful groundwork for the real pipeline.
    ///
    /// # What the real GPU pipeline requires (tracked as future work)
    ///
    /// A real GPU pipeline must solve these together:
    ///
    /// 1. **`wgpu::Surface` tied to the Wayland `wl_surface`** — this renderer
    ///    is currently headless (`new_without_display_handle()`). Surface setup
    ///    requires the raw-window-handle at init time and changes how
    ///    `GpuRenderer::new()` is called from `main.rs`.
    /// 2. **GPU-side render targets** — for blur/glass effects, the frame must
    ///    stay on the GPU between passes; readback defeats the purpose.
    /// 3. **At least one real GPU effect** (e.g. dual-Kawase blur for the
    ///    glass/frosted-panel aesthetic) that is the user-visible payoff.
    /// 4. **Output path decision**: dmabuf zero-copy import (ideal) or wl_shm
    ///    readback (acceptable interim). Neither is wired up yet.
    ///
    /// Until all four exist together, this stub is correct as-is.
    pub fn render_scene(&self, _scene: &super::scene::SceneGraph) -> Result<()> {
        Ok(())
    }
}

#[cfg(not(feature = "gpu-render"))]
pub struct GpuRenderer;

#[cfg(not(feature = "gpu-render"))]
impl GpuRenderer {
    pub async fn new() -> Result<Self> {
        anyhow::bail!("GPU rendering feature 'gpu-render' was disabled at compile time")
    }

    pub fn supports_storage_textures(&self) -> bool {
        false
    }

    pub fn render_scene(&self, _scene: &super::scene::SceneGraph) -> Result<()> {
        anyhow::bail!("GPU rendering feature 'gpu-render' was disabled at compile time")
    }
}
