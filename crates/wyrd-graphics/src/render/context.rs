use super::backend::RenderMode;
use crate::animator::Animator;
use crate::sync_helpers::lock_unpoisoned;
use cosmic_text::{Buffer, FontSystem, SwashCache};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

pub type ShadowCacheKey = (u32, u32, u32, u32, u32, i32, i32);

/// Maximum number of entries retained in [`DimensionCache`].
pub const MAX_DIMENSION_CACHE_ENTRIES: usize = 256;

#[derive(Debug, Clone)]
struct DimensionCacheEntry {
    dims: (u32, u32),
    mtime: Option<SystemTime>,
    last_used: u64,
}

/// Bounded LRU cache (up to 256 entries) for raster image and SVG dimensions,
/// invalidated automatically when a file's modification time (`mtime`) changes.
#[derive(Debug, Clone)]
pub struct DimensionCache {
    entries: HashMap<PathBuf, DimensionCacheEntry>,
    access_seq: u64,
    decode_counter: Arc<AtomicUsize>,
}

impl Default for DimensionCache {
    fn default() -> Self {
        Self::new()
    }
}

impl DimensionCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            access_seq: 0,
            decode_counter: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns the number of underlying file header/SVG decodes performed by this cache.
    pub fn decode_count(&self) -> usize {
        self.decode_counter.load(Ordering::Relaxed)
    }

    fn file_mtime(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
    }

    fn get_if_fresh(
        &mut self,
        path: &Path,
        current_mtime: Option<SystemTime>,
    ) -> Option<(u32, u32)> {
        if let Some(entry) = self.entries.get_mut(path) {
            if entry.mtime == current_mtime {
                self.access_seq = self.access_seq.wrapping_add(1);
                entry.last_used = self.access_seq;
                return Some(entry.dims);
            }
        }
        None
    }

    fn insert(&mut self, path: PathBuf, dims: (u32, u32), mtime: Option<SystemTime>) {
        if !self.entries.contains_key(&path) && self.entries.len() >= MAX_DIMENSION_CACHE_ENTRIES {
            if let Some(lru_key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&lru_key);
            }
        }
        self.access_seq = self.access_seq.wrapping_add(1);
        self.entries.insert(
            path,
            DimensionCacheEntry {
                dims,
                mtime,
                last_used: self.access_seq,
            },
        );
    }

    /// Resolves raster image dimensions with mtime-aware LRU caching.
    pub fn image_dimensions(&mut self, path: &Path) -> Option<(u32, u32)> {
        let current_mtime = Self::file_mtime(path);
        if let Some(cached) = self.get_if_fresh(path, current_mtime) {
            return Some(cached);
        }
        self.decode_counter.fetch_add(1, Ordering::Relaxed);
        let dims = image::image_dimensions(path).ok()?;
        self.insert(path.to_path_buf(), dims, current_mtime);
        Some(dims)
    }

    /// Resolves SVG intrinsic dimensions with mtime-aware LRU caching.
    pub fn svg_dimensions(&mut self, path: &Path) -> Option<(u32, u32)> {
        let current_mtime = Self::file_mtime(path);
        if let Some(cached) = self.get_if_fresh(path, current_mtime) {
            return Some(cached);
        }
        self.decode_counter.fetch_add(1, Ordering::Relaxed);
        let data = std::fs::read(path).ok()?;
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
        let dims = (
            tree.size().width().round().max(1.0) as u32,
            tree.size().height().round().max(1.0) as u32,
        );
        self.insert(path.to_path_buf(), dims, current_mtime);
        Some(dims)
    }
}

use std::sync::OnceLock;

static SHARED_SYSTEM_FONT_DB: OnceLock<cosmic_text::fontdb::Database> = OnceLock::new();
static SYSTEM_LOCALE: OnceLock<String> = OnceLock::new();
static FONT_LOADER_SPAWNED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn detect_system_locale() -> &'static str {
    SYSTEM_LOCALE
        .get_or_init(|| {
            sys_locale::get_locale().unwrap_or_else(|| {
                log::warn!("failed to get system locale, falling back to en-US");
                "en-US".to_string()
            })
        })
        .as_str()
}

