use anyhow::{anyhow, Context, Result};
use std::ptr::NonNull;
use wayland_client::{protocol::wl_surface::WlSurface, Connection, Proxy};
use wyrd_graphics::render::gpu::GpuRenderer;

pub struct WaylandGpuSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    _instance: wgpu::Instance,
    _connection: Connection,
    _wl_surface: WlSurface,
    device: wgpu::Device,
    backbuffer: Option<wgpu::Texture>,
    initialized: bool,
}

impl WaylandGpuSurface {
    pub async fn new_renderer(
        connection: &Connection,
        wl_surface: &WlSurface,
        width: u32,
        height: u32,
    ) -> Result<(GpuRenderer, Self)> {
        let instance = wyrd_graphics::render::gpu::GpuRenderer::create_wgpu_instance();
        let surface = Self::create_wayland_surface(&instance, connection, wl_surface)?;
        let renderer = GpuRenderer::new_for_surface(instance, &surface).await?;
        let target =
            Self::configure_surface(surface, connection, wl_surface, &renderer, width, height)?;
        Ok((renderer, target))
    }

    pub fn new(
        connection: &Connection,
        wl_surface: &WlSurface,
        renderer: &GpuRenderer,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let surface = Self::create_wayland_surface(&renderer.instance, connection, wl_surface)?;
        Self::configure_surface(surface, connection, wl_surface, renderer, width, height)
    }

    fn create_wayland_surface(
        instance: &wgpu::Instance,
        connection: &Connection,
        wl_surface: &WlSurface,
    ) -> Result<wgpu::Surface<'static>> {
        let display_ptr = connection.backend().display_ptr();
        let surface_ptr = wl_surface.id().as_ptr();
        let display = NonNull::new(display_ptr.cast())
            .context("wayland-client returned a null wl_display pointer")?;
        let window = NonNull::new(surface_ptr.cast())
            .context("wayland-client returned a null wl_surface pointer")?;

        let raw_display_handle =
            wgpu::rwh::RawDisplayHandle::Wayland(wgpu::rwh::WaylandDisplayHandle::new(display));
        let raw_window_handle =
            wgpu::rwh::RawWindowHandle::Wayland(wgpu::rwh::WaylandWindowHandle::new(window));

        // SAFETY: `connection` and `wl_surface` are owned by the containing Wayland backend and
        // outlive this target. The target is dropped before the Wayland surface is destroyed.
        unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(raw_display_handle),
                raw_window_handle,
            })
        }
        .context("failed to create wgpu surface from Wayland handles")
    }

    fn configure_surface(
        surface: wgpu::Surface<'static>,
        connection: &Connection,
        wl_surface: &WlSurface,
        renderer: &GpuRenderer,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let capabilities = surface.get_capabilities(&renderer.adapter);
        anyhow::ensure!(
            !capabilities.formats.is_empty(),
            "selected GPU adapter does not support a surface format for Wayland"
        );
        anyhow::ensure!(
            capabilities
                .usages
                .contains(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST),
            "Wayland surface does not support render-attachment and copy-destination usage"
        );
        let config = surface
            .get_default_config(&renderer.adapter, width.max(1), height.max(1))
            .ok_or_else(|| anyhow!("wgpu adapter does not support this Wayland surface"))?;
        anyhow::ensure!(
            capabilities.formats.contains(&config.format),
            "wgpu selected unsupported Wayland surface format {:?}",
            config.format
        );
        let mut config = config;
        config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST;
        surface.configure(&renderer.device, &config);

        log::debug!(
            "created wgpu Wayland surface with supported format {:?}",
            config.format
        );
        Ok(Self {
            surface,
            config,
            _instance: renderer.instance.clone(),
            _connection: connection.clone(),
            _wl_surface: wl_surface.clone(),
            device: renderer.device.clone(),
            backbuffer: None,
            initialized: false,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if (self.config.width, self.config.height) != (width, height) {
            self.config.width = width;
            self.config.height = height;
            let device = self.device.clone();
            self.surface.configure(&device, &self.config);
            self.backbuffer = None;
            self.initialized = false;
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn acquire(&mut self, device: &wgpu::Device) -> Result<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok(frame),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(device, &self.config);
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok(frame),
                    _ => {
                        anyhow::bail!("Wayland wgpu surface remained unavailable after reconfigure")
                    }
                }
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                anyhow::bail!("Wayland wgpu surface frame acquisition timed out")
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                anyhow::bail!("Wayland wgpu surface is occluded")
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("Wayland wgpu surface reported a validation error")
            }
        }
    }

    pub fn present(
        &self,
        queue: &wgpu::Queue,
        frame: wyrd_graphics::render::gpu::GpuPresented,
    ) -> u64 {
        let frame_id = frame.present(queue);
        log::trace!("presented Wayland GPU frame {frame_id}");
        frame_id
    }

    pub fn surface(&self) -> &wgpu::Surface<'static> {
        &self.surface
    }
}

impl wyrd_graphics::render::gpu::GpuRenderTarget for WaylandGpuSurface {
    fn acquire_frame(&mut self) -> Result<wyrd_graphics::render::gpu::GpuRenderFrame> {
        let device = self.device.clone();
        let surface_frame = self.acquire(&device)?;
        let backbuffer = self.backbuffer.get_or_insert_with(|| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("wyrd Wayland persistent damage buffer"),
                size: wgpu::Extent3d {
                    width: self.config.width,
                    height: self.config.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        });
        let view = backbuffer.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(
            wyrd_graphics::render::gpu::GpuRenderFrame::from_persistent_surface_target(
                view,
                backbuffer.clone(),
                surface_frame,
            ),
        )
    }

    fn preserves_contents(&self) -> bool {
        self.initialized
    }

    fn encode_present(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wyrd_graphics::render::gpu::GpuRenderFrame,
    ) -> Result<()> {
        let backbuffer = self
            .backbuffer
            .as_ref()
            .context("persistent Wayland GPU backbuffer was not allocated")?;
        let surface_frame = frame
            .surface_texture()
            .context("Wayland GPU frame has no acquired surface texture")?;
        encoder.copy_texture_to_texture(
            backbuffer.as_image_copy(),
            surface_frame.texture.as_image_copy(),
            wgpu::Extent3d {
                width: self.config.width,
                height: self.config.height,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    fn mark_rendered(&mut self) {
        self.initialized = true;
    }
}
