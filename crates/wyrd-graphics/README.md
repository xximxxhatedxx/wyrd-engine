# wyrd-graphics

2D scene graph (`SceneGraph`), `RenderContext`, software rasterization (`tiny-skia` + `cosmic-text`), optional experimental `wgpu` rendering, damage tracking (`DamageTracker`), spring-physics animations (`Animator`), `EventBus`, and non-blocking `ImageFetcher` for the Wyrd ecosystem.

## GPU rendering coverage

GPU rendering covers: solid rectangles with rounded corners; linear/radial gradients with all stops; solid/dashed/dotted outlines; SDF shadows, Gaussian-falloff drop shadows and inner glows (unified analytic SDF matching between CPU and GPU), progress arcs, and multiply tint; glyph-atlas text and pooled image/SVG textures; custom-path strokes; hierarchical matrices; transform-aware rectangular and rounded `clip_children` (via SDF distance evaluation); damaged regions in a persistent target; and bounds-limited separable Gaussian blur passes.

Two `#[ignore]`-annotated GPU smoke tests, `offscreen_target_contains_rendered_solid_rect` and `gpu_rounded_clip_excludes_corner_pixels`, run on every pull request in the `gpu-smoke` CI job with llvmpipe; the job invokes them with `--ignored`. The remaining GPU tests run in `gpu-nightly`.

Still on CPU: text shaping/layout on misses in a 256-entry layout cache, image decoding, SVG rasterization on cache misses, and custom-path curve flattening for each collected scene. Swash glyph rasterization is cached in a lazily allocated 2048×2048 RGBA atlas with a bounded LRU glyph index; atlas exhaustion clears the atlas before reuse. Image/SVG data uses a bounded GPU texture pool. Image sampling is linear for all assets; the scene API has no pixel-art/nearest-sampler hint. Blur is a bounds-limited two-pass post-process matching the CPU renderer's effect behavior; its intermediate texture and per-effect uniform buffers are reused, while bind groups are rebuilt for each blur frame. Drop-shadow and inner-glow use analytic SDF falloffs matching between CPU and GPU rather than a separable render-to-texture blur. Clip regions follow transformed parent geometry in the fragment shader with analytic rounded corner evaluation. On renderer/asset errors the widget bridge automatically falls back to a CPU frame. A Wayland integration test covers GPU surface presentation; it does not show that the regular `wyrd-shell` render loop uses GPU presentation.

## Font Loading Contract

`RenderContext::new(scale)` and `RenderContext::with_mode(scale, mode)` return immediately without blocking the caller on filesystem font discovery (`/usr/share/fonts`). As a result, `RenderContext::font_system` initially holds an empty font database while fonts are scanned on a background thread:

- **High-level text shaping**: Functions in `render::text` (`TextRenderer::measure_constrained`, `TextRenderer::render_aligned`) call `ctx.ensure_system_fonts()` automatically before layout or rasterization.
- **Direct database access**: If code queries `ctx.font_system` directly (bypassing `render::text`), it must invoke `ctx.ensure_system_fonts()` first; otherwise queries will see zero font faces without error.

Part of the [`wyrd-engine`](https://github.com/xximxxhatedxx/wyrd-engine) workspace.

## License

Dual-licensed under [MIT](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/xximxxhatedxx/wyrd-engine/blob/main/LICENSE-APACHE).
