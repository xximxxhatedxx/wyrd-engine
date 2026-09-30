# wyrd-graphics

2D scene graph (`SceneGraph`), `RenderContext`, software rasterization (`tiny-skia` + `cosmic-text`), damage tracking (`DamageTracker`), spring-physics animations (`Animator`), `EventBus`, and non-blocking `ImageFetcher` for the Wyrd ecosystem.

> **Note:** The optional `gpu-render` feature is currently experimental `wgpu` scaffolding; actual scene rasterization runs on the CPU via `tiny-skia`.

## Font Loading Contract

`RenderContext::new(scale)` and `RenderContext::with_mode(scale, mode)` return immediately without blocking the caller on filesystem font discovery (`/usr/share/fonts`). As a result, `RenderContext::font_system` initially holds an empty font database while fonts are scanned on a background thread:

- **High-level text shaping**: Functions in `render::text` (`TextRenderer::measure_constrained`, `TextRenderer::render_aligned`) call `ctx.ensure_system_fonts()` automatically before layout or rasterization.
- **Direct database access**: If code queries `ctx.font_system` directly (bypassing `render::text`), it must invoke `ctx.ensure_system_fonts()` first; otherwise queries will see zero font faces without error.

Part of the [`wyrd-engine`](https://github.com/xximxxhatedxx/wyrd-engine) workspace.

## License

Dual-licensed under [MIT](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-APACHE).
