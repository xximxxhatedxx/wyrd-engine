# wyrd-wayland

Wayland surface lifecycle abstraction (`zwlr-layer-shell-v1` and `xdg-shell` fallback), `ShmBufferPool` double-buffered shared memory pools, fractional scale/viewporter support, seat input (`wl_pointer`, `wl_keyboard`), and `SurfaceManager` for the Wyrd ecosystem.

## Surface Backends

`wyrd-wayland` currently supports two surface backends:

1. **`zwlr-layer-shell-v1`**: Native layer-shell protocol supported by wlroots-based compositors (Hyprland, Sway, Niri, River, Labwc, Wayfire).
2. **`xdg-shell` (`xdg_wm_base` + `xdg_toplevel`)**: Universal fallback for GNOME (Mutter), KDE Plasma (KWin), Weston, and any compositor without layer-shell.

`ext-layer-shell-v1` is NOT AVAILABLE in 0.1.0. Automatic detection selects `zwlr-layer-shell-v1` when available and otherwise uses `xdg-shell`.

### Backend Overrides

Set the `WYRD_FORCE_BACKEND` environment variable to override auto-detection:
- `WYRD_FORCE_BACKEND=auto` (default): Prefers `zwlr-layer-shell-v1`, then falls back to `xdg-shell`.
- `WYRD_FORCE_BACKEND=ext-layer-shell`: Returns an error because `ext-layer-shell-v1` is not available in 0.1.0.
- `WYRD_FORCE_BACKEND=zwlr-layer-shell`: Forces wlroots `zwlr-layer-shell-v1`.
- `WYRD_FORCE_BACKEND=xdg-shell`: Forces universal `xdg-shell` fallback.

Per-surface overrides are supported via the `backend` field in surface configuration (e.g. `backend = "xdg_shell"` in Lua). Requesting `ext-layer-shell` per surface logs a warning and keeps the automatically selected backend.

## GPU Wayland surface status

The `gpu-smoke` CI job runs the GPU smoke tests with llvmpipe on every pull request. The `gpu-nightly` job additionally exercises the Wayland Vulkan-present path with Sway using `WLR_RENDERER=pixman`.

## Compositor Compatibility

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

## Diagnostics

Use `WaylandBackend::print_protocol_support()` to log the active compositor protocol capabilities and chosen backend at startup:

```
Wayland protocol support:
  ext-layer-shell-v1   : not compiled
  zwlr-layer-shell-v1  : yes
  xdg-shell            : yes
  fractional-scale-v1  : yes
  viewporter           : yes
  cursor-shape-v1      : yes
  text-input-v3        : yes
  xdg-activation-v1    : yes
Selected surface backend: ZwlrLayerShell
```

## System Dependencies

- **Arch Linux**: `sudo pacman -S --needed wayland libxkbcommon pkgconf`
- **Debian / Ubuntu**: `sudo apt install libwayland-dev libxkbcommon-dev pkg-config`
- **Fedora**: `sudo dnf install wayland-devel libxkbcommon-devel pkgconf-pkg-config`

Part of the [`wyrd-engine`](https://github.com/xximxxhatedxx/wyrd-engine) workspace.

## License

Dual-licensed under [MIT](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-APACHE).
