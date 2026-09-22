use super::backend::RenderMode;
use crate::animator::Animator;
use cosmic_text::{Buffer, FontSystem, SwashCache};
use std::collections::HashMap;
use std::sync::Arc;

pub type ShadowCacheKey = (u32, u32, u32, u32, u32, i32, i32);

pub struct RenderContext {
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    pub image_cache: ImageCache,
    pub text_buffers: HashMap<(Arc<str>, u32), Buffer>,
    pub path_cache: HashMap<(u32, u32, u32), tiny_skia::Path>,
    pub shadow_cache: HashMap<ShadowCacheKey, Arc<tiny_skia::Pixmap>>,
    pub animator: Animator,
    pub scale: f64,
    pub debug_overlay: bool,
    pub render_mode: RenderMode,
    pub prev_scenes: HashMap<usize, super::scene::SceneGraph>,
    pub gpu: Option<super::gpu::GpuRenderer>,
}

impl RenderContext {
    pub fn new(scale: f64) -> Self {
        Self::with_mode(scale, RenderMode::Cpu)
    }

    pub fn with_mode(scale: f64, render_mode: RenderMode) -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            image_cache: ImageCache::new(),
            text_buffers: HashMap::new(),
            path_cache: HashMap::new(),
            shadow_cache: HashMap::new(),
            animator: Animator::new(),
            scale,
            debug_overlay: false,
            render_mode,
            prev_scenes: HashMap::new(),
            gpu: None,
        }
    }

    pub fn clear_path_cache(&mut self) {
        self.path_cache.clear();
    }

    pub fn resolve_font_family<'a>(&self, requested: &'a str) -> &'a str {
        for candidate in requested.split(',') {
            let name = candidate.trim();
            if name.is_empty() || name.eq_ignore_ascii_case("sans-serif") {
                continue;
            }
            if self.font_system.db().faces().any(|face| {
                face.families
                    .iter()
                    .any(|(family, _)| family.eq_ignore_ascii_case(name))
            }) {
                return name;
            }
        }

        // Smart fallback to installed Nerd Fonts if requested font wasn't found
        for preferred in &[
            "JetBrainsMono Nerd Font",
            "JetBrains Mono",
            "MesloLGS Nerd Font",
            "MesloLGL Nerd Font",
            "MesloLGMDZ Nerd Font",
            "FantasqueSansM Nerd Font",
            "DejaVu Sans Mono",
            "DejaVu Sans",
            "sans-serif",
        ] {
            if self.font_system.db().faces().any(|face| {
                face.families
                    .iter()
                    .any(|(family, _)| family.eq_ignore_ascii_case(preferred))
            }) {
                return preferred;
            }
        }

        "sans-serif"
    }
}

pub struct ImageCache {
    cache: HashMap<ImageKey, tiny_skia::Pixmap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImageKey {
    pub path: Arc<str>,
    pub is_svg: bool,
    pub width: u32,
    pub height: u32,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    #[inline]
    pub fn get(
        &self,
        path: &str,
        is_svg: bool,
        width: u32,
        height: u32,
    ) -> Option<&tiny_skia::Pixmap> {
        let key = ImageKey {
            path: Arc::from(path),
            is_svg,
            width,
            height,
        };
        self.cache.get(&key)
    }

    #[inline]
    pub fn insert(
        &mut self,
        path: &str,
        is_svg: bool,
        width: u32,
        height: u32,
        pixmap: tiny_skia::Pixmap,
    ) {
        let key = ImageKey {
            path: Arc::from(path),
            is_svg,
            width,
            height,
        };
        self.cache.insert(key, pixmap);
    }
}
