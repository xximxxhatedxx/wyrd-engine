# Changelog

All notable changes to `wyrd-engine` will be documented in this file.

## [0.1.0] - 2026-09-30

### Fixed
- **Trimmed Tokio Dependencies in `wyrd-graphics` & `wyrd-script`**:
  - `wyrd-graphics`: scoped runtime dependencies to `tokio = { default-features = false, features = ["sync"] }`, moving `macros` and `rt-multi-thread` to `[dev-dependencies]`. This eliminates unwanted Tokio executor overhead in non-test consumers.
  - `wyrd-script`: scoped runtime dependencies to `tokio = { default-features = false, features = ["fs", "sync"] }`.
- **Non-Blocking Background Font Database Loading**:
  - `RenderContext::new` and `RenderContext::with_mode` no longer synchronously block caller threads scanning filesystem fonts (`/usr/share/fonts`). Font discovery is offloaded to a background thread while `RenderContext::font_system` initially holds an empty database.
  - Added explicit contract and [`RenderContext::ensure_system_fonts()`] method to synchronize system fonts lazily before direct access. High-level text shaping and rendering in `TextRenderer` call this automatically.
  - Dynamic system locale detection via `sys-locale` cached in a process-wide `OnceLock<String>` replaces hardcoded fallback locales across font system initializations.

## [0.1.0] - 2026-09-29

### Added
- **Modular 9-Crate Workspace Architecture**:
  - `wyrd-state`: Cross-process atomic state file persistence (`theme.toml`, `wallpaper.toml`).
  - `wyrd-graphics`: SceneGraph, RenderContext, `tiny-skia` software rasterizer, `cosmic-text` Unicode text shaping, `EventBus`, `Animator`, and non-blocking `ImageFetcher`.
  - `wyrd-script`: Generic Lua 5.4 runtime (`LuaRuntime<A>`) with `LuaHostExtension` trait and hot-reload file watcher.
  - `wyrd-widgets`: Declarative `WidgetTree`, Flexbox/Grid/Absolute layout engine, data binding, style inheritance, and `render_bridge`.
  - `wyrd-config`: Strongly-typed `ShellConfig`, `SurfaceConfig`, `SurfaceKind`, `Layer`, `KeyboardInteractivity`, and Lua widget/surface/style parsers.
  - `wyrd-wayland`: Wayland `wlr-layer-shell` surface lifecycle, `ShmBufferPool`, fractional scale/viewporter, and seat/pointer/keyboard input.
  - `wyrd-compositor`: Hyprland, Sway, Niri, and fallback `CompositorIntegration` backends.
  - `wyrd-wasm`: Sandboxed Wasmtime guest host, capability-based permissions (`manifest.toml`), and `ModuleSupervisor`.
  - `wyrd-engine`: Unified facade crate re-exporting the entire engine stack with optional feature flags (`config`, `lua`, `wasm`, `gpu-render`, `http-fetch`).
