# wyrd-engine

> **Status (`v0.1.0`)**: Early public release. Internal Rust APIs and state file schemas may evolve before `1.0`.

Core rendering and runtime workspace for the Wyrd desktop ecosystem. Provides Wayland surface management (`zwlr-layer-shell-v1` with an `xdg-shell` fallback; `ext-layer-shell-v1` is not available in 0.1.0), Flexbox/Grid widget layout, Lua 5.4 scripting, sandboxed WebAssembly module execution, spring-physics animations, and 2D rendering (`tiny-skia` + `cosmic-text`) with opt-in GPU rendering via `gpu-render`.

This repository is a Cargo workspace providing both the unified [`wyrd-engine`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-engine) facade crate and granular subcrates used by [`wyrd-shell`](https://github.com/xximxxhatedxx/wyrd-shell), [`wyrd-greet`](https://github.com/xximxxhatedxx/wyrd-greet), and [`wyrd-wallpaper`](https://github.com/xximxxhatedxx/wyrd-wallpaper).

---

## Workspace Crates

| Crate | Internal Dependencies | Description |
| :--- | :--- | :--- |
| [`wyrd-state`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-state) | *(none)* | Cross-process atomic state file persistence (`theme.toml`, `wallpaper.toml`) |
| [`wyrd-sync`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-sync) | *(none)* | Poison-resilient `Mutex`/`RwLock` helpers |
| [`wyrd-graphics`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-graphics) | `wyrd-sync` | `SceneGraph`, `RenderContext`, `tiny-skia` + `cosmic-text` renderer, `Animator`, `EventBus`, `ImageFetcher` |
| [`wyrd-script`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-script) | `wyrd-sync` | Generic Lua 5.4 runtime (`LuaRuntime<A>`), `LuaHostExtension` trait, and hot-reload file watcher |
| [`wyrd-compositor`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-compositor) | `wyrd-sync` | `CompositorIntegration` trait & Hyprland, Sway, Niri, and fallback backends |
| [`wyrd-widgets`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-widgets) | `wyrd-graphics` | Declarative `WidgetTree`, Flexbox/Grid/Absolute layout engine, reactive data binding, `render_bridge` |
| [`wyrd-wayland`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-wayland) | `wyrd-graphics`, `wyrd-state`, `wyrd-sync` | Wayland surface backends (layer-shell + xdg-shell fallback), `ShmBufferPool`, seat input (`wl_pointer`, `wl_keyboard`), `SurfaceManager` |
| [`wyrd-wasm`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-wasm) | `wyrd-graphics`, `wyrd-state` | Sandboxed Wasmtime module host, capability checks (`manifest.toml`), and `ModuleSupervisor` |
| [`wyrd-settings`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-settings) | `wyrd-script`, `wyrd-widgets`, `wyrd-graphics`, `wyrd-sync` | Typed `ShellConfig`, `SurfaceConfig`, `SurfaceKind`, `Layer`, `KeyboardInteractivity`, and Lua parsers |
| [`wyrd-engine`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-engine) | All subcrates (`wyrd-settings` and `wyrd-wasm` optional; settings are enabled by the `config` feature) | Unified facade crate |

---

## Usage in `Cargo.toml`

### From crates.io or Git

```toml
# Full facade crate (for wyrd-shell):
[dependencies]
wyrd-engine = "0.1.0"
# or via Git:
# wyrd-engine = { git = "https://github.com/xximxxhatedxx/wyrd-engine.git", tag = "v0.1.0" }

# Facade for a renderer/settings consumer (e.g. wallpaper or greeter), without the
# shell's Wayland runtime, compositor integrations, or WASM host:
[dependencies]
wyrd-engine = { version = "0.1.0", default-features = false, features = ["config", "lua"] }
```

`wyrd-engine` defaults include `wayland` and `compositor` to preserve the shell
facade. Consumers that disable defaults can enable those independently; `compositor`
also enables `wayland`.

## Wayland surface backends

Two surface backends are currently supported:

1. `zwlr-layer-shell-v1` for wlroots compositors such as Hyprland, Sway, Niri, River, Labwc, and Wayfire.
2. `xdg-shell` as the universal fallback for GNOME/Mutter, KDE/KWin, Weston, and compositors without layer-shell.

`ext-layer-shell-v1` is NOT AVAILABLE in 0.1.0. Only `zwlr-layer-shell-v1` and `xdg-shell` are available. For example, a Lua surface can explicitly use the `xdg-shell` backend:

```lua
wyrd.create {
    type = "bar",
    name = "main_bar",
    backend = "xdg_shell",
    height = 32,
}
```

The `wyrd-shell` configuration path passes this value through to surface creation. Setting `WYRD_FORCE_BACKEND=ext-layer-shell` returns an error because this backend is not available in 0.1.0.
## Security and known limitations

- WASM HTTP(S) image access requires the module's `network:fetch:<host>` capability (or the supported wildcard grant); renderer widget updates do not fetch network URLs.
- Filesystem capability checks have a TOCTOU window between canonical-path validation and the later operation; `openat2(RESOLVE_BENEATH)` is a future hardening option.
- Seat capability changes handle pointer and keyboard hot-plug, but live hot-plug is not covered by automated tests and should be tested on the target compositor.
- Shared screencopy and session-lock helpers are not part of 0.1.0. Applications currently own those protocol clients; shared support is a possible 0.2.0 addition.


---

## System Dependencies

- **Arch Linux**: `sudo pacman -S --needed rust cargo wayland libxkbcommon pkgconf`
- **Debian / Ubuntu**: `sudo apt install cargo libwayland-dev libxkbcommon-dev pkg-config`
- **Fedora**: `sudo dnf install rust cargo wayland-devel libxkbcommon-devel pkgconf-pkg-config`

## Building & Testing

Core workspace: Rust 1.90+. `wyrd-wasm`: Rust 1.96+ (Wasmtime 49.x declares `rust-version = 1.96`).

```bash
cargo build --release --locked
cargo test --locked --all-targets --all-features
```

### Wayland integration tests

The real compositor integration suite requires a Wayland compositor and a writable `XDG_RUNTIME_DIR`. It runs only in an isolated environment and does not use the user's live Wayland session.

Local Podman execution:

```bash
set -euo pipefail
mkdir -p ci/logs
podman build -f ci/wayland-test.Dockerfile -t localhost/wyrd-wayland-test:local .
podman run -d --name wyrd-wayland-test \
  -v "$PWD":/work:Z \
  -v wyrd-target-cache:/work/target \
  -e WLR_BACKENDS=headless \
  -e WLR_RENDERER=pixman \
  -e WLR_LIBINPUT_NO_DEVICES=1 \
  -e XDG_RUNTIME_DIR=/tmp/wyrd-test-runtime \
  localhost/wyrd-wayland-test:local \
  bash -lc 'mkdir -p "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR" && exec sway -c /dev/null --unsupported-gpu >/tmp/sway.log 2>&1'

cleanup() {
  podman cp wyrd-wayland-test:/tmp/sway.log ci/logs/sway.log 2>/dev/null || true
  podman logs wyrd-wayland-test >ci/logs/podman-container.log 2>&1 || true
  podman rm -f wyrd-wayland-test >/dev/null 2>&1 || true
}
trap cleanup EXIT
podman exec wyrd-wayland-test bash -lc '
  set -euo pipefail
  for i in $(seq 1 60); do
    SOCK=$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -type s -name "wayland-*" -print -quit)
    if [ -n "$SOCK" ] && [ -S "$SOCK" ]; then
      export WAYLAND_DISPLAY=$(basename "$SOCK")
      break
    fi
    sleep 0.5
  done
  test -n "${WAYLAND_DISPLAY:-}"
  test -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"
  echo "WAYLAND_DISPLAY=$WAYLAND_DISPLAY"
  cargo test -p wyrd-wayland --test real_compositor_smoke -- --nocapture
  cargo test -p wyrd-wayland --test layer_shell_lifecycle -- --nocapture
' 2>&1 | tee ci/logs/wayland-integration.log
```

The container image is defined in [`ci/wayland-test.Dockerfile`](ci/wayland-test.Dockerfile). This uses the same isolated headless-Sway model as CI. Keyboard keymap regression tests run in the regular workspace suite. The example leaves both the integration test output and Sway log under `ci/logs/`.

### GPU rendering checks

GPU rendering (`gpu-render`) is opt-in and experimental in 0.1.0. It accelerates 2D scene rendering via `wgpu` and falls back to CPU (`tiny-skia`) automatically on initialization or swapchain failure, or when `WYRD_FORCE_CPU=1` is set. Text shaping, image decoding, SVG rasterization, and path flattening remain CPU work. See [`wyrd-graphics`](crates/wyrd-graphics/README.md) for coverage details.

GPU rendering covers: solid rectangles with rounded corners; linear/radial gradients with all stops; solid/dashed/dotted outlines; SDF shadows, Gaussian-falloff drop shadows and inner glows (unified analytic SDF matching between CPU and GPU), progress arcs, and multiply tint; glyph-atlas text and pooled image/SVG textures; custom-path strokes; hierarchical matrices; transform-aware rectangular and rounded `clip_children` (via SDF distance tests in fragment shader); damage scissoring into a persistent target; and bounds-limited separable Gaussian blur passes.

Still on CPU: text shaping/layout on misses in a 256-entry layout cache, image decoding, SVG rasterization on cache misses, and custom-path curve flattening for each collected scene. Swash glyph rasterization is cached in a lazily allocated 2048×2048 RGBA atlas with a bounded LRU glyph index; atlas exhaustion clears the atlas before reuse. Image/SVG assets use a bounded texture pool keyed by path and requested dimensions (plus image radius); curves are flattened to line segments on the CPU and evaluated as stroke distance in the GPU fragment shader. Image sampling is linear for all assets; the scene API has no pixel-art/nearest-sampler hint. Blur is a bounds-limited two-pass post-process matching the CPU renderer's effect behavior. Drop-shadow and inner-glow use analytic SDF falloffs matching between CPU and GPU, not a separable render-to-texture blur. Child clipping follows each transformed parent's rectangular and rounded geometry in the fragment shader. Renderer or asset errors automatically route the affected frame through `tiny-skia`. GPU rendering is supported via `render_surface_present` with automatic CPU fallback when GPU initialization or swapchain presentation fails, or when `WYRD_FORCE_CPU=1` is set.

The scheduled/manual `gpu-nightly` CI job runs offscreen pixel tests for gradients, transforms, clipping, paths, text, image/SVG textures, damage, a mixed 1000-node benchmark, idle CPU, and Wayland `wgpu` Vulkan-present. The Wayland test uses Sway with `WLR_RENDERER=pixman` and llvmpipe for the `wgpu` Vulkan adapter. It does not test Sway's own Vulkan renderer: in the checked Podman image Sway exits with `Could not match drm and vulkan device` because the container has no matching DRM/Vulkan device. The ordinary Wayland job remains pixman-only.

---

## Support

If you find the project useful, you can support it here.

[![ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/xximxxhatedxx)

---

## License

Dual-licensed under [MIT](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-APACHE).