fn load_or_init_system_font_db() -> &'static cosmic_text::fontdb::Database {
    SHARED_SYSTEM_FONT_DB.get_or_init(|| {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

fn spawn_background_font_loader() {
    if SHARED_SYSTEM_FONT_DB.get().is_none() && !FONT_LOADER_SPAWNED.swap(true, Ordering::SeqCst) {
        let _ = std::thread::Builder::new()
            .name("wyrd-font-loader".to_string())
            .spawn(|| {
                let _ = load_or_init_system_font_db();
            });
    }
}

pub struct RenderContext {
    /// The cosmic-text [`FontSystem`] used for font face resolution, text shaping, and caching.
    ///
    /// # Contract
    /// When a [`RenderContext`] is constructed via [`Self::new`] or [`Self::with_mode`], this field
    /// holds an **empty** font database so context initialization never blocks the caller on filesystem I/O.
    /// System fonts are scanned asynchronously on a background thread.
    ///
    /// If external code reads `self.font_system` directly (bypassing high-level rendering in
    /// `crate::render::text`), it **must** call [`Self::ensure_system_fonts`] first; otherwise queries against
    /// the database will see zero system font faces without raising an error.
    /// High-level text shaping and rendering functions (`TextRenderer::measure_constrained`, `TextRenderer::render_aligned`)
    /// call [`Self::ensure_system_fonts`] automatically before accessing this field.
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    pub image_cache: ImageCache,
    pub dimension_cache: DimensionCache,
    pub image_fetcher: Arc<dyn super::ImageFetcher>,
    pub text_buffers: HashMap<(Arc<str>, u32), Buffer>,
    pub path_cache: HashMap<(u32, u32, u32), tiny_skia::Path>,
    pub shadow_cache: HashMap<ShadowCacheKey, Arc<tiny_skia::Pixmap>>,
    pub font_family_cache: std::sync::Mutex<HashMap<String, Arc<str>>>,
    pub animator: Animator,
    pub scale: f64,
    pub debug_overlay: bool,
    pub render_mode: RenderMode,
    pub prev_scenes: HashMap<usize, super::scene::SceneGraph>,
    pub cached_surface_pixmaps: HashMap<usize, (u32, u32, tiny_skia::Pixmap)>,
    pub gpu: Option<super::gpu::GpuRenderer>,
    system_fonts_loaded: bool,
}

impl RenderContext {
    /// Creates a new CPU [`RenderContext`] for the specified display scale.
    ///
    /// # Font Loading Contract
    /// System font discovery is initiated on a background thread to prevent blocking the caller during
    /// startup. As a result, [`self.font_system`](Self::font_system) starts with an empty font database.
    /// See [`Self::ensure_system_fonts`] for details.
    pub fn new(scale: f64) -> Self {
        Self::with_mode(scale, RenderMode::Cpu)
    }

    /// Creates a new [`RenderContext`] with the specified display scale and [`RenderMode`].
    ///
    /// # Font Loading Contract
    /// System font discovery is initiated on a background thread to prevent blocking the caller during
    /// startup. As a result, [`self.font_system`](Self::font_system) starts with an empty font database.
    /// See [`Self::ensure_system_fonts`] for details.
    pub fn with_mode(scale: f64, render_mode: RenderMode) -> Self {
        spawn_background_font_loader();
        let locale = detect_system_locale();
        let (font_system, system_fonts_loaded) = match SHARED_SYSTEM_FONT_DB.get() {
            Some(db) => (
                FontSystem::new_with_locale_and_db(locale.to_string(), db.clone()),
                true,
            ),
            None => (
                FontSystem::new_with_locale_and_db(
                    locale.to_string(),
                    cosmic_text::fontdb::Database::new(),
                ),
                false,
            ),
        };

        Self {
            font_system,
            swash_cache: SwashCache::new(),
            image_cache: ImageCache::new(),
            dimension_cache: DimensionCache::new(),
            image_fetcher: super::default_image_fetcher(),
            text_buffers: HashMap::new(),
            path_cache: HashMap::new(),
            shadow_cache: HashMap::new(),
            font_family_cache: std::sync::Mutex::new(HashMap::new()),
            animator: Animator::new(),
            scale,
            debug_overlay: false,
            render_mode,
            prev_scenes: HashMap::new(),
            cached_surface_pixmaps: HashMap::new(),
            gpu: None,
            system_fonts_loaded,
        }
    }

    /// Ensures the background-loaded system font database has been swapped into [`self.font_system`](Self::font_system).
    ///
    /// # Contract
    /// [`self.font_system`](Self::font_system) starts with an empty font database when [`RenderContext`] is constructed.
    /// Any code that inspects or queries `self.font_system` directly must invoke `ensure_system_fonts()` first
    /// to guarantee that the system font database is populated.
    ///
    /// Standard rendering routines (`TextRenderer::measure_constrained`, `TextRenderer::render_aligned`)
    /// call this method automatically.
    pub fn ensure_system_fonts(&mut self) {
        if !self.system_fonts_loaded {
            let db = load_or_init_system_font_db().clone();
            let locale = detect_system_locale();
            self.font_system = FontSystem::new_with_locale_and_db(locale.to_string(), db);
            self.text_buffers.clear();
            lock_unpoisoned(&self.font_family_cache).clear();
            self.system_fonts_loaded = true;
        }
    }

    pub fn clear_path_cache(&mut self) {
        self.path_cache.clear();
    }

    /// Resolves a comma-separated font family list against the system font database,
    /// returning an [`Arc<str>`] cached inside `self.font_family_cache` without leaking memory.
    pub fn resolve_font_family(&self, requested: &str) -> Arc<str> {
        {
            let cache = lock_unpoisoned(&self.font_family_cache);
            if let Some(cached) = cache.get(requested) {
                return Arc::clone(cached);
            }
        }

        let db = if self.system_fonts_loaded {
            self.font_system.db()
        } else {
            load_or_init_system_font_db()
        };

        let resolved: Arc<str> = 'resolve: {
            for candidate in requested.split(',') {
                let name = candidate.trim();
                if name.is_empty() || name.eq_ignore_ascii_case("sans-serif") {
                    continue;
                }
                if db.faces().any(|face| {
                    face.families
                        .iter()
                        .any(|(family, _)| family.eq_ignore_ascii_case(name))
                }) {
                    break 'resolve Arc::from(name);
                }
            }

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
                if db.faces().any(|face| {
                    face.families
                        .iter()
                        .any(|(family, _)| family.eq_ignore_ascii_case(preferred))
                }) {
                    break 'resolve Arc::from(*preferred);
                }
            }

            Arc::from("sans-serif")
        };

        let mut cache = lock_unpoisoned(&self.font_family_cache);
        if cache.len() >= MAX_DIMENSION_CACHE_ENTRIES && !cache.contains_key(requested) {
            cache.clear();
        }
        cache.insert(requested.to_string(), Arc::clone(&resolved));
        resolved
    }
}

