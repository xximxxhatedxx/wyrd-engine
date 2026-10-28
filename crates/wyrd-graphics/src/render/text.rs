use crate::render::context::RenderContext;
use crate::render::scene::{truncate_text_spans, TextSpan};
use cosmic_text::{
    Align, Attrs, Buffer, Color as TextColor, Family, Metrics, Shaping, Style, UnderlineStyle,
    Weight, Wrap,
};
use std::sync::Arc;
use tiny_skia::{Color, PixmapMut};

pub struct TextRenderer;

pub(crate) fn rich_text_attrs<'a>(
    spans: &'a [TextSpan],
    font_family: &'a str,
    base_color: TextColor,
) -> Vec<(&'a str, Attrs<'a>)> {
    spans
        .iter()
        .map(|span| {
            let span_color = span.color.map(to_text_color).unwrap_or(base_color);
            let attrs = Attrs::new()
                .family(Family::Name(font_family))
                .color(span_color)
                .weight(if span.bold {
                    Weight::BOLD
                } else {
                    Weight::NORMAL
                })
                .style(if span.italic {
                    Style::Italic
                } else {
                    Style::Normal
                })
                .underline(if span.underline {
                    UnderlineStyle::Single
                } else {
                    UnderlineStyle::None
                });
            let attrs = if span.strikethrough {
                attrs.strikethrough()
            } else {
                attrs
            };
            (span.text.as_str(), attrs)
        })
        .collect()
}

impl TextRenderer {
    pub fn measure(
        ctx: &mut RenderContext,
        text: &str,
        font_family: &str,
        font_size: f32,
    ) -> (f32, f32) {
        Self::measure_constrained(ctx, text, font_family, font_size, None)
    }

    pub fn measure_constrained(
        ctx: &mut RenderContext,
        text: &str,
        font_family: &str,
        font_size: f32,
        max_width: Option<f32>,
    ) -> (f32, f32) {
        ctx.ensure_system_fonts();
        let size_key = (font_size * 100.0).round() as u32;
        let arc_family: Arc<str> = Arc::from(font_family);
        let key = (arc_family, size_key);

        if !ctx.text_buffers.contains_key(&key) {
            let metrics = Metrics::new(font_size, font_size * 1.25);
            let buf = Buffer::new(&mut ctx.font_system, metrics);
            ctx.text_buffers.insert(key.clone(), buf);
        }

        let buffer = ctx.text_buffers.get_mut(&key).unwrap();
        let mut borrowed = buffer.borrow_with(&mut ctx.font_system);
        if let Some(w) = max_width.filter(|w| *w > 10.0 && *w < 10000.0) {
            borrowed.set_size(Some(w), None);
            borrowed.set_wrap(Wrap::WordOrGlyph);
        } else {
            borrowed.set_size(None, None);
            borrowed.set_wrap(Wrap::None);
        }
        borrowed.set_text(
            text,
            &Attrs::new().family(Family::Name(font_family)),
            Shaping::Advanced,
            Some(Align::Left),
        );
        borrowed.shape_until_scroll(false);
        let line_height = font_size * 1.25;
        let mut width = 0.0_f32;
        let mut line_count = 0usize;
        for run in borrowed.layout_runs() {
            line_count += 1;
            let run_w = run.glyphs.last().map(|g| g.x + g.w).unwrap_or(0.0);
            width = width.max(run_w);
        }
        let total_lines = line_count.max(1);
        let height = total_lines as f32 * line_height;
        let max_w = max_width.unwrap_or(f32::INFINITY);
        (
            width.ceil().max(1.0).min(max_w),
            height.ceil().max(font_size),
        )
    }

