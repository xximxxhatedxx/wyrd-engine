//! Lua table parsers for surfaces, widgets, layouts, styles, and animations.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use mlua::{Lua, Table, Value};

use crate::config::{
    AnimationConfig, AnimationFrom, KeyboardInteractivity, Layer, LayoutConfig, MarginConfig,
    ShadowConfig, StyleConfig, StyleStateConfig, SurfaceConfig, SurfaceKind, WidgetConfig,
    WidgetKind,
};
use crate::sync_helpers::lock_unpoisoned;

use super::create_lua_state_snapshot;

pub fn parse_animation_from_table(table: Table) -> mlua::Result<AnimationFrom> {
    let x: Option<f32> = table.get::<Option<f64>>("x")?.map(|v| v as f32);
    let y: Option<f32> = table.get::<Option<f64>>("y")?.map(|v| v as f32);
    let opacity: Option<f32> = table.get::<Option<f64>>("opacity")?.map(|v| v as f32);
    let scale: Option<f32> = table.get::<Option<f64>>("scale")?.map(|v| v as f32);
    Ok(AnimationFrom {
        x,
        y,
        opacity,
        scale,
    })
}

pub fn parse_animation_config_table(table: Table) -> mlua::Result<AnimationConfig> {
    let kind: String = table.get("kind").unwrap_or_else(|_| "spring".to_string());
    let stiffness: Option<f32> = table.get::<Option<f64>>("stiffness")?.map(|v| v as f32);
    let damping: Option<f32> = table.get::<Option<f64>>("damping")?.map(|v| v as f32);
    let duration_ms: Option<u32> = table
        .get::<Option<u32>>("duration_ms")?
        .or_else(|| table.get::<Option<u32>>("duration").ok().flatten());
    let curve: Option<String> = table
        .get::<Option<String>>("curve")?
        .or_else(|| table.get::<Option<String>>("easing").ok().flatten());

    let from = match table.get::<Option<Table>>("from")? {
        Some(t) => Some(parse_animation_from_table(t)?),
        None => None,
    };
    let to = match table.get::<Option<Table>>("to")? {
        Some(t) => Some(parse_animation_from_table(t)?),
        None => None,
    };

    Ok(AnimationConfig {
        kind,
        stiffness,
        damping,
        duration_ms,
        curve,
        from,
        to,
    })
}

pub(crate) fn resolve_animation_value(
    val: Option<Value>,
    animations: &HashMap<String, AnimationConfig>,
) -> mlua::Result<Option<AnimationConfig>> {
    match val {
        Some(Value::String(s)) => {
            let key = s.to_str()?.to_string();
            let found = animations.get(&key).or_else(|| {
                if let Some(stripped) = key.strip_prefix('@') {
                    animations.get(stripped)
                } else {
                    animations.get(&format!("@{}", key))
                }
            });
            if let Some(cfg) = found {
                Ok(Some(cfg.clone()))
            } else {
                log::warn!(
                    "Referenced animation preset '{}' not found in animations registry",
                    key
                );
                Ok(None)
            }
        }
        Some(Value::Table(t)) => {
            let mut anim = parse_animation_config_table(t.clone())?;
            let preset_opt: Option<String> = t.get("preset").ok().or_else(|| t.get("extends").ok());
            if let Some(preset_name) = preset_opt {
                let parent_opt = animations.get(&preset_name).or_else(|| {
                    if let Some(stripped) = preset_name.strip_prefix('@') {
                        animations.get(stripped)
                    } else {
                        animations.get(&format!("@{}", preset_name))
                    }
                });
                if let Some(parent) = parent_opt {
                    let mut merged = parent.clone();
                    if t.contains_key("kind")? {
                        merged.kind = anim.kind;
                    }
                    if anim.stiffness.is_some() {
                        merged.stiffness = anim.stiffness;
                    }
                    if anim.damping.is_some() {
                        merged.damping = anim.damping;
                    }
                    if anim.duration_ms.is_some() {
                        merged.duration_ms = anim.duration_ms;
                    }
                    if anim.curve.is_some() {
                        merged.curve = anim.curve;
                    }
                    if anim.from.is_some() {
                        merged.from = anim.from;
                    }
                    if anim.to.is_some() {
                        merged.to = anim.to;
                    }
                    anim = merged;
                }
            }
            Ok(Some(anim))
        }
        _ => Ok(None),
    }
}