pub struct ImageCache {
    cache: HashMap<ImageKey, (tiny_skia::Pixmap, std::cell::Cell<u64>)>,
    access_seq: std::cell::Cell<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImageKey {
    pub path: Arc<str>,
    pub is_svg: bool,
    pub width: u32,
    pub height: u32,
    pub radius_tenth_px: u32,
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
            access_seq: std::cell::Cell::new(0),
        }
    }

    #[inline]
    pub fn get(
        &self,
        path: &str,
        is_svg: bool,
        width: u32,
        height: u32,
        radius: f32,
    ) -> Option<&tiny_skia::Pixmap> {
        let key = ImageKey {
            path: Arc::from(path),
            is_svg,
            width,
            height,
            radius_tenth_px: (radius.max(0.0) * 10.0).round() as u32,
        };
        if let Some((pixmap, last_used)) = self.cache.get(&key) {
            let next = self.access_seq.get().wrapping_add(1);
            self.access_seq.set(next);
            last_used.set(next);
            Some(pixmap)
        } else {
            None
        }
    }

    #[inline]
    pub fn insert(
        &mut self,
        path: &str,
        is_svg: bool,
        width: u32,
        height: u32,
        radius: f32,
        pixmap: tiny_skia::Pixmap,
    ) {
        if self.cache.len() >= 512 {
            let mut stamps: Vec<u64> = self.cache.values().map(|(_, seq)| seq.get()).collect();
            stamps.sort_unstable();
            let cutoff = stamps[stamps.len() / 2];
            self.cache.retain(|_, (_, seq)| seq.get() > cutoff);
        }
        let next = self.access_seq.get().wrapping_add(1);
        self.access_seq.set(next);
        let key = ImageKey {
            path: Arc::from(path),
            is_svg,
            width,
            height,
            radius_tenth_px: (radius.max(0.0) * 10.0).round() as u32,
        };
        self.cache.insert(key, (pixmap, std::cell::Cell::new(next)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_font_family_returns_arc_str_and_caches() {
        let ctx = RenderContext::new(1.0);
        let first = ctx.resolve_font_family("NonExistentFont12345, sans-serif");
        let second = ctx.resolve_font_family("NonExistentFont12345, sans-serif");
        assert!(!first.is_empty());
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn test_font_family_cache_bounded_for_1000_unique_families() {
        let ctx = RenderContext::new(1.0);
        for idx in 0..1000 {
            let family = format!("SyntheticMissingFont_{idx}, sans-serif");
            let resolved = ctx.resolve_font_family(&family);
            assert!(!resolved.is_empty());
        }
        let cached_len = lock_unpoisoned(&ctx.font_family_cache).len();
        assert!(
            cached_len <= MAX_DIMENSION_CACHE_ENTRIES,
            "font_family_cache must stay bounded at <= {MAX_DIMENSION_CACHE_ENTRIES}, got {cached_len}"
        );
    }

    #[test]
    fn test_load_or_cache_image_http_never_blocks_caller() {
        let start = std::time::Instant::now();
        let resolved = crate::render::resolve_or_cache_image_path(
            "https://127.0.0.1:1/nonexistent_album_art.png",
        );
        let elapsed = start.elapsed();
        assert!(resolved.is_none());
        assert!(
            elapsed < std::time::Duration::from_millis(250),
            "HTTP image resolution must never block the render caller (took {elapsed:?})"
        );
    }
}
