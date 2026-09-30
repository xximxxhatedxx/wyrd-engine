//! Bridge between the [`WidgetTree`] and the [`crate::render::scene::SceneGraph`].
//!
//! Converts layout-resolved widget trees into backend-agnostic scene nodes and
//! orchestrates surface rendering without introducing a cyclic dependency from
//! `wyrd-graphics` back into `wyrd-widgets`.

use crate::animator::Animator;
use crate::render::{
    backend, context, damage,
    scene::{Fill, OutlineStyle, SceneGraph, SceneNodeId, SceneNodeKind, SceneTransform},
    scene_to_pixmap, text,
};
use crate::widgets::{
    layout::{Align, JustifyContent},
    WidgetContent, WidgetId, WidgetNode, WidgetState, WidgetTree,
};
use log::error;
use std::borrow::Cow;
use std::panic::{catch_unwind, AssertUnwindSafe};
use tiny_skia::{Color, Pixmap};

fn is_node_hovered(tree: &WidgetTree, id: WidgetId, hovered_widget: Option<WidgetId>) -> bool {
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

fn is_node_focused(tree: &WidgetTree, id: WidgetId, focused_widget: Option<WidgetId>) -> bool {
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

fn draw_transformed_surface(
    pixmap: &mut Pixmap,
    raw_pixmap: &Pixmap,
    size: (u32, u32),
    scale: f64,
    surface_opacity: f32,
    surface_scale: f32,
    offset_origin: (f32, f32, i32),
) {
    let (phys_w, phys_h) = size;
    let (surf_ox, surf_oy, surf_origin) = offset_origin;
    let (pivot_x, pivot_y) = match surf_origin {
        1 => (phys_w as f32 * 0.5, phys_h as f32 * 0.92),
        2 => (phys_w as f32 * 0.08, phys_h as f32 * 0.5),
        _ => (phys_w as f32 * 0.5, phys_h as f32 * 0.08),
    };
    let tx = surf_ox * scale as f32 * 0.45;
    let ty = surf_oy * scale as f32 * 0.45;
    let transform = tiny_skia::Transform::from_translate(-pivot_x, -pivot_y)
        .post_scale(surface_scale, surface_scale)
        .post_translate(pivot_x + tx, pivot_y + ty);
    let paint = tiny_skia::PixmapPaint {
        opacity: surface_opacity,
        blend_mode: tiny_skia::BlendMode::SourceOver,
        quality: tiny_skia::FilterQuality::Nearest,
    };
    pixmap.draw_pixmap(0, 0, raw_pixmap.as_ref(), &paint, transform, None);
}

fn execute_scene_render(scene: &SceneGraph, pixmap: &mut Pixmap, ctx: &mut context::RenderContext) {
    match ctx.render_mode {
        backend::RenderMode::Gpu => {
            if let Some(ref gpu) = ctx.gpu {
                let _ = gpu.render_scene(scene);
            }
            scene_to_pixmap(scene, pixmap, ctx);
        }
        backend::RenderMode::Cpu | backend::RenderMode::Auto => {
            scene_to_pixmap(scene, pixmap, ctx);
        }
    }
}

/// Renders a widget tree into a Wayland surface [`Pixmap`] and updates its [`damage::DamageTracker`].
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
    hovered_widget: Option<WidgetId>,
    focused_widget: Option<WidgetId>,
    anim_progress: f64,
) -> Option<(usize, Pixmap, damage::DamageTracker)> {
    let image_ready = ctx.image_fetcher.is_completed();
    if !dirty && !ctx.animator.has_active() && !image_ready {
        return None;
    }
    if image_ready {
        ctx.cached_surface_pixmaps.remove(&surface_index);
        ctx.prev_scenes.remove(&surface_index);
    }

    damage.clear();
    let phys_w = (width as f64 * scale).round().max(1.0) as u32;
    let phys_h = (height as f64 * scale).round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(phys_w, phys_h)?;
    pixmap.fill(Color::TRANSPARENT);

    let surface_opacity = anim_progress.clamp(0.0, 1.0) as f32;
    let surface_scale = ctx
        .animator
        .get_named(&format!("surf_scale_{surface_index}"))
        .unwrap_or(1.0)
        .clamp(0.5, 1.2) as f32;
    let surf_ox = ctx
        .animator
        .get_named(&format!("surf_ox_{surface_index}"))
        .unwrap_or(0.0) as f32;
    let surf_oy = ctx
        .animator
        .get_named(&format!("surf_oy_{surface_index}"))
        .unwrap_or(0.0) as f32;
    let surf_origin = ctx
        .animator
        .get_named(&format!("surf_origin_{surface_index}"))
        .unwrap_or(0.0) as i32;

    let has_surface_transform = surface_opacity < 0.999
        || (surface_scale - 1.0).abs() > 0.001
        || surf_ox.abs() > 0.05
        || surf_oy.abs() > 0.05;
    let has_widget_anims = ctx
        .animator
        .has_active_for_prefix(&format!("s{surface_index}_"));

    if has_surface_transform
        && !tree.is_dirty()
        && !has_widget_anims
        && ctx
            .cached_surface_pixmaps
            .get(&surface_index)
            .is_some_and(|(cw, ch, _)| *cw == phys_w && *ch == phys_h)
    {
        if surface_opacity > 0.002 {
            if let Some((_, _, ref raw_pixmap)) = ctx.cached_surface_pixmaps.get(&surface_index) {
                draw_transformed_surface(
                    &mut pixmap,
                    raw_pixmap,
                    (phys_w, phys_h),
                    scale,
                    surface_opacity,
                    surface_scale,
                    (surf_ox, surf_oy, surf_origin),
                );
            }
        }
        damage.set_full_redraw();
        damage.optimize();
        return Some((surface_index, pixmap, damage.clone()));
    }

    let mut scene = SceneGraph::new();
    let root_scene_id = scene.root();
    if let Some(root) = tree.root() {
        build_scene_node_with_anim(
            tree,
            root,
            &mut scene,
            root_scene_id,
            scale,
            hovered_widget,
            focused_widget,
            0.0,
            0.0,
            1.0,
            Some(surface_index),
            Some(&mut ctx.animator),
        );
    }

    let prev_scene = ctx.prev_scenes.get(&surface_index);
    let scene_unchanged = scene.diff_against(prev_scene).is_empty();
    *damage = damage::DamageTracker::from_scene(&scene);

    if has_surface_transform {
        if surface_opacity > 0.002 {
            let needs_rerender = !scene_unchanged
                || !ctx
                    .cached_surface_pixmaps
                    .get(&surface_index)
                    .is_some_and(|(cw, ch, _)| *cw == phys_w && *ch == phys_h);
            if needs_rerender {
                if let Some(mut raw_pixmap) = Pixmap::new(phys_w, phys_h) {
                    raw_pixmap.fill(Color::TRANSPARENT);
                    execute_scene_render(&scene, &mut raw_pixmap, ctx);
                    ctx.cached_surface_pixmaps
                        .insert(surface_index, (phys_w, phys_h, raw_pixmap));
                }
            }
            if let Some((_, _, ref raw_pixmap)) = ctx.cached_surface_pixmaps.get(&surface_index) {
                draw_transformed_surface(
                    &mut pixmap,
                    raw_pixmap,
                    (phys_w, phys_h),
                    scale,
                    surface_opacity,
                    surface_scale,
                    (surf_ox, surf_oy, surf_origin),
                );
            }
        }
        damage.set_full_redraw();
    } else if let Some((cw, ch, cached_raw)) = ctx.cached_surface_pixmaps.remove(&surface_index) {
        if scene_unchanged && cw == phys_w && ch == phys_h {
            pixmap = cached_raw;
            damage.set_full_redraw();
        } else {
            execute_scene_render(&scene, &mut pixmap, ctx);
        }
    } else {
        execute_scene_render(&scene, &mut pixmap, ctx);
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
            Color::from_rgba8(255, 220, 120, 255),
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

/// Recursively builds a [`SceneGraph`] subtree from a [`WidgetTree`] node without animation state.
#[allow(clippy::too_many_arguments)]
pub fn build_scene_node(
    tree: &WidgetTree,
    id: WidgetId,
    scene: &mut SceneGraph,
    parent_scene_id: SceneNodeId,
    scale: f64,
    hovered_widget: Option<WidgetId>,
    focused_widget: Option<WidgetId>,
    offset_x: f32,
    offset_y: f32,
    surface_opacity: f32,
) {
    build_scene_node_with_anim(
        tree,
        id,
        scene,
        parent_scene_id,
        scale,
        hovered_widget,
        focused_widget,
        offset_x,
        offset_y,
        surface_opacity,
        None,
        None,
    );
}

#[derive(Clone, Copy)]
struct ResolvedNodeVisuals {
    id: WidgetId,
    widget_scene_id: SceneNodeId,
    bounds: (f32, f32, f32, f32),
    inner_bounds: (f32, f32, f32, f32),
    scale: f32,
    scaled_radius: f32,
    foreground: Option<Color>,
    accent: Option<Color>,
    hover_t: f32,
    is_focused: bool,
    transform: SceneTransform,
}

#[allow(clippy::too_many_arguments)]
fn animate_widget_colors(
    node: &WidgetNode,
    wkey: &Option<String>,
    animator: &mut Option<&mut Animator>,
    is_interactive: bool,
    is_hovered: bool,
    is_focused: bool,
    background: &mut Option<Fill>,
    foreground: &mut Option<Color>,
    outline_color: &mut Option<Color>,
) -> f32 {
    let mut hover_t = if is_hovered || is_focused { 1.0 } else { 0.0 };
    let (Some(ref key), Some(ref mut anim)) = (wkey, animator) else {
        return hover_t;
    };
    if is_interactive {
        let target_hov = if is_hovered || is_focused { 1.0 } else { 0.0 };
        hover_t = anim
            .animate_value(&format!("{key}_hov"), target_hov, 380.0, 28.0, None)
            .clamp(0.0, 1.0) as f32;
    }
    if is_interactive || node.id.is_some() {
        let has_solid_bg = matches!(background, Some(Fill::Solid(_)))
            || node
                .style
                .hover
                .as_ref()
                .and_then(|h| h.background.as_ref())
                .is_some_and(|f| matches!(f, Fill::Solid(_)));
        if has_solid_bg {
            let target_bg = match background {
                Some(Fill::Solid(c)) => [c.red(), c.green(), c.blue(), c.alpha()],
                _ => [0.0, 0.0, 0.0, 0.0],
            };
            let [r, g, b, a] = anim.animate_color(&format!("{key}_bg"), target_bg, 360.0, 28.0);
            if a > 0.002 {
                *background = Color::from_rgba(r, g, b, a).map(Fill::Solid);
            } else if matches!(background, Some(Fill::Solid(_))) {
                *background = None;
            }
        }
        if let Some(fg) = *foreground {
            let [r, g, b, a] = anim.animate_color(
                &format!("{key}_fg"),
                [fg.red(), fg.green(), fg.blue(), fg.alpha()],
                360.0,
                28.0,
            );
            *foreground = Color::from_rgba(r, g, b, a);
        }
        if outline_color.is_some()
            || node
                .style
                .hover
                .as_ref()
                .and_then(|h| h.outline_color)
                .is_some()
        {
            let target_ol = outline_color
                .map(|c| [c.red(), c.green(), c.blue(), c.alpha()])
                .unwrap_or([0.0, 0.0, 0.0, 0.0]);
            let [r, g, b, a] = anim.animate_color(&format!("{key}_ol"), target_ol, 360.0, 28.0);
            *outline_color = if a > 0.002 {
                Color::from_rgba(r, g, b, a)
            } else {
                None
            };
        }
    }
    hover_t
}

fn build_progress_scene(
    scene: &mut SceneGraph,
    vis: &ResolvedNodeVisuals,
    wkey: &Option<String>,
    animator: &mut Option<&mut Animator>,
    value: f32,
    max: f32,
    props: &serde_json::Value,
) {
    let (x, y, width, height) = vis.bounds;
    let direction = props
        .get("direction")
        .and_then(|v| v.as_str())
        .unwrap_or("ltr");
    let fill_radius = props
        .get("fill_radius")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);
    let raw_ratio = (value / max.max(0.001)).clamp(0.0, 1.0);
    let ratio = if let (Some(ref key), Some(ref mut anim)) = (wkey, animator) {
        anim.animate_value(
            &format!("{key}_val"),
            raw_ratio as f64,
            240.0,
            24.0,
            Some(0.0),
        )
        .clamp(0.0, 1.0) as f32
    } else {
        raw_ratio
    };
    let fill_color = vis.accent.or(vis.foreground).unwrap_or(Color::WHITE);
    let (fill_x, fill_y, fill_w, fill_h) = match direction {
        "rtl" => {
            let fw = (width * ratio).max(0.0);
            (x + width - fw, y, fw, height)
        }
        "ttb" => (x, y, width, (height * ratio).max(0.0)),
        "btt" => {
            let fh = (height * ratio).max(0.0);
            (x, y + height - fh, width, fh)
        }
        _ => (x, y, (width * ratio).max(0.0), height),
    };
    if fill_w > 0.0 && fill_h > 0.0 {
        let r = fill_radius.unwrap_or_else(|| vis.scaled_radius.min(fill_w.min(fill_h) / 2.0));
        scene.insert(
            vis.widget_scene_id,
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(fill_color)),
                radius: r,
            },
            (fill_x, fill_y, fill_w, fill_h),
            Some(vis.id),
            vis.transform,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn build_progress_ring_scene(
    node: &WidgetNode,
    scene: &mut SceneGraph,
    vis: &ResolvedNodeVisuals,
    wkey: &Option<String>,
    animator: &mut Option<&mut Animator>,
    value: f32,
    max: f32,
    stroke_width: Option<f32>,
    text: &Option<String>,
    props: &serde_json::Value,
) {
    let (x, y, width, height) = vis.bounds;
    let sw_prop = props
        .get("track_thickness")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);
    let sw = (sw_prop.or(stroke_width).unwrap_or(7.0) * vis.scale).max(2.0);
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

    let radius = (width.min(height) - sw) / 2.0;
    if radius <= 1.0 {
        return;
    }
    let track_color = node.style.background.as_ref().and_then(|f| match f {
        Fill::Solid(c) => Some(*c),
        Fill::LinearGradient { stops, .. } | Fill::RadialGradient { stops, .. } => {
            stops.first().map(|s| s.1)
        }
    });
    let raw_ratio = (value / max.max(0.001)).clamp(0.0, 1.0);
    let ratio = if let (Some(ref key), Some(ref mut anim)) = (wkey, animator) {
        anim.animate_value(
            &format!("{key}_val"),
            raw_ratio as f64,
            220.0,
            24.0,
            Some(0.0),
        )
        .clamp(0.0, 1.0) as f32
    } else {
        raw_ratio
    };
    let arc_color = if ratio > 0.002 {
        vis.accent.or(vis.foreground).or(Some(Color::WHITE))
    } else {
        None
    };
    scene.insert(
        vis.widget_scene_id,
        SceneNodeKind::ProgressArc {
            cx: x + width / 2.0,
            cy: y + height / 2.0,
            radius,
            stroke_width: sw,
            ratio,
            track_color,
            outline_color: node.style.outline_color,
            outline_width: (node.style.outline_width * vis.scale).max(1.0),
            arc_color,
            start_angle_deg,
            is_ccw: direction == "ccw",
        },
        vis.bounds,
        Some(vis.id),
        vis.transform,
    );
    if show_text {
        let label = prop_text
            .or_else(|| text.clone())
            .unwrap_or_else(|| format!("{value:.0}%"));
        if !label.is_empty() {
            let font_size = font_size_prop
                .map(|fs| fs * vis.scale)
                .or_else(|| {
                    (node.style.font_size > 0.0).then_some(node.style.font_size * vis.scale)
                })
                .unwrap_or_else(|| (radius * 0.52).clamp(10.0, 18.0) * vis.scale);
            scene.insert(
                vis.widget_scene_id,
                SceneNodeKind::Text {
                    text: label,
                    font_family: node.style.font_family.clone(),
                    font_size,
                    color: vis.foreground.unwrap_or(Color::WHITE),
                    align: cosmic_text::Align::Center,
                },
                vis.bounds,
                Some(vis.id),
                vis.transform,
            );
        }
    }
}

fn build_slider_thumb_scene(
    node: &WidgetNode,
    scene: &mut SceneGraph,
    vis: &ResolvedNodeVisuals,
    direction: &str,
    fill_bounds: (f32, f32, f32, f32),
    thumb_radius_prop: Option<f32>,
) {
    let (x, y, width, height) = vis.bounds;
    let (fill_x, fill_y, fill_w, fill_h) = fill_bounds;
    let base_thumb_r = thumb_radius_prop.unwrap_or_else(|| (height * 0.40).clamp(5.0, 9.0));
    let thumb_radius = base_thumb_r * (1.0 + 0.18 * vis.hover_t);
    let (thumb_cx, thumb_cy) = match direction {
        "rtl" => (
            fill_x.clamp(x + thumb_radius, x + width - thumb_radius),
            y + height / 2.0,
        ),
        "ttb" => (
            x + width / 2.0,
            (y + fill_h).clamp(y + thumb_radius, y + height - thumb_radius),
        ),
        "btt" => (
            x + width / 2.0,
            fill_y.clamp(y + thumb_radius, y + height - thumb_radius),
        ),
        _ => (
            (x + fill_w).clamp(x + thumb_radius, x + width - thumb_radius),
            y + height / 2.0,
        ),
    };
    if vis.hover_t > 0.01 {
        let halo_r = thumb_radius * (1.45 + 0.25 * vis.hover_t);
        let halo_base = vis.accent.or(vis.foreground).unwrap_or(Color::WHITE);
        if let Some(halo_col) = Color::from_rgba(
            halo_base.red(),
            halo_base.green(),
            halo_base.blue(),
            (0.22 * vis.hover_t).clamp(0.0, 1.0),
        ) {
            scene.insert(
                vis.widget_scene_id,
                SceneNodeKind::Rect {
                    fill: Some(Fill::Solid(halo_col)),
                    radius: halo_r,
                },
                (
                    thumb_cx - halo_r,
                    thumb_cy - halo_r,
                    halo_r * 2.0,
                    halo_r * 2.0,
                ),
                Some(vis.id),
                vis.transform,
            );
        }
    }
    let thumb_bounds = (
        thumb_cx - thumb_radius,
        thumb_cy - thumb_radius,
        thumb_radius * 2.0,
        thumb_radius * 2.0,
    );
    let thumb_color = vis.foreground.or(vis.accent).unwrap_or(Color::WHITE);
    scene.insert(
        vis.widget_scene_id,
        SceneNodeKind::Rect {
            fill: Some(Fill::Solid(thumb_color)),
            radius: thumb_radius,
        },
        thumb_bounds,
        Some(vis.id),
        vis.transform,
    );
    if let Some(outline_color) = node.style.outline_color.or(vis.accent) {
        scene.insert(
            vis.widget_scene_id,
            SceneNodeKind::Outline {
                color: outline_color,
                width: 1.5 * vis.scale,
                style: OutlineStyle::Solid,
                radius: thumb_radius,
            },
            thumb_bounds,
            Some(vis.id),
            vis.transform,
        );
    }
}

fn build_slider_scene(
    node: &WidgetNode,
    scene: &mut SceneGraph,
    vis: &ResolvedNodeVisuals,
    value: f32,
    min: f32,
    max: f32,
    props: &serde_json::Value,
) {
    let (x, y, width, height) = vis.bounds;
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
        .or_else(|| props.get("show_value"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let show_tick = props
        .get("show_tick")
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

    let ratio = ((value - min) / (max - min).max(0.001)).clamp(0.0, 1.0);
    let (fill_x, fill_y, fill_w, fill_h) = match direction {
        "rtl" => {
            let fw = (width * ratio).max(0.0);
            (x + width - fw, y, fw, height)
        }
        "ttb" => (x, y, width, (height * ratio).max(0.0)),
        "btt" => {
            let fh = (height * ratio).max(0.0);
            (x, y + height - fh, width, fh)
        }
        _ => (x, y, (width * ratio).max(0.0), height),
    };

    if fill_w > 0.0 && fill_h > 0.0 {
        let fill_color = vis.accent.or(vis.foreground).unwrap_or(Color::WHITE);
        let r = fill_radius_prop
            .unwrap_or_else(|| vis.scaled_radius.min(fill_w.min(fill_h) / 2.0).max(4.0));
        scene.insert(
            vis.widget_scene_id,
            SceneNodeKind::Rect {
                fill: Some(Fill::Solid(fill_color)),
                radius: r,
            },
            (fill_x, fill_y, fill_w, fill_h),
            Some(vis.id),
            vis.transform,
        );
    }

    if show_tick {
        if let Some(tick_val) = tick_at_prop.filter(|&tv| tv >= min && tv <= max && max > min) {
            let ratio_tick = (tick_val - min) / (max - min);
            let tick_x = if direction == "rtl" {
                x + width * (1.0 - ratio_tick)
            } else {
                x + width * ratio_tick
            };
            let mut tick_pb = tiny_skia::PathBuilder::new();
            tick_pb.move_to(tick_x, y + 1.0 * vis.scale);
            tick_pb.line_to(tick_x, y + height - 1.0 * vis.scale);
            if let Some(path) = tick_pb.finish() {
                scene.insert(
                    vis.widget_scene_id,
                    SceneNodeKind::CustomPath {
                        color: vis.foreground.unwrap_or(Color::WHITE),
                        stroke_width: 1.5 * vis.scale,
                        path,
                    },
                    vis.bounds,
                    Some(vis.id),
                    vis.transform,
                );
            }
        }
    }

    if show_percent_text {
        let pct_text = if percent_text_format.contains("{}") {
            percent_text_format.replace("{}", &format!("{value:.0}"))
        } else {
            format!("{value:.0}%")
        };
        scene.insert(
            vis.widget_scene_id,
            SceneNodeKind::Text {
                text: pct_text,
                font_family: node.style.font_family.clone(),
                font_size: (height * 0.52).clamp(9.0, 12.0) * vis.scale,
                color: vis.foreground.unwrap_or(Color::WHITE),
                align: cosmic_text::Align::Center,
            },
            vis.bounds,
            Some(vis.id),
            vis.transform,
        );
    }

    if show_thumb {
        build_slider_thumb_scene(
            node,
            scene,
            vis,
            direction,
            (fill_x, fill_y, fill_w, fill_h),
            thumb_radius_prop,
        );
    }
}

fn build_text_or_input_scene(node: &WidgetNode, scene: &mut SceneGraph, vis: &ResolvedNodeVisuals) {
    let fg_default = vis.foreground.unwrap_or(Color::WHITE);
    let (text, text_color): (Option<Cow<'_, str>>, Color) = match &node.content {
        WidgetContent::Text { text } => (Some(Cow::Borrowed(text.as_str())), fg_default),
        WidgetContent::Button { label, .. } => (Some(Cow::Borrowed(label.as_str())), fg_default),
        WidgetContent::TextInput {
            text,
            placeholder,
            focused,
            cursor_pos,
            ..
        } => {
            let active_focus = *focused || vis.is_focused;
            if text.is_empty() {
                let ph = if placeholder.is_empty() || active_focus {
                    "|"
                } else {
                    placeholder.as_str()
                };
                let alpha_scale = if active_focus { 0.7 } else { 0.5 };
                let fallback = if active_focus {
                    Color::from_rgba8(180, 175, 195, 180)
                } else {
                    Color::from_rgba8(140, 135, 150, 140)
                };
                let color = vis
                    .foreground
                    .and_then(|c| {
                        Color::from_rgba(
                            c.red(),
                            c.green(),
                            c.blue(),
                            (c.alpha() * alpha_scale).clamp(0.1, 1.0),
                        )
                    })
                    .unwrap_or(fallback);
                (Some(Cow::Borrowed(ph)), color)
            } else if active_focus {
                let mut display = text.clone();
                let pos = (*cursor_pos).min(display.len());
                let safe_pos = if display.is_char_boundary(pos) {
                    pos
                } else {
                    display.len()
                };
                display.insert(safe_pos, '|');
                (Some(Cow::Owned(display)), fg_default)
            } else {
                (Some(Cow::Borrowed(text.as_str())), fg_default)
            }
        }
        WidgetContent::Module { payload, .. } if node.children.is_empty() => (
            payload
                .get("text")
                .and_then(|v| v.as_str())
                .map(Cow::Borrowed),
            fg_default,
        ),
        _ => (None, fg_default),
    };

    if let Some(text) = text {
        let width = vis.bounds.2;
        let align = match node.layout.justify {
            JustifyContent::Center => cosmic_text::Align::Center,
            JustifyContent::End => cosmic_text::Align::Right,
            JustifyContent::Start => match &node.content {
                WidgetContent::Button { .. } => {
                    if width > 180.0
                        && node.layout.fixed_width.is_none()
                        && node.layout.weight == 0.0
                    {
                        cosmic_text::Align::Left
                    } else {
                        cosmic_text::Align::Center
                    }
                }
                _ => match node.layout.align {
                    Align::Center => cosmic_text::Align::Center,
                    _ => cosmic_text::Align::Left,
                },
            },
            _ => cosmic_text::Align::Left,
        };
        scene.insert(
            vis.widget_scene_id,
            SceneNodeKind::Text {
                text: text.into_owned(),
                font_family: node.style.font_family.clone(),
                font_size: node.style.font_size.max(10.0) * vis.scale,
                color: text_color,
                align,
            },
            vis.inner_bounds,
            Some(vis.id),
            vis.transform,
        );
    }
}

fn build_scrollbar_scene(
    tree: &WidgetTree,
    scroll_node: &WidgetNode,
    scene: &mut SceneGraph,
    vis: &ResolvedNodeVisuals,
) {
    if !scroll_node.layout.scroll_y || scroll_node.children.is_empty() {
        return;
    }
    let (x, y, width, height) = vis.bounds;
    let (raw_pt, _, raw_pb, _) = if scroll_node.layout.padding != (0.0, 0.0, 0.0, 0.0) {
        scroll_node.layout.padding
    } else {
        scroll_node.style.padding
    };
    let raw_children_h: f32 = scroll_node
        .children
        .iter()
        .filter_map(|cid| tree.get(*cid))
        .map(|c| {
            c.layout
                .fixed_height
                .unwrap_or(c.measured_size.1.max(c.final_rect.3))
        })
        .sum::<f32>()
        + scroll_node.layout.gap * (scroll_node.children.len().saturating_sub(1) as f32)
        + raw_pt
        + raw_pb;
    let raw_viewport_h = scroll_node.final_rect.3;
    if raw_children_h > raw_viewport_h + 1.0 && raw_viewport_h > 12.0 {
        let max_scroll = (raw_children_h - raw_viewport_h).max(1.0);
        let scroll_ratio = (scroll_node.layout.scroll_offset_y / max_scroll).clamp(0.0, 1.0);
        let track_pad = 4.0 * vis.scale;
        let track_h = (height - track_pad * 2.0).max(8.0);
        let thumb_h =
            ((raw_viewport_h / raw_children_h) * track_h).clamp(16.0 * vis.scale, track_h);
        let thumb_w = 3.5 * vis.scale;
        let thumb_x = x + width - thumb_w - 3.0 * vis.scale;
        let thumb_y = y + track_pad + (track_h - thumb_h) * scroll_ratio;
        let base_col = vis.accent.or(vis.foreground).unwrap_or(Color::WHITE);
        if let Some(thumb_col) =
            Color::from_rgba(base_col.red(), base_col.green(), base_col.blue(), 0.45)
        {
            scene.insert(
                vis.widget_scene_id,
                SceneNodeKind::Rect {
                    fill: Some(Fill::Solid(thumb_col)),
                    radius: thumb_w * 0.5,
                },
                (thumb_x, thumb_y, thumb_w, thumb_h),
                Some(vis.id),
                vis.transform,
            );
        }
    }
}

/// Recursively builds a [`SceneGraph`] subtree from a [`WidgetTree`] node with optional spring animations.
#[allow(clippy::too_many_arguments)]
pub fn build_scene_node_with_anim(
    tree: &WidgetTree,
    id: WidgetId,
    scene: &mut SceneGraph,
    parent_scene_id: SceneNodeId,
    scale: f64,
    hovered_widget: Option<WidgetId>,
    focused_widget: Option<WidgetId>,
    offset_x: f32,
    offset_y: f32,
    surface_opacity: f32,
    surface_index: Option<usize>,
    mut animator: Option<&mut Animator>,
) {
    let saved_children_len = scene
        .get(parent_scene_id)
        .map(|n| n.children.len())
        .unwrap_or(0);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let Some(node) = tree.get(id) else { return };
        if node.failed {
            return;
        }
        let is_focused = is_node_focused(tree, id, focused_widget);
        let is_hovered = is_node_hovered(tree, id, hovered_widget);
        let state = if is_focused {
            WidgetState::Focus
        } else if is_hovered {
            WidgetState::Hover
        } else {
            WidgetState::Normal
        };
        let (
            mut background,
            mut foreground,
            accent,
            base_opacity,
            mut outline_color,
            mut outline_width,
            outline_style,
            shadow,
            shadow_color,
        ) = node.style.for_state(state);
        let opacity = (base_opacity.clamp(0.0, 1.0) * surface_opacity).clamp(0.0, 1.0);
        let (mut x, mut y, mut width, mut height) = node.final_rect;

        let wkey = surface_index.map(|sidx| {
            if let Some(ref wid) = node.id {
                format!("s{sidx}_{wid}")
            } else {
                format!(
                    "s{sidx}_{}_{}_{}_{}",
                    x.round() as i32,
                    y.round() as i32,
                    width.round() as i32,
                    height.round() as i32
                )
            }
        });

        let is_interactive = node.style.hover.is_some()
            || node.on_click.is_some()
            || matches!(
                node.content,
                WidgetContent::Button { .. } | WidgetContent::Slider { .. }
            );

        if is_focused && is_interactive {
            outline_width = outline_width.max(1.5);
            if outline_color.is_none() {
                outline_color = accent
                    .or(foreground)
                    .or(Some(Color::from_rgba8(137, 180, 250, 225)));
            }
            if background.is_none() {
                background = Some(Fill::Solid(Color::from_rgba8(255, 255, 255, 22)));
            }
        }

        let hover_t = animate_widget_colors(
            node,
            &wkey,
            &mut animator,
            is_interactive,
            is_hovered,
            is_focused,
            &mut background,
            &mut foreground,
            &mut outline_color,
        );

        if let Some(t) = node.layout.transform.as_ref() {
            let sx = t.scale_x.unwrap_or(1.0);
            let sy = t.scale_y.unwrap_or(1.0);
            let new_w = width * sx;
            let new_h = height * sy;
            x += t.translate_x.unwrap_or(0.0) - (new_w - width) * 0.5;
            y += t.translate_y.unwrap_or(0.0) - (new_h - height) * 0.5;
            width = new_w;
            height = new_h;
        }

        let scale_f = scale as f32;
        let (x, y, width, height) = (
            x * scale_f + offset_x,
            y * scale_f + offset_y,
            width * scale_f,
            height * scale_f,
        );
        if width <= 0.0 || height <= 0.0 {
            return;
        }

        let bounds = (x, y, width, height);
        let scaled_radius = node.style.radius * scale_f;
        let mut matrix = glam::Mat4::IDENTITY;
        if let Some(t) = node.layout.transform.as_ref() {
            if let Some(tx) = t.translate_x {
                matrix *= glam::Mat4::from_translation(glam::Vec3::new(tx * scale_f, 0.0, 0.0));
            }
            if let Some(ty) = t.translate_y {
                matrix *= glam::Mat4::from_translation(glam::Vec3::new(0.0, ty * scale_f, 0.0));
            }
            if let Some(rot) = t.rotate {
                matrix *= glam::Mat4::from_rotation_z(rot.to_radians());
            }
            let sx = t.scale_x.unwrap_or(1.0);
            let sy = t.scale_y.unwrap_or(1.0);
            if (sx - 1.0).abs() > f32::EPSILON || (sy - 1.0).abs() > f32::EPSILON {
                matrix *= glam::Mat4::from_scale(glam::Vec3::new(sx, sy, 1.0));
            }
        }
        let transform = SceneTransform { matrix, opacity };
        let widget_scene_id = scene
            .insert(
                parent_scene_id,
                SceneNodeKind::Container,
                bounds,
                Some(id),
                transform,
            )
            .unwrap_or(parent_scene_id);

        if node.layout.clip || node.layout.scroll_y {
            scene.set_clip_children(widget_scene_id, true);
        }

        let has_visible_bg = match &background {
            Some(Fill::Solid(c)) => c.alpha() > 0.001,
            Some(Fill::LinearGradient { stops, .. }) | Some(Fill::RadialGradient { stops, .. }) => {
                stops.iter().any(|(_, c)| c.alpha() > 0.001)
            }
            None => false,
        };
        let has_visible_outline =
            outline_color.is_some_and(|c| c.alpha() > 0.001 && outline_width > 0.0);
        let is_pure_text = matches!(node.content, WidgetContent::Text { .. })
            && !has_visible_bg
            && !has_visible_outline;

        if !is_pure_text {
            if let Some((s_radius, s_opacity, s_ox, s_oy)) = shadow {
                if s_opacity > 0.0 && s_radius > 0.0 {
                    scene.insert(
                        widget_scene_id,
                        SceneNodeKind::Shadow {
                            radius: s_radius * scale_f,
                            opacity: s_opacity,
                            offset_x: s_ox * scale_f,
                            offset_y: s_oy * scale_f,
                            color: shadow_color
                                .or(node.style.shadow_color)
                                .unwrap_or(Color::BLACK),
                            corner_radius: scaled_radius,
                        },
                        bounds,
                        Some(id),
                        transform,
                    );
                }
            }
        }

        if !matches!(node.content, WidgetContent::ProgressRing { .. }) {
            if let Some(fill) = background {
                scene.insert(
                    widget_scene_id,
                    SceneNodeKind::Rect {
                        fill: Some(fill),
                        radius: scaled_radius,
                    },
                    bounds,
                    Some(id),
                    transform,
                );
            }
            if let Some(outline_col) = outline_color {
                scene.insert(
                    widget_scene_id,
                    SceneNodeKind::Outline {
                        color: outline_col,
                        width: (outline_width * scale_f).max(1.0),
                        style: outline_style,
                        radius: scaled_radius,
                    },
                    bounds,
                    Some(id),
                    transform,
                );
            }
        }

        let (pt, pr, pb, pl) = if node.layout.padding != (0.0, 0.0, 0.0, 0.0) {
            node.layout.padding
        } else {
            node.style.padding
        };
        let (mut pt, mut pr, mut pb, mut pl) =
            (pt * scale_f, pr * scale_f, pb * scale_f, pl * scale_f);
        let min_text_h = (node.style.font_size.max(10.0) * 1.35 * scale_f).min(height);
        if matches!(node.content, WidgetContent::Button { .. }) && (pl + pr > width * 0.24) {
            let pad = (width * 0.08).floor();
            pl = pad;
            pr = pad;
        } else if pl + pr >= width {
            pl = 0.0;
            pr = 0.0;
        }
        if height - (pt + pb) < min_text_h {
            let rem = ((height - min_text_h).max(0.0)) / 2.0;
            pt = rem;
            pb = rem;
        }
        let inner_bounds = (
            x + pl,
            y + pt,
            (width - pl - pr).max(1.0),
            (height - pt - pb).max(1.0),
        );

        let vis = ResolvedNodeVisuals {
            id,
            widget_scene_id,
            bounds,
            inner_bounds,
            scale: scale_f,
            scaled_radius,
            foreground,
            accent,
            hover_t,
            is_focused,
            transform,
        };

        match &node.content {
            WidgetContent::Progress { value, max, props } => {
                build_progress_scene(scene, &vis, &wkey, &mut animator, *value, *max, props);
            }
            WidgetContent::ProgressRing {
                value,
                max,
                stroke_width,
                text,
                props,
            } => {
                build_progress_ring_scene(
                    node,
                    scene,
                    &vis,
                    &wkey,
                    &mut animator,
                    *value,
                    *max,
                    *stroke_width,
                    text,
                    props,
                );
            }
            WidgetContent::Slider {
                value,
                min,
                max,
                props,
            } => {
                build_slider_scene(node, scene, &vis, *value, *min, *max, props);
            }
            WidgetContent::Image { path } => {
                scene.insert(
                    widget_scene_id,
                    SceneNodeKind::Image {
                        path: path.clone(),
                        radius: scaled_radius.max(0.0),
                    },
                    inner_bounds,
                    Some(id),
                    transform,
                );
            }
            WidgetContent::Svg { path } => {
                scene.insert(
                    widget_scene_id,
                    SceneNodeKind::Svg { path: path.clone() },
                    inner_bounds,
                    Some(id),
                    transform,
                );
            }
            _ => {}
        }

        build_text_or_input_scene(node, scene, &vis);

        let mut children: Vec<WidgetId> =
            tree.get(id).map(|n| n.children.clone()).unwrap_or_default();
        children.sort_by_key(|child_id| {
            tree.get(*child_id)
                .and_then(|child| child.layout.z_index)
                .unwrap_or(0)
        });
        for child in &children {
            build_scene_node_with_anim(
                tree,
                *child,
                scene,
                widget_scene_id,
                scale,
                hovered_widget,
                focused_widget,
                offset_x,
                offset_y,
                opacity,
                surface_index,
                animator.as_deref_mut(),
            );
        }

        if let Some(scroll_node) = tree.get(id) {
            build_scrollbar_scene(tree, scroll_node, scene, &vis);
        }
    }));

    if result.is_err() {
        scene.truncate_children(parent_scene_id, saved_children_len);
        error!("Widget {:?} panicked during scene building", id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::{parse_fill, WidgetNode, WidgetStyle};

    #[test]
    fn test_widget_style_default_opacity_is_one() {
        let style = WidgetStyle::default();
        assert_eq!(style.opacity, 1.0);
    }

    #[test]
    fn test_transparent_background_renders_invisible_and_does_not_fallback() {
        let mut tree = WidgetTree::new();
        let mut node = WidgetNode::new(WidgetContent::Container);
        node.style.background = parse_fill("transparent");
        node.final_rect = (0.0, 0.0, 100.0, 100.0);
        let id = tree.insert(node);

        let mut scene = SceneGraph::new();
        let root_id = scene.root();
        let mut render_ctx = context::RenderContext::new(1.0);
        build_scene_node(
            &tree, id, &mut scene, root_id, 1.0, None, None, 0.0, 0.0, 1.0,
        );

        let mut pixmap = Pixmap::new(100, 100).unwrap();
        pixmap.fill(Color::TRANSPARENT);
        scene_to_pixmap(&scene, &mut pixmap, &mut render_ctx);

        for pixel in pixmap.data().chunks_exact(4) {
            assert_eq!(
                pixel[3], 0,
                "Expected alpha to be 0 for transparent background"
            );
        }
    }

    #[test]
    fn test_pure_text_widget_skips_box_shadow() {
        let mut tree = WidgetTree::new();
        let mut node = WidgetNode::new(WidgetContent::Text {
            text: "Hello World".to_string(),
        });
        node.style.shadow = Some((20.0, 0.5, 0.0, 0.0));
        node.style.shadow_color = Some(Color::from_rgba8(167, 139, 250, 255));
        node.style.background = parse_fill("transparent");
        node.final_rect = (0.0, 0.0, 100.0, 30.0);
        let id = tree.insert(node);

        let mut scene = SceneGraph::new();
        let root_id = scene.root();
        build_scene_node(
            &tree, id, &mut scene, root_id, 1.0, None, None, 0.0, 0.0, 1.0,
        );

        let has_shadow = scene.diff_against(None).into_iter().any(|node_id| {
            scene
                .get(node_id)
                .is_some_and(|n| matches!(n.kind, SceneNodeKind::Shadow { .. }))
        });
        assert!(
            !has_shadow,
            "Pure text widget should not generate a box shadow node"
        );
    }

    #[test]
    fn test_progress_ring_props_custom_angle_ccw_no_text() {
        let mut tree = WidgetTree::new();
        let mut node = WidgetNode::new(WidgetContent::ProgressRing {
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

        let mut scene = SceneGraph::new();
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
            .any(|n| matches!(n.kind, SceneNodeKind::Text { .. }));
        assert!(!has_text, "show_text=false should omit the text node");

        let arc_node = nodes
            .iter()
            .find(|n| matches!(n.kind, SceneNodeKind::ProgressArc { .. }))
            .expect("ProgressArc node should exist");
        if let SceneNodeKind::ProgressArc {
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
        let mut tree = WidgetTree::new();
        let mut node = WidgetNode::new(WidgetContent::Slider {
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

        let mut scene = SceneGraph::new();
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
            .any(|n| matches!(n.kind, SceneNodeKind::Text { .. }));
        assert!(
            !has_text,
            "show_percent_text=false should omit the label text"
        );

        let rect_count = nodes
            .iter()
            .filter(|n| matches!(n.kind, SceneNodeKind::Rect { .. }))
            .count();
        assert_eq!(
            rect_count, 1,
            "Should only have fill Rect when thumb is disabled"
        );
    }

    #[test]
    fn test_slider_props_rtl_direction() {
        let mut tree = WidgetTree::new();
        let mut node = WidgetNode::new(WidgetContent::Slider {
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

        let mut scene = SceneGraph::new();
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
            .find(|n| matches!(n.kind, SceneNodeKind::Rect { .. }))
            .expect("Fill rect should exist");

        let (bx, by, bw, bh) = fill_rect.bounds;
        assert_eq!(bx, 160.0);
        assert_eq!(by, 10.0);
        assert_eq!(bw, 50.0);
        assert_eq!(bh, 30.0);
    }

    #[test]
    fn test_widget_props_baseline_defaults() {
        let mut tree = WidgetTree::new();

        let mut ring = WidgetNode::new(WidgetContent::ProgressRing {
            value: 75.0,
            max: 100.0,
            stroke_width: None,
            text: None,
            props: serde_json::Value::Null,
        });
        ring.final_rect = (0.0, 0.0, 100.0, 100.0);
        let ring_id = tree.insert(ring);

        let mut ring_scene = SceneGraph::new();
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
            .find(|n| matches!(n.kind, SceneNodeKind::ProgressArc { .. }))
            .unwrap();
        if let SceneNodeKind::ProgressArc {
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
            .any(|n| matches!(n.kind, SceneNodeKind::Text { .. })));

        let mut slider = WidgetNode::new(WidgetContent::Slider {
            value: 50.0,
            min: 0.0,
            max: 100.0,
            props: serde_json::Value::Null,
        });
        slider.final_rect = (0.0, 0.0, 100.0, 20.0);
        let slider_id = tree.insert(slider);

        let mut slider_scene = SceneGraph::new();
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
            .any(|n| matches!(n.kind, SceneNodeKind::Text { .. })));
        let slider_rects = slider_nodes
            .iter()
            .filter(|n| matches!(n.kind, SceneNodeKind::Rect { .. }))
            .count();
        assert!(
            slider_rects >= 2,
            "Default slider should have fill rect and thumb rect"
        );

        let mut progress = WidgetNode::new(WidgetContent::Progress {
            value: 20.0,
            max: 100.0,
            props: serde_json::Value::Null,
        });
        progress.final_rect = (10.0, 10.0, 100.0, 10.0);
        let progress_id = tree.insert(progress);

        let mut prog_scene = SceneGraph::new();
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
            .find(|n| matches!(n.kind, SceneNodeKind::Rect { .. }))
            .unwrap();
        assert_eq!(fill.bounds.0, 10.0);
        assert_eq!(fill.bounds.2, 20.0);
    }

    #[test]
    fn test_image_download_completed_flag_consumed_after_single_render() {
        let mut tree = WidgetTree::new();
        let mut node = WidgetNode::new(WidgetContent::Container);
        node.final_rect = (0.0, 0.0, 64.0, 64.0);
        let _root_id = tree.insert(node);

        let mut render_ctx = context::RenderContext::new(1.0);
        let mut damage = damage::DamageTracker::default();

        render_ctx.image_fetcher.mark_completed_for_test();

        let first_pass = render_surface(
            0,
            64,
            64,
            1.0,
            false,
            &tree,
            &mut render_ctx,
            &mut damage,
            None,
            None,
            1.0,
        );
        assert!(
            first_pass.is_some(),
            "Expected render_surface to redraw when an image download completes"
        );

        let second_pass = render_surface(
            0,
            64,
            64,
            1.0,
            false,
            &tree,
            &mut render_ctx,
            &mut damage,
            None,
            None,
            1.0,
        );
        assert!(
            second_pass.is_none(),
            "Expected completion flag to be reset after a single render pass"
        );
    }
}
