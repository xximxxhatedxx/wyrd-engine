# wyrd-settings

Strongly-typed shell configuration (`ShellConfig`, `SurfaceConfig`, `SurfaceKind`, `Layer`, `KeyboardInteractivity`), `TypedShellAdapter` for `wyrd-script`, and Lua widget/surface/layout/style parsers for the Wyrd ecosystem.

## Surface Backend Configuration

`SurfaceConfig` supports an optional `backend` field to specify or override the Wayland surface backend (`"auto"`, `"zwlr-layer-shell"`, or `"xdg-shell"`):

```lua
wyrd.create {
    type = "bar",
    name = "main_bar",
    backend = "xdg_shell", -- "auto" (default) | "zwlr-layer-shell" | "xdg-shell"
    height = 32,
    widgets = { ... },
}
```

The `wyrd-shell` runtime forwards this field to surface creation. When omitted or set to `"auto"`, the runtime selects `zwlr-layer-shell-v1` when available and otherwise uses `xdg-shell`. `ext-layer-shell-v1` is NOT AVAILABLE in 0.1.0; an explicit per-surface request logs a warning and keeps the automatically selected backend.

Part of the [`wyrd-engine`](https://github.com/xximxxhatedxx/wyrd-engine) workspace.

## License

Dual-licensed under [MIT](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-APACHE).
