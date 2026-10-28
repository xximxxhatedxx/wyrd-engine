use anyhow::Result;

#[cfg(feature = "gpu-render")]
use std::collections::HashMap;
#[cfg(feature = "gpu-render")]
use std::sync::Arc;

#[cfg(feature = "gpu-render")]
pub struct GpuRenderFrame {
    pub(crate) view: wgpu::TextureView,
    pub(crate) source_texture: wgpu::Texture,
    pub(crate) format: wgpu::TextureFormat,
    pub(crate) width: u32,
    pub(crate) height: u32,
    surface_texture: Option<wgpu::SurfaceTexture>,
}

#[cfg(feature = "gpu-render")]
impl GpuRenderFrame {
    pub fn from_surface_texture(surface_texture: wgpu::SurfaceTexture) -> Self {
        let texture = &surface_texture.texture;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            view,
            source_texture: texture.clone(),
            format: texture.format(),
            width: texture.width(),
            height: texture.height(),
            surface_texture: Some(surface_texture),
        }
    }

    pub fn from_persistent_surface_target(
        view: wgpu::TextureView,
        source_texture: wgpu::Texture,
        surface_texture: wgpu::SurfaceTexture,
    ) -> Self {
        let texture = &surface_texture.texture;
        Self {
            view,
            source_texture,
            format: texture.format(),
            width: texture.width(),
            height: texture.height(),
            surface_texture: Some(surface_texture),
        }
    }

    pub fn from_texture_view(
        view: wgpu::TextureView,
        source_texture: wgpu::Texture,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            view,
            source_texture,
            format,
            width,
            height,
            surface_texture: None,
        }
    }

    pub fn surface_texture(&self) -> Option<&wgpu::SurfaceTexture> {
        self.surface_texture.as_ref()
    }
}

#[cfg(feature = "gpu-render")]
pub struct GpuPresented {
    pub frame_id: u64,
    frame: GpuRenderFrame,
}

#[cfg(feature = "gpu-render")]
impl GpuPresented {
    pub fn present(self, queue: &wgpu::Queue) -> u64 {
        if let Some(frame) = self.frame.surface_texture {
            queue.present(frame);
        }
        self.frame_id
    }
}

#[cfg(feature = "gpu-render")]
pub trait GpuRenderTarget {
    fn acquire_frame(&mut self) -> Result<GpuRenderFrame>;
    fn preserves_contents(&self) -> bool {
        false
    }
    fn encode_present(
        &mut self,
        _encoder: &mut wgpu::CommandEncoder,
        _frame: &GpuRenderFrame,
    ) -> Result<()> {
        Ok(())
    }
    fn mark_rendered(&mut self) {}
}

#[cfg(feature = "gpu-render")]
pub struct OffscreenGpuTarget {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    initialized: bool,
}