pub fn parse_surface_params(
    lua: &Lua,
    params: Table,
    styles: &mut HashMap<String, StyleConfig>,
    animations: &HashMap<String, AnimationConfig>,
    builders: &Arc<Mutex<HashMap<String, mlua::RegistryKey>>>,
    last_error_time: &Arc<Mutex<HashMap<String, Instant>>>,
    initial_data_store: &HashMap<String, serde_json::Value>,
) -> mlua::Result<SurfaceConfig> {
    let ty_str: String = params.get("type")?;
    let ty = SurfaceKind::try_from(ty_str.as_str()).map_err(mlua::Error::RuntimeError)?;
    let name: String = params.get("name").unwrap_or_else(|_| ty_str.clone());
    let layer_str: String = params.get("layer").unwrap_or_else(|_| "top".to_string());
    let layer = Layer::try_from(layer_str.as_str()).map_err(mlua::Error::RuntimeError)?;
    let output: Option<String> = params.get("output").ok();

    let anchor: Vec<String> = params
        .get::<Option<Table>>("anchor")
        .ok()
        .flatten()
        .map(|t| {
            t.sequence_values::<String>()
                .filter_map(|v| v.ok())
                .filter(|a| a != "center")
                .collect()
        })
        .unwrap_or_else(|| vec!["top".to_string(), "left".to_string(), "right".to_string()]);

    let is_vertical_span =
        anchor.iter().any(|a| a == "top") && anchor.iter().any(|a| a == "bottom");
    let height: u32 = params
        .get("height")
        .unwrap_or(if is_vertical_span { 0 } else { 30 });

    let width: Option<u32> = params
        .get::<Option<u32>>("width")
        .ok()
        .flatten()
        .or_else(|| {
            params
                .get::<Option<String>>("width")
                .ok()
                .flatten()
                .and_then(|s| {
                    if s.ends_with('%') {
                        None
                    } else {
                        s.parse::<u32>().ok()
                    }
                })
        });
    let exclusive_zone: i32 = params
        .get("exclusive_zone")
        .unwrap_or(if ty == SurfaceKind::Bar {
            height as i32
        } else {
            0
        });
    let keyboard_str: String = params
        .get("keyboard")
        .unwrap_or_else(|_| "none".to_string());
    let keyboard = KeyboardInteractivity::try_from(keyboard_str.as_str())
        .map_err(mlua::Error::RuntimeError)?;
    let visible: bool = params.get::<Option<bool>>("visible")?.unwrap_or(true);
    let pinned: bool = params.get::<Option<bool>>("pinned")?.unwrap_or(false);

    let margin = params.get::<Option<Table>>("margin").ok().flatten();
    let margin_cfg = margin
        .map(|m| MarginConfig {
            top: m.get("top").unwrap_or(0),
            left: m.get("left").unwrap_or(0),
            right: m.get("right").unwrap_or(0),
            bottom: m.get("bottom").unwrap_or(0),
        })
        .unwrap_or_default();

    if let Ok(Some(style_tbl)) = params.get::<Option<Table>>("style") {
        if let Ok(st) = parse_style_params(lua, style_tbl) {
            styles.insert(name.clone(), st);
        }
    }

    let build_fn: Option<mlua::Function> = params.get("build").ok();
    let has_build_fn = build_fn.is_some();

    let widgets = if let Some(func) = build_fn {
        if let Ok(key) = lua.create_registry_value(func.clone()) {
            lock_unpoisoned(builders).insert(name.clone(), key);
        }

        let state_table = create_lua_state_snapshot(lua, initial_data_store)?;
        match func.call::<Value>(state_table) {
            Ok(Value::Table(tbl)) => match parse_widgets_array(lua, tbl, styles, animations) {
                Ok(w) => w,
                Err(e) => {
                    log::error!(
                        "Lua build() error parsing widgets for surface '{}':\n{}",
                        name,
                        e
                    );
                    lock_unpoisoned(last_error_time).insert(name.clone(), Instant::now());
                    Vec::new()
                }
            },
            Ok(other) => {
                log::error!(
                    "Lua build() for surface '{}' returned non-table value: {:?}",
                    name,
                    other
                );
                lock_unpoisoned(last_error_time).insert(name.clone(), Instant::now());
                Vec::new()
            }
            Err(e) => {
                log::error!("Lua build() error for surface '{}':\n{}", name, e);
                lock_unpoisoned(last_error_time).insert(name.clone(), Instant::now());
                Vec::new()
            }
        }
    } else {
        match params.get::<Option<Table>>("widgets")? {
            Some(table) => parse_widgets_array(lua, table, styles, animations)?,
            None => Vec::new(),
        }
    };

    let module: Option<String> = params.get::<Option<String>>("module")?;
    let module_channel: Option<String> = params
        .get::<Option<String>>("module_channel")?
        .or_else(|| params.get::<Option<String>>("channel").ok().flatten())
        .or_else(|| module.clone());
    let anchor_to: Option<String> = params
        .get::<Option<String>>("anchor_to")?
        .or_else(|| params.get::<Option<String>>("anchor_widget").ok().flatten());
    let parent: Option<String> = params.get::<Option<String>>("parent")?;
    let style: Option<String> = match params.get::<Value>("style").ok() {
        Some(Value::String(s)) => s.to_str().ok().map(|s| s.to_string()),
        _ => None,
    };
    let drag_strip_height: Option<f32> = params
        .get::<Option<f32>>("drag_strip_height")
        .ok()
        .flatten();

    let open_animation =
        resolve_animation_value(params.get::<Option<Value>>("open_animation")?, animations)?;
    let close_animation =
        resolve_animation_value(params.get::<Option<Value>>("close_animation")?, animations)?;

    Ok(SurfaceConfig {
        name,
        ty,
        layer,
        output,
        anchor,
        margin: margin_cfg,
        height,
        width,
        exclusive_zone,
        keyboard,
        visible,
        module,
        module_channel,
        anchor_to,
        open_animation,
        close_animation,
        pinned,
        parent,
        has_build_fn,
        style,
        drag_strip_height,
        widgets,
    })
}

