# wyrd-engine

Unified facade crate re-exporting [`wyrd-state`](https://crates.io/crates/wyrd-state), [`wyrd-graphics`](https://crates.io/crates/wyrd-graphics), [`wyrd-script`](https://crates.io/crates/wyrd-script), [`wyrd-compositor`](https://crates.io/crates/wyrd-compositor), [`wyrd-widgets`](https://crates.io/crates/wyrd-widgets), [`wyrd-wayland`](https://crates.io/crates/wyrd-wayland), [`wyrd-wasm`](https://crates.io/crates/wyrd-wasm), and [`wyrd-settings`](https://crates.io/crates/wyrd-settings) for the Wyrd desktop ecosystem.

The default feature set includes configuration, Lua, HTTP fetching, the Wayland runtime, and compositor integrations. The `full` feature additionally enables the optional `wasm` feature, which pulls in `wyrd-wasm` and Wasmtime 49.x and therefore requires Rust 1.96+; `wasm` is intentionally excluded from the Rust 1.90 default build. The `wayland` and `compositor` features can be omitted by renderer/configuration consumers that provide their own Wayland client; `compositor` also enables `wayland`.

> **GPU status:** GPU rendering (`gpu-render`) is opt-in and experimental in 0.1.0. It accelerates 2D scene rendering via `wgpu` and falls back to CPU (`tiny-skia`) automatically on initialization or swapchain failure, or when `WYRD_FORCE_CPU=1` is set. Text shaping, image decoding, SVG rasterization, and path flattening remain CPU work. See [`wyrd-graphics`](../wyrd-graphics/README.md) for coverage details.
 
## Compositor Compatibility
 
`wyrd-engine` uses `zwlr-layer-shell-v1` when available and otherwise falls back to `xdg-shell`, allowing operation across wlroots and standard Wayland compositors. These are the two available backends in 0.1.0. `ext-layer-shell-v1` is NOT AVAILABLE in 0.1.0:
 
| Compositor | Protocol path | Status |
|---|---|---|
| Hyprland | zwlr-layer-shell-v1 | native |
| Sway | zwlr-layer-shell-v1 | native |
| Niri | zwlr-layer-shell-v1 | native |
| River | zwlr-layer-shell-v1 | native |
| Labwc | zwlr-layer-shell-v1 | native |
| Wayfire | zwlr-layer-shell-v1 | native |
| KDE Plasma (KWin) | xdg-shell | fallback |
| GNOME (Mutter) | xdg-shell | fallback |
| Weston | xdg-shell | fallback |
 
## Security and known limitations

- WASM HTTP(S) image access requires the module's `network:fetch:<host>` capability (or the supported wildcard grant); renderer widget updates do not fetch network URLs.
- Filesystem capability checks have a TOCTOU window between canonical-path validation and the later operation; `openat2(RESOLVE_BENEATH)` is a future hardening option.
- Seat capability changes handle pointer and keyboard hot-plug, but live hot-plug is not covered by automated tests and should be tested on the target compositor.
- Shared screencopy and session-lock helpers are not part of 0.1.0. Applications currently own those protocol clients; shared support is a possible 0.2.0 addition.

## Troubleshooting
 
- **Check Compositor Protocols**: Run `wayland-info` (from `wayland-utils`) to inspect globals advertised by your compositor.
- **Inspect Selected Backend**: Look for `Selected surface backend: <kind>` in logs, or call `WaylandBackend::print_protocol_support()`.
- **GNOME / KDE Fallback Behavior**: On compositors without layer-shell support (GNOME/Mutter, KDE Plasma/KWin), Wyrd automatically falls back to `xdg-shell`. Exclusive zones and custom layer ordering are layer-shell specific and degrade gracefully under `xdg-shell`.
- **Force a Specific Backend**: Set `WYRD_FORCE_BACKEND=xdg-shell`, `zwlr-layer-shell`, or `auto`. `WYRD_FORCE_BACKEND=ext-layer-shell` returns an error because the backend is not available in 0.1.0.

## GPU Integration in wyrd-shell

`render_surface_present` returns either a GPU frame `PresentedSurface::Gpu(frame)` or a CPU pixel buffer `PresentedSurface::Cpu { pixmap, damage, .. }`. As of 0.1.0, the `wyrd-shell` production render loop does not use GPU presentation by default; the API is provided for consumers that opt in.

Example surface loop integration:

```rust
use wyrd_engine::widgets::{render_surface_present, PresentedSurface};

let presented = render_surface_present(
    surface_id,
    width,
    height,
    scale,
    force_full_redraw,
    &widget_tree,
    &mut render_context,
    &mut damage_tracker,
    None,
    None,
    opacity,
    gpu_target.as_mut(),
)?;

match presented {
    PresentedSurface::Gpu(frame) => {
        surface.submit_frame_gpu(&qh, &queue, *frame, &damage_tracker.regions)?;
    }
    PresentedSurface::Cpu { pixmap, damage, .. } => {
        surface.submit_frame(&shm, &qh, idx, &pixmap, &damage.regions, &mut released)?;
    }
}
```

To force CPU software rendering (`wl_shm`) and bypass GPU acceleration, set the environment variable:

```bash
WYRD_FORCE_CPU=1
```

## Usage

```toml
[dependencies]
wyrd-engine = "0.1.0"
```

For a renderer/configuration consumer that implements its own Wayland client,
omit the default Wayland/compositor runtime while retaining the typed config,
Lua runtime, widgets, renderer, and state APIs:

```toml
[dependencies]
wyrd-engine = { version = "0.1.0", default-features = false, features = ["config", "lua"] }
```

## System Dependencies

- **Arch Linux**: `sudo pacman -S --needed wayland libxkbcommon pkgconf`
- **Debian / Ubuntu**: `sudo apt install libwayland-dev libxkbcommon-dev pkg-config`
- **Fedora**: `sudo dnf install wayland-devel libxkbcommon-devel pkgconf-pkg-config`

Repository: [`https://github.com/xximxxhatedxx/wyrd-engine`](https://github.com/xximxxhatedxx/wyrd-engine)

## License

Dual-licensed under [MIT](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-APACHE).
