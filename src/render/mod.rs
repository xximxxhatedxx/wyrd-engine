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

use crate::widgets::WidgetTree;
use log::{error, warn};
use std::panic::{catch_unwind, AssertUnwindSafe};
use tiny_skia::Pixmap;

#[allow(clippy::too_many_arguments)]
pub fn render_surface(
    surface_index: usize,
    width: u32,
    height: u32,
    scale: f64,
    dirty: bool,
    tree: &WidgetTree,
    ctx: &mut context::RenderContext,
    damage: &mut damage::DamageTracker,
    hovered_widget: Option<crate::widgets::WidgetId>,
    focused_widget: Option<crate::widgets::WidgetId>,
    anim_progress: f64,
) -> Option<(usize, Pixmap, damage::DamageTracker)> {
    if !dirty && !ctx.animator.has_active() {
        return None;
    }

    damage.clear();

    let phys_w = (width as f64 * scale).round().max(1.0) as u32;
    let phys_h = (height as f64 * scale).round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(phys_w, phys_h)?;
    pixmap.fill(tiny_skia::Color::TRANSPARENT);

    let progress = if anim_progress <= 0.001 && !ctx.animator.has_active() {
        1.0
    } else {
        anim_progress.clamp(0.0, 1.0) as f32
    };
    let surface_opacity = if progress <= 0.001 { 1.0 } else { progress };

    let mut scene = scene::SceneGraph::new();
    let root_scene_id = scene.root();
    if let Some(root) = tree.root() {
        build_scene_node(
            tree,
            root,
            &mut scene,
            root_scene_id,
            scale,
            hovered_widget,
            focused_widget,
            0.0,
            0.0,
            surface_opacity,
        );
    }

    let prev_scene = ctx.prev_scenes.get(&surface_index);
    scene.diff_against(prev_scene);

    *damage = damage::DamageTracker::from_scene(&scene);

    match ctx.render_mode {
        backend::RenderMode::Gpu => {
            if let Some(ref gpu) = ctx.gpu {
                let _ = gpu.render_scene(&scene);
            }
            scene_to_pixmap(&scene, &mut pixmap, ctx);
        }
        backend::RenderMode::Cpu | backend::RenderMode::Auto => {
            scene_to_pixmap(&scene, &mut pixmap, ctx);
        }
    }

    ctx.prev_scenes.insert(surface_index, scene);

    if ctx.debug_overlay {
        let mut target = pixmap.as_mut();
        let overlay = format!(
            "damage:{} anim:{} scale:{:.2}",
            damage.regions.len(),
            ctx.animator.has_active(),
            scale,
        );
        text::TextRenderer::render(
            ctx,
            &mut target,
            &overlay,
            "sans-serif",
            11.0 * scale as f32,
            tiny_skia::Color::from_rgba8(255, 220, 120, 255),
            4.0 * scale as f32,
            2.0 * scale as f32,
            phys_w as f32,
        );
    }
    if damage.regions.is_empty() {
        damage.set_full_redraw();
    }
    damage.optimize();
    Some((surface_index, pixmap, damage.clone()))
}

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

fn is_node_hovered(
    tree: &crate::widgets::WidgetTree,
    id: crate::widgets::WidgetId,
    hovered_widget: Option<crate::widgets::WidgetId>,
) -> bool {
    let Some(mut cur) = hovered_widget else {
        return false;
    };
    while let Some(node) = tree.get(cur) {
        if cur == id {
            return true;
        }
        let Some(p) = node.parent else { break };
        cur = p;
    }
    false
}

fn is_node_focused(
    tree: &crate::widgets::WidgetTree,
    id: crate::widgets::WidgetId,
    focused_widget: Option<crate::widgets::WidgetId>,
) -> bool {
    let Some(mut cur) = focused_widget else {
        return false;
    };
    while let Some(node) = tree.get(cur) {
        if cur == id {
            return true;
        }
        let Some(p) = node.parent else { break };
        cur = p;
    }
    false
}