#[cfg(feature = "gpu-render")]
impl OffscreenGpuTarget {
    pub fn new(renderer: &GpuRenderer, width: u32, height: u32) -> Result<Self> {
        anyhow::ensure!(
            width > 0 && height > 0,
            "GPU target dimensions must be positive"
        );
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let texture = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wyrd offscreen target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Ok(Self {
            texture,
            width,
            height,
            format,
            initialized: false,
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn estimated_bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 4
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
}

#[cfg(feature = "gpu-render")]
impl GpuRenderTarget for OffscreenGpuTarget {
    fn acquire_frame(&mut self) -> Result<GpuRenderFrame> {
        let view = self
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        Ok(GpuRenderFrame::from_texture_view(
            view,
            self.texture.clone(),
            self.format,
            self.width,
            self.height,
        ))
    }

    fn preserves_contents(&self) -> bool {
        self.initialized
    }

    fn mark_rendered(&mut self) {
        self.initialized = true;
    }
}

// repr(C) guarantees deterministic 96-byte memory layout matching `ClipRegion` in `scene.wgsl`:
// bounds: 16B, corner_radius: 4B, _pad: 12B, inverse_transform: 64B (mat4x4 requires 16-byte alignment).
#[cfg(feature = "gpu-render")]
#[repr(C)]
#[derive(Clone)]
struct GpuClipRegion {
    bounds: [f32; 4],
    corner_radius: f32,
    _pad: [f32; 3],
    inverse_transform: [f32; 16],
}

#[cfg(feature = "gpu-render")]
struct DrawInstance {
    bounds: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
    extra: [f32; 4],
    clip_regions: Vec<GpuClipRegion>,
    matrix: [f32; 16],
    gradient_stops: Vec<(f32, tiny_skia::Color)>,
    path_segments: Vec<[f32; 4]>,
    texture_key: Option<String>,
}

#[cfg(feature = "gpu-render")]
struct GpuTimestampQueries {
    query_set: wgpu::QuerySet,
    resolve_buffer: wgpu::Buffer,
    readback_buffer: wgpu::Buffer,
}

#[cfg(feature = "gpu-render")]
struct GpuTextureAsset {
    _texture: wgpu::Texture,
    _view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
    last_used: u64,
}

#[cfg(feature = "gpu-render")]
#[derive(Clone, Copy)]
struct GlyphAtlasEntry {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    left: i32,
    top: i32,
    color: bool,
    last_used: u64,
}

#[cfg(feature = "gpu-render")]
struct GlyphAtlas {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
    glyphs: HashMap<cosmic_text::CacheKey, GlyphAtlasEntry>,
    pen_x: u32,
    pen_y: u32,
    row_height: u32,
    access_seq: u64,
}

#[cfg(feature = "gpu-render")]
struct TextLayoutCacheEntry {
    glyphs: Vec<(cosmic_text::PhysicalGlyph, Option<cosmic_text::Color>)>,
    vertical_offset: f32,
    last_used: u64,
}

#[cfg(feature = "gpu-render")]
impl GlyphAtlas {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let width = 2048;
        let height = 2048;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wyrd glyph atlas"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("wyrd glyph atlas nearest sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let bind_group = GpuRenderer::create_texture_bind_group(device, layout, &sampler, &view);
        Self {
            texture,
            bind_group,
            width,
            height,
            glyphs: HashMap::new(),
            pen_x: 1,
            pen_y: 1,
            row_height: 0,
            access_seq: 0,
        }
    }
}

#[cfg(feature = "gpu-render")]
struct BlurIntermediate {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
}

#[cfg(feature = "gpu-render")]
impl DrawInstance {
    fn append_bytes(&self, dst: &mut Vec<u8>, extra: [f32; 4], clip_range: [u32; 2]) {
        for value in self
            .bounds
            .iter()
            .chain(self.color.iter())
            .chain(self.params.iter())
            .chain(extra.iter())
        {
            dst.extend_from_slice(&value.to_ne_bytes());
        }
        for value in clip_range {
            dst.extend_from_slice(&value.to_ne_bytes());
        }
        dst.extend_from_slice(&[0; 8]);
    }
}

#[cfg(feature = "gpu-render")]
pub struct GpuRenderer {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    shader: wgpu::ShaderModule,
    bind_group_layout: wgpu::BindGroupLayout,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    texture_sampler: wgpu::Sampler,
    _white_texture: wgpu::Texture,
    _white_texture_view: wgpu::TextureView,
    white_texture_bind_group: wgpu::BindGroup,
    glyph_atlas: Option<GlyphAtlas>,
    text_layout_cache: HashMap<String, TextLayoutCacheEntry>,
    text_layout_access_seq: u64,
    texture_assets: HashMap<String, GpuTextureAsset>,
    texture_cache_capacity: usize,
    texture_access_seq: u64,
    text_context: Box<super::context::RenderContext>,
    blur_shader: wgpu::ShaderModule,
    blur_bind_group_layout: wgpu::BindGroupLayout,
    blur_sampler: wgpu::Sampler,
    blur_params: Vec<(wgpu::Buffer, wgpu::Buffer)>,
    blur_pipelines: HashMap<wgpu::TextureFormat, Arc<wgpu::RenderPipeline>>,
    blur_intermediate: Option<BlurIntermediate>,
    viewport_buffer: wgpu::Buffer,
    instances_buffer: wgpu::Buffer,
    instances_capacity: u64,
    matrices_buffer: wgpu::Buffer,
    gradient_stops_buffer: wgpu::Buffer,
    gradient_stops_capacity: u64,
    path_segments_buffer: wgpu::Buffer,
    path_segments_capacity: u64,
    clip_regions_buffer: wgpu::Buffer,
    clip_regions_capacity: u64,
    bind_group: wgpu::BindGroup,
    pipelines: HashMap<(wgpu::TextureFormat, bool), Arc<wgpu::RenderPipeline>>,
    timestamps: Option<GpuTimestampQueries>,
    frame_id: u64,
}

#[cfg(feature = "gpu-render")]
impl GpuRenderer {
    pub fn create_wgpu_instance() -> wgpu::Instance {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = descriptor.backends.with_env();
        wgpu::Instance::new(descriptor)
    }

    fn tessellate_path(path: &tiny_skia::Path, origin_x: f32, origin_y: f32) -> Vec<[f32; 4]> {
        use tiny_skia::PathSegment;
        let mut segments = Vec::new();
        let mut current = None;
        let mut subpath_start = None;
        let mut push = |from: tiny_skia::Point, to: tiny_skia::Point| {
            segments.push([
                from.x - origin_x,
                from.y - origin_y,
                to.x - origin_x,
                to.y - origin_y,
            ]);
        };
        for segment in path.segments() {
            match segment {
                PathSegment::MoveTo(point) => {
                    current = Some(point);
                    subpath_start = Some(point);
                }
                PathSegment::LineTo(point) => {
                    if let Some(from) = current {
                        push(from, point);
                    }
                    current = Some(point);
                }
                PathSegment::QuadTo(control, end) => {
                    if let Some(start) = current {
                        let mut previous = start;
                        for step in 1..=12 {
                            let t = step as f32 / 12.0;
                            let inverse = 1.0 - t;
                            let point = tiny_skia::Point::from_xy(
                                inverse * inverse * start.x
                                    + 2.0 * inverse * t * control.x
                                    + t * t * end.x,
                                inverse * inverse * start.y
                                    + 2.0 * inverse * t * control.y
                                    + t * t * end.y,
                            );
                            push(previous, point);
                            previous = point;
                        }
                    }
                    current = Some(end);
                }
                PathSegment::CubicTo(control_1, control_2, end) => {
                    if let Some(start) = current {
                        let mut previous = start;
                        for step in 1..=16 {
                            let t = step as f32 / 16.0;
                            let inverse = 1.0 - t;
                            let point = tiny_skia::Point::from_xy(
                                inverse.powi(3) * start.x
                                    + 3.0 * inverse.powi(2) * t * control_1.x
                                    + 3.0 * inverse * t * t * control_2.x
                                    + t.powi(3) * end.x,
                                inverse.powi(3) * start.y
                                    + 3.0 * inverse.powi(2) * t * control_1.y
                                    + 3.0 * inverse * t * t * control_2.y
                                    + t.powi(3) * end.y,
                            );
                            push(previous, point);
                            previous = point;
                        }
                    }
                    current = Some(end);
                }
                PathSegment::Close => {
                    if let (Some(from), Some(to)) = (current, subpath_start) {
                        push(from, to);
                        current = Some(to);
                    }
                }
            }
        }
        segments
    }

    fn world_transform(
        scene: &super::scene::SceneGraph,
        node_id: super::scene::SceneNodeId,
    ) -> glam::Mat4 {
        let mut chain = Vec::new();
        let mut current = Some(node_id);
        while let Some(id) = current {
            let Some(node) = scene.get(id) else {
                break;
            };
            chain.push(node.transform.matrix);
            current = node.parent;
        }
        chain
            .into_iter()
            .rev()
            .fold(glam::Mat4::IDENTITY, |parent, local| parent * local)
    }

    fn world_opacity(scene: &super::scene::SceneGraph, node_id: super::scene::SceneNodeId) -> f32 {
        let mut opacity = 1.0;
        let mut current = Some(node_id);
        while let Some(id) = current {
            let Some(node) = scene.get(id) else {
                break;
            };
            opacity *= node.transform.opacity.clamp(0.0, 1.0);
            current = node.parent;
        }
        opacity
    }

    fn transformed_bounds(
        matrix: glam::Mat4,
        bounds: (f32, f32, f32, f32),
    ) -> (f32, f32, f32, f32) {
        let (x, y, width, height) = bounds;
        let corners = [
            glam::Vec3::new(x, y, 0.0),
            glam::Vec3::new(x + width, y, 0.0),
            glam::Vec3::new(x, y + height, 0.0),
            glam::Vec3::new(x + width, y + height, 0.0),
        ];
        let mut min = glam::Vec2::splat(f32::INFINITY);
        let mut max = glam::Vec2::splat(f32::NEG_INFINITY);
        for corner in corners {
            let point = matrix.project_point3(corner).truncate();
            min = min.min(point);
            max = max.max(point);
        }
        (min.x, min.y, max.x - min.x, max.y - min.y)
    }

    pub async fn new() -> Result<Self> {
        let instance = Self::create_wgpu_instance();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await?;
        Self::new_with_adapter(instance, adapter).await
    }

    pub async fn new_for_surface(
        instance: wgpu::Instance,
        surface: &wgpu::Surface<'_>,
    ) -> Result<Self> {
        let mut adapters = instance.enumerate_adapters(wgpu::Backends::all()).await;
        adapters.sort_by_key(|adapter| match adapter.get_info().device_type {
            wgpu::DeviceType::DiscreteGpu => 0,
            wgpu::DeviceType::IntegratedGpu => 1,
            wgpu::DeviceType::VirtualGpu => 2,
            wgpu::DeviceType::Cpu => 3,
            wgpu::DeviceType::Other => 4,
        });
        let mut selected = None;
        for adapter in adapters {
            let capabilities = surface.get_capabilities(&adapter);
            let format = capabilities
                .formats
                .iter()
                .copied()
                .find(|format| format.is_srgb())
                .or_else(|| capabilities.formats.first().copied());
            if let (Some(format), Some(present_mode)) =
                (format, capabilities.present_modes.first().copied())
            {
                selected = Some((adapter, format, present_mode));
                break;
            }
        }
        let (adapter, format, present_mode) = selected.ok_or_else(|| {
            anyhow::anyhow!("no GPU adapter exposes a supported surface format and present mode")
        })?;
        log::info!(
            "selected GPU adapter for surface: {} ({:?}), format {:?}, present mode {:?}",
            adapter.get_info().name,
            adapter.get_info().backend,
            format,
            present_mode
        );
        Self::new_with_adapter(instance, adapter).await
    }

    async fn new_with_adapter(instance: wgpu::Instance, adapter: wgpu::Adapter) -> Result<Self> {
        let info = adapter.get_info();
        let timestamp_supported = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let mut device_descriptor = wgpu::DeviceDescriptor::default();
        if timestamp_supported {
            device_descriptor.required_features = wgpu::Features::TIMESTAMP_QUERY;
        }
        let (device, queue) = adapter.request_device(&device_descriptor).await?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wyrd scene shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/scene.wgsl").into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("wyrd scene bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("wyrd sampled texture layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let texture_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("wyrd linear texture sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wyrd separable Gaussian blur shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blur.wgsl").into()),
        });
        let blur_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("wyrd blur bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let blur_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("wyrd blur sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let white_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wyrd default white texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            white_texture.as_image_copy(),
            &[255, 255, 255, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_texture_view = white_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let white_texture_bind_group = Self::create_texture_bind_group(
            &device,
            &texture_bind_group_layout,
            &texture_sampler,
            &white_texture_view,
        );
        let viewport_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wyrd viewport uniforms"),
            size: 8,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instances_capacity = 80 * 64;
        let instances_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wyrd scene instances"),
            size: instances_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let matrices_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wyrd scene transforms"),
            size: instances_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let gradient_stops_capacity = 16;
        let gradient_stops_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wyrd gradient stops"),
            size: gradient_stops_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let path_segments_capacity = 16;
        let path_segments_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wyrd custom path segments"),
            size: path_segments_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let clip_regions_capacity = 96;
        let clip_regions_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wyrd scene clip regions"),
            size: clip_regions_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = Self::create_bind_group(
            &device,
            &bind_group_layout,
            &viewport_buffer,
            &instances_buffer,
            &matrices_buffer,
            &gradient_stops_buffer,
            &path_segments_buffer,
            &clip_regions_buffer,
        );
        let timestamps = timestamp_supported.then(|| {
            let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("wyrd render timestamp queries"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            });
            let resolve_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("wyrd timestamp resolve buffer"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("wyrd timestamp readback buffer"),
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            GpuTimestampQueries {
                query_set,
                resolve_buffer,
                readback_buffer,
            }
        });

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
            shader,
            bind_group_layout,
            texture_bind_group_layout,
            texture_sampler,
            _white_texture: white_texture,
            _white_texture_view: white_texture_view,
            white_texture_bind_group,
            glyph_atlas: None,
            text_layout_cache: HashMap::new(),
            text_layout_access_seq: 0,
            texture_assets: HashMap::new(),
            texture_cache_capacity: 256,
            texture_access_seq: 0,
            text_context: Box::new(super::context::RenderContext::new(1.0)),
            blur_shader,
            blur_bind_group_layout,
            blur_sampler,
            blur_params: Vec::new(),
            blur_pipelines: HashMap::new(),
            blur_intermediate: None,
            viewport_buffer,
            instances_buffer,
            instances_capacity,
            matrices_buffer,
            gradient_stops_buffer,
            gradient_stops_capacity,
            path_segments_buffer,
            path_segments_capacity,
            clip_regions_buffer,
            clip_regions_capacity,
            bind_group,
            pipelines: HashMap::new(),
            timestamps,
            frame_id: 0,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn create_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        viewport: &wgpu::Buffer,
        instances: &wgpu::Buffer,
        matrices: &wgpu::Buffer,
        gradient_stops: &wgpu::Buffer,
        path_segments: &wgpu::Buffer,
        clip_regions: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wyrd scene bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: viewport.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: instances.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: matrices.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: gradient_stops.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: path_segments.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: clip_regions.as_entire_binding(),
                },
            ],
        })
    }

    fn create_texture_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wyrd sampled texture bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    fn ensure_texture_asset(
        &mut self,
        key: String,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<()> {
        anyhow::ensure!(
            width > 0 && height > 0 && rgba.len() == width as usize * height as usize * 4,
            "invalid GPU texture asset dimensions or data length"
        );
        let max_dimension = self.device.limits().max_texture_dimension_2d;
        anyhow::ensure!(
            width <= max_dimension && height <= max_dimension,
            "GPU texture asset {width}x{height} exceeds adapter limit {max_dimension}"
        );
        self.texture_access_seq = self.texture_access_seq.wrapping_add(1);
        if let Some(asset) = self.texture_assets.get_mut(&key) {
            asset.last_used = self.texture_access_seq;
            return Ok(());
        }
        if self.texture_assets.len() >= self.texture_cache_capacity {
            if let Some(oldest) = self
                .texture_assets
                .iter()
                .min_by_key(|(_, asset)| asset.last_used)
                .map(|(key, _)| key.clone())
            {
                self.texture_assets.remove(&oldest);
            }
        }

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wyrd cached scene texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = Self::create_texture_bind_group(
            &self.device,
            &self.texture_bind_group_layout,
            &self.texture_sampler,
            &view,
        );
        self.texture_assets.insert(
            key,
            GpuTextureAsset {
                _texture: texture,
                _view: view,
                bind_group,
                width,
                height,
                last_used: self.texture_access_seq,
            },
        );
        Ok(())
    }

    fn glyph_atlas_entry(
        &mut self,
        cache_key: cosmic_text::CacheKey,
    ) -> Result<Option<GlyphAtlasEntry>> {
        if self.glyph_atlas.is_none() {
            self.glyph_atlas = Some(GlyphAtlas::new(
                &self.device,
                &self.texture_bind_group_layout,
            ));
        }
        let atlas = self
            .glyph_atlas
            .as_mut()
            .expect("glyph atlas was initialized");
        atlas.access_seq = atlas.access_seq.wrapping_add(1);
        let access_seq = atlas.access_seq;
        if let Some(entry) = atlas.glyphs.get_mut(&cache_key) {
            entry.last_used = access_seq;
            return Ok(Some(*entry));
        }

        let rasterized = {
            let context = &mut self.text_context;
            context
                .swash_cache
                .get_image(&mut context.font_system, cache_key)
                .as_ref()
                .and_then(|image| {
                    let width = image.placement.width;
                    let height = image.placement.height;
                    if width == 0 || height == 0 {
                        return None;
                    }
                    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
                    let is_color = match image.content {
                        cosmic_text::SwashContent::Mask => {
                            for alpha in &image.data {
                                rgba.extend_from_slice(&[255, 255, 255, *alpha]);
                            }
                            false
                        }
                        cosmic_text::SwashContent::Color => {
                            rgba.extend_from_slice(&image.data);
                            true
                        }
                        cosmic_text::SwashContent::SubpixelMask => return None,
                    };
                    Some((
                        width,
                        height,
                        image.placement.left,
                        image.placement.top,
                        is_color,
                        rgba,
                    ))
                })
        };
        let Some((width, height, left, top, color, rgba)) = rasterized else {
            return Ok(None);
        };
        anyhow::ensure!(
            rgba.len() == width as usize * height as usize * 4,
            "swash glyph image has an invalid pixel buffer"
        );

        let atlas = self
            .glyph_atlas
            .as_mut()
            .expect("glyph atlas was initialized");
        if atlas.glyphs.len() >= 4096 {
            if let Some(oldest) = atlas
                .glyphs
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            {
                atlas.glyphs.remove(&oldest);
            }
        }
        let padded_width = width + 2;
        let padded_height = height + 2;
        if atlas.pen_x + padded_width > atlas.width {
            atlas.pen_x = 1;
            atlas.pen_y += atlas.row_height;
            atlas.row_height = 0;
        }
        if atlas.pen_y + padded_height > atlas.height {
            atlas.glyphs.clear();
            atlas.pen_x = 1;
            atlas.pen_y = 1;
            atlas.row_height = 0;
        }
        anyhow::ensure!(
            padded_width + 1 < atlas.width && padded_height + 1 < atlas.height,
            "glyph is too large for the GPU atlas"
        );
        let x = atlas.pen_x + 1;
        let y = atlas.pen_y + 1;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &atlas.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        atlas.pen_x += padded_width;
        atlas.row_height = atlas.row_height.max(padded_height);
        let entry = GlyphAtlasEntry {
            x,
            y,
            width,
            height,
            left,
            top,
            color,
            last_used: access_seq,
        };
        atlas.glyphs.insert(cache_key, entry);
        Ok(Some(entry))
    }

    #[allow(clippy::too_many_arguments)]
    fn append_text_glyphs(
        &mut self,
        instances: &mut Vec<DrawInstance>,
        text: &str,
        spans: &[crate::render::scene::TextSpan],
        font_family: &str,
        font_size: f32,
        color: tiny_skia::Color,
        align: cosmic_text::Align,
        bounds: [f32; 4],
        opacity: f32,
        matrix: [f32; 16],
    ) -> Result<()> {
        use super::scene::truncate_text_spans;
        use cosmic_text::{Attrs, Buffer, Color as TextColor, Metrics, Shaping, Wrap};
        let [x, y, width, height] = bounds;
        let font_size = font_size.max(1.0);
        self.text_context.ensure_system_fonts();
        let resolved_family = self.text_context.resolve_font_family(font_family);
        let buffer_key = (
            std::sync::Arc::<str>::from(resolved_family.as_ref()),
            (font_size * 100.0).round() as u32,
        );
        if !self.text_context.text_buffers.contains_key(&buffer_key) {
            self.text_context.text_buffers.insert(
                buffer_key.clone(),
                Buffer::new(
                    &mut self.text_context.font_system,
                    Metrics::new(font_size, font_size * 1.25),
                ),
            );
        }
        let line_height = font_size * 1.25;
        let box_width = width.max(1.0);
        let box_height = height.max(1.0);
        let layout_key = format!(
            "{resolved_family}\0{:08x}\0{:08x}\0{:08x}\0{align:?}\0{text}\0{spans:?}",
            font_size.to_bits(),
            box_width.to_bits(),
            box_height.to_bits(),
        );
        self.text_layout_access_seq = self.text_layout_access_seq.wrapping_add(1);
        let layout_access_seq = self.text_layout_access_seq;
        let (glyphs, y_offset) = if let Some(entry) = self.text_layout_cache.get_mut(&layout_key) {
            entry.last_used = layout_access_seq;
            (entry.glyphs.clone(), entry.vertical_offset)
        } else {
            let (glyphs, line_count) = {
                let buffer = self
                    .text_context
                    .text_buffers
                    .get_mut(&buffer_key)
                    .expect("text buffer inserted");
                let mut buffer = buffer.borrow_with(&mut self.text_context.font_system);
                let single_line = box_height < line_height * 1.5;
                buffer.set_size(Some(box_width), Some(box_height));
                buffer.set_wrap(if single_line {
                    Wrap::None
                } else {
                    Wrap::WordOrGlyph
                });
                let base_color = TextColor::rgba(
                    (color.red() * 255.0).round() as u8,
                    (color.green() * 255.0).round() as u8,
                    (color.blue() * 255.0).round() as u8,
                    (color.alpha() * 255.0).round() as u8,
                );
                let attrs = Attrs::new()
                    .family(cosmic_text::Family::Name(&resolved_family))
                    .color(base_color);
                if spans.is_empty()
                    || spans
                        .iter()
                        .map(|span| span.text.as_str())
                        .collect::<String>()
                        != text
                {
                    buffer.set_text(text, &attrs, Shaping::Advanced, Some(align));
                } else {
                    buffer.set_rich_text(
                        super::text::rich_text_attrs(spans, &resolved_family, base_color),
                        &attrs,
                        Shaping::Advanced,
                        Some(align),
                    );
                }
                buffer.shape_until_scroll(false);
                if single_line {
                    let cut_byte = buffer.layout_runs().next().and_then(|run| {
                        let run_width = run
                            .glyphs
                            .last()
                            .map(|glyph| glyph.x + glyph.w)
                            .unwrap_or(0.0);
                        if run_width > box_width + 1.0 && text.chars().count() > 3 {
                            let target_width = (box_width - font_size * 0.7).max(font_size * 0.8);
                            let mut byte_index = 0;
                            for glyph in run.glyphs {
                                if glyph.x + glyph.w <= target_width {
                                    byte_index = glyph.end.min(text.len());
                                } else {
                                    break;
                                }
                            }
                            while byte_index > 0 && !text.is_char_boundary(byte_index) {
                                byte_index -= 1;
                            }
                            (byte_index > 0 && byte_index < text.len()).then_some(byte_index)
                        } else {
                            None
                        }
                    });
                    if let Some(cut) = cut_byte {
                        let truncated = format!("{}…", text[..cut].trim_end());
                        let clipped_spans = truncate_text_spans(spans, &truncated);
                        buffer.set_rich_text(
                            super::text::rich_text_attrs(
                                &clipped_spans,
                                &resolved_family,
                                base_color,
                            ),
                            &attrs,
                            Shaping::Advanced,
                            Some(align),
                        );
                        buffer.shape_until_scroll(false);
                    }
                }
                let runs = buffer.layout_runs().collect::<Vec<_>>();
                let line_count = runs.len().max(1);
                let glyphs = runs
                    .iter()
                    .flat_map(|run| {
                        run.glyphs
                            .iter()
                            .map(|glyph| (glyph.physical((0.0, run.line_y), 1.0), glyph.color_opt))
                    })
                    .collect::<Vec<_>>();
                (glyphs, line_count)
            };
            let total_text_height = line_count as f32 * line_height;
            let y_offset = if total_text_height < box_height {
                ((box_height - total_text_height) / 2.0).max(0.0)
            } else {
                0.0
            };
            if self.text_layout_cache.len() >= 256 {
                if let Some(oldest) = self
                    .text_layout_cache
                    .iter()
                    .min_by_key(|(_, entry)| entry.last_used)
                    .map(|(key, _)| key.clone())
                {
                    self.text_layout_cache.remove(&oldest);
                }
            }
            self.text_layout_cache.insert(
                layout_key,
                TextLayoutCacheEntry {
                    glyphs: glyphs.clone(),
                    vertical_offset: y_offset,
                    last_used: layout_access_seq,
                },
            );
            (glyphs, y_offset)
        };
        let snap_x = x.round() as i32;
        let snap_y = (y + y_offset).round() as i32;
        for (physical, color_override) in glyphs {
            let Some(atlas_entry) = self.glyph_atlas_entry(physical.cache_key)? else {
                continue;
            };
            let atlas = self
                .glyph_atlas
                .as_ref()
                .expect("glyph atlas was initialized");
            let glyph_x = snap_x + physical.x + atlas_entry.left;
            let glyph_y = snap_y + physical.y - atlas_entry.top;
            let clip_left = (x.round() as i32).max(glyph_x);
            let clip_top = (y.round() as i32).max(glyph_y);
            let clip_right =
                ((x + box_width).ceil() as i32).min(glyph_x + atlas_entry.width as i32);
            let clip_bottom =
                ((y + box_height).ceil() as i32).min(glyph_y + atlas_entry.height as i32);
            if clip_right <= clip_left || clip_bottom <= clip_top {
                continue;
            }
            let cropped_x = (clip_left - glyph_x) as f32;
            let cropped_y = (clip_top - glyph_y) as f32;
            let u0 = (atlas_entry.x as f32 + cropped_x + 0.5) / atlas.width as f32;
            let v0 = (atlas_entry.y as f32 + cropped_y + 0.5) / atlas.height as f32;
            let u1 = (atlas_entry.x as f32 + cropped_x + (clip_right - clip_left) as f32 - 0.5)
                / atlas.width as f32;
            let v1 = (atlas_entry.y as f32 + cropped_y + (clip_bottom - clip_top) as f32 - 0.5)
                / atlas.height as f32;
            let glyph_color = color_override.map_or(color, |glyph_color| {
                let [r, g, b, a] = glyph_color.as_rgba();
                tiny_skia::Color::from_rgba8(r, g, b, a)
            });
            let color = if atlas_entry.color {
                [1.0, 1.0, 1.0, opacity]
            } else {
                [
                    glyph_color.red(),
                    glyph_color.green(),
                    glyph_color.blue(),
                    glyph_color.alpha() * opacity,
                ]
            };
            instances.push(DrawInstance {
                bounds: [
                    clip_left as f32,
                    clip_top as f32,
                    (clip_right - clip_left) as f32,
                    (clip_bottom - clip_top) as f32,
                ],
                color,
                params: [0.0, 6.0, 0.0, 0.0],
                extra: [u0, v0, u1, v1],
                clip_regions: Vec::new(),
                matrix,
                gradient_stops: Vec::new(),
                path_segments: Vec::new(),
                texture_key: Some("\0wyrd-glyph-atlas".to_string()),
            });
        }
        Ok(())
    }

    fn image_texture(
        &mut self,
        path: &str,
        width: u32,
        height: u32,
        radius: f32,
    ) -> Result<String> {
        let metadata = std::fs::metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        let key = format!("image\0{path}\0{width}x{height}\0{radius:.3}\0{metadata}");
        if self.texture_assets.contains_key(&key) {
            self.texture_access_seq = self.texture_access_seq.wrapping_add(1);
            self.texture_assets.get_mut(&key).unwrap().last_used = self.texture_access_seq;
            return Ok(key);
        }
        let bytes = std::fs::read(path)?;
        let image = image::load_from_memory(&bytes)?;
        let image = image
            .resize(
                width.max(1),
                height.max(1),
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgba8();
        self.ensure_texture_asset(key.clone(), image.width(), image.height(), &image)?;
        Ok(key)
    }

    fn svg_texture(&mut self, path: &str, width: u32, height: u32) -> Result<(String, u32, u32)> {
        let metadata = std::fs::metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        let key = format!("svg\0{path}\0{width}x{height}\0{metadata}");
        if self.texture_assets.contains_key(&key) {
            self.texture_access_seq = self.texture_access_seq.wrapping_add(1);
            self.texture_assets.get_mut(&key).unwrap().last_used = self.texture_access_seq;
            let asset = &self.texture_assets[&key];
            return Ok((key, asset.width, asset.height));
        }
        let data = std::fs::read(path)?;
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default())?;
        let scale = (width as f32 / tree.size().width().max(1.0))
            .min(height as f32 / tree.size().height().max(1.0));
        let actual_width = (tree.size().width() * scale).round().max(1.0) as u32;
        let actual_height = (tree.size().height() * scale).round().max(1.0) as u32;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(actual_width, actual_height)
            .ok_or_else(|| anyhow::anyhow!("unable to allocate SVG raster cache"))?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        let rgba = pixmap.take();
        let mut rgba = rgba;
        for pixel in rgba.as_chunks_mut::<4>().0 {
            let alpha = u16::from(pixel[3]);
            if alpha > 0 {
                for channel in &mut pixel[..3] {
                    *channel = (u16::from(*channel) * 255)
                        .checked_div(alpha)
                        .unwrap_or_default()
                        .min(255) as u8;
                }
            }
        }
        self.ensure_texture_asset(key.clone(), actual_width, actual_height, &rgba)?;
        Ok((key, actual_width, actual_height))
    }

    pub fn supports_storage_textures(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
    }

    pub fn supports_timestamp_queries(&self) -> bool {
        self.timestamps.is_some()
    }

    pub fn estimated_buffer_bytes(&self) -> u64 {
        self.instances_capacity
            + self.matrices_buffer.size()
            + self.gradient_stops_capacity
            + self.path_segments_capacity
            + 8
            + if self.timestamps.is_some() { 48 } else { 0 }
    }

    pub fn estimated_texture_bytes(&self) -> u64 {
        self.texture_assets
            .values()
            .map(|asset| u64::from(asset.width) * u64::from(asset.height) * 4)
            .sum::<u64>()
            + self.glyph_atlas.as_ref().map_or(0, |atlas| {
                u64::from(atlas.width) * u64::from(atlas.height) * 4
            })
            + self.blur_intermediate.as_ref().map_or(0, |texture| {
                u64::from(texture.width) * u64::from(texture.height) * 4
            })
            + 4
    }

    pub fn wait_for_idle(&self) -> Result<()> {
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(())
    }

    pub fn gpu_time_ms(&self) -> Result<Option<f64>> {
        let Some(timestamps) = &self.timestamps else {
            return Ok(None);
        };
        let slice = timestamps.readback_buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.wait_for_idle()?;
        receiver.recv()??;
        let mapped = slice.get_mapped_range()?;
        let start = u64::from_ne_bytes(mapped[0..8].try_into()?);
        let end = u64::from_ne_bytes(mapped[8..16].try_into()?);
        drop(mapped);
        timestamps.readback_buffer.unmap();
        let period_ns = self.queue.get_timestamp_period();
        Ok(Some(
            end.saturating_sub(start) as f64 * f64::from(period_ns) / 1_000_000.0,
        ))
    }

    fn pipeline(
        &mut self,
        format: wgpu::TextureFormat,
        multiply_blend: bool,
    ) -> Arc<wgpu::RenderPipeline> {
        self.pipelines
            .entry((format, multiply_blend))
            .or_insert_with(|| {
                let layout = self
                    .device
                    .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("wyrd scene pipeline layout"),
                        bind_group_layouts: &[
                            Some(&self.bind_group_layout),
                            Some(&self.texture_bind_group_layout),
                        ],
                        immediate_size: 0,
                    });
                Arc::new(
                    self.device
                        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                            label: Some("wyrd scene pipeline"),
                            layout: Some(&layout),
                            vertex: wgpu::VertexState {
                                module: &self.shader,
                                entry_point: Some("vs_main"),
                                buffers: &[],
                                compilation_options: Default::default(),
                            },
                            primitive: wgpu::PrimitiveState::default(),
                            depth_stencil: None,
                            multisample: wgpu::MultisampleState::default(),
                            fragment: Some(wgpu::FragmentState {
                                module: &self.shader,
                                entry_point: Some("fs_main"),
                                targets: &[Some(wgpu::ColorTargetState {
                                    format,
                                    blend: Some(if multiply_blend {
                                        wgpu::BlendState {
                                            color: wgpu::BlendComponent {
                                                src_factor: wgpu::BlendFactor::Dst,
                                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                                operation: wgpu::BlendOperation::Add,
                                            },
                                            alpha: wgpu::BlendComponent {
                                                src_factor: wgpu::BlendFactor::One,
                                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                                operation: wgpu::BlendOperation::Add,
                                            },
                                        }
                                    } else {
                                        wgpu::BlendState::ALPHA_BLENDING
                                    }),
                                    write_mask: wgpu::ColorWrites::ALL,
                                })],
                                compilation_options: Default::default(),
                            }),
                            multiview_mask: None,
                            cache: None,
                        }),
                )
            })
            .clone()
    }

    fn blur_pipeline(&mut self, format: wgpu::TextureFormat) -> Arc<wgpu::RenderPipeline> {
        self.blur_pipelines
            .entry(format)
            .or_insert_with(|| {
                let layout = self
                    .device
                    .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("wyrd blur pipeline layout"),
                        bind_group_layouts: &[Some(&self.blur_bind_group_layout)],
                        immediate_size: 0,
                    });
                Arc::new(
                    self.device
                        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                            label: Some("wyrd separable Gaussian blur pipeline"),
                            layout: Some(&layout),
                            vertex: wgpu::VertexState {
                                module: &self.blur_shader,
                                entry_point: Some("vs_main"),
                                buffers: &[],
                                compilation_options: Default::default(),
                            },
                            primitive: wgpu::PrimitiveState::default(),
                            depth_stencil: None,
                            multisample: wgpu::MultisampleState::default(),
                            fragment: Some(wgpu::FragmentState {
                                module: &self.blur_shader,
                                entry_point: Some("fs_main"),
                                targets: &[Some(wgpu::ColorTargetState {
                                    format,
                                    blend: None,
                                    write_mask: wgpu::ColorWrites::ALL,
                                })],
                                compilation_options: Default::default(),
                            }),
                            multiview_mask: None,
                            cache: None,
                        }),
                )
            })
            .clone()
    }

    fn prepare_blur_resources(
        &mut self,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        count: usize,
    ) {
        if count == 0 {
            return;
        }
        self.blur_pipeline(format);
        let needs_intermediate = self.blur_intermediate.as_ref().is_none_or(|texture| {
            texture.width != width || texture.height != height || texture.format != format
        });
        if needs_intermediate {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("wyrd blur intermediate"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.blur_intermediate = Some(BlurIntermediate {
                _texture: texture,
                view,
                width,
                height,
                format,
            });
        }
        while self.blur_params.len() < count {
            let make_params = |label| {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: 16,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            self.blur_params.push((
                make_params("wyrd horizontal blur params"),
                make_params("wyrd vertical blur params"),
            ));
        }
    }

    fn encode_blur(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &GpuRenderFrame,
        radius: f32,
        bounds: [f32; 4],
        blur_index: usize,
    ) {
        if radius <= 0.0 {
            return;
        }
        let pipeline = self
            .blur_pipelines
            .get(&frame.format)
            .expect("blur pipeline was prepared before encoding");
        let (horizontal_params, vertical_params) = &self.blur_params[blur_index];
        let sigma = (radius * 0.5).max(0.5);
        for (buffer, direction) in [
            (horizontal_params, [1.0_f32, 0.0_f32]),
            (vertical_params, [0.0_f32, 1.0_f32]),
        ] {
            let params = [direction[0], direction[1], radius.min(32.0), sigma]
                .into_iter()
                .flat_map(f32::to_ne_bytes)
                .collect::<Vec<_>>();
            self.queue.write_buffer(buffer, 0, &params);
        }
        let source_view = frame
            .source_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let horizontal_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wyrd horizontal blur bind group"),
            layout: &self.blur_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.blur_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: horizontal_params.as_entire_binding(),
                },
            ],
        });
        let intermediate = self
            .blur_intermediate
            .as_ref()
            .expect("blur intermediate has been allocated");
        let vertical_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wyrd vertical blur bind group"),
            layout: &self.blur_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&intermediate.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.blur_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: vertical_params.as_entire_binding(),
                },
            ],
        });

        let bx = bounds[0].max(0.0) as u32;
        let by = bounds[1].max(0.0) as u32;
        let bw = (bounds[2].max(0.0) as u32).min(frame.width.saturating_sub(bx));
        let bh = (bounds[3].max(0.0) as u32).min(frame.height.saturating_sub(by));
        if bw == 0 || bh == 0 {
            return;
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("wyrd horizontal Gaussian blur pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &intermediate.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &horizontal_bind_group, &[]);
            pass.set_scissor_rect(bx, by, bw, bh);
            pass.draw(0..3, 0..1);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("wyrd vertical Gaussian blur pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &frame.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &vertical_bind_group, &[]);
            pass.set_scissor_rect(bx, by, bw, bh);
            pass.draw(0..3, 0..1);
        }
    }

    fn ensure_instances_capacity(&mut self, required: u64) {
        if required <= self.instances_capacity {
            return;
        }
        self.instances_capacity = required.next_power_of_two();
        self.instances_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wyrd scene instances"),
            size: self.instances_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.matrices_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wyrd scene transforms"),
            size: self.instances_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.bind_group = Self::create_bind_group(
            &self.device,
            &self.bind_group_layout,
            &self.viewport_buffer,
            &self.instances_buffer,
            &self.matrices_buffer,
            &self.gradient_stops_buffer,
            &self.path_segments_buffer,
            &self.clip_regions_buffer,
        );
    }

    fn ensure_gradient_stops_capacity(&mut self, required: u64) {
        if required <= self.gradient_stops_capacity {
            return;
        }
        self.gradient_stops_capacity = required.next_power_of_two();
        self.gradient_stops_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wyrd gradient stops"),
            size: self.gradient_stops_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.bind_group = Self::create_bind_group(
            &self.device,
            &self.bind_group_layout,
            &self.viewport_buffer,
            &self.instances_buffer,
            &self.matrices_buffer,
            &self.gradient_stops_buffer,
            &self.path_segments_buffer,
            &self.clip_regions_buffer,
        );
    }

    fn ensure_path_segments_capacity(&mut self, required: u64) {
        if required <= self.path_segments_capacity {
            return;
        }
        self.path_segments_capacity = required.next_power_of_two();
        self.path_segments_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wyrd custom path segments"),
            size: self.path_segments_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.bind_group = Self::create_bind_group(
            &self.device,
            &self.bind_group_layout,
            &self.viewport_buffer,
            &self.instances_buffer,
            &self.matrices_buffer,
            &self.gradient_stops_buffer,
            &self.path_segments_buffer,
            &self.clip_regions_buffer,
        );
    }

    fn ensure_clip_regions_capacity(&mut self, required: u64) {
        if required <= self.clip_regions_capacity {
            return;
        }
        self.clip_regions_capacity = required.next_power_of_two();
        self.clip_regions_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wyrd scene clip regions"),
            size: self.clip_regions_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.bind_group = Self::create_bind_group(
            &self.device,
            &self.bind_group_layout,
            &self.viewport_buffer,
            &self.instances_buffer,
            &self.matrices_buffer,
            &self.gradient_stops_buffer,
            &self.path_segments_buffer,
            &self.clip_regions_buffer,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_scene_segment(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &GpuRenderFrame,
        pipeline: &wgpu::RenderPipeline,
        tint_pipeline: &wgpu::RenderPipeline,
        batches: &[(u32, u32, Option<&String>, &wgpu::BindGroup, bool)],
        start: u32,
        end: u32,
        regions: &[tiny_skia::IntRect],
        clear: bool,
        write_timestamps: bool,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("wyrd scene pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &frame.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: write_timestamps
                .then(|| {
                    self.timestamps
                        .as_ref()
                        .map(|timestamps| wgpu::RenderPassTimestampWrites {
                            query_set: &timestamps.query_set,
                            beginning_of_pass_write_index: Some(0),
                            end_of_pass_write_index: Some(1),
                        })
                })
                .flatten(),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        for region in regions {
            let left = region.left().max(0) as u32;
            let top = region.top().max(0) as u32;
            let right = (region.right().max(0) as u32).min(frame.width);
            let bottom = (region.bottom().max(0) as u32).min(frame.height);
            if right <= left || bottom <= top {
                continue;
            }
            pass.set_scissor_rect(left, top, right - left, bottom - top);
            for (batch_start, batch_end, _, texture_group, multiply_blend) in batches {
                if *batch_end <= start || *batch_start >= end {
                    continue;
                }
                let draw_start = (*batch_start).max(start);
                let draw_end = (*batch_end).min(end);
                pass.set_pipeline(if *multiply_blend {
                    tint_pipeline
                } else {
                    pipeline
                });
                pass.set_bind_group(1, *texture_group, &[]);
                pass.draw(0..6, draw_start..draw_end);
            }
        }
    }

    pub fn render_scene(
        &mut self,
        scene: &super::scene::SceneGraph,
        target: &mut dyn GpuRenderTarget,
        damage: &super::damage::DamageTracker,
    ) -> Result<GpuPresented> {
        let frame = target.acquire_frame()?;
        let instances = self.collect_instances(scene, frame.width, frame.height)?;
        let blur_count = instances
            .iter()
            .filter(|instance| instance.params[1] == 9.0)
            .count();
        let has_blur = blur_count > 0;
        let use_damage = !has_blur
            && target.preserves_contents()
            && !damage.full_redraw
            && !damage.regions.is_empty()
            && damage.regions.len() <= super::damage::MAX_DAMAGE_REGIONS;
        let required = (instances.len().max(1) * 80) as u64;
        self.ensure_instances_capacity(required);

        let viewport_bytes = [frame.width as f32, frame.height as f32]
            .into_iter()
            .flat_map(f32::to_ne_bytes)
            .collect::<Vec<_>>();
        self.queue
            .write_buffer(&self.viewport_buffer, 0, &viewport_bytes);

        let mut gradient_stop_bytes = Vec::new();
        let mut path_segment_bytes = Vec::new();
        let mut clip_region_bytes = Vec::new();
        let mut instance_bytes = Vec::with_capacity(instances.len().max(1) * 80);
        let mut matrix_bytes = Vec::with_capacity(instances.len().max(1) * 64);
        for instance in &instances {
            let mut extra = instance.extra;
            if !instance.gradient_stops.is_empty() {
                extra[0] = (gradient_stop_bytes.len() / 16) as f32;
                extra[1] = instance.gradient_stops.len() as f32;
                for (offset, color) in &instance.gradient_stops {
                    gradient_stop_bytes.extend_from_slice(&offset.to_ne_bytes());
                    gradient_stop_bytes.extend_from_slice(&0.0_f32.to_ne_bytes());
                    gradient_stop_bytes.extend_from_slice(&0.0_f32.to_ne_bytes());
                    gradient_stop_bytes.extend_from_slice(&0.0_f32.to_ne_bytes());
                    for channel in [color.red(), color.green(), color.blue(), color.alpha()] {
                        gradient_stop_bytes.extend_from_slice(&channel.to_ne_bytes());
                    }
                }
            }
            if !instance.path_segments.is_empty() {
                extra[0] = (path_segment_bytes.len() / 16) as f32;
                extra[1] = instance.path_segments.len() as f32;
                for segment in &instance.path_segments {
                    for value in segment {
                        path_segment_bytes.extend_from_slice(&value.to_ne_bytes());
                    }
                }
            }
            let clip_start = (clip_region_bytes.len() / 96) as u32;
            for clip in &instance.clip_regions {
                for value in clip.bounds.iter() {
                    clip_region_bytes.extend_from_slice(&value.to_ne_bytes());
                }
                clip_region_bytes.extend_from_slice(&clip.corner_radius.to_ne_bytes());
                for pad in &clip._pad {
                    clip_region_bytes.extend_from_slice(&pad.to_ne_bytes());
                }
                for value in clip.inverse_transform.iter() {
                    clip_region_bytes.extend_from_slice(&value.to_ne_bytes());
                }
            }
            instance.append_bytes(
                &mut instance_bytes,
                extra,
                [clip_start, instance.clip_regions.len() as u32],
            );
            for value in instance.matrix {
                matrix_bytes.extend_from_slice(&value.to_ne_bytes());
            }
        }
        let required_stops = gradient_stop_bytes.len().max(16) as u64;
        self.ensure_gradient_stops_capacity(required_stops);
        let required_path_segments = path_segment_bytes.len().max(16) as u64;
        self.ensure_path_segments_capacity(required_path_segments);
        let required_clip_regions = clip_region_bytes.len().max(96) as u64;
        self.ensure_clip_regions_capacity(required_clip_regions);
        self.prepare_blur_resources(frame.format, frame.width, frame.height, blur_count);
        if instances.is_empty() {
            self.queue.write_buffer(&self.instances_buffer, 0, &[0; 80]);
            self.queue.write_buffer(&self.matrices_buffer, 0, &[0; 64]);
        } else {
            self.queue
                .write_buffer(&self.instances_buffer, 0, &instance_bytes);
            self.queue
                .write_buffer(&self.matrices_buffer, 0, &matrix_bytes);
        }
        if gradient_stop_bytes.is_empty() {
            self.queue
                .write_buffer(&self.gradient_stops_buffer, 0, &[0; 16]);
        } else {
            self.queue
                .write_buffer(&self.gradient_stops_buffer, 0, &gradient_stop_bytes);
        }
        if path_segment_bytes.is_empty() {
            self.queue
                .write_buffer(&self.path_segments_buffer, 0, &[0; 16]);
        } else {
            self.queue
                .write_buffer(&self.path_segments_buffer, 0, &path_segment_bytes);
        }
        if clip_region_bytes.is_empty() {
            self.queue
                .write_buffer(&self.clip_regions_buffer, 0, &[0; 96]);
        } else {
            self.queue
                .write_buffer(&self.clip_regions_buffer, 0, &clip_region_bytes);
        }

        let pipeline = self.pipeline(frame.format, false);
        let tint_pipeline = self.pipeline(frame.format, true);
        let mut batches: Vec<(u32, u32, Option<&String>, &wgpu::BindGroup, bool)> = Vec::new();
        for (index, instance) in instances.iter().enumerate() {
            if instance.params[1] == 9.0 {
                continue;
            }
            let texture_group = match instance.texture_key.as_deref() {
                Some("\0wyrd-glyph-atlas") => {
                    &self
                        .glyph_atlas
                        .as_ref()
                        .expect("glyph atlas bind group exists")
                        .bind_group
                }
                Some(key) => self
                    .texture_assets
                    .get(key)
                    .map_or(&self.white_texture_bind_group, |asset| &asset.bind_group),
                None => &self.white_texture_bind_group,
            };
            let index = index as u32;
            let multiply_blend = instance.params[1] == 11.0;
            if let Some((_, end, previous_key, _, previous_blend)) = batches.last_mut() {
                if *end == index
                    && *previous_key == instance.texture_key.as_ref()
                    && *previous_blend == multiply_blend
                {
                    *end = index + 1;
                    continue;
                }
            }
            batches.push((
                index,
                index + 1,
                instance.texture_key.as_ref(),
                texture_group,
                multiply_blend,
            ));
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("wyrd scene encoder"),
            });
        let regions = if !use_damage {
            vec![
                tiny_skia::IntRect::from_xywh(0, 0, frame.width, frame.height)
                    .expect("positive GPU target dimensions"),
            ]
        } else {
            damage.regions.clone()
        };
        let mut segment_start = 0_u32;
        let mut first_segment = true;
        let mut blur_index = 0;
        for (index, instance) in instances.iter().enumerate() {
            if instance.params[1] != 9.0 {
                continue;
            }
            let index = index as u32;
            self.encode_scene_segment(
                &mut encoder,
                &frame,
                &pipeline,
                &tint_pipeline,
                &batches,
                segment_start,
                index,
                &regions,
                first_segment && !use_damage,
                first_segment,
            );
            first_segment = false;
            self.encode_blur(
                &mut encoder,
                &frame,
                instance.params[2],
                instance.bounds,
                blur_index,
            );
            blur_index += 1;
            segment_start = index + 1;
        }
        if first_segment || segment_start < instances.len() as u32 {
            self.encode_scene_segment(
                &mut encoder,
                &frame,
                &pipeline,
                &tint_pipeline,
                &batches,
                segment_start,
                instances.len() as u32,
                &regions,
                first_segment && !use_damage,
                first_segment,
            );
        }
        if let Some(timestamps) = &self.timestamps {
            encoder.resolve_query_set(&timestamps.query_set, 0..2, &timestamps.resolve_buffer, 0);
            encoder.copy_buffer_to_buffer(
                &timestamps.resolve_buffer,
                0,
                &timestamps.readback_buffer,
                0,
                16,
            );
        }
        target.encode_present(&mut encoder, &frame)?;
        self.queue.submit([encoder.finish()]);
        target.mark_rendered();
        self.frame_id = self.frame_id.wrapping_add(1);
        Ok(GpuPresented {
            frame_id: self.frame_id,
            frame,
        })
    }

    fn collect_instances(
        &mut self,
        scene: &super::scene::SceneGraph,
        width: u32,
        height: u32,
    ) -> Result<Vec<DrawInstance>> {
        use super::scene::{Fill, SceneNodeKind};
        let mut instances = Vec::with_capacity(scene.nodes().len());
        for (node_id, node) in scene.nodes().iter() {
            let (x, y, w, h) = node.bounds;
            let world_matrix = Self::world_transform(scene, node_id);
            let mut clip_regions = Vec::new();
            let mut ancestor = node.parent;
            while let Some(parent_id) = ancestor {
                let Some(parent) = scene.get(parent_id) else {
                    break;
                };
                if parent.clip_children {
                    let clip_transform = Self::world_transform(scene, parent_id);
                    let determinant = clip_transform.determinant();
                    let invertible = determinant.is_finite() && determinant.abs() > 1.0e-8;
                    let corner_radius = match &parent.kind {
                        SceneNodeKind::Rect { radius, .. } => *radius,
                        SceneNodeKind::Image { radius, .. } => *radius,
                        SceneNodeKind::Outline { radius, .. } => *radius,
                        SceneNodeKind::Shadow { corner_radius, .. } => *corner_radius,
                        _ => 0.0,
                    };
                    clip_regions.push(GpuClipRegion {
                        bounds: if invertible {
                            [
                                parent.bounds.0,
                                parent.bounds.1,
                                parent.bounds.2,
                                parent.bounds.3,
                            ]
                        } else {
                            [0.0, 0.0, -1.0, -1.0]
                        },
                        corner_radius,
                        _pad: [0.0; 3],
                        inverse_transform: if invertible {
                            clip_transform.inverse().to_cols_array()
                        } else {
                            glam::Mat4::IDENTITY.to_cols_array()
                        },
                    });
                }
                ancestor = parent.parent;
            }
            let (screen_x, screen_y, screen_w, screen_h) =
                Self::transformed_bounds(world_matrix, (x, y, w, h));
            if w <= 0.0
                || h <= 0.0
                || ![screen_x, screen_y, screen_w, screen_h]
                    .into_iter()
                    .all(f32::is_finite)
                || screen_x >= width as f32
                || screen_y >= height as f32
                || screen_x + screen_w <= 0.0
                || screen_y + screen_h <= 0.0
            {
                continue;
            }
            let first_instance = instances.len();
            let opacity = Self::world_opacity(scene, node_id);
            let mut add = |color: tiny_skia::Color, radius: f32, kind: f32, stroke: f32| {
                let alpha = color.alpha() * opacity;
                instances.push(DrawInstance {
                    bounds: [x, y, w, h],
                    color: [color.red(), color.green(), color.blue(), alpha],
                    params: [radius.max(0.0), kind, stroke.max(0.0), 0.0],
                    extra: [0.0; 4],
                    clip_regions: Vec::new(),
                    matrix: world_matrix.to_cols_array(),
                    gradient_stops: Vec::new(),
                    path_segments: Vec::new(),
                    texture_key: None,
                });
            };
            match &node.kind {
                SceneNodeKind::Root | SceneNodeKind::Container => {}
                SceneNodeKind::Rect {
                    fill: Some(Fill::Solid(color)),
                    radius,
                } => add(*color, *radius, 0.0, 0.0),
                SceneNodeKind::Outline {
                    color,
                    width,
                    radius,
                    style,
                } => {
                    add(*color, *radius, 1.0, *width);
                    if let Some(instance) = instances.last_mut() {
                        instance.extra[0] = match style {
                            super::scene::OutlineStyle::Solid => 0.0,
                            super::scene::OutlineStyle::Dashed => 1.0,
                            super::scene::OutlineStyle::Dotted => 2.0,
                        };
                    }
                }
                SceneNodeKind::Rect {
                    fill: Some(Fill::LinearGradient { angle_deg, stops }),
                    radius,
                } => {
                    add(
                        stops
                            .first()
                            .map(|(_, color)| *color)
                            .unwrap_or(tiny_skia::Color::TRANSPARENT),
                        *radius,
                        4.0,
                        0.0,
                    );
                    if let Some(instance) = instances.last_mut() {
                        instance.color[3] = opacity;
                        instance.extra[2] = angle_deg.to_radians();
                        instance.gradient_stops = stops.clone();
                    }
                }
                SceneNodeKind::Rect {
                    fill:
                        Some(Fill::RadialGradient {
                            cx,
                            cy,
                            radius: gradient_radius,
                            stops,
                        }),
                    radius,
                } => {
                    add(
                        stops
                            .first()
                            .map(|(_, color)| *color)
                            .unwrap_or(tiny_skia::Color::TRANSPARENT),
                        *radius,
                        5.0,
                        0.0,
                    );
                    if let Some(instance) = instances.last_mut() {
                        instance.color[3] = opacity;
                        let center_x = if (0.0..=1.0).contains(cx) {
                            cx * w
                        } else {
                            *cx
                        };
                        let center_y = if (0.0..=1.0).contains(cy) {
                            cy * h
                        } else {
                            *cy
                        };
                        let grad_radius = if *gradient_radius > 0.0 {
                            if *gradient_radius <= 1.0 {
                                gradient_radius * (w.max(h) / 2.0)
                            } else {
                                *gradient_radius
                            }
                        } else {
                            (w.max(h) / 2.0).max(1.0)
                        };
                        instance.params[2] = center_x;
                        instance.params[3] = center_y;
                        instance.extra[2] = grad_radius.max(1.0);
                        instance.gradient_stops = stops.clone();
                    }
                }
                SceneNodeKind::Rect { fill: None, .. } => {}
                SceneNodeKind::Text {
                    text,
                    spans,
                    font_family,
                    font_size,
                    color,
                    align,
                } => {
                    self.append_text_glyphs(
                        &mut instances,
                        text,
                        spans,
                        font_family,
                        *font_size,
                        *color,
                        *align,
                        [x, y, w, h],
                        opacity,
                        world_matrix.to_cols_array(),
                    )?;
                }
                SceneNodeKind::Image { path, radius } => {
                    let texture_key = self.image_texture(
                        path,
                        w.ceil().max(1.0) as u32,
                        h.ceil().max(1.0) as u32,
                        *radius,
                    )?;
                    instances.push(DrawInstance {
                        bounds: [x, y, w, h],
                        color: [1.0, 1.0, 1.0, opacity],
                        params: [*radius, 6.0, 0.0, 0.0],
                        extra: [0.0, 0.0, 1.0, 1.0],
                        clip_regions: Vec::new(),
                        matrix: world_matrix.to_cols_array(),
                        gradient_stops: Vec::new(),
                        path_segments: Vec::new(),
                        texture_key: Some(texture_key),
                    });
                }
                SceneNodeKind::Svg { path } => {
                    let width = w.ceil().max(1.0) as u32;
                    let height = h.ceil().max(1.0) as u32;
                    let (texture_key, texture_width, texture_height) = if path.ends_with(".png")
                        || path.ends_with(".jpg")
                        || path.ends_with(".jpeg")
                    {
                        (self.image_texture(path, width, height, 0.0)?, width, height)
                    } else {
                        self.svg_texture(path, width, height)?
                    };
                    let draw_width = texture_width as f32;
                    let draw_height = texture_height as f32;
                    instances.push(DrawInstance {
                        bounds: [
                            x + (w - draw_width) * 0.5,
                            y + (h - draw_height) * 0.5,
                            draw_width,
                            draw_height,
                        ],
                        color: [1.0, 1.0, 1.0, opacity],
                        params: [0.0, 6.0, 0.0, 0.0],
                        extra: [0.0, 0.0, 1.0, 1.0],
                        clip_regions: Vec::new(),
                        matrix: world_matrix.to_cols_array(),
                        gradient_stops: Vec::new(),
                        path_segments: Vec::new(),
                        texture_key: Some(texture_key),
                    });
                }
                SceneNodeKind::CustomPath {
                    color,
                    stroke_width,
                    path,
                } => {
                    let path_segments = Self::tessellate_path(path, x, y);
                    instances.push(DrawInstance {
                        bounds: [x, y, w, h],
                        color: [
                            color.red(),
                            color.green(),
                            color.blue(),
                            color.alpha() * opacity,
                        ],
                        params: [0.0, 8.0, *stroke_width, 0.0],
                        extra: [0.0; 4],
                        clip_regions: Vec::new(),
                        matrix: world_matrix.to_cols_array(),
                        gradient_stops: Vec::new(),
                        path_segments,
                        texture_key: None,
                    });
                }
                SceneNodeKind::Shadow {
                    radius,
                    opacity: shadow_opacity,
                    offset_x,
                    offset_y,
                    color,
                    corner_radius,
                } => {
                    if *radius > 0.0 && *shadow_opacity > 0.0 {
                        let sigma = *radius / 2.5;
                        let blur_pad = (*radius * 1.5).ceil() + 4.0;
                        let color_alpha = if color.alpha() > 0.001 {
                            color.alpha()
                        } else {
                            1.0
                        };
                        let alpha = (color_alpha * shadow_opacity * opacity).clamp(0.0, 1.0);
                        instances.push(DrawInstance {
                            bounds: [
                                x + offset_x - blur_pad,
                                y + offset_y - blur_pad,
                                w + blur_pad * 2.0,
                                h + blur_pad * 2.0,
                            ],
                            color: [color.red(), color.green(), color.blue(), alpha],
                            params: [corner_radius.max(0.0), 2.0, blur_pad, sigma],
                            extra: [blur_pad - offset_x, blur_pad - offset_y, 0.0, 0.0],
                            clip_regions: Vec::new(),
                            matrix: world_matrix.to_cols_array(),
                            gradient_stops: Vec::new(),
                            path_segments: Vec::new(),
                            texture_key: None,
                        });
                    }
                }
                SceneNodeKind::ProgressArc {
                    cx,
                    cy,
                    radius,
                    stroke_width,
                    ratio,
                    track_color,
                    outline_color,
                    outline_width,
                    arc_color,
                    start_angle_deg,
                    is_ccw,
                } => {
                    let mut push_arc = |color: tiny_skia::Color, width: f32, arc_ratio: f32| {
                        let alpha = color.alpha() * opacity;
                        instances.push(DrawInstance {
                            bounds: [x, y, w, h],
                            color: [color.red(), color.green(), color.blue(), alpha],
                            params: [*radius, 3.0, width, arc_ratio.clamp(0.0, 1.0)],
                            extra: [
                                start_angle_deg.to_radians(),
                                if *is_ccw { 1.0 } else { 0.0 },
                                *cx - (x + w * 0.5),
                                *cy - (y + h * 0.5),
                            ],
                            clip_regions: Vec::new(),
                            matrix: world_matrix.to_cols_array(),
                            gradient_stops: Vec::new(),
                            path_segments: Vec::new(),
                            texture_key: None,
                        });
                    };
                    if let Some(color) = outline_color {
                        push_arc(*color, *stroke_width + outline_width * 2.0, 1.0);
                    }
                    if let Some(color) = track_color {
                        push_arc(*color, *stroke_width, 1.0);
                    }
                    if let Some(color) = arc_color {
                        push_arc(*color, *stroke_width, *ratio);
                    }
                }
                SceneNodeKind::Effect(super::scene::Effect::Blur { radius }) => {
                    let blur_w = if w > 0.0 { w } else { width as f32 };
                    let blur_h = if h > 0.0 { h } else { height as f32 };
                    instances.push(DrawInstance {
                        bounds: [x, y, blur_w, blur_h],
                        color: [0.0; 4],
                        params: [0.0, 9.0, radius.max(0.0), 0.0],
                        extra: [0.0; 4],
                        clip_regions: Vec::new(),
                        matrix: glam::Mat4::IDENTITY.to_cols_array(),
                        gradient_stops: Vec::new(),
                        path_segments: Vec::new(),
                        texture_key: None,
                    });
                }
                SceneNodeKind::Effect(super::scene::Effect::Tint(color)) => {
                    add(*color, 0.0, 11.0, 0.0);
                }
                SceneNodeKind::Effect(super::scene::Effect::DropShadow {
                    radius,
                    offset_x,
                    offset_y,
                    color,
                }) => {
                    if *radius > 0.0 {
                        let sigma = *radius / 2.5;
                        let blur_pad = (*radius * 1.5).ceil() + 4.0;
                        let color_alpha = if color.alpha() > 0.001 {
                            color.alpha()
                        } else {
                            1.0
                        };
                        instances.push(DrawInstance {
                            bounds: [
                                x + offset_x - blur_pad,
                                y + offset_y - blur_pad,
                                w + blur_pad * 2.0,
                                h + blur_pad * 2.0,
                            ],
                            color: [
                                color.red(),
                                color.green(),
                                color.blue(),
                                color_alpha * opacity,
                            ],
                            params: [0.0, 2.0, blur_pad, sigma],
                            extra: [blur_pad - offset_x, blur_pad - offset_y, 0.0, 0.0],
                            clip_regions: Vec::new(),
                            matrix: world_matrix.to_cols_array(),
                            gradient_stops: Vec::new(),
                            path_segments: Vec::new(),
                            texture_key: None,
                        });
                    }
                }
                SceneNodeKind::Effect(super::scene::Effect::InnerGlow { radius, color }) => {
                    add(*color, 0.0, 10.0, *radius);
                }
            }
            for instance in &mut instances[first_instance..] {
                instance.clip_regions.clone_from(&clip_regions);
            }
        }
        Ok(instances)
    }
}

#[cfg(all(test, feature = "gpu-render"))]
mod tests {
    use super::{GpuRenderer, OffscreenGpuTarget};
    use crate::render::scene::{Fill, SceneGraph, SceneNodeKind, SceneTransform};

    fn read_target_pixels(
        renderer: &GpuRenderer,
        target: &OffscreenGpuTarget,
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let unaligned_bytes_per_row = width * 4;
        let bytes_per_row = unaligned_bytes_per_row.div_ceil(alignment) * alignment;
        let readback = renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test readback"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = renderer
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("test readback encoder"),
            });
        encoder.copy_texture_to_buffer(
            target.texture().as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        renderer.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        renderer
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        receiver.recv().unwrap().unwrap();
        let mapped = slice.get_mapped_range().unwrap();
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let offset = y * bytes_per_row as usize + x * 4;
                pixels.extend_from_slice(&mapped[offset..offset + 4]);
            }
        }
        drop(mapped);
        readback.unmap();
        pixels
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn offscreen_target_contains_rendered_solid_rect() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 8, 8).unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(1.0, 0.0, 0.0, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (2.0, 2.0, 4.0, 4.0),
            None,
            SceneTransform::default(),
        );

        let frame = renderer
            .render_scene(
                &scene,
                &mut target,
                &crate::render::damage::DamageTracker::default(),
            )
            .unwrap();
        assert_eq!(frame.frame_id, 1);
        drop(frame);
        let mut damage = crate::render::damage::DamageTracker::default();
        damage.add_rect(tiny_skia::IntRect::from_xywh(2, 2, 1, 1).unwrap());
        let mut changed_scene = SceneGraph::new();
        changed_scene.insert(
            changed_scene.root(),
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(0.0, 0.0, 1.0, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (2.0, 2.0, 1.0, 1.0),
            None,
            SceneTransform::default(),
        );
        let frame = renderer
            .render_scene(&changed_scene, &mut target, &damage)
            .unwrap();
        assert_eq!(frame.frame_id, 2);
        drop(frame);

        let bytes_per_row = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test readback"),
            size: u64::from(bytes_per_row) * 8,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = renderer
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("test readback encoder"),
            });
        encoder.copy_texture_to_buffer(
            target.texture().as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(8),
                },
            },
            wgpu::Extent3d {
                width: 8,
                height: 8,
                depth_or_array_layers: 1,
            },
        );
        renderer.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        renderer
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        receiver.recv().unwrap().unwrap();
        let pixels = slice.get_mapped_range().unwrap();
        let pixel = |x: usize, y: usize| {
            let offset = y * bytes_per_row as usize + x * 4;
            &pixels[offset..offset + 4]
        };
        assert_eq!(pixel(2, 2), &[0, 0, 255, 255]);
        assert_eq!(pixel(3, 3), &[255, 0, 0, 255]);
        assert_eq!(pixel(0, 0), &[0, 0, 0, 0]);
    }

    #[test]
    #[ignore = "requires a usable GPU adapter with timestamp-query support"]
    fn damaged_64_by_64_frame_uses_less_gpu_time_than_full_redraw() {
        use crate::render::damage::DamageTracker;

        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let adapter = renderer.adapter.get_info();
        println!(
            "damage_test_adapter={} backend={:?}",
            adapter.name, adapter.backend
        );
        if !renderer
            .adapter
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
        {
            eprintln!("skipping: adapter does not support timestamp queries");
            return;
        }
        let mut target = OffscreenGpuTarget::new(&renderer, 512, 512).unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(0.2, 0.5, 0.8, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (0.0, 0.0, 512.0, 512.0),
            None,
            SceneTransform::default(),
        );

        drop(
            renderer
                .render_scene(&scene, &mut target, &DamageTracker::default())
                .unwrap(),
        );
        let full_redraw_ms = renderer
            .gpu_time_ms()
            .unwrap()
            .expect("timestamp queries were requested on this adapter");

        let mut damage = DamageTracker::default();
        damage.add_rect(tiny_skia::IntRect::from_xywh(32, 32, 64, 64).unwrap());
        drop(renderer.render_scene(&scene, &mut target, &damage).unwrap());
        let damage_redraw_ms = renderer
            .gpu_time_ms()
            .unwrap()
            .expect("timestamp queries were requested on this adapter");

        println!(
            "damage_timing full_redraw_gpu_ms={full_redraw_ms:.6} damage_64x64_gpu_ms={damage_redraw_ms:.6} speedup={:.2}x",
            full_redraw_ms / damage_redraw_ms.max(f64::MIN_POSITIVE)
        );
        assert!(
            damage_redraw_ms < full_redraw_ms * 0.5,
            "damaged render ({damage_redraw_ms:.6} ms) was not at least 2x faster than full redraw ({full_redraw_ms:.6} ms)"
        );
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn clip_children_restricts_gpu_child_to_parent_bounds() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 8, 8).unwrap();
        let mut scene = SceneGraph::new();
        let parent = scene
            .insert(
                scene.root(),
                SceneNodeKind::Container,
                (2.0, 2.0, 4.0, 4.0),
                None,
                SceneTransform::default(),
            )
            .unwrap();
        scene.set_clip_children(parent, true);
        scene.insert(
            parent,
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(1.0, 0.0, 0.0, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (0.0, 0.0, 8.0, 8.0),
            None,
            SceneTransform::default(),
        );

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let pixels = read_target_pixels(&renderer, &target, 8, 8);
        let pixel = |x: usize, y: usize| &pixels[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4];
        assert_eq!(pixel(3, 3), &[255, 0, 0, 255]);
        assert_eq!(pixel(1, 3), &[0, 0, 0, 0]);
        assert_eq!(pixel(6, 3), &[0, 0, 0, 0]);
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn clip_children_uses_transformed_parent_geometry() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 8, 8).unwrap();
        let mut scene = SceneGraph::new();
        let rotate_about_center = glam::Mat4::from_translation(glam::Vec3::new(4.0, 4.0, 0.0))
            * glam::Mat4::from_rotation_z(std::f32::consts::FRAC_PI_4)
            * glam::Mat4::from_translation(glam::Vec3::new(-4.0, -4.0, 0.0));
        let parent = scene
            .insert(
                scene.root(),
                SceneNodeKind::Container,
                (2.0, 2.0, 4.0, 4.0),
                None,
                SceneTransform {
                    matrix: rotate_about_center,
                    opacity: 1.0,
                },
            )
            .unwrap();
        scene.set_clip_children(parent, true);
        scene.insert(
            parent,
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(1.0, 0.0, 0.0, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (0.0, 0.0, 8.0, 8.0),
            None,
            SceneTransform::default(),
        );

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let pixels = read_target_pixels(&renderer, &target, 8, 8);
        let pixel = |x: usize, y: usize| &pixels[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4];
        assert_eq!(pixel(2, 2), &[0, 0, 0, 0]);
        assert_eq!(pixel(4, 4), &[255, 0, 0, 255]);
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_rounded_clip_excludes_corner_pixels() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 16, 16).unwrap();
        let mut scene = SceneGraph::new();

        // Parent container with rounded rect bounds (0, 0, 16, 16) and radius 6.0
        let parent = scene
            .insert(
                scene.root(),
                SceneNodeKind::Rect {
                    fill: None,
                    radius: 6.0,
                },
                (0.0, 0.0, 16.0, 16.0),
                None,
                SceneTransform::default(),
            )
            .unwrap();
        scene.set_clip_children(parent, true);

        // Child fills the entire (0, 0, 16, 16) with solid green
        scene.insert(
            parent,
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(0.0, 1.0, 0.0, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (0.0, 0.0, 16.0, 16.0),
            None,
            SceneTransform::default(),
        );

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let pixels = read_target_pixels(&renderer, &target, 16, 16);
        let pixel = |x: usize, y: usize| &pixels[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4];

        // Center should be green
        assert_eq!(pixel(8, 8), &[0, 255, 0, 255]);
        // Top-left corner pixel (0, 0) should be clipped out by the radius=6.0 rounded clip
        assert_eq!(pixel(0, 0), &[0, 0, 0, 0]);
        assert_eq!(pixel(1, 0), &[0, 0, 0, 0]);
        // Pixel at edge center (8, 0) should remain inside
        assert_eq!(pixel(8, 0), &[0, 255, 0, 255]);
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_rounded_clip_matches_cpu() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 32, 32).unwrap();
        let mut scene = SceneGraph::new();

        let parent = scene
            .insert(
                scene.root(),
                SceneNodeKind::Rect {
                    fill: None,
                    radius: 8.0,
                },
                (0.0, 0.0, 32.0, 32.0),
                None,
                SceneTransform::default(),
            )
            .unwrap();
        scene.set_clip_children(parent, true);

        scene.insert(
            parent,
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(0.0, 0.0, 1.0, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (0.0, 0.0, 32.0, 32.0),
            None,
            SceneTransform::default(),
        );

        let mut cpu_context = crate::render::context::RenderContext::new(1.0);
        let mut cpu = tiny_skia::Pixmap::new(32, 32).unwrap();
        crate::render::scene_to_pixmap(&scene, &mut cpu, &mut cpu_context);

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let gpu = read_target_pixels(&renderer, &target, 32, 32);

        // Verify corner pixels match CPU (clipped out)
        for (x, y) in [
            (0_usize, 0_usize),
            (1, 0),
            (0, 1),
            (31, 0),
            (0, 31),
            (31, 31),
        ] {
            let offset = (y * 32 + x) * 4;
            let gpu_a = gpu[offset + 3];
            let cpu_a = cpu.data()[offset + 3];
            assert_eq!(
                gpu_a, cpu_a,
                "corner clip alpha mismatch at ({x},{y}): GPU={gpu_a}, CPU={cpu_a}"
            );
        }
        // Verify center pixel matches CPU
        let center_offset = (16 * 32 + 16) * 4;
        assert_eq!(gpu[center_offset + 2], 255);
        assert_eq!(cpu.data()[center_offset + 2], 255);
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_linear_gradient_interpolates_all_stops_like_cpu() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 64, 16).unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::Rect {
                fill: Some(Fill::LinearGradient {
                    angle_deg: 90.0,
                    stops: vec![
                        (0.0, tiny_skia::Color::from_rgba8(255, 0, 0, 255)),
                        (0.5, tiny_skia::Color::from_rgba8(0, 255, 0, 255)),
                        (1.0, tiny_skia::Color::from_rgba8(0, 0, 255, 255)),
                    ],
                }),
                radius: 0.0,
            },
            (0.0, 0.0, 64.0, 16.0),
            None,
            SceneTransform::default(),
        );
        let mut cpu_context = crate::render::context::RenderContext::new(1.0);
        let mut cpu = tiny_skia::Pixmap::new(64, 16).unwrap();
        crate::render::scene_to_pixmap(&scene, &mut cpu, &mut cpu_context);

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let gpu = read_target_pixels(&renderer, &target, 64, 16);
        for x in [4_usize, 16, 31, 48, 60] {
            let offset = (8 * 64 + x) * 4;
            for channel in 0..3 {
                let actual = i16::from(gpu[offset + channel]);
                let expected = i16::from(cpu.data()[offset + channel]);
                assert!(
                    (actual - expected).abs() <= 12,
                    "gradient mismatch at x={x}, channel={channel}: GPU={actual}, CPU={expected}"
                );
            }
        }
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_radial_gradient_interpolates_all_stops_like_cpu() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 64, 64).unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::Rect {
                fill: Some(Fill::RadialGradient {
                    cx: 0.5,
                    cy: 0.5,
                    radius: 0.5,
                    stops: vec![
                        (0.0, tiny_skia::Color::from_rgba8(255, 0, 0, 255)),
                        (0.5, tiny_skia::Color::from_rgba8(0, 255, 0, 255)),
                        (1.0, tiny_skia::Color::from_rgba8(0, 0, 255, 255)),
                    ],
                }),
                radius: 0.0,
            },
            (0.0, 0.0, 64.0, 64.0),
            None,
            SceneTransform::default(),
        );
        let mut cpu_context = crate::render::context::RenderContext::new(1.0);
        let mut cpu = tiny_skia::Pixmap::new(64, 64).unwrap();
        crate::render::scene_to_pixmap(&scene, &mut cpu, &mut cpu_context);

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let gpu = read_target_pixels(&renderer, &target, 64, 64);
        for (x, y) in [(32_usize, 32_usize), (40, 32), (47, 32), (55, 32)] {
            let offset = (y * 64 + x) * 4;
            for channel in 0..3 {
                let actual = i16::from(gpu[offset + channel]);
                let expected = i16::from(cpu.data()[offset + channel]);
                assert!(
                    (actual - expected).abs() <= 12,
                    "radial gradient mismatch at ({x},{y}), channel={channel}: GPU={actual}, CPU={expected}"
                );
            }
        }
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_blur_uses_two_passes_and_softens_edges() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 64, 64).unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(1.0, 0.0, 0.0, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (24.0, 24.0, 16.0, 16.0),
            None,
            SceneTransform::default(),
        );
        scene.insert(
            scene.root(),
            SceneNodeKind::Effect(crate::render::scene::Effect::Blur { radius: 8.0 }),
            (0.0, 0.0, 64.0, 64.0),
            None,
            SceneTransform::default(),
        );
        scene.insert(
            scene.root(),
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(0.0, 0.0, 1.0, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (48.0, 24.0, 8.0, 16.0),
            None,
            SceneTransform::default(),
        );

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let pixels = read_target_pixels(&renderer, &target, 64, 64);
        let pixel = |x: usize, y: usize| &pixels[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
        assert!(pixel(32, 32)[0] > 240 && pixel(32, 32)[3] > 240);
        assert_eq!(pixel(32, 32)[1..3], [0, 0]);
        assert!(
            pixel(22, 32)[3] > 0,
            "the blur should spread alpha beyond the original rectangle"
        );
        assert_eq!(pixel(0, 0)[3], 0);
        assert_eq!(pixel(50, 32), &[0, 0, 255, 255]);
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_blur_restricted_to_bounds_leaves_outside_unaffected() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 64, 64).unwrap();
        let mut scene = SceneGraph::new();

        // Draw a solid red rect on the left half (0..32, 0..64)
        scene.insert(
            scene.root(),
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(1.0, 0.0, 0.0, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (0.0, 0.0, 32.0, 64.0),
            None,
            SceneTransform::default(),
        );

        // Apply blur ONLY to region (0, 0, 32, 32)
        scene.insert(
            scene.root(),
            SceneNodeKind::Effect(crate::render::scene::Effect::Blur { radius: 8.0 }),
            (0.0, 0.0, 32.0, 32.0),
            None,
            SceneTransform::default(),
        );

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let pixels = read_target_pixels(&renderer, &target, 64, 64);
        let pixel = |x: usize, y: usize| &pixels[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];

        // The lower half (y > 32) was outside blur bounds and should remain solid red
        assert_eq!(pixel(16, 48), &[255, 0, 0, 255]);
        // The right half (x > 32) outside blur bounds was transparent and should remain transparent
        assert_eq!(pixel(48, 48), &[0, 0, 0, 0]);
        assert_eq!(pixel(48, 16), &[0, 0, 0, 0]);
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_blur_large_target_corner_rect_has_no_scissor_edge_artifacts() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 512, 512).unwrap();
        let mut scene = SceneGraph::new();

        // 40x40 rect placed at corner (0, 0, 40, 40)
        scene.insert(
            scene.root(),
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(1.0, 0.0, 0.0, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (0.0, 0.0, 40.0, 40.0),
            None,
            SceneTransform::default(),
        );

        // Blur effect covering bounds (0, 0, 40, 40) with radius 8.0
        scene.insert(
            scene.root(),
            SceneNodeKind::Effect(crate::render::scene::Effect::Blur { radius: 8.0 }),
            (0.0, 0.0, 40.0, 40.0),
            None,
            SceneTransform::default(),
        );

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let pixels = read_target_pixels(&renderer, &target, 512, 512);
        let pixel = |x: usize, y: usize| &pixels[(y * 512 + x) * 4..(y * 512 + x) * 4 + 4];

        // 1. Pixels outside bounds (e.g. x >= 40 or y >= 40) must be completely untouched (transparent)
        assert_eq!(pixel(40, 0), &[0, 0, 0, 0]);
        assert_eq!(pixel(0, 40), &[0, 0, 0, 0]);
        assert_eq!(pixel(100, 100), &[0, 0, 0, 0]);
        assert_eq!(pixel(500, 500), &[0, 0, 0, 0]);

        // 2. Pixels inside bounds near corner (0, 0) should have blurred red
        assert!(pixel(0, 0)[0] > 100);
        assert!(pixel(20, 20)[0] > 150);

        // 3. Near boundary (e.g. x=39, y=20), blurred alpha smoothly decreases towards edge
        let inside_val = pixel(20, 20)[3];
        let near_edge_val = pixel(39, 20)[3];
        assert!(near_edge_val <= inside_val);
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_blur_matches_cpu() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 64, 64).unwrap();
        let mut scene = SceneGraph::new();

        scene.insert(
            scene.root(),
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(1.0, 0.0, 0.0, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (16.0, 16.0, 32.0, 32.0),
            None,
            SceneTransform::default(),
        );
        scene.insert(
            scene.root(),
            SceneNodeKind::Effect(crate::render::scene::Effect::Blur { radius: 6.0 }),
            (16.0, 16.0, 32.0, 32.0),
            None,
            SceneTransform::default(),
        );

        let mut cpu_context = crate::render::context::RenderContext::new(1.0);
        let mut cpu = tiny_skia::Pixmap::new(64, 64).unwrap();
        crate::render::scene_to_pixmap(&scene, &mut cpu, &mut cpu_context);

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let gpu = read_target_pixels(&renderer, &target, 64, 64);

        // Outside the blur bounds (e.g. y = 4), both CPU and GPU should be transparent
        assert_eq!(gpu[(4 * 64 + 4) * 4 + 3], 0);
        assert_eq!(cpu.data()[(4 * 64 + 4) * 4 + 3], 0);

        // Center of blurred rect should remain strongly colored in both
        let center = (32 * 64 + 32) * 4;
        assert!(gpu[center] > 200);
        assert!(cpu.data()[center] > 200);

        // Compute mean absolute channel error between CPU (2-pass box blur)
        // and GPU blur across the active 64x64 pixmap. While CPU uses a fast box blur
        // and GPU uses shader blur approximations, the mean channel difference
        // should remain tight (well below 25/255).
        let total_diff: u64 = gpu
            .iter()
            .zip(cpu.data().iter())
            .map(|(&g, &c)| (g as i64 - c as i64).unsigned_abs())
            .sum();
        let pixel_count = (64 * 64 * 4) as f64;
        let mae = total_diff as f64 / pixel_count;
        println!("gpu_blur_matches_cpu MAE: {:.2}", mae);
        // Threshold is intentionally loose (6x typical MAE ~4.3) because
        // different Vulkan adapters (llvmpipe, Intel, AMD) produce
        // slightly different float rounding in the blur taps.
        // A tighter bound would flake across CI runners.
        assert!(
            mae < 25.0,
            "Mean absolute error between GPU and CPU blur too high: {mae:.2} >= 25.0"
        );
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_drop_shadow_uses_gaussian_sdf_falloff() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 64, 32).unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::Effect(crate::render::scene::Effect::DropShadow {
                radius: 4.0,
                offset_x: 4.0,
                offset_y: 0.0,
                color: tiny_skia::Color::from_rgba8(0, 0, 0, 128),
            }),
            (20.0, 8.0, 16.0, 16.0),
            None,
            SceneTransform::default(),
        );
        let mut cpu_context = crate::render::context::RenderContext::new(1.0);
        let mut cpu = tiny_skia::Pixmap::new(64, 32).unwrap();
        crate::render::scene_to_pixmap(&scene, &mut cpu, &mut cpu_context);

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let pixels = read_target_pixels(&renderer, &target, 64, 32);
        let alpha = |x: usize, y: usize| pixels[(y * 64 + x) * 4 + 3];
        let inside = alpha(38, 16);
        let penumbra = alpha(41, 16);
        let outside = alpha(52, 16);
        assert!(
            (70..=136).contains(&inside),
            "shifted shadow alpha: {inside}"
        );
        assert!(
            penumbra > 30 && penumbra < inside,
            "shadow penumbra alpha: {penumbra}"
        );
        assert!(outside < 3, "shadow cutoff alpha: {outside}");
        for x in [30_usize, 38, 41, 52] {
            let gpu_alpha = i16::from(alpha(x, 16));
            let cpu_alpha = i16::from(cpu.data()[(16 * 64 + x) * 4 + 3]);
            assert!(
                (gpu_alpha - cpu_alpha).abs() <= 3,
                "drop shadow differs at x={x}: GPU={gpu_alpha}, CPU={cpu_alpha}"
            );
        }
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_inner_glow_fades_toward_shape_center() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 32, 32).unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::Effect(crate::render::scene::Effect::InnerGlow {
                radius: 4.0,
                color: tiny_skia::Color::from_rgba8(255, 0, 0, 255),
            }),
            (8.0, 8.0, 16.0, 16.0),
            None,
            SceneTransform::default(),
        );
        let mut cpu_context = crate::render::context::RenderContext::new(1.0);
        let mut cpu = tiny_skia::Pixmap::new(32, 32).unwrap();
        crate::render::scene_to_pixmap(&scene, &mut cpu, &mut cpu_context);

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let pixels = read_target_pixels(&renderer, &target, 32, 32);
        let pixel = |x: usize, y: usize| &pixels[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert!(pixel(8, 16)[3] > 180);
        assert!(pixel(16, 16)[3] < 8);
        for (x, y) in [(8_usize, 16_usize), (12, 12), (16, 16)] {
            let gpu_alpha = i16::from(pixel(x, y)[3]);
            let cpu_alpha = i16::from(cpu.data()[(y * 32 + x) * 4 + 3]);
            assert!(
                (gpu_alpha - cpu_alpha).abs() <= 2,
                "inner glow alpha differs at ({x},{y}): GPU={gpu_alpha}, CPU={cpu_alpha}"
            );
        }
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_tint_multiplies_destination_like_cpu() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 32, 32).unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(tiny_skia::Color::from_rgba8(
                    100, 160, 220, 255,
                ))),
                radius: 0.0,
            },
            (8.0, 8.0, 16.0, 16.0),
            None,
            SceneTransform::default(),
        );
        scene.insert(
            scene.root(),
            SceneNodeKind::Effect(crate::render::scene::Effect::Tint(
                tiny_skia::Color::from_rgba8(255, 128, 64, 128),
            )),
            (8.0, 8.0, 16.0, 16.0),
            None,
            SceneTransform::default(),
        );
        let mut cpu_context = crate::render::context::RenderContext::new(1.0);
        let mut cpu = tiny_skia::Pixmap::new(32, 32).unwrap();
        crate::render::scene_to_pixmap(&scene, &mut cpu, &mut cpu_context);

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let gpu = read_target_pixels(&renderer, &target, 32, 32);
        let offset = (12 * 32 + 12) * 4;
        for channel in 0..4 {
            let actual = i16::from(gpu[offset + channel]);
            let expected = i16::from(cpu.data()[offset + channel]);
            assert!(
                (actual - expected).abs() <= 2,
                "tint mismatch at channel={channel}: GPU={actual}, CPU={expected}"
            );
        }
    }

    #[test]
    #[ignore = "requires a usable GPU adapter and system fonts"]
    fn gpu_text_glyph_atlas_matches_cpu_raster() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 160, 40).unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::Text {
                text: "Hello World".to_string(),
                spans: Vec::new(),
                font_family: "DejaVu Sans".to_string(),
                font_size: 22.0,
                color: tiny_skia::Color::from_rgba8(220, 80, 40, 255),
                align: cosmic_text::Align::Left,
            },
            (0.0, 0.0, 160.0, 40.0),
            None,
            SceneTransform::default(),
        );
        let mut cpu_context = crate::render::context::RenderContext::new(1.0);
        let mut cpu = tiny_skia::Pixmap::new(160, 40).unwrap();
        crate::render::scene_to_pixmap(&scene, &mut cpu, &mut cpu_context);

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let gpu = read_target_pixels(&renderer, &target, 160, 40);
        let cpu_pixels = cpu.data().as_chunks::<4>().0;
        let gpu_pixels = gpu.as_chunks::<4>().0;
        let cpu_covered = cpu_pixels.iter().filter(|pixel| pixel[3] > 0).count();
        let gpu_covered = gpu_pixels.iter().filter(|pixel| pixel[3] > 0).count();
        let mean_absolute_error = cpu_pixels
            .iter()
            .zip(gpu_pixels)
            .flat_map(|(cpu, gpu)| {
                cpu.iter()
                    .zip(gpu)
                    .map(|(left, right)| left.abs_diff(*right))
            })
            .map(u64::from)
            .sum::<u64>() as f64
            / (cpu_pixels.len() * 4) as f64;
        println!(
            "glyph_cpu_gpu_comparison cpu_covered={cpu_covered} gpu_covered={gpu_covered} mean_absolute_channel_error={mean_absolute_error:.3}"
        );
        assert!(cpu_covered > 50, "CPU reference rendered no text");
        assert!(
            gpu_covered > 50,
            "GPU text texture rendered no glyph pixels"
        );
        let coverage_delta = cpu_covered.abs_diff(gpu_covered);
        assert!(
            coverage_delta < cpu_covered / 8 + 8,
            "GPU and CPU glyph coverage differ: CPU={cpu_covered}, GPU={gpu_covered}"
        );
        assert!(
            mean_absolute_error < 8.0,
            "GPU and CPU glyph pixels differ too much: MAE={mean_absolute_error:.3}"
        );
        assert!(!renderer
            .glyph_atlas
            .as_ref()
            .expect("text rendering creates the glyph atlas")
            .glyphs
            .is_empty());
        assert!(renderer.texture_assets.is_empty());
        let glyph_count = renderer.glyph_atlas.as_ref().unwrap().glyphs.len();
        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        assert_eq!(
            renderer.glyph_atlas.as_ref().unwrap().glyphs.len(),
            glyph_count
        );
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_text_glyph_atlas_respects_horizontal_alignment() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 210, 36).unwrap();
        let mut scene = SceneGraph::new();
        for (x, align) in [
            (0.0, cosmic_text::Align::Left),
            (70.0, cosmic_text::Align::Center),
            (140.0, cosmic_text::Align::Right),
        ] {
            scene.insert(
                scene.root(),
                SceneNodeKind::Text {
                    text: "Hi".to_string(),
                    spans: Vec::new(),
                    font_family: "DejaVu Sans".to_string(),
                    font_size: 20.0,
                    color: tiny_skia::Color::WHITE,
                    align,
                },
                (x, 0.0, 70.0, 30.0),
                None,
                SceneTransform::default(),
            );
        }
        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let pixels = read_target_pixels(&renderer, &target, 210, 36);
        let leftmost = |range: std::ops::Range<usize>| {
            for y in 0..36 {
                for x in range.clone() {
                    if pixels[(y * 210 + x) * 4 + 3] > 0 {
                        return x;
                    }
                }
            }
            panic!("aligned text produced pixels")
        };
        let left = leftmost(0..70);
        let center = leftmost(70..140);
        let right = leftmost(140..210);
        assert!(center > left + 10, "center alignment should shift text");
        assert!(right > center + 10, "right alignment should shift text");
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_image_texture_pool_matches_cpu_pixels_and_rounding() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let width = 16;
        let height = 16;
        let path = std::env::temp_dir().join(format!(
            "wyrd-gpu-image-{}-{}.png",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let mut image = image::RgbaImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = if x < width / 2 && y < height / 2 {
                image::Rgba([255, 0, 0, 255])
            } else if x >= width / 2 && y < height / 2 {
                image::Rgba([0, 255, 0, 255])
            } else if x < width / 2 {
                image::Rgba([0, 0, 255, 255])
            } else {
                image::Rgba([255, 255, 0, 255])
            };
        }
        image.save(&path).unwrap();
        let path_string = path.to_string_lossy().into_owned();
        let mut target = OffscreenGpuTarget::new(&renderer, width, height).unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::Image {
                path: path_string.clone(),
                radius: 5.0,
            },
            (0.0, 0.0, width as f32, height as f32),
            None,
            SceneTransform::default(),
        );
        let mut cpu_context = crate::render::context::RenderContext::new(1.0);
        let mut cpu = tiny_skia::Pixmap::new(width, height).unwrap();
        crate::render::scene_to_pixmap(&scene, &mut cpu, &mut cpu_context);

        let frame = renderer
            .render_scene(
                &scene,
                &mut target,
                &crate::render::damage::DamageTracker::default(),
            )
            .unwrap();
        drop(frame);
        let gpu = read_target_pixels(&renderer, &target, width, height);
        let cpu = cpu.data();
        for (x, y) in [(7_usize, 7_usize), (12, 7), (7, 12), (12, 12), (0, 0)] {
            let offset = (y * width as usize + x) * 4;
            for channel in 0..4 {
                assert!(
                    i16::from(gpu[offset + channel]).abs_diff(i16::from(cpu[offset + channel]))
                        <= 10,
                    "image mismatch at ({x},{y}), channel {channel}: GPU={} CPU={}",
                    gpu[offset + channel],
                    cpu[offset + channel]
                );
            }
        }
        assert_eq!(renderer.texture_assets.len(), 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_svg_texture_pool_matches_cpu_render() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let path = std::env::temp_dir().join(format!("wyrd-gpu-svg-{}.svg", std::process::id()));
        std::fs::write(
            &path,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect x="2" y="2" width="12" height="12" fill="#22aa66"/></svg>"##,
        )
        .unwrap();
        let path_string = path.to_string_lossy().into_owned();
        let mut target = OffscreenGpuTarget::new(&renderer, 32, 32).unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::Svg { path: path_string },
            (0.0, 0.0, 32.0, 32.0),
            None,
            SceneTransform::default(),
        );
        let mut cpu_context = crate::render::context::RenderContext::new(1.0);
        let mut cpu = tiny_skia::Pixmap::new(32, 32).unwrap();
        crate::render::scene_to_pixmap(&scene, &mut cpu, &mut cpu_context);

        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let gpu = read_target_pixels(&renderer, &target, 32, 32);
        let cpu = cpu.data();
        for (x, y) in [(8_usize, 8_usize), (16, 16), (23, 23), (1, 1)] {
            let offset = (y * 32 + x) * 4;
            for channel in 0..4 {
                assert!(
                    i16::from(gpu[offset + channel]).abs_diff(i16::from(cpu[offset + channel]))
                        <= 12,
                    "SVG mismatch at ({x},{y}), channel {channel}: GPU={} CPU={}",
                    gpu[offset + channel],
                    cpu[offset + channel]
                );
            }
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_custom_path_strokes_tessellated_segments() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut path_builder = tiny_skia::PathBuilder::new();
        path_builder.move_to(2.0, 2.0);
        path_builder.line_to(13.0, 2.0);
        path_builder.line_to(13.0, 13.0);
        let path = path_builder.finish().unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::CustomPath {
                color: tiny_skia::Color::from_rgba8(255, 180, 20, 255),
                stroke_width: 2.0,
                path,
            },
            (0.0, 0.0, 16.0, 16.0),
            None,
            SceneTransform::default(),
        );
        let mut target = OffscreenGpuTarget::new(&renderer, 16, 16).unwrap();
        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let pixels = read_target_pixels(&renderer, &target, 16, 16);
        let coverage = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[3] > 0)
            .count();
        assert!(coverage >= 30, "path stroke coverage too small: {coverage}");
        assert!(
            pixels[(2 * 16 + 8) * 4 + 3] > 0,
            "horizontal segment is missing"
        );
        assert!(
            pixels[(8 * 16 + 13) * 4 + 3] > 0,
            "vertical segment is missing"
        );
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_scene_matrix_translates_geometry() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 8, 8).unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(1.0, 0.0, 0.0, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (1.0, 1.0, 2.0, 2.0),
            None,
            SceneTransform {
                matrix: glam::Mat4::from_translation(glam::Vec3::new(3.0, 0.0, 0.0)),
                opacity: 1.0,
            },
        );
        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let pixels = read_target_pixels(&renderer, &target, 8, 8);
        let pixel = |x: usize, y: usize| &pixels[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4];
        assert_eq!(pixel(4, 2), &[255, 0, 0, 255]);
        assert_eq!(pixel(1, 2), &[0, 0, 0, 0]);
    }

    #[test]
    #[ignore = "requires a usable GPU adapter"]
    fn gpu_scene_matrix_applies_rotation_and_scale() {
        let mut renderer = pollster::block_on(GpuRenderer::new())
            .expect("a usable GPU adapter is required for this test");
        let mut target = OffscreenGpuTarget::new(&renderer, 8, 8).unwrap();
        let mut scene = SceneGraph::new();
        scene.insert(
            scene.root(),
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(
                    tiny_skia::Color::from_rgba(1.0, 0.0, 0.0, 1.0).unwrap(),
                )),
                radius: 0.0,
            },
            (0.0, 0.0, 1.0, 2.0),
            None,
            SceneTransform {
                matrix: glam::Mat4::from_translation(glam::Vec3::new(6.0, 1.0, 0.0))
                    * glam::Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2)
                    * glam::Mat4::from_scale(glam::Vec3::new(2.0, 1.0, 1.0)),
                opacity: 1.0,
            },
        );
        drop(
            renderer
                .render_scene(
                    &scene,
                    &mut target,
                    &crate::render::damage::DamageTracker::default(),
                )
                .unwrap(),
        );
        let pixels = read_target_pixels(&renderer, &target, 8, 8);
        let pixel = |x: usize, y: usize| &pixels[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4];
        assert_eq!(pixel(4, 1), &[255, 0, 0, 255]);
        assert_eq!(pixel(5, 2), &[255, 0, 0, 255]);
        assert_eq!(pixel(1, 1), &[0, 0, 0, 0]);
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

    pub fn supports_timestamp_queries(&self) -> bool {
        false
    }

    pub fn render_scene(&self, _scene: &super::scene::SceneGraph) -> Result<()> {
        anyhow::bail!("GPU rendering feature 'gpu-render' was disabled at compile time")
    }
}
