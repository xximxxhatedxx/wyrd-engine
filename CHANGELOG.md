# Changelog

All notable changes to `wyrd-engine` will be documented in this file.

## [0.1.0] - 2026-10-10

This release addresses security findings A1–A8 and correctness findings B1,
B2, B4, B5, and B9. B3 is partially addressed: device capability transitions
are handled, but live hot-plug coverage remains manual. The available audit
source did not describe B6–B8; no fixes for those identifiers are claimed.

### MSRV
- `wyrd-wasm` now requires Rust 1.96 due to Wasmtime 49.x; the rest of the workspace remains on 1.90.

### Known Limitations
- `ext-layer-shell-v1` is not available in 0.1.0; only `zwlr-layer-shell-v1` and `xdg-shell` are supported.
- GPU rendering is opt-in via `gpu-render` and experimental; CPU rendering remains available as a fallback.
- `fs:write` is rejected at runtime in 0.1.0; the host call returns `-403`.
- `network:fetch` gates `host_image_cached`; the renderer never fetches HTTP(S) URLs from widget updates. Module authors must use `host_image_cached` for network images.
- Filesystem capability checks have a TOCTOU window between canonical-path validation and the later operation; `openat2(RESOLVE_BENEATH)` is planned.
- Live seat hot-plug is not covered by automated tests and should be tested manually before release.

### Added
- **Modular 10-crate workspace**:
  - `wyrd-state`: Cross-process atomic state file persistence (`theme.toml`, `wallpaper.toml`).
  - `wyrd-sync`: Poison-resilient `Mutex`/`RwLock` synchronization helpers.
  - `wyrd-graphics`: SceneGraph, `tiny-skia` software rasterization, `cosmic-text` text shaping, `EventBus`, `Animator`, and `ImageFetcher`.
  - `wyrd-script`: Generic Lua 5.4 runtime (`LuaRuntime<A>`) and hot-reload file watcher.
  - `wyrd-widgets`: Declarative widget tree, layout engine, data binding, and `render_bridge`.
  - `wyrd-settings`: Typed shell/surface configuration and Lua parsers.
  - `wyrd-wayland`: Layer-shell surface lifecycle, `ShmBufferPool`, fractional scaling, and input handling.
  - `wyrd-compositor`: Hyprland, Sway, Niri, and fallback integrations.
  - `wyrd-wasm`: Sandboxed Wasmtime host and capability-based module permissions.
  - `wyrd-engine`: Unified facade with optional feature flags.
- **Wayland integration coverage**:
  - Added `real_compositor_smoke` for compositor connection and `wl_shm` buffer validation.
  - Added `layer_shell_lifecycle` for bounded layer-shell configure, buffer attach, commit, and destruction.
  - Added keyboard keymap regression coverage and capability-aware pointer/keyboard proxies.
  - Added the `wayland-integration` CI job using isolated headless Sway.
- **Universal Wayland surface backends**:
  - Added automatic backend selection: `zwlr-layer-shell-v1` -> `xdg-shell` fallback.
  - Added `SurfaceBackendKind`, `SurfaceCapabilities`, `WYRD_FORCE_BACKEND`, and protocol diagnostics.
  - Added `CompositorIntegration::backend_hint()` and `BackendHint::Wlroots`.
- **GPU rendering pipeline**:
  - Added optional `wgpu` rendering for solid rectangles, multi-stop gradients, outlines, SDF shadows, Gaussian-falloff drop shadows and inner glows (unified analytic SDF matching between CPU and GPU), progress arcs, multiply tint, cached text/image/SVG texture draws, custom-path strokes, hierarchical transforms, transformed rectangular and rounded `clip_children`, damage scissoring, and bounds-limited separable Gaussian blur passes.
  - Text uses a bounded swash glyph atlas; shaping/layout, asset decoding/rasterization, and path-curve flattening remain CPU work.
  - Added an opt-in experimental GPU rendering path via `render_surface_present` with offscreen and Wayland targets, CPU fallback on GPU failure or `WYRD_FORCE_CPU=1`, and parity testing across CPU and GPU paths.
- **Surface backend configuration**:
  - Added optional `backend` configuration to `SurfaceConfig` and the Lua `wyrd.create` parser.

### Changed
- Scoped Tokio features in `wyrd-graphics` and `wyrd-script` to avoid unwanted executor overhead in downstream consumers.
- Made system font discovery asynchronous and documented the `RenderContext::ensure_system_fonts()` contract.
- Added dynamic system locale detection for font-system initialization.
- Marked extensible public enums `#[non_exhaustive]`; `CoreMessage` and `ModuleMessage` remain exhaustive for `wyrd-shell` compatibility.
- Added a CI crate-DAG boundary check that excludes the `cargo tree` root package before matching dependencies (D3).

### Fixed
- Corrected the `wyrd-engine` no-`config` facade path.
- Validated Wayland shared-memory dimensions, stride, size, and `memfd` creation before mapping.
- Made keyboard keymap loading return explicit errors for unsupported formats and invalid keymaps.
- Allowed non-wlroots compositors to initialize through the `xdg-shell` fallback when `zwlr-layer-shell-v1` is absent.
- Child reaping now dispatches a separate wait thread per child so a long-lived child cannot block later reaps.
- Input events now wait for a poison-resilient synchronous `std::sync::RwLock`; lock contention no longer silently drops events.
- Seat capability changes dynamically create and release `WlPointer`/`WlKeyboard` proxies; cursor-shape devices follow pointer hot-plug.
- `memfd_create` uses `MFD_CLOEXEC`.
- Guest input event coalescing was removed; events are delivered to guest modules in order.
- Re-spawning a module with the same name stops the previous task instead of orphaning it.
- Prevented stale scene-cache reuse when widgets are removed; removals force a full redraw.
- `WallpaperState::default()` now matches its Serde defaults, and atomic state temp files use unique per-write names.
- Saturated interval/keycode arithmetic and simplified numeric binding formatting, including negative zero.

### Security
- Removed `.cwasm` disk-cache deserialization; modules compile from their `.wasm` source.
- `process:spawn` checks a plain binary name and resolves only through `$PATH`; guest-supplied absolute and relative paths are rejected.
- Manifest lookup no longer searches CWD-relative directories, and `entry` must be a plain file name.
- Added `network:fetch:<host>` and wildcard capabilities; `host_image_cached` requires authorization for HTTP(S) URLs. Renderer widget updates cannot initiate network fetches.
- Filesystem read, write, and watch path checks canonicalize paths and reject symlink escapes; empty and relative XDG application-data directories are ignored.
- `host_write_file` always returns `-403` in 0.1.0.
- Image cache files use `$XDG_CACHE_HOME/wyrd/images` (mode `0700`) with exclusive temporary files and symlink rejection.
- Relative Unix socket requests now require an absolute `XDG_RUNTIME_DIR`; the `/tmp` fallback is removed.
- Added Wasmtime epoch interruption, a 64 MiB store memory limit, a 1 MiB stack limit, bounded file/directory/socket reads and process output, a 5-second process timeout, and a 2-second Unix-socket connect timeout.
- Guest traps emit `ModuleMessage::Down` and return an error instead of being reported as clean shutdown.