#[allow(clippy::too_many_arguments)]
pub fn build_scene_node(
    tree: &WidgetTree,
    id: crate::widgets::WidgetId,
    scene: &mut scene::SceneGraph,
    parent_scene_id: scene::SceneNodeId,
    scale: f64,
    hovered_widget: Option<crate::widgets::WidgetId>,
    focused_widget: Option<crate::widgets::WidgetId>,
    offset_x: f32,
    offset_y: f32,
    surface_opacity: f32,
) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let Some(node) = tree.get(id) else { return };
        if node.failed {
            return;
        }
        let is_focused = is_node_focused(tree, id, focused_widget);
        let is_hovered = is_node_hovered(tree, id, hovered_widget);
        let state = if is_focused {
            crate::style::WidgetState::Focus
        } else if is_hovered {
            crate::style::WidgetState::Hover
        } else {
            crate::style::WidgetState::Normal
        };
        let (
            background,
            foreground,
            accent,
            base_opacity,
            outline_color,
            outline_width,
            outline_style,
            shadow,
            shadow_color,
        ) = node.style.for_state(state);
        let node_opacity = if base_opacity <= 0.001 {
            1.0
        } else {
            base_opacity.clamp(0.0, 1.0)
        };
        let opacity = (node_opacity * surface_opacity).clamp(0.0, 1.0);
        let (x, y, width, height) = node.final_rect;
        let (x, y, width, height) = (
            x * scale as f32 + offset_x,
            y * scale as f32 + offset_y,
            width * scale as f32,
            height * scale as f32,
        );

        if width <= 0.0 || height <= 0.0 {
            return;
        }

        let bounds = (x, y, width, height);
        let scaled_radius = node.style.radius * scale as f32;
        let mut matrix = glam::Mat4::IDENTITY;
        if let Some(t) = node.layout.transform.as_ref() {
            if let Some(tx) = t.translate_x {
                matrix *=
                    glam::Mat4::from_translation(glam::Vec3::new(tx * scale as f32, 0.0, 0.0));
            }
            if let Some(ty) = t.translate_y {
                matrix *=
                    glam::Mat4::from_translation(glam::Vec3::new(0.0, ty * scale as f32, 0.0));
            }
            if let Some(rot) = t.rotate {
                let rad = rot.to_radians();
                matrix *= glam::Mat4::from_rotation_z(rad);
            }
            if let (Some(sx), Some(sy)) = (t.scale_x, t.scale_y) {
                matrix *= glam::Mat4::from_scale(glam::Vec3::new(sx, sy, 1.0));
            } else if let Some(sx) = t.scale_x {
                matrix *= glam::Mat4::from_scale(glam::Vec3::new(sx, 1.0, 1.0));
            } else if let Some(sy) = t.scale_y {
                matrix *= glam::Mat4::from_scale(glam::Vec3::new(1.0, sy, 1.0));
            }
        }
        let transform = scene::SceneTransform { matrix, opacity };

        let widget_scene_id = scene
            .insert(
                parent_scene_id,
                scene::SceneNodeKind::Container,
                bounds,
                Some(id),
                transform,
            )
            .unwrap_or(parent_scene_id);

        if node.layout.clip || node.layout.scroll_y {
            scene.set_clip_children(widget_scene_id, true);
        }

        // 1. Soft Drop Shadow
        let has_visible_bg = match &background {
            Some(scene::Fill::Solid(c)) => c.alpha() > 0.001,
            Some(scene::Fill::LinearGradient { stops, .. })
            | Some(scene::Fill::RadialGradient { stops, .. }) => {
                stops.iter().any(|(_, c)| c.alpha() > 0.001)
            }
            None => false,
        };
        let has_visible_outline = outline_color
            .map(|c| c.alpha() > 0.001 && outline_width > 0.0)
            .unwrap_or(false);
        let is_pure_text = matches!(node.content, crate::widgets::WidgetContent::Text { .. })
            && !has_visible_bg
            && !has_visible_outline;

        if !is_pure_text {
            if let Some((s_radius, s_opacity, s_ox, s_oy)) = shadow {
                if s_opacity > 0.0 && s_radius > 0.0 {
                    let col = shadow_color
                        .or(node.style.shadow_color)
                        .unwrap_or(tiny_skia::Color::BLACK);
                    scene.insert(
                        widget_scene_id,
                        scene::SceneNodeKind::Shadow {
                            radius: s_radius * scale as f32,
                            opacity: s_opacity,
                            offset_x: s_ox * scale as f32,
                            offset_y: s_oy * scale as f32,
                            color: col,
                            corner_radius: scaled_radius,
                        },
                        bounds,
                        Some(id),
                        transform,
                    );
                }
            }
        }

        let is_ring = matches!(
            node.content,
            crate::widgets::WidgetContent::ProgressRing { .. }
        );

        // 2. Background
        if !is_ring {
            if let Some(fill) = background {
                scene.insert(
                    widget_scene_id,
                    scene::SceneNodeKind::Rect {
                        fill: Some(fill),
                        radius: scaled_radius,
                    },
                    bounds,
                    Some(id),
                    transform,
                );
            }
        }

        // 3. Outline / Border
        if !is_ring {
            if let Some(outline_col) = outline_color {
                let stroke_width = (outline_width * scale as f32).max(1.0);
                scene.insert(
                    widget_scene_id,
                    scene::SceneNodeKind::Outline {
                        color: outline_col,
                        width: stroke_width,
                        style: outline_style,
                        radius: scaled_radius,
                    },
                    bounds,
                    Some(id),
                    transform,
                );
            }
        }

        // 4. Content Specifics
        match &node.content {
            crate::widgets::WidgetContent::Progress { value, max, props } => {
                let direction = props
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("ltr");
                let fill_radius = props
                    .get("fill_radius")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32);
                let ratio = (value / max.max(0.001)).clamp(0.0, 1.0);
                let fill_color = accent.or(foreground).unwrap_or(tiny_skia::Color::WHITE);

                let (fill_x, fill_y, fill_w, fill_h) = match direction {
                    "rtl" => {
                        let fw = (width * ratio).max(0.0);
                        (x + width - fw, y, fw, height)
                    }
                    "ttb" => {
                        let fh = (height * ratio).max(0.0);
                        (x, y, width, fh)
                    }
                    "btt" => {
                        let fh = (height * ratio).max(0.0);
                        (x, y + height - fh, width, fh)
                    }
                    _ => {
                        let fw = (width * ratio).max(0.0);
                        (x, y, fw, height)
                    }
                };

                if fill_w > 0.0 && fill_h > 0.0 {
                    let r =
                        fill_radius.unwrap_or_else(|| scaled_radius.min(fill_w.min(fill_h) / 2.0));
                    scene.insert(
                        widget_scene_id,
                        scene::SceneNodeKind::Rect {
                            fill: Some(scene::Fill::Solid(fill_color)),
                            radius: r,
                        },
                        (fill_x, fill_y, fill_w, fill_h),
                        Some(id),
                        transform,
                    );
                }
            }
            crate::widgets::WidgetContent::ProgressRing {
                value,
                max,
                stroke_width,
                text,
                props,
            } => {
                let sw_prop = props
                    .get("track_thickness")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32);
                let sw = (sw_prop.or(*stroke_width).unwrap_or(7.0) * scale as f32).max(2.0);
                let start_angle_deg = props
                    .get("start_angle_deg")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32)
                    .unwrap_or(-90.0);
                let direction = props
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("cw");
                let show_text = props
                    .get("show_text")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let prop_text = props
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);
                let font_size_prop = props
                    .get("font_size")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32);

                let size = width.min(height);
                let radius = (size - sw) / 2.0;
                let cx = x + width / 2.0;
                let cy = y + height / 2.0;

                if radius > 1.0 {
                    let track_color = node.style.background.as_ref().and_then(|f| match f {
                        scene::Fill::Solid(c) => Some(*c),
                        scene::Fill::LinearGradient { stops, .. }
                        | scene::Fill::RadialGradient { stops, .. } => stops.first().map(|s| s.1),
                    });
                    let outline_color = node.style.outline_color;
                    let outline_width = (node.style.outline_width * scale as f32).max(1.0);
                    let ratio = (value / max.max(0.001)).clamp(0.0, 1.0);
                    let arc_color = if ratio > 0.002 {
                        accent.or(foreground).or(Some(tiny_skia::Color::WHITE))
                    } else {
                        None
                    };

                    scene.insert(
                        widget_scene_id,
                        scene::SceneNodeKind::ProgressArc {
                            cx,
                            cy,
                            radius,
                            stroke_width: sw,
                            ratio,
                            track_color,
                            outline_color,
                            outline_width,
                            arc_color,
                            start_angle_deg,
                            is_ccw: direction == "ccw",
                        },
                        bounds,
                        Some(id),
                        transform,
                    );

                    if show_text {
                        let label = prop_text
                            .or_else(|| text.clone())
                            .unwrap_or_else(|| format!("{:.0}%", value));
                        if !label.is_empty() {
                            let text_color = foreground.unwrap_or(tiny_skia::Color::WHITE);
                            let font_size = if let Some(fs) = font_size_prop {
                                fs * scale as f32
                            } else if node.style.font_size > 0.0 {
                                node.style.font_size * scale as f32
                            } else {
                                (radius * 0.52).clamp(10.0, 18.0) * scale as f32
                            };
                            scene.insert(
                                widget_scene_id,
                                scene::SceneNodeKind::Text {
                                    text: label,
                                    font_family: node.style.font_family.clone(),
                                    font_size,
                                    color: text_color,
                                    align: cosmic_text::Align::Center,
                                },
                                bounds,
                                Some(id),
                                transform,
                            );
                        }
                    }
                }
            }
            crate::widgets::WidgetContent::Slider {
                value,
                min,
                max,
                props,
            } => {
                let show_thumb = props
                    .get("show_thumb")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let thumb_radius_prop = props
                    .get("thumb_radius")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32);
                let show_percent_text = props
                    .get("show_percent_text")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let percent_text_format = props
                    .get("percent_text_format")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{:.0}%");
                let direction = props
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("ltr");
                let fill_radius_prop = props
                    .get("fill_radius")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32);
                let tick_at_prop = props
                    .get("tick_at")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32);

                if let Some(track_bg) = node.style.background.clone() {
                    scene.insert(
                        widget_scene_id,
                        scene::SceneNodeKind::Rect {
                            fill: Some(track_bg),
                            radius: scaled_radius.max(6.0),
                        },
                        bounds,
                        Some(id),
                        transform,
                    );
                }
                if let Some(outline_color) = node.style.outline_color {
                    scene.insert(
                        widget_scene_id,
                        scene::SceneNodeKind::Outline {
                            color: outline_color,
                            width: (node.style.outline_width * scale as f32).max(1.0),
                            style: node.style.outline_style,
                            radius: scaled_radius.max(6.0),
                        },
                        bounds,
                        Some(id),
                        transform,
                    );
                }

                let ratio = ((value - min) / (max - min).max(0.001)).clamp(0.0, 1.0);
                let (fill_x, fill_y, fill_w, fill_h) = match direction {
                    "rtl" => {
                        let fw = (width * ratio).max(0.0);
                        (x + width - fw, y, fw, height)
                    }
                    "ttb" => {
                        let fh = (height * ratio).max(0.0);
                        (x, y, width, fh)
                    }
                    "btt" => {
                        let fh = (height * ratio).max(0.0);
                        (x, y + height - fh, width, fh)
                    }
                    _ => {
                        let fw = (width * ratio).max(0.0);
                        (x, y, fw, height)
                    }
                };

                if fill_w > 0.0 && fill_h > 0.0 {
                    let fill_color = accent.or(foreground).unwrap_or(tiny_skia::Color::WHITE);
                    let r = fill_radius_prop
                        .unwrap_or_else(|| scaled_radius.min(fill_w.min(fill_h) / 2.0).max(4.0));
                    scene.insert(
                        widget_scene_id,
                        scene::SceneNodeKind::Rect {
                            fill: Some(scene::Fill::Solid(fill_color)),
                            radius: r,
                        },
                        (fill_x, fill_y, fill_w, fill_h),
                        Some(id),
                        transform,
                    );
                }

                // Tick mark
                let target_tick = tick_at_prop.or({
                    if *max > 100.0 && *min < 100.0 {
                        Some(100.0)
                    } else {
                        None
                    }
                });
                if let Some(tick_val) = target_tick {
                    if tick_val >= *min && tick_val <= *max && *max > *min {
                        let ratio_tick = (tick_val - min) / (max - min);
                        let tick_x = if direction == "rtl" {
                            x + width * (1.0 - ratio_tick)
                        } else {
                            x + width * ratio_tick
                        };
                        let mut tick_pb = tiny_skia::PathBuilder::new();
                        tick_pb.move_to(tick_x, y + 1.0 * scale as f32);
                        tick_pb.line_to(tick_x, y + height - 1.0 * scale as f32);
                        if let Some(path) = tick_pb.finish() {
                            let tick_color = foreground.unwrap_or(tiny_skia::Color::WHITE);
                            scene.insert(
                                widget_scene_id,
                                scene::SceneNodeKind::CustomPath {
                                    color: tick_color,
                                    stroke_width: 1.5 * scale as f32,
                                    path,
                                },
                                bounds,
                                Some(id),
                                transform,
                            );
                        }
                    }
                }

                // Percentage / Value text
                if show_percent_text {
                    let pct_text = if percent_text_format == "{:.0}%" {
                        format!("{:.0}%", value)
                    } else if percent_text_format.contains("{}") {
                        percent_text_format.replace("{}", &format!("{:.0}", value))
                    } else {
                        format!("{:.0}%", value)
                    };
                    scene.insert(
                        widget_scene_id,
                        scene::SceneNodeKind::Text {
                            text: pct_text,
                            font_family: node.style.font_family.clone(),
                            font_size: (height * 0.52).clamp(9.0, 12.0) * scale as f32,
                            color: foreground.unwrap_or(tiny_skia::Color::WHITE),
                            align: cosmic_text::Align::Center,
                        },
                        bounds,
                        Some(id),
                        transform,
                    );
                }

                // Thumb
                if show_thumb {
                    let thumb_radius =
                        thumb_radius_prop.unwrap_or_else(|| (height * 0.40).clamp(5.0, 9.0));
                    let (thumb_cx, thumb_cy) = match direction {
                        "rtl" => (
                            fill_x.clamp(x + thumb_radius, x + width - thumb_radius),
                            y + height / 2.0,
                        ),
                        "ttb" => (
                            (x + width / 2.0),
                            (y + fill_h).clamp(y + thumb_radius, y + height - thumb_radius),
                        ),
                        "btt" => (
                            (x + width / 2.0),
                            fill_y.clamp(y + thumb_radius, y + height - thumb_radius),
                        ),
                        _ => (
                            (x + fill_w).clamp(x + thumb_radius, x + width - thumb_radius),
                            y + height / 2.0,
                        ),
                    };
                    let thumb_bounds = (
                        thumb_cx - thumb_radius,
                        thumb_cy - thumb_radius,
                        thumb_radius * 2.0,
                        thumb_radius * 2.0,
                    );
                    let thumb_color = foreground.or(accent).unwrap_or(tiny_skia::Color::WHITE);
                    scene.insert(
                        widget_scene_id,
                        scene::SceneNodeKind::Rect {
                            fill: Some(scene::Fill::Solid(thumb_color)),
                            radius: thumb_radius,
                        },
                        thumb_bounds,
                        Some(id),
                        transform,
                    );
                    if let Some(outline_color) = node.style.outline_color.or(accent) {
                        scene.insert(
                            widget_scene_id,
                            scene::SceneNodeKind::Outline {
                                color: outline_color,
                                width: 1.5 * scale as f32,
                                style: scene::OutlineStyle::Solid,
                                radius: thumb_radius,
                            },
                            thumb_bounds,
                            Some(id),
                            transform,
                        );
                    }
                }
            }
            _ => {}
        }

        // 4. Inner bounds (respecting layout / style padding)
        let (pt, pr, pb, pl) = if node.layout.padding != (0.0, 0.0, 0.0, 0.0) {
            node.layout.padding
        } else {
            node.style.padding
        };
        let (pt, pr, pb, pl) = (
            pt * scale as f32,
            pr * scale as f32,
            pb * scale as f32,
            pl * scale as f32,
        );
        let (pl, pr) = if pl + pr >= width {
            (0.0, 0.0)
        } else {
            (pl, pr)
        };
        let (pt, pb) = if pt + pb >= height {
            (0.0, 0.0)
        } else {
            (pt, pb)
        };
        let inner_bounds = (
            x + pl,
            y + pt,
            (width - pl - pr).max(1.0),
            (height - pt - pb).max(1.0),
        );

        match &node.content {
            crate::widgets::WidgetContent::Image { path } => {
                scene.insert(
                    widget_scene_id,
                    scene::SceneNodeKind::Image { path: path.clone() },
                    inner_bounds,
                    Some(id),
                    transform,
                );
            }
            crate::widgets::WidgetContent::Svg { path } => {
                scene.insert(
                    widget_scene_id,
                    scene::SceneNodeKind::Svg { path: path.clone() },
                    inner_bounds,
                    Some(id),
                    transform,
                );
            }
            _ => {}
        }

        // 5. Text & Label & Button & Module Text & TextInput

        let (text, text_color): (Option<std::borrow::Cow<'_, str>>, tiny_skia::Color) = match &node
            .content
        {
            crate::widgets::WidgetContent::Text { text } => (
                Some(std::borrow::Cow::Borrowed(text.as_str())),
                foreground.unwrap_or(tiny_skia::Color::WHITE),
            ),
            crate::widgets::WidgetContent::Button { label, .. } => (
                Some(std::borrow::Cow::Borrowed(label.as_str())),
                foreground.unwrap_or(tiny_skia::Color::WHITE),
            ),
            crate::widgets::WidgetContent::TextInput {
                text,
                placeholder,
                focused,
                cursor_pos,
                selection: _,
            } => {
                let active_focus = *focused || is_focused;
                if text.is_empty() {
                    let ph = if placeholder.is_empty() || active_focus {
                        "|"
                    } else {
                        placeholder.as_str()
                    };
                    let color = if active_focus {
                        foreground
                            .and_then(|c| {
                                tiny_skia::Color::from_rgba(
                                    c.red(),
                                    c.green(),
                                    c.blue(),
                                    (c.alpha() * 0.7).clamp(0.1, 1.0),
                                )
                            })
                            .unwrap_or(tiny_skia::Color::from_rgba8(180, 175, 195, 180))
                    } else {
                        foreground
                            .and_then(|c| {
                                tiny_skia::Color::from_rgba(
                                    c.red(),
                                    c.green(),
                                    c.blue(),
                                    (c.alpha() * 0.5).clamp(0.1, 1.0),
                                )
                            })
                            .unwrap_or(tiny_skia::Color::from_rgba8(140, 135, 150, 140))
                    };
                    (Some(std::borrow::Cow::Borrowed(ph)), color)
                } else if active_focus {
                    let mut display = text.clone();
                    let pos = (*cursor_pos).min(display.len());
                    let safe_pos = if display.is_char_boundary(pos) {
                        pos
                    } else {
                        display.len()
                    };
                    display.insert(safe_pos, '|');
                    (
                        Some(std::borrow::Cow::Owned(display)),
                        foreground.unwrap_or(tiny_skia::Color::WHITE),
                    )
                } else {
                    (
                        Some(std::borrow::Cow::Borrowed(text.as_str())),
                        foreground.unwrap_or(tiny_skia::Color::WHITE),
                    )
                }
            }
            crate::widgets::WidgetContent::Module { payload, .. } if node.children.is_empty() => (
                payload
                    .get("text")
                    .and_then(|value| value.as_str())
                    .map(std::borrow::Cow::Borrowed),
                foreground.unwrap_or(tiny_skia::Color::WHITE),
            ),
            _ => (None, foreground.unwrap_or(tiny_skia::Color::WHITE)),
        };

        if let Some(text) = text {
            let align = match node.layout.justify {
                crate::widgets::layout::JustifyContent::Center => cosmic_text::Align::Center,
                crate::widgets::layout::JustifyContent::End => cosmic_text::Align::Right,
                crate::widgets::layout::JustifyContent::Start => match &node.content {
                    crate::widgets::WidgetContent::Button { .. } => cosmic_text::Align::Center,
                    _ => match node.layout.align {
                        crate::widgets::layout::Align::Center => cosmic_text::Align::Center,
                        _ => cosmic_text::Align::Left,
                    },
                },
                _ => cosmic_text::Align::Left,
            };
            scene.insert(
                widget_scene_id,
                scene::SceneNodeKind::Text {
                    text: text.into_owned(),
                    font_family: node.style.font_family.clone(),
                    font_size: node.style.font_size.max(10.0) * scale as f32,
                    color: text_color,
                    align,
                },
                inner_bounds,
                Some(id),
                transform,
            );
        }

        // 6. Recursive Children (sorted by z_index)
        let mut children: Vec<crate::widgets::WidgetId> =
            tree.get(id).map(|n| n.children.clone()).unwrap_or_default();
        children.sort_by_key(|child_id| {
            tree.get(*child_id)
                .and_then(|child| child.layout.z_index)
                .unwrap_or(0)
        });
        for child in children {
            build_scene_node(
                tree,
                child,
                scene,
                widget_scene_id,
                scale,
                hovered_widget,
                focused_widget,
                offset_x,
                offset_y,
                surface_opacity,
            );
        }
    }));

    if result.is_err() {
        error!("Widget {:?} panicked during scene building", id);
    }
}

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
        let cw = width.ceil() as u32;
        let ch = height.ceil() as u32;
        if let Some(mut sub_pixmap) = Pixmap::new(cw, ch) {
            sub_pixmap.fill(tiny_skia::Color::TRANSPARENT);
            for &child_id in &node.children {
                render_scene_node_offset(scene, child_id, &mut sub_pixmap, ctx, -orig_x, -orig_y);
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
            offset_x,
            offset_y,
            color,
            corner_radius,
        } => {
            if *s_opacity > 0.0 && *radius > 0.0 && width > 0.0 && height > 0.0 {
                let color_alpha = if color.alpha() > 0.001 {
                    color.alpha()
                } else {
                    1.0
                };
                let shadow_alpha =
                    (color_alpha * s_opacity.clamp(0.0, 1.0) * opacity).clamp(0.0, 1.0);
                let blur_pad = (*radius * 1.5).ceil() as u32 + 4;
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
                    (*radius * 10.0).round() as u32,
                    (*corner_radius * 10.0).round() as u32,
                    col_u32,
                    (*offset_x * 10.0).round() as i32,
                    (*offset_y * 10.0).round() as i32,
                );

                let shadow_pixmap = if let Some(cached) = ctx.shadow_cache.get(&cache_key) {
                    cached.clone()
                } else if let Some(mut new_pixmap) =
                    tiny_skia::Pixmap::new(offscreen_w, offscreen_h)
                {
                    let mut shadow_paint = tiny_skia::Paint::default();
                    shadow_paint
                        .set_color(tiny_skia::Color::from_rgba8(col_r, col_g, col_b, col_a));

                    if let Some(spath) = rounded_rect_path(
                        blur_pad as f32,
                        blur_pad as f32,
                        width,
                        height,
                        *corner_radius,
                    ) {
                        new_pixmap.fill_path(
                            &spath,
                            &shadow_paint,
                            tiny_skia::FillRule::Winding,
                            tiny_skia::Transform::identity(),
                            None,
                        );
                        let blur_r = (*radius / 2.5).round().max(1.0) as usize;
                        apply_box_blur(&mut new_pixmap, blur_r);

                        // Clear the inner footprint of the element so the outer box-shadow is cast strictly
                        // OUTSIDE the element bounds (matching CSS specification).
                        let clear_paint = tiny_skia::Paint {
                            blend_mode: tiny_skia::BlendMode::Clear,
                            ..Default::default()
                        };
                        let clip_x = blur_pad as f32 - offset_x;
                        let clip_y = blur_pad as f32 - offset_y;
                        if let Some(clip_path) =
                            rounded_rect_path(clip_x, clip_y, width, height, *corner_radius)
                        {
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
        }
        scene::SceneNodeKind::Rect { fill, radius } => {
            if let Some(fill) = fill {
                let rx = x.round();
                let ry = y.round();
                let rw = width.round();
                let rh = height.round();
                if rw > 0.0 && rh > 0.0 {
                    let mut paint = tiny_skia::Paint::default();
                    match fill {
                        scene::Fill::Solid(col) => {
                            paint.set_color(apply_opacity(*col, opacity));
                        }
                        scene::Fill::LinearGradient { angle_deg, stops } => {
                            if let Some(shader) =
                                create_linear_gradient_shader(*angle_deg, stops, rw, rh, opacity)
                            {
                                paint.shader = shader;
                            } else if let Some((_, first_col)) = stops.first() {
                                paint.set_color(apply_opacity(*first_col, opacity));
                            }
                        }
                        scene::Fill::RadialGradient {
                            cx,
                            cy,
                            radius,
                            stops,
                        } => {
                            if let Some(shader) = create_radial_gradient_shader(
                                *cx, *cy, *radius, stops, rw, rh, opacity,
                            ) {
                                paint.shader = shader;
                            } else if let Some((_, first_col)) = stops.first() {
                                paint.set_color(apply_opacity(*first_col, opacity));
                            }
                        }
                    }
                    if let Some(path) = get_cached_rounded_rect_path(ctx, rw, rh, *radius) {
                        pixmap.fill_path(
                            path,
                            &paint,
                            tiny_skia::FillRule::Winding,
                            tiny_skia::Transform::from_translate(rx, ry),
                            None,
                        );
                    }
                }
            }
        }
        scene::SceneNodeKind::Outline {
            color,
            width: stroke_w,
            style,
            radius,
        } => {
            let rx = x.round();
            let ry = y.round();
            let rw = width.round();
            let rh = height.round();
            if rw > 0.0 && rh > 0.0 {
                let mut stroke_paint = tiny_skia::Paint::default();
                stroke_paint.set_color(apply_opacity(*color, opacity));
                let mut stroke = tiny_skia::Stroke {
                    width: *stroke_w,
                    ..Default::default()
                };
                match style {
                    scene::OutlineStyle::Solid => {}
                    scene::OutlineStyle::Dashed => {
                        let dash_len = (*stroke_w * 3.0).max(4.0);
                        let gap_len = (*stroke_w * 2.0).max(3.0);
                        if let Some(dash) = tiny_skia::StrokeDash::new(vec![dash_len, gap_len], 0.0)
                        {
                            stroke.dash = Some(dash);
                        }
                    }
                    scene::OutlineStyle::Dotted => {
                        let dot_len = (*stroke_w * 1.0).max(1.0);
                        let gap_len = (*stroke_w * 2.0).max(3.0);
                        if let Some(dash) = tiny_skia::StrokeDash::new(vec![dot_len, gap_len], 0.0)
                        {
                            stroke.dash = Some(dash);
                        }
                        stroke.line_cap = tiny_skia::LineCap::Round;
                    }
                }
                let half_w = stroke_w / 2.0;
                let inner_w = (rw - stroke_w).max(0.0);
                let inner_h = (rh - stroke_w).max(0.0);
                let inner_r = (radius - half_w).max(0.0);
                if let Some(path) = get_cached_rounded_rect_path(ctx, inner_w, inner_h, inner_r) {
                    pixmap.stroke_path(
                        path,
                        &stroke_paint,
                        &stroke,
                        tiny_skia::Transform::from_translate(rx + half_w, ry + half_w),
                        None,
                    );
                }
            }
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
            if *radius > 1.0 {
                if let Some(tc) = track_color {
                    let mut track_paint = tiny_skia::Paint::default();
                    track_paint.set_color(apply_opacity(*tc, opacity));
                    let track_stroke = tiny_skia::Stroke {
                        width: *stroke_width,
                        ..Default::default()
                    };
                    let mut track_pb = tiny_skia::PathBuilder::new();
                    track_pb.push_circle(*cx, *cy, *radius);
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
                    outline_paint.set_color(apply_opacity(*oc, opacity));
                    let rim_stroke = tiny_skia::Stroke {
                        width: *outline_width,
                        ..Default::default()
                    };

                    let outer_r = radius + stroke_width / 2.0;
                    let inner_r = (radius - stroke_width / 2.0).max(1.0);

                    let mut rim_pb = tiny_skia::PathBuilder::new();
                    rim_pb.push_circle(*cx, *cy, outer_r);
                    rim_pb.push_circle(*cx, *cy, inner_r);
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

                if let Some(ac) = arc_color {
                    if *ratio > 0.002 {
                        let mut arc_paint = tiny_skia::Paint::default();
                        arc_paint.set_color(apply_opacity(*ac, opacity));
                        let arc_stroke = tiny_skia::Stroke {
                            width: *stroke_width,
                            line_cap: tiny_skia::LineCap::Round,
                            ..Default::default()
                        };

                        let start_angle = start_angle_deg.to_radians();
                        let sweep_angle = if *is_ccw {
                            -*ratio * 2.0 * std::f32::consts::PI
                        } else {
                            *ratio * 2.0 * std::f32::consts::PI
                        };

                        if let Some(arc_path) =
                            circular_arc_path(*cx, *cy, *radius, start_angle, sweep_angle)
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
            }
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
                tiny_skia::Transform::identity(),
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
            text::TextRenderer::render_aligned(
                ctx,
                &mut target,
                text,
                ctx.resolve_font_family(font_family),
                *font_size,
                apply_opacity(*color, opacity),
                x,
                y,
                width,
                height,
                *align,
            );
        }
        scene::SceneNodeKind::Image { path } => {
            draw_image(ctx, pixmap, path, x, y, width, height);
        }
        scene::SceneNodeKind::Svg { path } => {
            draw_svg(ctx, pixmap, path, x, y, width, height);
        }
        scene::SceneNodeKind::Effect(_) => {}
    }

    for &child_id in &node.children {
        render_scene_node_offset(scene, child_id, pixmap, ctx, offset_x, offset_y);
    }
}

fn draw_image(
    ctx: &mut context::RenderContext,
    target: &mut Pixmap,
    path: &str,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
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
    if ctx.image_cache.get(trimmed, false, w, h).is_none() {
        let Ok(image) = image::open(trimmed) else {
            warn!("Unable to load image widget asset: {}", trimmed);
            return;
        };
        let image = image
            .resize(w, h, image::imageops::FilterType::Lanczos3)
            .to_rgba8();
        let (actual_w, actual_h) = (image.width(), image.height());
        let mut data = image.into_raw();
        for pixel in data.chunks_exact_mut(4) {
            let alpha = pixel[3] as u16;
            pixel[0] = (pixel[0] as u16 * alpha / 255) as u8;
            pixel[1] = (pixel[1] as u16 * alpha / 255) as u8;
            pixel[2] = (pixel[2] as u16 * alpha / 255) as u8;
        }
        if let Some(size) = tiny_skia::IntSize::from_wh(actual_w, actual_h) {
            if let Some(pixmap) = Pixmap::from_vec(data, size) {
                ctx.image_cache.insert(path, false, w, h, pixmap);
            }
        }
    }
    if let Some(source) = ctx.image_cache.get(path, false, w, h) {
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
        draw_image(ctx, target, path, x, y, width, height);
        return;
    }
    let w = width.max(1.0) as u32;
    let h = height.max(1.0) as u32;
    if ctx.image_cache.get(path, true, w, h).is_none() {
        let Ok(data) = std::fs::read(path) else {
            error!("Unable to load SVG widget asset: {}", path);
            return;
        };
        let tree = match resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()) {
            Ok(tree) => tree,
            Err(error) => {
                error!("Unable to parse SVG widget asset {}: {}", path, error);
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
            ctx.image_cache.insert(path, true, w, h, rendered);
        }
    }
    if let Some(source) = ctx.image_cache.get(path, true, w, h) {
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

/// Converts standard RGBA pixel slice to Wayland ARGB8888 (little-endian BGRA) format.
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

    #[test]
    fn test_to_wayland_bgra_conversion() {
        let rgba = [10, 20, 30, 255, 40, 50, 60, 200];
        let bgra = to_wayland_bgra(&rgba);
        assert_eq!(bgra, vec![30, 20, 10, 255, 60, 50, 40, 200]);
    }
    use tiny_skia::Color;

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
    fn test_widget_style_default_opacity_is_one() {
        let style = crate::widgets::WidgetStyle::default();
        assert_eq!(style.opacity, 1.0);
    }

    #[test]
    fn test_transparent_background_renders_invisible_and_does_not_fallback() {
        let mut tree = crate::widgets::WidgetTree::new();
        let mut node = crate::widgets::WidgetNode::new(crate::widgets::WidgetContent::Container);
        node.style.background = crate::widgets::parse_fill("transparent");
        node.final_rect = (0.0, 0.0, 100.0, 100.0);
        let id = tree.insert(node);

        let mut scene = scene::SceneGraph::new();
        let root_id = scene.root();
        let mut render_ctx = context::RenderContext::new(1.0);
        build_scene_node(
            &tree, id, &mut scene, root_id, 1.0, None, None, 0.0, 0.0, 1.0,
        );

        let mut pixmap = tiny_skia::Pixmap::new(100, 100).unwrap();
        pixmap.fill(tiny_skia::Color::TRANSPARENT);
        render_scene_node(&scene, root_id, &mut pixmap, &mut render_ctx);

        // Pixmap should remain 100% transparent (no opaque fallback fill drawn anywhere)
        for pixel in pixmap.data().chunks_exact(4) {
            assert_eq!(
                pixel[3], 0,
                "Expected alpha to be 0 for transparent background"
            );
        }
    }

    #[test]
    fn test_pure_text_widget_skips_box_shadow() {
        let mut tree = crate::widgets::WidgetTree::new();
        let mut node = crate::widgets::WidgetNode::new(crate::widgets::WidgetContent::Text {
            text: "Hello World".to_string(),
        });
        node.style.shadow = Some((20.0, 0.5, 0.0, 0.0));
        node.style.shadow_color = Some(tiny_skia::Color::from_rgba8(167, 139, 250, 255));
        node.style.background = crate::widgets::parse_fill("transparent");
        node.final_rect = (0.0, 0.0, 100.0, 30.0);
        let id = tree.insert(node);

        let mut scene = scene::SceneGraph::new();
        let root_id = scene.root();
        build_scene_node(
            &tree, id, &mut scene, root_id, 1.0, None, None, 0.0, 0.0, 1.0,
        );

        // Assert that NO Shadow node exists in the scene graph for this pure text widget
        let has_shadow = scene.diff_against(None).into_iter().any(|node_id| {
            if let Some(n) = scene.get(node_id) {
                matches!(n.kind, scene::SceneNodeKind::Shadow { .. })
            } else {
                false
            }
        });
        assert!(
            !has_shadow,
            "Pure text widget should not generate a box shadow node"
        );
    }

    #[test]
    fn test_progress_ring_props_custom_angle_ccw_no_text() {
        let mut tree = crate::widgets::WidgetTree::new();
        let mut node =
            crate::widgets::WidgetNode::new(crate::widgets::WidgetContent::ProgressRing {
                value: 50.0,
                max: 100.0,
                stroke_width: Some(4.0),
                text: Some("50%".to_string()),
                props: serde_json::json!({
                    "start_angle_deg": 90.0,
                    "direction": "ccw",
                    "show_text": false,
                }),
            });
        node.final_rect = (0.0, 0.0, 100.0, 100.0);
        let id = tree.insert(node);

        let mut scene = scene::SceneGraph::new();
        let root_id = scene.root();
        build_scene_node(
            &tree, id, &mut scene, root_id, 1.0, None, None, 0.0, 0.0, 1.0,
        );

        let nodes: Vec<_> = scene
            .diff_against(None)
            .into_iter()
            .filter_map(|nid| scene.get(nid))
            .collect();
        let has_text = nodes
            .iter()
            .any(|n| matches!(n.kind, scene::SceneNodeKind::Text { .. }));
        assert!(!has_text, "show_text=false should omit the text node");

        let arc_node = nodes
            .iter()
            .find(|n| matches!(n.kind, scene::SceneNodeKind::ProgressArc { .. }))
            .expect("ProgressArc node should exist");
        if let scene::SceneNodeKind::ProgressArc {
            start_angle_deg,
            is_ccw,
            ..
        } = arc_node.kind
        {
            assert_eq!(start_angle_deg, 90.0);
            assert!(is_ccw);
        } else {
            unreachable!();
        }
    }

    #[test]
    fn test_slider_props_no_thumb_no_percent_text() {
        let mut tree = crate::widgets::WidgetTree::new();
        let mut node = crate::widgets::WidgetNode::new(crate::widgets::WidgetContent::Slider {
            value: 40.0,
            min: 0.0,
            max: 100.0,
            props: serde_json::json!({
                "show_thumb": false,
                "show_percent_text": false,
            }),
        });
        node.final_rect = (0.0, 0.0, 200.0, 30.0);
        let id = tree.insert(node);

        let mut scene = scene::SceneGraph::new();
        let root_id = scene.root();
        build_scene_node(
            &tree, id, &mut scene, root_id, 1.0, None, None, 0.0, 0.0, 1.0,
        );

        let nodes: Vec<_> = scene
            .diff_against(None)
            .into_iter()
            .filter_map(|nid| scene.get(nid))
            .collect();
        let has_text = nodes
            .iter()
            .any(|n| matches!(n.kind, scene::SceneNodeKind::Text { .. }));
        assert!(
            !has_text,
            "show_percent_text=false should omit the label text"
        );

        let rect_count = nodes
            .iter()
            .filter(|n| matches!(n.kind, scene::SceneNodeKind::Rect { .. }))
            .count();
        assert_eq!(
            rect_count, 1,
            "Should only have fill Rect when thumb is disabled"
        );
    }

    #[test]
    fn test_slider_props_rtl_direction() {
        let mut tree = crate::widgets::WidgetTree::new();
        let mut node = crate::widgets::WidgetNode::new(crate::widgets::WidgetContent::Slider {
            value: 25.0,
            min: 0.0,
            max: 100.0,
            props: serde_json::json!({
                "direction": "rtl",
                "show_thumb": false,
                "show_percent_text": false,
            }),
        });
        node.final_rect = (10.0, 10.0, 200.0, 30.0);
        let id = tree.insert(node);

        let mut scene = scene::SceneGraph::new();
        let root_id = scene.root();
        build_scene_node(
            &tree, id, &mut scene, root_id, 1.0, None, None, 0.0, 0.0, 1.0,
        );

        let nodes: Vec<_> = scene
            .diff_against(None)
            .into_iter()
            .filter_map(|nid| scene.get(nid))
            .collect();
        let fill_rect = nodes
            .iter()
            .find(|n| matches!(n.kind, scene::SceneNodeKind::Rect { .. }))
            .expect("Fill rect should exist");

        let (bx, by, bw, bh) = fill_rect.bounds;
        assert_eq!(bx, 160.0);
        assert_eq!(by, 10.0);
        assert_eq!(bw, 50.0);
        assert_eq!(bh, 30.0);
    }

    #[test]
    fn test_widget_props_baseline_defaults() {
        let mut tree = crate::widgets::WidgetTree::new();

        // 1. ProgressRing default
        let mut ring =
            crate::widgets::WidgetNode::new(crate::widgets::WidgetContent::ProgressRing {
                value: 75.0,
                max: 100.0,
                stroke_width: None,
                text: None,
                props: serde_json::Value::Null,
            });
        ring.final_rect = (0.0, 0.0, 100.0, 100.0);
        let ring_id = tree.insert(ring);

        let mut ring_scene = scene::SceneGraph::new();
        let r_root = ring_scene.root();
        build_scene_node(
            &tree,
            ring_id,
            &mut ring_scene,
            r_root,
            1.0,
            None,
            None,
            0.0,
            0.0,
            1.0,
        );

        let ring_nodes: Vec<_> = ring_scene
            .diff_against(None)
            .into_iter()
            .filter_map(|nid| ring_scene.get(nid))
            .collect();
        let arc = ring_nodes
            .iter()
            .find(|n| matches!(n.kind, scene::SceneNodeKind::ProgressArc { .. }))
            .unwrap();
        if let scene::SceneNodeKind::ProgressArc {
            start_angle_deg,
            is_ccw,
            ..
        } = arc.kind
        {
            assert_eq!(start_angle_deg, -90.0);
            assert!(!is_ccw);
        } else {
            unreachable!();
        }
        assert!(ring_nodes
            .iter()
            .any(|n| matches!(n.kind, scene::SceneNodeKind::Text { .. })));

        // 2. Slider default
        let mut slider = crate::widgets::WidgetNode::new(crate::widgets::WidgetContent::Slider {
            value: 50.0,
            min: 0.0,
            max: 100.0,
            props: serde_json::Value::Null,
        });
        slider.final_rect = (0.0, 0.0, 100.0, 20.0);
        let slider_id = tree.insert(slider);

        let mut slider_scene = scene::SceneGraph::new();
        let s_root = slider_scene.root();
        build_scene_node(
            &tree,
            slider_id,
            &mut slider_scene,
            s_root,
            1.0,
            None,
            None,
            0.0,
            0.0,
            1.0,
        );

        let slider_nodes: Vec<_> = slider_scene
            .diff_against(None)
            .into_iter()
            .filter_map(|nid| slider_scene.get(nid))
            .collect();
        assert!(slider_nodes
            .iter()
            .any(|n| matches!(n.kind, scene::SceneNodeKind::Text { .. })));
        let slider_rects = slider_nodes
            .iter()
            .filter(|n| matches!(n.kind, scene::SceneNodeKind::Rect { .. }))
            .count();
        assert!(
            slider_rects >= 2,
            "Default slider should have fill rect and thumb rect"
        );

        // 3. Progress default
        let mut progress =
            crate::widgets::WidgetNode::new(crate::widgets::WidgetContent::Progress {
                value: 20.0,
                max: 100.0,
                props: serde_json::Value::Null,
            });
        progress.final_rect = (10.0, 10.0, 100.0, 10.0);
        let progress_id = tree.insert(progress);

        let mut prog_scene = scene::SceneGraph::new();
        let p_root = prog_scene.root();
        build_scene_node(
            &tree,
            progress_id,
            &mut prog_scene,
            p_root,
            1.0,
            None,
            None,
            0.0,
            0.0,
            1.0,
        );

        let prog_nodes: Vec<_> = prog_scene
            .diff_against(None)
            .into_iter()
            .filter_map(|nid| prog_scene.get(nid))
            .collect();
        let fill = prog_nodes
            .iter()
            .find(|n| matches!(n.kind, scene::SceneNodeKind::Rect { .. }))
            .unwrap();
        assert_eq!(fill.bounds.0, 10.0);
        assert_eq!(fill.bounds.2, 20.0);
    }
}