pub fn parse_widgets_array(
    lua: &Lua,
    table: Table,
    styles: &mut HashMap<String, StyleConfig>,
    animations: &HashMap<String, AnimationConfig>,
) -> mlua::Result<Vec<WidgetConfig>> {
    let mut widgets = Vec::new();
    if table.contains_key("type")? {
        widgets.push(parse_widget(lua, table, styles, animations)?);
        return Ok(widgets);
    }
    for value in table.sequence_values::<Value>() {
        if let Value::Table(t) = value? {
            if t.contains_key("type")? {
                widgets.push(parse_widget(lua, t, styles, animations)?);
            } else {
                let nested = parse_widgets_array(lua, t, styles, animations)?;
                widgets.extend(nested);
            }
        }
    }
    Ok(widgets)
}

pub fn parse_widget(
    lua: &Lua,
    table: Table,
    styles: &mut HashMap<String, StyleConfig>,
    animations: &HashMap<String, AnimationConfig>,
) -> mlua::Result<WidgetConfig> {
    let mut ty_str: String = table
        .get("type")
        .unwrap_or_else(|_| "container".to_string());
    let id: Option<String> = table.get("id").ok();
    let module: Option<String> = table.get("module").ok();
    let text: Option<String> = table.get("text").ok();
    if ty_str == "widget" {
        let has_children = table
            .get::<Option<Table>>("children")
            .ok()
            .flatten()
            .is_some();
        if module.is_some() || (!has_children && text.is_none() && id.is_some()) {
            ty_str = "module".to_string();
        } else {
            ty_str = "container".to_string();
        }
    }
    let ty = WidgetKind::try_from(ty_str.as_str()).map_err(mlua::Error::RuntimeError)?;
    let path: Option<String> = table.get("path").or_else(|_| table.get("src")).ok();

    let mut style_name: Option<String> = match table.get::<Value>("style").ok() {
        Some(Value::String(s)) => s.to_str().ok().map(|s| s.to_string()),
        Some(Value::Table(t)) => {
            if let Ok(st) = parse_style_params(lua, t) {
                let anon_id = format!("__inline_style_{}", styles.len());
                styles.insert(anon_id.clone(), st);
                Some(anon_id)
            } else {
                None
            }
        }
        _ => None,
    };

    if let Ok(Some(override_table)) = table.get::<Option<Table>>("style_override") {
        if let Ok(override_style) = parse_style_params(lua, override_table) {
            let anon_id = format!("__override_style_{}", styles.len());
            let merged = if let Some(ref base_id) = style_name {
                if let Some(base) = styles.get(base_id) {
                    crate::widgets::merge_style(base, &override_style)
                } else {
                    override_style
                }
            } else {
                override_style
            };
            styles.insert(anon_id.clone(), merged);
            style_name = Some(anon_id);
        }
    }

    let popup_prop: Option<String> = table.get("popup").ok();
    let on_click: Option<String> = table
        .get("on_click")
        .or_else(|_| table.get("action"))
        .ok()
        .or_else(|| {
            popup_prop.as_ref().map(|p| {
                if p.starts_with("popup:") {
                    p.clone()
                } else {
                    format!("popup:{}", p)
                }
            })
        });
    let on_right_click: Option<String> = table
        .get("on_right_click")
        .or_else(|_| table.get("context_action"))
        .ok();
    let on_scroll: Option<String> = table.get("on_scroll").ok();
    let on_change: Option<String> = table.get("on_change").ok();
    let placeholder: Option<String> = table.get("placeholder").ok();
    let repeat_over: Option<String> = table
        .get("repeat_over")
        .or_else(|_| table.get("for_each"))
        .ok();
    let bind: Option<String> = table.get("bind").ok();
    let format: Option<String> = table.get("format").ok();
    let min: Option<f32> = table.get("min").ok();
    let max: Option<f32> = table.get("max").ok();
    let value: Option<f32> = table.get("value").ok();
    let stroke_width: Option<f32> = table.get("stroke_width").ok();
    let tooltip: Option<String> = table.get("tooltip").ok();

    let children: Vec<WidgetConfig> = match table.get::<Option<Table>>("children")? {
        Some(table) => parse_widgets_array(lua, table, styles, animations)?,
        None => Vec::new(),
    };

    let mut layout = table
        .get::<Option<Table>>("layout")
        .ok()
        .flatten()
        .and_then(|l| parse_layout(lua, l).ok());

    let direct_width: Option<f32> = table
        .get("width")
        .or_else(|_| table.get("fixed_width"))
        .ok();
    let direct_height: Option<f32> = table
        .get("height")
        .or_else(|_| table.get("fixed_height"))
        .ok();
    let direct_min_width: Option<f32> = table.get("min_width").ok();
    let direct_max_width: Option<f32> = table.get("max_width").ok();
    let direct_min_height: Option<f32> = table.get("min_height").ok();
    let direct_max_height: Option<f32> = table.get("max_height").ok();

    if direct_width.is_some()
        || direct_height.is_some()
        || direct_min_width.is_some()
        || direct_max_width.is_some()
        || direct_min_height.is_some()
        || direct_max_height.is_some()
    {
        let mut l = layout.unwrap_or_default();
        if let Some(w) = direct_width {
            l.width = Some(w);
        }
        if let Some(h) = direct_height {
            l.height = Some(h);
        }
        if let Some(w) = direct_min_width {
            l.min_width = Some(w);
        }
        if let Some(w) = direct_max_width {
            l.max_width = Some(w);
        }
        if let Some(h) = direct_min_height {
            l.min_height = Some(h);
        }
        if let Some(h) = direct_max_height {
            l.max_height = Some(h);
        }
        layout = Some(l);
    }

    let opacity: Option<f32> = table.get("opacity").ok();

    let mut props_map = match table.get::<Option<mlua::Table>>("props").ok().flatten() {
        Some(t) => match serde_json::to_value(mlua::Value::Table(t)).unwrap_or_default() {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        },
        None => serde_json::Map::new(),
    };

    for prop_key in [
        "start_angle_deg",
        "direction",
        "show_text",
        "track_thickness",
        "font_size",
        "show_thumb",
        "thumb_radius",
        "show_percent_text",
        "percent_text_format",
        "fill_radius",
        "tick_at",
    ] {
        if !props_map.contains_key(prop_key) {
            if let Ok(val) = table.get::<mlua::Value>(prop_key) {
                if val != mlua::Value::Nil {
                    if let Ok(v) = serde_json::to_value(val) {
                        props_map.insert(prop_key.to_string(), v);
                    }
                }
            }
        }
    }

    let props = if props_map.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(props_map))
    };

    let hover_animation = resolve_animation_value(
        table
            .get::<Option<Value>>("hover_animation")?
            .or_else(|| table.get::<Option<Value>>("hover_anim").ok().flatten()),
        animations,
    )?;

    let scroll_y = table.get::<Option<bool>>("scroll_y").ok().flatten();
    let scrollable = table.get::<Option<bool>>("scrollable").ok().flatten();
    let clip = table.get::<Option<bool>>("clip").ok().flatten();

    Ok(WidgetConfig {
        ty,
        id,
        module,
        text,
        path,
        style: style_name,
        children,
        layout,
        on_click,
        on_right_click,
        on_scroll,
        on_change,
        placeholder,
        repeat_over,
        bind,
        format,
        min,
        max,
        value,
        stroke_width,
        tooltip,
        width: direct_width,
        height: direct_height,
        min_width: direct_min_width,
        max_width: direct_max_width,
        min_height: direct_min_height,
        max_height: direct_max_height,
        opacity,
        props,
        hover_animation,
        scroll_y,
        scrollable,
        clip,
    })
}

