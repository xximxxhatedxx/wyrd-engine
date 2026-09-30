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
