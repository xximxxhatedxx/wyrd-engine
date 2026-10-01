# wyrd-engine

Unified facade crate re-exporting [`wyrd-state`](https://crates.io/crates/wyrd-state), [`wyrd-graphics`](https://crates.io/crates/wyrd-graphics), [`wyrd-script`](https://crates.io/crates/wyrd-script), [`wyrd-compositor`](https://crates.io/crates/wyrd-compositor), [`wyrd-widgets`](https://crates.io/crates/wyrd-widgets), [`wyrd-wayland`](https://crates.io/crates/wyrd-wayland), [`wyrd-wasm`](https://crates.io/crates/wyrd-wasm), and [`wyrd-config`](https://crates.io/crates/wyrd-config) for the Wyrd desktop ecosystem.

> **Note:** The `gpu-render` feature is currently experimental `wgpu` scaffolding; actual scene rasterization runs on the CPU via `tiny-skia`.

## Usage

```toml
[dependencies]
wyrd-engine = "0.1.1"
```

## System Dependencies

- **Arch Linux**: `sudo pacman -S --needed wayland libxkbcommon pkgconf`
- **Debian / Ubuntu**: `sudo apt install libwayland-dev libxkbcommon-dev pkg-config`
- **Fedora**: `sudo dnf install wayland-devel libxkbcommon-devel pkgconf-pkg-config`

Repository: [`https://github.com/xximxxhatedxx/wyrd-engine`](https://github.com/xximxxhatedxx/wyrd-engine)

## License

Dual-licensed under [MIT](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-APACHE).