fn parse_number_or_array_or_table(val: Option<Value>) -> Option<Vec<f32>> {
    match val {
        Some(Value::Number(n)) => Some(vec![n as f32]),
        Some(Value::Integer(i)) => Some(vec![i as f32]),
        Some(Value::Table(t)) => {
            let seq: Vec<f32> = t.sequence_values::<f32>().filter_map(Result::ok).collect();
            if !seq.is_empty() {
                Some(seq)
            } else {
                let vert: f32 = t.get("vertical").unwrap_or(0.0);
                let horiz: f32 = t.get("horizontal").unwrap_or(0.0);
                let top: f32 = t.get("top").unwrap_or(vert);
                let bottom: f32 = t.get("bottom").unwrap_or(vert);
                let left: f32 = t.get("left").unwrap_or(horiz);
                let right: f32 = t.get("right").unwrap_or(horiz);
                Some(vec![top, right, bottom, left])
            }
        }
        _ => None,
    }
}

pub fn parse_layout(_lua: &Lua, table: Table) -> mlua::Result<LayoutConfig> {
    let absolute_table = table.get::<Option<Table>>("absolute").ok().flatten();
    let mode: Option<String> = table.get("mode").ok().or_else(|| {
        if absolute_table.is_some() {
            Some("absolute".to_string())
        } else {
            None
        }
    });
    let direction: Option<String> = table.get("direction").ok();
    let gap: Option<f32> = table.get("gap").ok();
    let align: Option<String> = table.get("align").ok();
    let justify: Option<String> = table
        .get::<Option<String>>("justify")
        .ok()
        .flatten()
        .map(|j| j.replace('-', "_"));
    let padding: Option<Vec<f32>> = parse_number_or_array_or_table(table.get("padding").ok());
    let x: Option<f32> = table
        .get("x")
        .ok()
        .or_else(|| absolute_table.as_ref().and_then(|t| t.get("x").ok()));
    let y: Option<f32> = table
        .get("y")
        .ok()
        .or_else(|| absolute_table.as_ref().and_then(|t| t.get("y").ok()));
    let width: Option<f32> = table
        .get("width")
        .or_else(|_| table.get("fixed_width"))
        .ok()
        .or_else(|| absolute_table.as_ref().and_then(|t| t.get("width").ok()));
    let height: Option<f32> = table
        .get("height")
        .or_else(|_| table.get("fixed_height"))
        .ok()
        .or_else(|| absolute_table.as_ref().and_then(|t| t.get("height").ok()));
    let min_width: Option<f32> = table.get("min_width").ok();
    let max_width: Option<f32> = table.get("max_width").ok();
    let min_height: Option<f32> = table.get("min_height").ok();
    let max_height: Option<f32> = table.get("max_height").ok();
    let weight: Option<f32> = table.get("weight").or_else(|_| table.get("flex")).ok();
    let z_index: Option<i32> = table.get("z_index").or_else(|_| table.get("z")).ok();
    let format: Option<String> = table.get("format").ok();

    let transform = table
        .get::<Option<Table>>("transform")
        .ok()
        .flatten()
        .map(|t| crate::widgets::layout::Transform {
            translate_x: t.get("translate_x").or_else(|_| t.get("x")).ok(),
            translate_y: t.get("translate_y").or_else(|_| t.get("y")).ok(),
            rotate: t
                .get("rotate")
                .or_else(|_| t.get("rotation"))
                .or_else(|_| t.get("angle"))
                .ok(),
            scale_x: t.get("scale_x").or_else(|_| t.get("scale")).ok(),
            scale_y: t.get("scale_y").or_else(|_| t.get("scale")).ok(),
        });

    let columns = table
        .get::<Option<Table>>("columns")
        .ok()
        .flatten()
        .map(|t| {
            t.sequence_values::<Value>()
                .filter_map(Result::ok)
                .map(|v| match v {
                    Value::String(s) => s.to_str().map(|b| b.to_string()).unwrap_or_default(),
                    Value::Number(n) => n.to_string(),
                    Value::Integer(i) => i.to_string(),
                    _ => "auto".to_string(),
                })
                .collect()
        });
    let rows = table.get::<Option<Table>>("rows").ok().flatten().map(|t| {
        t.sequence_values::<Value>()
            .filter_map(Result::ok)
            .map(|v| match v {
                Value::String(s) => s.to_str().map(|b| b.to_string()).unwrap_or_default(),
                Value::Number(n) => n.to_string(),
                Value::Integer(i) => i.to_string(),
                _ => "auto".to_string(),
            })
            .collect()
    });
    let grid_gap = parse_number_or_array_or_table(table.get("grid_gap").ok());
    let absolute = table
        .get::<Option<Table>>("absolute")
        .ok()
        .flatten()
        .map(|abs| crate::widgets::AbsolutePositionConfig {
            x: abs.get("x").ok(),
            y: abs.get("y").ok(),
            width: abs.get("width").ok().or_else(|| abs.get("w").ok()),
            height: abs.get("height").ok().or_else(|| abs.get("h").ok()),
        });

    let scroll_y = table.get::<Option<bool>>("scroll_y").ok().flatten();
    let scrollable = table.get::<Option<bool>>("scrollable").ok().flatten();
    let clip = table.get::<Option<bool>>("clip").ok().flatten();

    Ok(LayoutConfig {
        mode,
        direction,
        gap,
        padding,
        align,
        justify,
        x,
        y,
        width,
        height,
        min_width,
        max_width,
        min_height,
        max_height,
        weight,
        z_index,
        transform,
        format,
        columns,
        rows,
        grid_gap,
        absolute,
        scroll_y,
        scrollable,
        clip,
        border_radius: table.get("border_radius").ok(),
    })
}

