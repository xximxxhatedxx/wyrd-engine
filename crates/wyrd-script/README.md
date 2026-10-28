# wyrd-script

Generic Lua 5.4 runtime (`LuaRuntime<A>`), `LuaRuntimeAdapter`, `LuaHostExtension` trait, and hot-reload configuration watcher for the Wyrd ecosystem.

Depends only on `wyrd-sync` (a zero-dependency micro-crate for poison-resilient locks), so lightweight consumers such as `wyrd-wallpaper` can still embed the Lua runtime without pulling in widget layout or Wayland surface layers.

Part of the [`wyrd-engine`](https://github.com/xximxxhatedxx/wyrd-engine) workspace.

## License

Dual-licensed under [MIT](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-APACHE).
