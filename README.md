# wyrd-engine

[![CI](https://github.com/xximxxhatedxx/wyrd-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/xximxxhatedxx/wyrd-engine/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/wyrd-engine.svg)](https://crates.io/crates/wyrd-engine)
[![docs.rs](https://docs.rs/wyrd-engine/badge.svg)](https://docs.rs/wyrd-engine)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)

Core Wayland runtime, 2D graphics rasterizer, Lua configuration engine, and sandboxed WebAssembly host for the Wyrd desktop ecosystem.

`wyrd-engine` provides the low-level foundation used by [`wyrd-shell`](https://github.com/xximxxhatedxx/wyrd-shell), [`wyrd-wallpaper`](https://github.com/xximxxhatedxx/wyrd-wallpaper), and custom Wayland desktop components.

---

## Features

- **2D Graphics & Text Rendering**:
  - High-performance CPU rasterization powered by `tiny-skia` and `cosmic-text`.
  - SVG rendering support via `resvg`.
  - Scene graph with sub-tree caching and fine-grained damage tracking for sub-millisecond redraws.
  - Linear gradients, drop shadows, clipping, and border radiuses.
- **Declarative Widget & Layout Engine**:
  - Flexbox and absolute layout systems.
  - Reactive template expressions and mathematical property bindings.
  - Dynamic spring and cubic-bezier property animations (`Animator`).
  - Hit testing and event propagation for pointer and keyboard input.
- **Wayland Protocol Runtime**:
  - Native `wlr-layer-shell-v1` integration for status bars, floating panels, popups, and full-screen overlays.
  - Multi-output monitor hotplugging and per-output scaling.
  - Wayland seat, pointer, touch, and keyboard handling via `xkbcommon`.
- **Lua Configuration Engine**:
  - Embedded Lua 5.4 runtime powered by `mlua`.
  - Declarative surface definition API (`wyrd.create`, `wyrd.style`).
  - Live configuration hot-reloading with safe rollback on syntax errors.
- **Sandboxed WASM Host**:
  - WebAssembly module execution via `wasmtime`.
  - Capability-based security sandbox (controlled D-Bus, IPC, and filesystem access).
- **Compositor Integration**:
  - Built-in native IPC connectors for Hyprland, Sway, and Niri (workspace tracking, focus events, keybind sync).
  - Protocol-level `wlr-layer-shell-v1` compatibility for River, Wayfire, and all wlroots-compliant compositors.

---

## Installation

Add `wyrd-engine` to your `Cargo.toml`:

```toml
[dependencies]
wyrd-engine = "0.1.0"
```

### Feature Flags

- `gpu-render`: Optional hardware-accelerated rendering pipeline via `wgpu` (experimental).
- `systemd`: Integration with systemd user session lifecycle.

---

## Architecture Overview

```
wyrd_engine
├── render/            # tiny-skia 2D rasterizer, cosmic-text shaping, damage optimization
├── widgets/           # Flexbox layout, widget tree, property bindings, hit testing
├── animator.rs        # Spring physics and easing curve animation engine
├── wayland/           # Wayland client, layer-shell surface management, seat/output tracking
├── surface_manager.rs # Lifecycle and monitor routing for desktop surfaces
├── config/            # Lua 5.4 configuration parser and theme engine
├── modules/           # WASM plugin host (wasmtime) with capability security
├── compositor/        # IPC clients for Hyprland, Sway, and Niri
└── event_bus/         # Asynchronous inter-module event distribution
```

---

## Ecosystem

- [wyrd-shell](https://github.com/xximxxhatedxx/wyrd-shell) — The complete extensible Wayland desktop shell.
- [wyrd-wallpaper](https://github.com/xximxxhatedxx/wyrd-wallpaper) — Dynamic Material You wallpaper daemon.

---

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE) or http://opensource.org/licenses/MIT)

at your option.