    /// Renders left-aligned text into `pixmap` at `(x, y)` within `max_width`.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        ctx: &mut RenderContext,
        pixmap: &mut PixmapMut,
        text: &str,
        font_family: &str,
        font_size: f32,
        color: Color,
        x: f32,
        y: f32,
        max_width: f32,
    ) {
        Self::render_aligned(
            ctx,
            pixmap,
            text,
            font_family,
            font_size,
            color,
            x,
            y,
            max_width,
            font_size * 1.25,
            Align::Left,
        );
    }

    /// Renders aligned text into `pixmap` within the bounding box `(x, y, max_width, max_height)`.
    #[allow(clippy::too_many_arguments)]
    pub fn render_aligned(
        ctx: &mut RenderContext,
        pixmap: &mut PixmapMut,
        text: &str,
        font_family: &str,
        font_size: f32,
        color: Color,
        x: f32,
        y: f32,
        box_width: f32,
        box_height: f32,
        align: Align,
    ) {
        Self::render_aligned_with_spans(
            ctx,
            pixmap,
            text,
            &[],
            font_family,
            font_size,
            color,
            x,
            y,
            box_width,
            box_height,
            align,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_aligned_with_spans(
        ctx: &mut RenderContext,
        pixmap: &mut PixmapMut,
        text: &str,
        spans: &[TextSpan],
        font_family: &str,
        font_size: f32,
        color: Color,
        x: f32,
        y: f32,
        box_width: f32,
        box_height: f32,
        align: Align,
    ) {
        ctx.ensure_system_fonts();
        let size_key = (font_size * 100.0).round() as u32;
        let arc_family: Arc<str> = Arc::from(font_family);
        let key = (arc_family, size_key);

        if !ctx.text_buffers.contains_key(&key) {
            let metrics = Metrics::new(font_size, font_size * 1.25);
            let buf = Buffer::new(&mut ctx.font_system, metrics);
            ctx.text_buffers.insert(key.clone(), buf);
        }

        let line_height = font_size * 1.25;
        let effective_box_w = box_width.max(1.0);
        let effective_box_h = box_height.max(1.0);

        let buffer = ctx.text_buffers.get_mut(&key).unwrap();
        let mut borrowed = buffer.borrow_with(&mut ctx.font_system);

        let is_single_line = effective_box_h < line_height * 1.5;
        borrowed.set_size(Some(effective_box_w), Some(effective_box_h));
        if !is_single_line {
            borrowed.set_wrap(Wrap::WordOrGlyph);
        } else {
            borrowed.set_wrap(Wrap::None);
        }
        let base_color = to_text_color(color);
        let attrs = Attrs::new()
            .family(Family::Name(font_family))
            .color(base_color);
        if spans.is_empty()
            || spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>()
                != text
        {
            borrowed.set_text(text, &attrs, Shaping::Advanced, Some(align));
        } else {
            borrowed.set_rich_text(
                rich_text_attrs(spans, font_family, base_color),
                &attrs,
                Shaping::Advanced,
                Some(align),
            );
        }
        borrowed.shape_until_scroll(false);

        if is_single_line {
            let cut_byte = borrowed.layout_runs().next().and_then(|run| {
                let run_w = run.glyphs.last().map(|g| g.x + g.w).unwrap_or(0.0);
                if run_w > effective_box_w + 1.0 && text.chars().count() > 3 {
                    let target_w = (effective_box_w - font_size * 0.7).max(font_size * 0.8);
                    let mut byte_idx = 0usize;
                    for g in run.glyphs.iter() {
                        if g.x + g.w <= target_w {
                            byte_idx = g.end.min(text.len());
                        } else {
                            break;
                        }
                    }
                    while byte_idx > 0 && !text.is_char_boundary(byte_idx) {
                        byte_idx -= 1;
                    }
                    if byte_idx > 0 && byte_idx < text.len() {
                        Some(byte_idx)
                    } else {
                        None
                    }
                } else {
                    None
                }
            });
            if let Some(cut) = cut_byte {
                let truncated = format!("{}…", text[..cut].trim_end());
                let clipped_spans = truncate_text_spans(spans, &truncated);
                borrowed.set_rich_text(
                    rich_text_attrs(&clipped_spans, font_family, base_color),
                    &attrs,
                    Shaping::Advanced,
                    Some(align),
                );
                borrowed.shape_until_scroll(false);
            }
        }

        let runs_count = borrowed.layout_runs().count().max(1);
        let total_text_h = runs_count as f32 * line_height;
        let y_offset = if total_text_h < effective_box_h {
            ((effective_box_h - total_text_h) / 2.0).max(0.0)
        } else {
            0.0
        };

        let rgba = [color.red(), color.green(), color.blue(), color.alpha()]
            .map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);

        let snap_x = x.round() as i32;
        let snap_y = (y + y_offset).round() as i32;
        let min_x = snap_x.max(0);
        let max_x = (x + effective_box_w).ceil() as i32;
        let min_y = y.round().max(0.0) as i32;
        let max_y = (y + effective_box_h).ceil() as i32;
        let pix_w = pixmap.width() as i32;
        let pix_h = pixmap.height() as i32;
        let pix_slice = pixmap.pixels_mut();

        borrowed.draw(
            &mut ctx.swash_cache,
            TextColor::rgba(rgba[0], rgba[1], rgba[2], rgba[3]),
            |px, py, _width, _height, glyph_color| {
                let glyph_rgba = glyph_color.as_rgba();
                let sa = glyph_rgba[3] as u32;
                if sa == 0 {
                    return;
                }
                let target_x = snap_x + px;
                let target_y = snap_y + py;
                if target_x >= min_x
                    && target_x < max_x
                    && target_x < pix_w
                    && target_y >= min_y
                    && target_y < max_y
                    && target_y < pix_h
                {
                    let idx = (target_y as usize) * (pix_w as usize) + (target_x as usize);
                    let dest_pix = &mut pix_slice[idx];
                    let dr = dest_pix.red() as u32;
                    let dg = dest_pix.green() as u32;
                    let db = dest_pix.blue() as u32;
                    let da = dest_pix.alpha() as u32;

                    let sr = glyph_rgba[0] as u32;
                    let sg = glyph_rgba[1] as u32;
                    let sb = glyph_rgba[2] as u32;

                    let inv_sa = 255 - sa;
                    // Straight source to premultiplied over blend:
                    let out_r = ((sr * sa + dr * inv_sa + 127) / 255).min(255) as u8;
                    let out_g = ((sg * sa + dg * inv_sa + 127) / 255).min(255) as u8;
                    let out_b = ((sb * sa + db * inv_sa + 127) / 255).min(255) as u8;
                    let out_a = ((sa * 255 + da * inv_sa + 127) / 255).min(255) as u8;

                    if let Some(p) =
                        tiny_skia::PremultipliedColorU8::from_rgba(out_r, out_g, out_b, out_a)
                    {
                        *dest_pix = p;
                    }
                }
            },
        );
    }
}

fn to_text_color(color: Color) -> TextColor {
    TextColor::rgba(
        (color.red() * 255.0).round() as u8,
        (color.green() * 255.0).round() as u8,
        (color.blue() * 255.0).round() as u8,
        (color.alpha() * 255.0).round() as u8,
    )
}
