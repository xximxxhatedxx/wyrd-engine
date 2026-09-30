# wyrd-engine

> **Status (`v0.1.0`)**: Early public release. Internal Rust APIs and state file schemas may evolve before `1.0`.

Core rendering and runtime workspace for the Wyrd desktop ecosystem. Provides Wayland `wlr-layer-shell` surface management, Flexbox/Grid widget layout, Lua 5.4 scripting, sandboxed WebAssembly module execution, spring-physics animations, and 2D software rasterization (`tiny-skia` + `cosmic-text`, with experimental `wgpu` scaffolding behind the `gpu-render` feature).

This repository is a Cargo workspace publishing both the unified [`wyrd-engine`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-engine) facade crate and granular subcrates used by [`wyrd-shell`](https://github.com/xximxxhatedxx/wyrd-shell), [`wyrd-greet`](https://github.com/xximxxhatedxx/wyrd-greet), and [`wyrd-wallpaper`](https://github.com/xximxxhatedxx/wyrd-wallpaper).

---

## Workspace Crates

| Crate | Internal Dependencies | Description |
| :--- | :--- | :--- |
| [`wyrd-state`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-state) | *(none)* | Cross-process atomic state file persistence (`theme.toml`, `wallpaper.toml`) |
| [`wyrd-graphics`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-graphics) | *(none)* | `SceneGraph`, `RenderContext`, `tiny-skia` + `cosmic-text` renderer, `Animator`, `EventBus`, `ImageFetcher` |
| [`wyrd-script`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-script) | *(none)* | Generic Lua 5.4 runtime (`LuaRuntime<A>`), `LuaHostExtension` trait, and hot-reload file watcher |
| [`wyrd-compositor`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-compositor) | *(none)* | `CompositorIntegration` trait & Hyprland, Sway, Niri, and fallback backends |
| [`wyrd-widgets`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-widgets) | `wyrd-graphics` | Declarative `WidgetTree`, Flexbox/Grid/Absolute layout engine, reactive data binding, `render_bridge` |
| [`wyrd-wayland`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-wayland) | `wyrd-graphics`, `wyrd-state` | Wayland `wlr-layer-shell` surfaces, `ShmBufferPool`, seat input (`wl_pointer`, `wl_keyboard`), `SurfaceManager` |
| [`wyrd-wasm`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-wasm) | `wyrd-graphics`, `wyrd-state` | Sandboxed Wasmtime module host, capability checks (`manifest.toml`), and `ModuleSupervisor` |
| [`wyrd-config`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-config) | `wyrd-script`, `wyrd-widgets`, `wyrd-graphics` | Typed `ShellConfig`, `SurfaceConfig`, `SurfaceKind`, `Layer`, `KeyboardInteractivity`, and Lua parsers |
| [`wyrd-engine`](https://github.com/xximxxhatedxx/wyrd-engine/tree/main/crates/wyrd-engine) | All subcrates (`config` & `wasm` optional) | Unified facade crate |

> **Note on `gpu-render` feature:** The `gpu-render` Cargo feature is currently experimental scaffolding (`wgpu` adapter/device setup); actual scene rasterization still runs on the CPU via `tiny-skia`.

---

## Usage in `Cargo.toml`

### From crates.io or Git

```toml
# Full facade crate (for wyrd-shell):
[dependencies]
wyrd-engine = "0.1.0"
# or via Git:
# wyrd-engine = { git = "https://github.com/xximxxhatedxx/wyrd-engine.git", tag = "v0.1.0" }

# Lightweight subcrates (e.g., for wallpaper/lock daemons — without wasm or compositor):
[dependencies]
wyrd-graphics = { version = "0.1.0", default-features = false }
wyrd-script   = "0.1.0"
wyrd-state    = "0.1.0"
```

---

## System Dependencies

- **Arch Linux**: `sudo pacman -S --needed rust cargo wayland libxkbcommon pkgconf`
- **Debian / Ubuntu**: `sudo apt install cargo libwayland-dev libxkbcommon-dev pkg-config`
- **Fedora**: `sudo dnf install rust cargo wayland-devel libxkbcommon-devel pkgconf-pkg-config`

## Building & Testing

Requires **Rust 1.90+** (`rust-version = "1.90"`).

```bash
cargo build --release --locked
cargo test --locked --all-targets --all-features
```

## Publishing to crates.io

With **Rust 1.90+**, Cargo automatically publishes all workspace crates in topological dependency order:

```bash
cargo publish --workspace
```

---

## Support

If you find the project useful, you can support it here.

[![ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/xximxxhatedxx)

---

## License

Dual-licensed under [MIT](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-APACHE).
