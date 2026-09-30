# wyrd-compositor

Wayland compositor IPC integrations (`Hyprland`, `Sway`, `Niri`, and fallback `NoneIntegration`) implementing `CompositorIntegration` for workspace, active window, and keyboard layout tracking.

Has zero internal `wyrd-*` dependencies.

## System Dependencies

- **Arch Linux**: `sudo pacman -S --needed wayland libxkbcommon pkgconf`
- **Debian / Ubuntu**: `sudo apt install libwayland-dev libxkbcommon-dev pkg-config`
- **Fedora**: `sudo dnf install wayland-devel libxkbcommon-devel pkgconf-pkg-config`

Part of the [`wyrd-engine`](https://github.com/xximxxhatedxx/wyrd-engine) workspace.

## License

Dual-licensed under [MIT](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-APACHE).