pub fn parse_style_params(_lua: &Lua, table: Table) -> mlua::Result<StyleConfig> {
    fn parse_state(table: Option<Table>) -> Option<Box<StyleStateConfig>> {
        table.map(|table| {
            let shadow = table
                .get::<Option<Table>>("shadow")
                .ok()
                .flatten()
                .map(|shadow| ShadowConfig {
                    radius: shadow.get("radius").unwrap_or(0.0),
                    opacity: shadow.get("opacity").unwrap_or(0.0),
                    offset_x: shadow.get("offset_x").or_else(|_| shadow.get("x")).ok(),
                    offset_y: shadow.get("offset_y").or_else(|_| shadow.get("y")).ok(),
                    color: shadow.get("color").ok(),
                });
            let outline = table
                .get("outline")
                .ok()
                .or_else(|| table.get("border").ok());
            Box::new(StyleStateConfig {
                background: table.get("background").or_else(|_| table.get("bg")).ok(),
                foreground: table.get("foreground").or_else(|_| table.get("color")).ok(),
                accent: table.get("accent").ok(),
                opacity: table.get("opacity").ok(),
                outline,
                shadow,
            })
        })
    }
    let shadow = table
        .get::<Option<Table>>("shadow")
        .ok()
        .flatten()
        .map(|shadow| ShadowConfig {
            radius: shadow.get("radius").unwrap_or(0.0),
            opacity: shadow.get("opacity").unwrap_or(0.0),
            offset_x: shadow.get("offset_x").or_else(|_| shadow.get("x")).ok(),
            offset_y: shadow.get("offset_y").or_else(|_| shadow.get("y")).ok(),
            color: shadow.get("color").ok(),
        });
    let outline = table
        .get("outline")
        .ok()
        .or_else(|| table.get("border").ok());
    let padding = parse_number_or_array_or_table(table.get("padding").ok());
    let margin = parse_number_or_array_or_table(table.get("margin").ok());
    let width: Option<f32> = table
        .get("width")
        .or_else(|_| table.get("fixed_width"))
        .ok();
    let height: Option<f32> = table
        .get("height")
        .or_else(|_| table.get("fixed_height"))
        .ok();
    let min_width: Option<f32> = table.get("min_width").ok();
    let max_width: Option<f32> = table.get("max_width").ok();
    let min_height: Option<f32> = table.get("min_height").ok();
    let max_height: Option<f32> = table.get("max_height").ok();

    let extends: Option<Vec<String>> = match table.get::<mlua::Value>("extends").ok() {
        Some(mlua::Value::String(s)) => s.to_str().ok().map(|str_val| vec![str_val.to_string()]),
        Some(mlua::Value::Table(t)) => {
            let seq: Vec<String> = t
                .sequence_values::<String>()
                .filter_map(Result::ok)
                .collect();
            if seq.is_empty() {
                None
            } else {
                Some(seq)
            }
        }
        _ => None,
    };

    Ok(StyleConfig {
        default: table.get::<bool>("default").unwrap_or(false),
        role: table.get("role").ok(),
        extends,
        background: table.get("background").or_else(|_| table.get("bg")).ok(),
        surface: table.get("surface").ok(),
        foreground: table.get("foreground").or_else(|_| table.get("color")).ok(),
        accent: table.get("accent").ok(),
        outline,
        opacity: table.get("opacity").ok(),
        shadow,
        radius: table
            .get("radius")
            .or_else(|_| table.get("border_radius"))
            .ok(),
        font: table.get("font").or_else(|_| table.get("font_family")).ok(),
        font_size: table.get("font_size").or_else(|_| table.get("size")).ok(),
        width,
        height,
        min_width,
        max_width,
        min_height,
        max_height,
        gap: table.get("gap").ok(),
        padding,
        margin,
        hover: parse_state(table.get("hover").ok().flatten()),
        active: parse_state(table.get("active").ok().flatten()),
        disabled: parse_state(table.get("disabled").ok().flatten()),
        focus: parse_state(table.get("focus").ok().flatten()),
    })
}
