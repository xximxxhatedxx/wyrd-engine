//! Render Pipeline
//!
//! CPU rendering via tiny-skia, cosmic-text for shaping,
//! damage tracking, double-buffered wl_shm pools.

pub mod backend;
pub mod context;
pub mod damage;
pub mod gpu;
pub mod scene;
pub mod text;

use log::{error, warn};
use tiny_skia::Pixmap;

fn rounded_rect_path(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
) -> Option<tiny_skia::Path> {
    if radius <= 0.0 {
        let rect = tiny_skia::Rect::from_xywh(x, y, width.max(0.0), height.max(0.0))?;
        return Some(tiny_skia::PathBuilder::from_rect(rect));
    }
    let r = radius.min(width.min(height) / 2.0);
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + width - r, y);
    pb.quad_to(x + width, y, x + width, y + r);
    pb.line_to(x + width, y + height - r);
    pb.quad_to(x + width, y + height, x + width - r, y + height);
    pb.line_to(x + r, y + height);
    pb.quad_to(x, y + height, x, y + height - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

fn get_cached_rounded_rect_path(
    ctx: &mut context::RenderContext,
    width: f32,
    height: f32,
    radius: f32,
) -> Option<&tiny_skia::Path> {
    let w_key = (width.round() as u32).max(1);
    let h_key = (height.round() as u32).max(1);
    let r_key = (radius.round() as u32).min(w_key.min(h_key) / 2);
    let key = (w_key, h_key, r_key);

    if !ctx.path_cache.contains_key(&key) {
        if ctx.path_cache.len() >= 128 {
            ctx.path_cache.clear();
        }
        let path = rounded_rect_path(0.0, 0.0, w_key as f32, h_key as f32, r_key as f32)?;
        ctx.path_cache.insert(key, path);
    }
    ctx.path_cache.get(&key)
}

fn box_blur_horizontal(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    if w == 0 || h == 0 || r == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let div = (r + r + 1) as u32;
    for y in 0..h {
        let row_offset = y * w * 4;
        let mut sum_r: u32 = 0;
        let mut sum_g: u32 = 0;
        let mut sum_b: u32 = 0;
        let mut sum_a: u32 = 0;

        let f_r = src[row_offset] as u32;
        let f_g = src[row_offset + 1] as u32;
        let f_b = src[row_offset + 2] as u32;
        let f_a = src[row_offset + 3] as u32;

        for _ in 0..r {
            sum_r += f_r;
            sum_g += f_g;
            sum_b += f_b;
            sum_a += f_a;
        }
        for i in 0..=r {
            let px = i.min(w - 1) * 4;
            sum_r += src[row_offset + px] as u32;
            sum_g += src[row_offset + px + 1] as u32;
            sum_b += src[row_offset + px + 2] as u32;
            sum_a += src[row_offset + px + 3] as u32;
        }

        for x in 0..w {
            let px_out = row_offset + x * 4;
            dst[px_out] = (sum_r / div) as u8;
            dst[px_out + 1] = (sum_g / div) as u8;
            dst[px_out + 2] = (sum_b / div) as u8;
            dst[px_out + 3] = (sum_a / div) as u8;

            let next_x = (x + r + 1).min(w - 1) * 4;
            let prev_x = x.saturating_sub(r) * 4;
            sum_r = sum_r + src[row_offset + next_x] as u32 - src[row_offset + prev_x] as u32;
            sum_g =
                sum_g + src[row_offset + next_x + 1] as u32 - src[row_offset + prev_x + 1] as u32;
            sum_b =
                sum_b + src[row_offset + next_x + 2] as u32 - src[row_offset + prev_x + 2] as u32;
            sum_a =
                sum_a + src[row_offset + next_x + 3] as u32 - src[row_offset + prev_x + 3] as u32;
        }
    }
}

fn box_blur_vertical(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    if w == 0 || h == 0 || r == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let div = (r + r + 1) as u32;
    for x in 0..w {
        let mut sum_r: u32 = 0;
        let mut sum_g: u32 = 0;
        let mut sum_b: u32 = 0;
        let mut sum_a: u32 = 0;

        let f_r = src[x * 4] as u32;
        let f_g = src[x * 4 + 1] as u32;
        let f_b = src[x * 4 + 2] as u32;
        let f_a = src[x * 4 + 3] as u32;

        for _ in 0..r {
            sum_r += f_r;
            sum_g += f_g;
            sum_b += f_b;
            sum_a += f_a;
        }
        for i in 0..=r {
            let py = i.min(h - 1);
            let idx = (py * w + x) * 4;
            sum_r += src[idx] as u32;
            sum_g += src[idx + 1] as u32;
            sum_b += src[idx + 2] as u32;
            sum_a += src[idx + 3] as u32;
        }

        for y in 0..h {
            let px_out = (y * w + x) * 4;
            dst[px_out] = (sum_r / div) as u8;
            dst[px_out + 1] = (sum_g / div) as u8;
            dst[px_out + 2] = (sum_b / div) as u8;
            dst[px_out + 3] = (sum_a / div) as u8;

            let next_y = (y + r + 1).min(h - 1);
            let prev_y = y.saturating_sub(r);
            let next_idx = (next_y * w + x) * 4;
            let prev_idx = (prev_y * w + x) * 4;
            sum_r = sum_r + src[next_idx] as u32 - src[prev_idx] as u32;
            sum_g = sum_g + src[next_idx + 1] as u32 - src[prev_idx + 1] as u32;
            sum_b = sum_b + src[next_idx + 2] as u32 - src[prev_idx + 2] as u32;
            sum_a = sum_a + src[next_idx + 3] as u32 - src[prev_idx + 3] as u32;
        }
    }
}

fn apply_box_blur(pixmap: &mut tiny_skia::Pixmap, radius: usize) {
    let w = pixmap.width() as usize;
    let h = pixmap.height() as usize;
    if w == 0 || h == 0 || radius == 0 {
        return;
    }
    let mut temp = vec![0u8; w * h * 4];
    let r = radius.max(1);

    for _ in 0..2 {
        box_blur_horizontal(pixmap.data(), &mut temp, w, h, r);
        box_blur_vertical(&temp, pixmap.data_mut(), w, h, r);
    }
}

/// Builds a cubic-bezier approximation [`tiny_skia::Path`] for a circular arc.
pub fn circular_arc_path(
    cx: f32,
    cy: f32,
    r: f32,
    start_angle: f32,
    sweep_angle: f32,
) -> Option<tiny_skia::Path> {
    if sweep_angle.abs() < 0.001 || r <= 0.0 {
        return None;
    }
    let mut pb = tiny_skia::PathBuilder::new();
    let sweep = sweep_angle.clamp(
        -2.0 * std::f32::consts::PI + 0.0001,
        2.0 * std::f32::consts::PI - 0.0001,
    );
    let num_segments = ((sweep.abs() / (std::f32::consts::PI / 2.0)).ceil() as usize).max(1);
    let step = sweep / num_segments as f32;

    let start_x = cx + r * start_angle.cos();
    let start_y = cy + r * start_angle.sin();
    pb.move_to(start_x, start_y);

    let mut current_angle = start_angle;
    for _ in 0..num_segments {
        let next_angle = current_angle + step;
        let half_step = step / 2.0;
        let k = 4.0 / 3.0 * (half_step / 2.0).tan();

        let cp1_x = cx + r * (current_angle.cos() - k * current_angle.sin());
        let cp1_y = cy + r * (current_angle.sin() + k * current_angle.cos());

        let cp2_x = cx + r * (next_angle.cos() + k * next_angle.sin());
        let cp2_y = cy + r * (next_angle.sin() - k * next_angle.cos());

        let end_x = cx + r * next_angle.cos();
        let end_y = cy + r * next_angle.sin();

        pb.cubic_to(cp1_x, cp1_y, cp2_x, cp2_y, end_x, end_y);
        current_angle = next_angle;
    }
    pb.finish()
}

#[inline]
fn apply_opacity(color: tiny_skia::Color, opacity: f32) -> tiny_skia::Color {
    let opacity = opacity.clamp(0.0, 1.0);
    if (opacity - 1.0).abs() < 0.001 {
        return color;
    }
    let new_a = (color.alpha() * opacity).clamp(0.0, 1.0);
    if new_a <= 0.0001 {
        return tiny_skia::Color::TRANSPARENT;
    }
    tiny_skia::Color::from_rgba(color.red(), color.green(), color.blue(), new_a).unwrap_or(color)
}

/// Renders a complete [`scene::SceneGraph`] into a [`Pixmap`].
pub fn scene_to_pixmap(
    scene: &scene::SceneGraph,
    pixmap: &mut Pixmap,
    ctx: &mut context::RenderContext,
) {
    render_scene_node(scene, scene.root(), pixmap, ctx);
}

pub use scene_to_pixmap as render_scene_to_pixmap;

fn render_scene_node(
    scene: &scene::SceneGraph,
    node_id: scene::SceneNodeId,
    pixmap: &mut Pixmap,
    ctx: &mut context::RenderContext,
) {
    render_scene_node_offset(scene, node_id, pixmap, ctx, 0.0, 0.0);
}

#[allow(clippy::too_many_arguments)]
fn render_shadow_node(
    ctx: &mut context::RenderContext,
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    opacity: f32,
    radius: f32,
    s_opacity: f32,
    offset_x: f32,
    offset_y: f32,
    color: tiny_skia::Color,
    corner_radius: f32,
) {
    if s_opacity <= 0.0 || radius <= 0.0 || width <= 0.0 || height <= 0.0 {
        return;
    }
    let color_alpha = if color.alpha() > 0.001 {
        color.alpha()
    } else {
        1.0
    };
    let shadow_alpha = (color_alpha * s_opacity.clamp(0.0, 1.0) * opacity).clamp(0.0, 1.0);
    let blur_pad = (radius * 1.5).ceil() as u32 + 4;
    let offscreen_w = width.ceil() as u32 + blur_pad * 2;
    let offscreen_h = height.ceil() as u32 + blur_pad * 2;

    let col_r = (color.red() * 255.0).round() as u8;
    let col_g = (color.green() * 255.0).round() as u8;
    let col_b = (color.blue() * 255.0).round() as u8;
    let col_a = (shadow_alpha * 255.0).round() as u8;
    let col_u32 = u32::from_ne_bytes([col_r, col_g, col_b, col_a]);
    let cache_key = (
        offscreen_w,
        offscreen_h,
        (radius * 10.0).round() as u32,
        (corner_radius * 10.0).round() as u32,
        col_u32,
        (offset_x * 10.0).round() as i32,
        (offset_y * 10.0).round() as i32,
    );

    let shadow_pixmap = if let Some(cached) = ctx.shadow_cache.get(&cache_key) {
        cached.clone()
    } else if let Some(mut new_pixmap) = tiny_skia::Pixmap::new(offscreen_w, offscreen_h) {
        let mut shadow_paint = tiny_skia::Paint::default();
        shadow_paint.set_color(tiny_skia::Color::from_rgba8(col_r, col_g, col_b, col_a));
        if let Some(spath) = rounded_rect_path(
            blur_pad as f32,
            blur_pad as f32,
            width,
            height,
            corner_radius,
        ) {
            new_pixmap.fill_path(
                &spath,
                &shadow_paint,
                tiny_skia::FillRule::Winding,
                tiny_skia::Transform::identity(),
                None,
            );
            let blur_r = (radius / 2.5).round().max(1.0) as usize;
            apply_box_blur(&mut new_pixmap, blur_r);
            let clear_paint = tiny_skia::Paint {
                blend_mode: tiny_skia::BlendMode::Clear,
                ..Default::default()
            };
            if let Some(clip_path) = rounded_rect_path(
                blur_pad as f32 - offset_x,
                blur_pad as f32 - offset_y,
                width,
                height,
                corner_radius,
            ) {
                new_pixmap.fill_path(
                    &clip_path,
                    &clear_paint,
                    tiny_skia::FillRule::Winding,
                    tiny_skia::Transform::identity(),
                    None,
                );
            }
        }
        let arc_pixmap = std::sync::Arc::new(new_pixmap);
        if ctx.shadow_cache.len() >= 32 {
            ctx.shadow_cache.clear();
        }
        ctx.shadow_cache.insert(cache_key, arc_pixmap.clone());
        arc_pixmap
    } else {
        return;
    };

    let stamp_x = (x + offset_x - blur_pad as f32).round() as i32;
    let stamp_y = (y + offset_y - blur_pad as f32).round() as i32;
    pixmap.draw_pixmap(
        stamp_x,
        stamp_y,
        shadow_pixmap.as_ref().as_ref(),
        &tiny_skia::PixmapPaint::default(),
        tiny_skia::Transform::identity(),
        None,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_rect_node(
    ctx: &mut context::RenderContext,
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    opacity: f32,
    fill: &Option<scene::Fill>,
    radius: f32,
) {
    let Some(fill) = fill else { return };
    let (rx, ry, rw, rh) = (x.round(), y.round(), width.round(), height.round());
    if rw <= 0.0 || rh <= 0.0 {
        return;
    }
    let mut paint = tiny_skia::Paint::default();
    match fill {
        scene::Fill::Solid(col) => {
            paint.set_color(apply_opacity(*col, opacity));
        }
        scene::Fill::LinearGradient { angle_deg, stops } => {
            if let Some(shader) = create_linear_gradient_shader(*angle_deg, stops, rw, rh, opacity)
            {
                paint.shader = shader;
            } else if let Some((_, first_col)) = stops.first() {
                paint.set_color(apply_opacity(*first_col, opacity));
            }
        }
        scene::Fill::RadialGradient {
            cx,
            cy,
            radius: grad_r,
            stops,
        } => {
            if let Some(shader) =
                create_radial_gradient_shader(*cx, *cy, *grad_r, stops, rw, rh, opacity)
            {
                paint.shader = shader;
            } else if let Some((_, first_col)) = stops.first() {
                paint.set_color(apply_opacity(*first_col, opacity));
            }
        }
    }
    if let Some(path) = get_cached_rounded_rect_path(ctx, rw, rh, radius) {
        pixmap.fill_path(
            path,
            &paint,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::from_translate(rx, ry),
            None,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_outline_node(
    ctx: &mut context::RenderContext,
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    opacity: f32,
    color: tiny_skia::Color,
    stroke_w: f32,
    style: scene::OutlineStyle,
    radius: f32,
) {
    let (rx, ry, rw, rh) = (x.round(), y.round(), width.round(), height.round());
    if rw <= 0.0 || rh <= 0.0 {
        return;
    }
    let mut stroke_paint = tiny_skia::Paint::default();
    stroke_paint.set_color(apply_opacity(color, opacity));
    let mut stroke = tiny_skia::Stroke {
        width: stroke_w,
        ..Default::default()
    };
    match style {
        scene::OutlineStyle::Solid => {}
        scene::OutlineStyle::Dashed => {
            let dash_len = (stroke_w * 3.0).max(4.0);
            let gap_len = (stroke_w * 2.0).max(3.0);
            stroke.dash = tiny_skia::StrokeDash::new(vec![dash_len, gap_len], 0.0);
        }
        scene::OutlineStyle::Dotted => {
            let dot_len = stroke_w.max(1.0);
            let gap_len = (stroke_w * 2.0).max(3.0);
            stroke.dash = tiny_skia::StrokeDash::new(vec![dot_len, gap_len], 0.0);
            stroke.line_cap = tiny_skia::LineCap::Round;
        }
    }
    let half_w = stroke_w / 2.0;
    if let Some(path) = get_cached_rounded_rect_path(
        ctx,
        (rw - stroke_w).max(0.0),
        (rh - stroke_w).max(0.0),
        (radius - half_w).max(0.0),
    ) {
        pixmap.stroke_path(
            path,
            &stroke_paint,
            &stroke,
            tiny_skia::Transform::from_translate(rx + half_w, ry + half_w),
            None,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_progress_arc_node(
    pixmap: &mut Pixmap,
    opacity: f32,
    cx: f32,
    cy: f32,
    radius: f32,
    stroke_width: f32,
    ratio: f32,
    track_color: Option<tiny_skia::Color>,
    outline_color: Option<tiny_skia::Color>,
    outline_width: f32,
    arc_color: Option<tiny_skia::Color>,
    start_angle_deg: f32,
    is_ccw: bool,
) {
    if radius <= 1.0 {
        return;
    }
    if let Some(tc) = track_color {
        let mut track_paint = tiny_skia::Paint::default();
        track_paint.set_color(apply_opacity(tc, opacity));
        let track_stroke = tiny_skia::Stroke {
            width: stroke_width,
            ..Default::default()
        };
        let mut track_pb = tiny_skia::PathBuilder::new();
        track_pb.push_circle(cx, cy, radius);
        if let Some(track_path) = track_pb.finish() {
            pixmap.stroke_path(
                &track_path,
                &track_paint,
                &track_stroke,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }
    if let Some(oc) = outline_color {
        let mut outline_paint = tiny_skia::Paint::default();
        outline_paint.set_color(apply_opacity(oc, opacity));
        let rim_stroke = tiny_skia::Stroke {
            width: outline_width,
            ..Default::default()
        };
        let mut rim_pb = tiny_skia::PathBuilder::new();
        rim_pb.push_circle(cx, cy, radius + stroke_width / 2.0);
        rim_pb.push_circle(cx, cy, (radius - stroke_width / 2.0).max(1.0));
        if let Some(rim_path) = rim_pb.finish() {
            pixmap.stroke_path(
                &rim_path,
                &outline_paint,
                &rim_stroke,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }
    if let Some(ac) = arc_color.filter(|_| ratio > 0.002) {
        let mut arc_paint = tiny_skia::Paint::default();
        arc_paint.set_color(apply_opacity(ac, opacity));
        let arc_stroke = tiny_skia::Stroke {
            width: stroke_width,
            line_cap: tiny_skia::LineCap::Round,
            ..Default::default()
        };
        let sweep = if is_ccw {
            -ratio * 2.0 * std::f32::consts::PI
        } else {
            ratio * 2.0 * std::f32::consts::PI
        };
        if let Some(arc_path) =
            circular_arc_path(cx, cy, radius, start_angle_deg.to_radians(), sweep)
        {
            pixmap.stroke_path(
                &arc_path,
                &arc_paint,
                &arc_stroke,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }
}

fn render_effect_node(
    ctx: &mut context::RenderContext,
    pixmap: &mut Pixmap,
    bounds: (f32, f32, f32, f32),
    opacity: f32,
    effect: &scene::Effect,
) {
    let (x, y, width, height) = bounds;
    match effect {
        scene::Effect::Blur { radius } => {
            let r = radius.round().max(0.0) as usize;
            if r > 0 {
                apply_box_blur(pixmap, r);
            }
        }
        scene::Effect::DropShadow {
            radius,
            offset_x,
            offset_y,
            color,
        } => {
            render_shadow_node(
                ctx, pixmap, x, y, width, height, opacity, *radius, 1.0, *offset_x, *offset_y,
                *color, 0.0,
            );
        }
        scene::Effect::InnerGlow { radius, color } => {
            render_outline_node(
                ctx,
                pixmap,
                x,
                y,
                width,
                height,
                opacity,
                *color,
                radius.max(1.0),
                scene::OutlineStyle::Solid,
                0.0,
            );
        }
        scene::Effect::Tint(color) => {
            let fill = Some(scene::Fill::Solid(*color));
            render_rect_node(ctx, pixmap, x, y, width, height, opacity, &fill, 0.0);
        }
    }
}

fn render_scene_node_offset(
    scene: &scene::SceneGraph,
    node_id: scene::SceneNodeId,
    pixmap: &mut Pixmap,
    ctx: &mut context::RenderContext,
    offset_x: f32,
    offset_y: f32,
) {
    let Some(node) = scene.get(node_id) else {
        return;
    };
    let (orig_x, orig_y, width, height) = node.bounds;
    let x = orig_x + offset_x;
    let y = orig_y + offset_y;
    let opacity = node.transform.opacity;

    if node.clip_children && width > 0.0 && height > 0.0 {
        for &child_id in &node.children {
            if matches!(
                scene.get(child_id).map(|c| &c.kind),
                Some(scene::SceneNodeKind::Shadow { .. })
            ) {
                render_scene_node_offset(scene, child_id, pixmap, ctx, offset_x, offset_y);
            }
        }
        if let Some(mut sub_pixmap) = Pixmap::new(width.ceil() as u32, height.ceil() as u32) {
            sub_pixmap.fill(tiny_skia::Color::TRANSPARENT);
            for &child_id in &node.children {
                if !matches!(
                    scene.get(child_id).map(|c| &c.kind),
                    Some(scene::SceneNodeKind::Shadow { .. })
                ) {
                    render_scene_node_offset(
                        scene,
                        child_id,
                        &mut sub_pixmap,
                        ctx,
                        -orig_x,
                        -orig_y,
                    );
                }
            }
            let paint = tiny_skia::PixmapPaint {
                opacity,
                blend_mode: tiny_skia::BlendMode::SourceOver,
                quality: tiny_skia::FilterQuality::Nearest,
            };
            pixmap.draw_pixmap(
                x.round() as i32,
                y.round() as i32,
                sub_pixmap.as_ref(),
                &paint,
                tiny_skia::Transform::identity(),
                None,
            );
            return;
        }
    }

    match &node.kind {
        scene::SceneNodeKind::Root | scene::SceneNodeKind::Container => {}
        scene::SceneNodeKind::Shadow {
            radius,
            opacity: s_opacity,
            offset_x: s_ox,
            offset_y: s_oy,
            color,
            corner_radius,
        } => {
            render_shadow_node(
                ctx,
                pixmap,
                x,
                y,
                width,
                height,
                opacity,
                *radius,
                *s_opacity,
                *s_ox,
                *s_oy,
                *color,
                *corner_radius,
            );
        }
        scene::SceneNodeKind::Rect { fill, radius } => {
            render_rect_node(ctx, pixmap, x, y, width, height, opacity, fill, *radius);
        }
        scene::SceneNodeKind::Outline {
            color,
            width: stroke_w,
            style,
            radius,
        } => {
            render_outline_node(
                ctx, pixmap, x, y, width, height, opacity, *color, *stroke_w, *style, *radius,
            );
        }
        scene::SceneNodeKind::ProgressArc {
            cx,
            cy,
            radius,
            stroke_width,
            ratio,
            track_color,
            outline_color,
            outline_width,
            arc_color,
            start_angle_deg,
            is_ccw,
        } => {
            render_progress_arc_node(
                pixmap,
                opacity,
                *cx + offset_x,
                *cy + offset_y,
                *radius,
                *stroke_width,
                *ratio,
                *track_color,
                *outline_color,
                *outline_width,
                *arc_color,
                *start_angle_deg,
                *is_ccw,
            );
        }
        scene::SceneNodeKind::CustomPath {
            color,
            stroke_width,
            path,
        } => {
            let mut paint = tiny_skia::Paint::default();
            paint.set_color(apply_opacity(*color, opacity));
            let stroke = tiny_skia::Stroke {
                width: *stroke_width,
                ..Default::default()
            };
            pixmap.stroke_path(
                path,
                &paint,
                &stroke,
                tiny_skia::Transform::from_translate(offset_x, offset_y),
                None,
            );
        }
        scene::SceneNodeKind::Text {
            text,
            font_family,
            font_size,
            color,
            align,
        } => {
            let mut target = pixmap.as_mut();
            let resolved_family = ctx.resolve_font_family(font_family);
            text::TextRenderer::render_aligned(
                ctx,
                &mut target,
                text,
                &resolved_family,
                *font_size,
                apply_opacity(*color, opacity),
                x,
                y,
                width,
                height,
                *align,
            );
        }
        scene::SceneNodeKind::Image { path, radius } => {
            draw_image(ctx, pixmap, path, x, y, width, height, *radius);
        }
        scene::SceneNodeKind::Svg { path } => {
            draw_svg(ctx, pixmap, path, x, y, width, height);
        }
        scene::SceneNodeKind::Effect(effect) => {
            render_effect_node(ctx, pixmap, (x, y, width, height), opacity, effect);
        }
    }

    for &child_id in &node.children {
        render_scene_node_offset(scene, child_id, pixmap, ctx, offset_x, offset_y);
    }
}

/// Pluggable backend for fetching remote HTTP(S) image assets into the local cache directory.
pub trait ImageFetcher: Send + Sync {
    /// Initiates an asynchronous download of `url` into `dest`. Returns `Ok(())` if accepted.
    fn fetch(&self, url: &str, dest: &std::path::Path) -> anyhow::Result<()>;

    /// Atomically checks whether a background download finished since the last check,
    /// consuming the completion flag so surfaces are redrawn once instead of perpetually.
    fn is_completed(&self) -> bool {
        false
    }

    /// Non-consuming check for whether a completed download is pending redraw.
    fn peek_completed(&self) -> bool {
        false
    }

    /// Returns `true` if any download is currently in flight.
    fn has_in_flight(&self) -> bool {
        false
    }

    /// Test helper to simulate a completed background download.
    fn mark_completed_for_test(&self) {}
}

/// Fallback [`ImageFetcher`] used when HTTP fetching is disabled.
#[derive(Debug, Default)]
pub struct NoopFetcher {
    completed: std::sync::atomic::AtomicBool,
}

impl ImageFetcher for NoopFetcher {
    fn fetch(&self, url: &str, _dest: &std::path::Path) -> anyhow::Result<()> {
        anyhow::bail!(
            "HTTP image fetching is disabled for URL '{url}' (enable the 'http-fetch' feature or configure a custom ImageFetcher)"
        )
    }

    fn is_completed(&self) -> bool {
        self.completed
            .swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    fn mark_completed_for_test(&self) {
        self.completed
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

/// Default background [`ImageFetcher`] backed by `curl`, available behind the `http-fetch` feature.
#[cfg(feature = "http-fetch")]
#[derive(Default)]
pub struct CurlFetcher {
    in_flight: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    completed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "http-fetch")]
impl CurlFetcher {
    /// Creates a new [`CurlFetcher`] with empty in-flight and completion state.
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(feature = "http-fetch")]
impl ImageFetcher for CurlFetcher {
    fn fetch(&self, url: &str, dest: &std::path::Path) -> anyhow::Result<()> {
        let should_spawn =
            crate::sync_helpers::lock_unpoisoned(&self.in_flight).insert(url.to_string());
        if !should_spawn {
            return Ok(());
        }

        let url_owned = url.to_string();
        let dest_owned = dest.to_path_buf();
        let in_flight = std::sync::Arc::clone(&self.in_flight);
        let completed = std::sync::Arc::clone(&self.completed);

        std::thread::spawn(move || {
            struct InFlightGuard {
                url: String,
                set: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
            }
            impl Drop for InFlightGuard {
                fn drop(&mut self) {
                    crate::sync_helpers::lock_unpoisoned(&self.set).remove(&self.url);
                }
            }
            let _guard = InFlightGuard {
                url: url_owned.clone(),
                set: in_flight,
            };

            if let Some(parent) = dest_owned.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let tmp_file = dest_owned.with_extension("tmp");
            if let Ok(status) = std::process::Command::new("curl")
                .args(["-fsSL", "--connect-timeout", "2", "--max-time", "4", "-o"])
                .arg(&tmp_file)
                .arg(&url_owned)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
            {
                if status.success() {
                    if let Ok(meta) = std::fs::metadata(&tmp_file) {
                        if meta.len() > 64 && std::fs::rename(&tmp_file, &dest_owned).is_ok() {
                            completed.store(true, std::sync::atomic::Ordering::Release);
                        }
                    }
                }
            }
            let _ = std::fs::remove_file(&tmp_file);
        });
        Ok(())
    }

    fn is_completed(&self) -> bool {
        self.completed
            .swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    fn peek_completed(&self) -> bool {
        self.completed.load(std::sync::atomic::Ordering::Acquire)
    }

    fn has_in_flight(&self) -> bool {
        !crate::sync_helpers::lock_unpoisoned(&self.in_flight).is_empty()
    }

    fn mark_completed_for_test(&self) {
        self.completed
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

pub(crate) static DEFAULT_IMAGE_FETCHER: std::sync::LazyLock<std::sync::Arc<dyn ImageFetcher>> =
    std::sync::LazyLock::new(|| {
        #[cfg(feature = "http-fetch")]
        {
            std::sync::Arc::new(CurlFetcher::new())
        }
        #[cfg(not(feature = "http-fetch"))]
        {
            std::sync::Arc::new(NoopFetcher::default())
        }
    });

/// Returns a shared handle to the process-wide default [`ImageFetcher`].
pub fn default_image_fetcher() -> std::sync::Arc<dyn ImageFetcher> {
    std::sync::Arc::clone(&DEFAULT_IMAGE_FETCHER)
}

/// Atomically consumes and returns whether a global background image download finished.
pub fn take_image_download_completed() -> bool {
    DEFAULT_IMAGE_FETCHER.is_completed()
}

/// Returns `true` if a global background image download has finished without consuming the flag.
pub fn has_completed_image_downloads() -> bool {
    DEFAULT_IMAGE_FETCHER.peek_completed()
}

/// Returns `true` if any global background image download is currently in flight.
pub fn has_in_flight_image_downloads() -> bool {
    DEFAULT_IMAGE_FETCHER.has_in_flight()
}

/// Resolves a local path or schedules a background download via the default [`ImageFetcher`].
pub fn resolve_or_cache_image_path(path: &str) -> Option<std::path::PathBuf> {
    resolve_or_cache_image_path_with(path, DEFAULT_IMAGE_FETCHER.as_ref())
}

/// Resolves a local `file://` or filesystem path, or delegates remote URLs to `fetcher`.
pub fn resolve_or_cache_image_path_with(
    path: &str,
    fetcher: &dyn ImageFetcher,
) -> Option<std::path::PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("file://") {
        let mut decoded = Vec::with_capacity(rest.len());
        let bytes = rest.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let (Some(h1), Some(h2)) = (
                    (bytes[i + 1] as char).to_digit(16),
                    (bytes[i + 2] as char).to_digit(16),
                ) {
                    decoded.push((h1 * 16 + h2) as u8);
                    i += 3;
                    continue;
                }
            }
            decoded.push(bytes[i]);
            i += 1;
        }
        let local = String::from_utf8_lossy(&decoded).to_string();
        let p = std::path::PathBuf::from(&local);
        if p.is_file() {
            return Some(p);
        }
        return None;
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in trimmed.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        let cache_dir = std::env::temp_dir().join("wyrd-image-cache");
        let cache_file = cache_dir.join(format!("art_{hash:016x}.img"));
        if cache_file.is_file() {
            if let Ok(meta) = std::fs::metadata(&cache_file) {
                if meta.len() > 64 {
                    return Some(cache_file);
                }
            }
        }
        if let Err(err) = fetcher.fetch(trimmed, &cache_file) {
            log::debug!("ImageFetcher declined or failed to schedule '{trimmed}': {err}");
        }
        return None;
    }
    let p = std::path::PathBuf::from(trimmed);
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_image(
    ctx: &mut context::RenderContext,
    target: &mut Pixmap,
    path: &str,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
) {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return;
    }
    if trimmed.ends_with(".svg") {
        draw_svg(ctx, target, trimmed, x, y, width, height);
        return;
    }
    let w = width.max(1.0) as u32;
    let h = height.max(1.0) as u32;
    if ctx.image_cache.get(trimmed, false, w, h, radius).is_none() {
        let Some(resolved_path) =
            resolve_or_cache_image_path_with(trimmed, ctx.image_fetcher.as_ref())
        else {
            log::debug!("Image asset not yet available locally: {trimmed}");
            return;
        };
        let loaded = std::fs::read(&resolved_path)
            .ok()
            .and_then(|bytes| image::load_from_memory(&bytes).ok())
            .or_else(|| image::open(&resolved_path).ok());
        let Some(image) = loaded else {
            warn!("Unable to decode image widget asset: {trimmed}");
            return;
        };
        let image = if image.width() == w && image.height() == h {
            image.into_rgba8()
        } else {
            image
                .resize_to_fill(w, h, image::imageops::FilterType::Lanczos3)
                .to_rgba8()
        };
        let (actual_w, actual_h) = (image.width(), image.height());
        let mut data = image.into_raw();
        let corner_r = radius.clamp(0.0, actual_w.min(actual_h) as f32 * 0.5);
        let fw = actual_w as f32;
        let fh = actual_h as f32;
        for (idx, pixel) in data.chunks_exact_mut(4).enumerate() {
            let mask = if corner_r > 0.0 {
                let px = (idx as u32 % actual_w) as f32 + 0.5;
                let py = (idx as u32 / actual_w) as f32 + 0.5;
                let dx = if px < corner_r {
                    corner_r - px
                } else if px > fw - corner_r {
                    px - (fw - corner_r)
                } else {
                    0.0
                };
                let dy = if py < corner_r {
                    corner_r - py
                } else if py > fh - corner_r {
                    py - (fh - corner_r)
                } else {
                    0.0
                };
                if dx > 0.0 && dy > 0.0 {
                    let dist = (dx * dx + dy * dy).sqrt();
                    (corner_r + 0.5 - dist).clamp(0.0, 1.0)
                } else {
                    1.0
                }
            } else {
                1.0
            };
            let alpha = ((pixel[3] as f32 * mask).round() as u16).min(255);
            pixel[3] = alpha as u8;
            pixel[0] = (pixel[0] as u16 * alpha / 255) as u8;
            pixel[1] = (pixel[1] as u16 * alpha / 255) as u8;
            pixel[2] = (pixel[2] as u16 * alpha / 255) as u8;
        }
        if let Some(size) = tiny_skia::IntSize::from_wh(actual_w, actual_h) {
            if let Some(pixmap) = Pixmap::from_vec(data, size) {
                ctx.image_cache.insert(trimmed, false, w, h, radius, pixmap);
            }
        }
    }
    if let Some(source) = ctx.image_cache.get(trimmed, false, w, h, radius) {
        let mut target = target.as_mut();
        let off_x = x + (width - source.width() as f32) / 2.0;
        let off_y = y + (height - source.height() as f32) / 2.0;
        target.draw_pixmap(
            off_x.round() as i32,
            off_y.round() as i32,
            source.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::identity(),
            None,
        );
    }
}

fn draw_svg(
    ctx: &mut context::RenderContext,
    target: &mut Pixmap,
    path: &str,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    if path.ends_with(".png") || path.ends_with(".jpg") || path.ends_with(".jpeg") {
        draw_image(ctx, target, path, x, y, width, height, 0.0);
        return;
    }
    let w = width.max(1.0) as u32;
    let h = height.max(1.0) as u32;
    if ctx.image_cache.get(path, true, w, h, 0.0).is_none() {
        let Ok(data) = std::fs::read(path) else {
            error!("Unable to load SVG widget asset: {path}");
            return;
        };
        let tree = match resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()) {
            Ok(tree) => tree,
            Err(error) => {
                error!("Unable to parse SVG widget asset {path}: {error}");
                return;
            }
        };
        let tree_w = tree.size().width().max(1.0);
        let tree_h = tree.size().height().max(1.0);
        let scale_val = (w as f32 / tree_w).min(h as f32 / tree_h);
        let ren_w = (tree_w * scale_val).round().max(1.0) as u32;
        let ren_h = (tree_h * scale_val).round().max(1.0) as u32;
        let Some(mut rendered) = resvg::tiny_skia::Pixmap::new(ren_w, ren_h) else {
            return;
        };
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale_val, scale_val),
            &mut rendered.as_mut(),
        );
        let Some(size) = tiny_skia::IntSize::from_wh(ren_w, ren_h) else {
            return;
        };
        if let Some(rendered) = Pixmap::from_vec(rendered.take(), size) {
            ctx.image_cache.insert(path, true, w, h, 0.0, rendered);
        }
    }
    if let Some(source) = ctx.image_cache.get(path, true, w, h, 0.0) {
        let mut target = target.as_mut();
        let off_x = x + (width - source.width() as f32) / 2.0;
        let off_y = y + (height - source.height() as f32) / 2.0;
        target.draw_pixmap(
            off_x.round() as i32,
            off_y.round() as i32,
            source.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::identity(),
            None,
        );
    }
}

fn create_linear_gradient_shader(
    angle_deg: f32,
    stops: &[(f32, tiny_skia::Color)],
    width: f32,
    height: f32,
    opacity: f32,
) -> Option<tiny_skia::Shader<'static>> {
    if stops.is_empty() || width <= 0.0 || height <= 0.0 {
        return None;
    }

    let rad = angle_deg.to_radians();
    let dx = rad.sin();
    let dy = -rad.cos();

    let cx = width / 2.0;
    let cy = height / 2.0;

    let len = (width * dx.abs() + height * dy.abs()) / 2.0;
    let p0 = tiny_skia::Point::from_xy(cx - dx * len, cy - dy * len);
    let p1 = tiny_skia::Point::from_xy(cx + dx * len, cy + dy * len);

    let skia_stops: Vec<tiny_skia::GradientStop> = stops
        .iter()
        .map(|(offset, col)| {
            let color = apply_opacity(*col, opacity);
            tiny_skia::GradientStop::new(*offset, color)
        })
        .collect();

    tiny_skia::LinearGradient::new(
        p0,
        p1,
        skia_stops,
        tiny_skia::SpreadMode::Pad,
        tiny_skia::Transform::identity(),
    )
}

fn create_radial_gradient_shader(
    cx_ratio: f32,
    cy_ratio: f32,
    radius: f32,
    stops: &[(f32, tiny_skia::Color)],
    width: f32,
    height: f32,
    opacity: f32,
) -> Option<tiny_skia::Shader<'static>> {
    let cx = if (0.0..=1.0).contains(&cx_ratio) {
        cx_ratio * width
    } else {
        cx_ratio
    };
    let cy = if (0.0..=1.0).contains(&cy_ratio) {
        cy_ratio * height
    } else {
        cy_ratio
    };

    let r = if radius > 0.0 {
        if radius <= 1.0 {
            radius * (width.max(height) / 2.0)
        } else {
            radius
        }
    } else {
        (width.max(height) / 2.0).max(1.0)
    };

    let p0 = tiny_skia::Point::from_xy(cx, cy);
    let p1 = tiny_skia::Point::from_xy(cx, cy);

    let skia_stops: Vec<tiny_skia::GradientStop> = stops
        .iter()
        .map(|(offset, col)| {
            let color = apply_opacity(*col, opacity);
            tiny_skia::GradientStop::new(*offset, color)
        })
        .collect();

    tiny_skia::RadialGradient::new(
        p0,
        0.0,
        p1,
        r.max(1.0),
        skia_stops,
        tiny_skia::SpreadMode::Pad,
        tiny_skia::Transform::identity(),
    )
}

/// Converts premultiplied RGBA pixel bytes into Wayland `wl_shm` ARGB8888 (little-endian BGRA).
pub fn to_wayland_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut bgra = vec![0u8; rgba.len()];
    for (src, dst) in rgba.chunks_exact(4).zip(bgra.chunks_exact_mut(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
    bgra
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Color;

    #[test]
    fn test_to_wayland_bgra_conversion() {
        let rgba = [10, 20, 30, 255, 40, 50, 60, 200];
        let bgra = to_wayland_bgra(&rgba);
        assert_eq!(bgra, vec![30, 20, 10, 255, 60, 50, 40, 200]);
    }

    #[test]
    fn test_apply_opacity_scaling() {
        let col = Color::from_rgba8(22, 10, 23, 242);
        assert_eq!(apply_opacity(col, 1.0), col);

        let scaled = apply_opacity(col, 0.5);
        assert_eq!((scaled.alpha() * 255.0).round() as u8, 121);
        assert_eq!((scaled.red() * 255.0).round() as u8, 22);
        assert_eq!((scaled.green() * 255.0).round() as u8, 10);
        assert_eq!((scaled.blue() * 255.0).round() as u8, 23);

        let zero = apply_opacity(col, 0.0);
        assert_eq!(zero, Color::TRANSPARENT);
    }

    #[test]
    fn test_effect_node_renders() {
        let mut scene = scene::SceneGraph::new();
        let root_id = scene.root();
        scene.insert(
            root_id,
            scene::SceneNodeKind::Effect(scene::Effect::Tint(Color::from_rgba8(255, 0, 0, 128))),
            (0.0, 0.0, 16.0, 16.0),
            None,
            scene::SceneTransform::default(),
        );
        let mut pixmap = Pixmap::new(16, 16).unwrap();
        let mut ctx = context::RenderContext::new(1.0);
        scene_to_pixmap(&scene, &mut pixmap, &mut ctx);
        assert!(pixmap.data().chunks_exact(4).any(|px| px[3] > 0));
    }
}
