use super::{layout, parse_color, parse_fill, AnimationConfig, WidgetNode, WidgetStateStyle};
use serde::{Deserialize, Serialize};
use tiny_skia::Color;

pub(crate) fn find_default_style(
    styles: &std::collections::HashMap<String, StyleConfig>,
) -> Option<&StyleConfig> {
    styles.values().find(|s| s.default)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarginConfig {
    pub top: i32,
    pub left: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetKind {
    #[serde(
        alias = "box",
        alias = "row",
        alias = "column",
        alias = "spacer",
        alias = "divider",
        alias = "separator",
        alias = "scroll"
    )]
    Container,
    #[default]
    Item,
    #[serde(alias = "label")]
    Text,
    Image,
    Svg,
    Button,
    Progress,
    #[serde(alias = "ring", alias = "circle_progress")]
    ProgressRing,
    Slider,
    #[serde(alias = "input")]
    TextInput,
    Module,
}

impl std::fmt::Display for WidgetKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Container => "container",
            Self::Item => "item",
            Self::Text => "text",
            Self::Image => "image",
            Self::Svg => "svg",
            Self::Button => "button",
            Self::Progress => "progress",
            Self::ProgressRing => "progress_ring",
            Self::Slider => "slider",
            Self::TextInput => "text_input",
            Self::Module => "module",
        })
    }
}

impl TryFrom<&str> for WidgetKind {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.trim().to_ascii_lowercase().as_str() {
            "container" | "box" | "row" | "column" | "spacer" | "divider" | "separator"
            | "scroll" => Ok(Self::Container),
            "item" => Ok(Self::Item),
            "text" | "label" => Ok(Self::Text),
            "image" => Ok(Self::Image),
            "svg" => Ok(Self::Svg),
            "button" => Ok(Self::Button),
            "progress" => Ok(Self::Progress),
            "progress_ring" | "ring" | "circle_progress" => Ok(Self::ProgressRing),
            "slider" => Ok(Self::Slider),
            "text_input" | "input" => Ok(Self::TextInput),
            "module" => Ok(Self::Module),
            other => Err(format!(
                "invalid WidgetKind '{}': expected one of \"container\", \"item\", \"text\", \"image\", \"svg\", \"button\", \"progress\", \"progress_ring\", \"slider\", \"text_input\", \"module\"",
                other
            )),
        }
    }
}

impl TryFrom<String> for WidgetKind {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WidgetConfig {
    #[serde(default, alias = "type")]
    pub ty: WidgetKind,
    pub id: Option<String>,
    pub module: Option<String>,
    #[serde(alias = "label", alias = "name")]
    pub text: Option<String>,
    #[serde(alias = "src", alias = "image", alias = "icon")]
    pub path: Option<String>,
    pub style: Option<String>,
    #[serde(default)]
    pub children: Vec<WidgetConfig>,
    pub layout: Option<LayoutConfig>,
    #[serde(alias = "action")]
    pub on_click: Option<String>,
    #[serde(alias = "context_action")]
    pub on_right_click: Option<String>,
    pub on_scroll: Option<String>,
    pub on_change: Option<String>,
    #[serde(default)]
    pub tooltip: Option<String>,
    pub placeholder: Option<String>,
    pub repeat_over: Option<String>,
    pub bind: Option<String>,
    pub format: Option<String>,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub value: Option<f32>,
    pub stroke_width: Option<f32>,
    #[serde(alias = "fixed_width")]
    pub width: Option<f32>,
    #[serde(alias = "fixed_height")]
    pub height: Option<f32>,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    pub min_height: Option<f32>,
    pub max_height: Option<f32>,
    pub opacity: Option<f32>,
    #[serde(default)]
    pub props: Option<serde_json::Value>,
    #[serde(default)]
    pub hover_animation: Option<AnimationConfig>,
    pub scroll_y: Option<bool>,
    pub scrollable: Option<bool>,
    pub clip: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AbsolutePositionConfig {
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LayoutConfig {
    pub mode: Option<String>,      // "flex", "absolute", "stack", "grid"
    pub direction: Option<String>, // "horizontal", "vertical"
    pub gap: Option<f32>,
    pub padding: Option<Vec<f32>>,
    pub align: Option<String>,   // "start", "center", "end", "stretch"
    pub justify: Option<String>, // "start", "center", "end", "space-between", "space-around", "space-evenly"
    pub x: Option<f32>,
    pub y: Option<f32>,
    #[serde(alias = "fixed_width")]
    pub width: Option<f32>,
    #[serde(alias = "fixed_height")]
    pub height: Option<f32>,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    pub min_height: Option<f32>,
    pub max_height: Option<f32>,
    pub weight: Option<f32>,
    pub z_index: Option<i32>,
    pub transform: Option<layout::Transform>,
    pub format: Option<String>,
    pub columns: Option<Vec<String>>,
    pub rows: Option<Vec<String>>,
    pub grid_gap: Option<Vec<f32>>,
    pub absolute: Option<AbsolutePositionConfig>,
    pub scroll_y: Option<bool>,
    pub scrollable: Option<bool>,
    pub clip: Option<bool>,
    #[serde(default)]
    pub border_radius: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct StyleConfig {
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub extends: Option<Vec<String>>,
    pub background: Option<String>,
    pub surface: Option<String>,
    pub foreground: Option<String>,
    pub accent: Option<String>,
    pub outline: Option<String>,
    pub opacity: Option<f32>,
    pub shadow: Option<ShadowConfig>,
    pub radius: Option<f32>,
    pub font: Option<String>,
    pub font_size: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    pub min_height: Option<f32>,
    pub max_height: Option<f32>,
    pub gap: Option<f32>,
    pub padding: Option<Vec<f32>>,
    pub margin: Option<Vec<f32>>,
    pub hover: Option<Box<StyleStateConfig>>,
    pub active: Option<Box<StyleStateConfig>>,
    pub disabled: Option<Box<StyleStateConfig>>,
    pub focus: Option<Box<StyleStateConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct StyleStateConfig {
    pub background: Option<String>,
    pub foreground: Option<String>,
    pub accent: Option<String>,
    pub opacity: Option<f32>,
    pub outline: Option<String>,
    pub shadow: Option<ShadowConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShadowConfig {
    pub radius: f32,
    pub opacity: f32,
    pub offset_x: Option<f32>,
    pub offset_y: Option<f32>,
    pub color: Option<String>,
}

pub fn merge_style_state(base: &StyleStateConfig, child: &StyleStateConfig) -> StyleStateConfig {
    StyleStateConfig {
        background: child.background.clone().or_else(|| base.background.clone()),
        foreground: child.foreground.clone().or_else(|| base.foreground.clone()),
        accent: child.accent.clone().or_else(|| base.accent.clone()),
        opacity: child.opacity.or(base.opacity),
        outline: child.outline.clone().or_else(|| base.outline.clone()),
        shadow: child.shadow.clone().or_else(|| base.shadow.clone()),
    }
}

pub fn merge_style(base: &StyleConfig, child: &StyleConfig) -> StyleConfig {
    let merge_state = |b: Option<&StyleStateConfig>, c: Option<&StyleStateConfig>| match (b, c) {
        (Some(b_st), Some(c_st)) => Some(Box::new(merge_style_state(b_st, c_st))),
        (Some(b_st), None) => Some(Box::new(b_st.clone())),
        (None, Some(c_st)) => Some(Box::new(c_st.clone())),
        (None, None) => None,
    };

    StyleConfig {
        default: child.default,
        role: child.role.clone().or_else(|| base.role.clone()),
        extends: None,
        background: child.background.clone().or_else(|| base.background.clone()),
        surface: child.surface.clone().or_else(|| base.surface.clone()),
        foreground: child.foreground.clone().or_else(|| base.foreground.clone()),
        accent: child.accent.clone().or_else(|| base.accent.clone()),
        outline: child.outline.clone().or_else(|| base.outline.clone()),
        opacity: child.opacity.or(base.opacity),
        shadow: child.shadow.clone().or_else(|| base.shadow.clone()),
        radius: child.radius.or(base.radius),
        font: child.font.clone().or_else(|| base.font.clone()),
        font_size: child.font_size.or(base.font_size),
        width: child.width.or(base.width),
        height: child.height.or(base.height),
        min_width: child.min_width.or(base.min_width),
        max_width: child.max_width.or(base.max_width),
        min_height: child.min_height.or(base.min_height),
        max_height: child.max_height.or(base.max_height),
        gap: child.gap.or(base.gap),
        padding: child.padding.clone().or_else(|| base.padding.clone()),
        margin: child.margin.clone().or_else(|| base.margin.clone()),
        hover: merge_state(base.hover.as_deref(), child.hover.as_deref()),
        active: merge_state(base.active.as_deref(), child.active.as_deref()),
        disabled: merge_state(base.disabled.as_deref(), child.disabled.as_deref()),
        focus: merge_state(base.focus.as_deref(), child.focus.as_deref()),
    }
}

pub fn resolve_style_inheritance(
    styles: &mut std::collections::HashMap<String, StyleConfig>,
) -> Result<(), String> {
    if !styles.values().any(|s| s.default) {
        let pure_typography_root = styles
            .iter()
            .find(|(_, s)| {
                s.extends.is_none()
                    && s.font.is_some()
                    && s.font_size.is_some()
                    && s.outline.is_none()
                    && s.shadow.is_none()
                    && s.radius.is_none()
                    && s.padding.is_none()
                    && s.width.is_none()
                    && s.height.is_none()
            })
            .map(|(k, _)| k.clone());

        if let Some(root_name) = pure_typography_root {
            if let Some(root_style) = styles.get_mut(&root_name) {
                root_style.default = true;
            }
        } else {
            let mut ref_counts: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            for s in styles.values() {
                if let Some(ref parents) = s.extends {
                    for p in parents {
                        if styles.get(p).is_some_and(|ps| ps.extends.is_none()) {
                            *ref_counts.entry(p.clone()).or_insert(0) += 1;
                        }
                    }
                }
            }
            if let Some((root_name, _)) = ref_counts.into_iter().max_by_key(|(_, count)| *count) {
                if let Some(root_style) = styles.get_mut(&root_name) {
                    root_style.default = true;
                }
            }
        }
    }
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Unvisited,
        Visiting,
        Visited,
    }

    let mut states: std::collections::HashMap<String, State> = styles
        .keys()
        .map(|k| (k.clone(), State::Unvisited))
        .collect();

    let mut path: Vec<String> = Vec::new();

    fn dfs(
        node: &str,
        styles: &mut std::collections::HashMap<String, StyleConfig>,
        states: &mut std::collections::HashMap<String, State>,
        path: &mut Vec<String>,
    ) -> Result<(), String> {
        states.insert(node.to_string(), State::Visiting);
        path.push(node.to_string());

        let parents = styles.get(node).and_then(|s| s.extends.clone());
        if let Some(parents) = parents {
            for parent in &parents {
                if !styles.contains_key(parent) {
                    return Err(format!(
                        "Parent style '{}' referenced by '{}' does not exist",
                        parent, node
                    ));
                }

                match states.get(parent).copied().unwrap_or(State::Unvisited) {
                    State::Visiting => {
                        path.push(parent.clone());
                        let cycle = path.join(" -> ");
                        return Err(format!("Cyclic style inheritance detected: {}", cycle));
                    }
                    State::Unvisited => {
                        dfs(parent, styles, states, path)?;
                    }
                    State::Visited => {}
                }
            }

            // Merge parents in order (rightmost parent overrides leftmost)
            let mut merged_parent: Option<StyleConfig> = None;
            for parent in &parents {
                let p_style = match styles.get(parent) {
                    Some(s) => s.clone(),
                    None => return Err(format!("Parent style '{}' not found", parent)),
                };
                merged_parent = match merged_parent {
                    None => Some(p_style),
                    Some(prev) => Some(merge_style(&prev, &p_style)),
                };
            }

            let child_style = match styles.get(node) {
                Some(s) => s.clone(),
                None => return Err(format!("Style '{}' not found", node)),
            };
            let fully_resolved = if let Some(p) = merged_parent {
                merge_style(&p, &child_style)
            } else {
                let mut s = child_style;
                s.extends = None;
                s
            };

            styles.insert(node.to_string(), fully_resolved);
        }

        states.insert(node.to_string(), State::Visited);
        path.pop();
        Ok(())
    }

    let names: Vec<String> = styles.keys().cloned().collect();
    for name in names {
        if states.get(&name).copied() == Some(State::Unvisited) {
            dfs(&name, styles, &mut states, &mut path)?;
        }
    }

    if let Some((default_font, default_font_size)) =
        find_default_style(styles).map(|s| (s.font.clone(), s.font_size))
    {
        for style in styles.values_mut() {
            if style.font.is_none() {
                style.font = default_font.clone();
            }
            if style.font_size.is_none() {
                style.font_size = default_font_size;
            }
        }
    }

    Ok(())
}

pub fn get_style<'a>(
    name: &str,
    styles: &'a std::collections::HashMap<String, StyleConfig>,
) -> Option<std::borrow::Cow<'a, StyleConfig>> {
    styles.get(name).map(std::borrow::Cow::Borrowed)
}

pub fn parse_outline(
    outline_str: &str,
) -> (f32, crate::render::scene::OutlineStyle, Option<Color>) {
    let trimmed = outline_str.trim();
    if trimmed == "0px transparent" || trimmed == "none" || trimmed == "0" || trimmed == "0px" {
        return (0.0, crate::render::scene::OutlineStyle::Solid, None);
    }
    if let Some(color) = parse_color(trimmed) {
        return (1.0, crate::render::scene::OutlineStyle::Solid, Some(color));
    }
    let mut width = 1.0;
    let mut style = crate::render::scene::OutlineStyle::Solid;
    let mut rest = trimmed;
    if let Some(first_space) = trimmed.find(' ') {
        let first_tok = &trimmed[..first_space];
        if let Ok(w) = first_tok.trim_end_matches("px").parse::<f32>() {
            width = w;
            rest = trimmed[first_space..].trim();
        }
    }
    if let Some(after_solid) = rest.strip_prefix("solid") {
        style = crate::render::scene::OutlineStyle::Solid;
        rest = after_solid.trim();
    } else if let Some(after_dashed) = rest.strip_prefix("dashed") {
        style = crate::render::scene::OutlineStyle::Dashed;
        rest = after_dashed.trim();
    } else if let Some(after_dotted) = rest.strip_prefix("dotted") {
        style = crate::render::scene::OutlineStyle::Dotted;
        rest = after_dotted.trim();
    }
    if let Some(color) = parse_color(rest) {
        (width.max(0.5), style, Some(color))
    } else {
        (0.0, crate::render::scene::OutlineStyle::Solid, None)
    }
}

pub fn parse_shadow(shadow: &ShadowConfig) -> ((f32, f32, f32, f32), Option<Color>) {
    (
        (
            shadow.radius,
            shadow.opacity,
            shadow.offset_x.unwrap_or(0.0),
            shadow.offset_y.unwrap_or(0.0),
        ),
        shadow.color.as_deref().and_then(parse_color),
    )
}

pub fn apply_style_to_node(node: &mut WidgetNode, style: &StyleConfig) {
    if let Some(bg) = style.background.as_deref().and_then(parse_fill) {
        node.style.background = Some(bg);
    } else if let Some(surf) = style.surface.as_deref().and_then(parse_fill) {
        node.style.background = Some(surf);
    }
    if let Some(fg) = style.foreground.as_deref().and_then(parse_color) {
        node.style.foreground = Some(fg);
    }
    if let Some(accent) = style.accent.as_deref().and_then(parse_color) {
        node.style.accent = Some(accent);
    }
    if let Some(outline_str) = style.outline.as_deref() {
        let (w, s, c) = parse_outline(outline_str);
        node.style.outline_width = w;
        node.style.outline_style = s;
        node.style.outline_color = c;
    }

    let parse_state = |state: Option<&Box<StyleStateConfig>>| {
        state.as_ref().map(|state| {
            let (outline_width, outline_style, outline_color) =
                if let Some(outline_str) = state.outline.as_deref() {
                    let (w, s, c) = parse_outline(outline_str);
                    (Some(w), Some(s), c)
                } else {
                    (None, None, None)
                };
            let (shadow, shadow_color) = if let Some(s) = state.shadow.as_ref() {
                let (sh, c) = parse_shadow(s);
                (Some(sh), c)
            } else {
                (None, None)
            };
            WidgetStateStyle {
                background: state.background.as_deref().and_then(parse_fill),
                foreground: state.foreground.as_deref().and_then(parse_color),
                accent: state.accent.as_deref().and_then(parse_color),
                opacity: state.opacity.map(|value| value.clamp(0.0, 1.0)),
                outline_color,
                outline_width,
                outline_style,
                shadow,
                shadow_color,
            }
        })
    };
    if let Some(h) = parse_state(style.hover.as_ref()) {
        node.style.hover = Some(h);
    }
    if let Some(a) = parse_state(style.active.as_ref()) {
        node.style.active = Some(a);
    }
    if let Some(d) = parse_state(style.disabled.as_ref()) {
        node.style.disabled = Some(d);
    }
    if let Some(f) = parse_state(style.focus.as_ref()) {
        node.style.focus = Some(f);
    }
    if let Some(op) = style.opacity {
        node.style.opacity = op.clamp(0.0, 1.0);
    }
    if let Some(shadow) = style.shadow.as_ref() {
        let (sh, c) = parse_shadow(shadow);
        node.style.shadow = Some(sh);
        node.style.shadow_color = c;
    }
    if let Some(radius) = style.radius {
        node.style.radius = radius;
    }
    if let Some(font) = &style.font {
        node.style.font_family = font.clone();
    }
    if let Some(font_size) = style.font_size {
        node.style.font_size = font_size;
    }
    if let Some(gap) = style.gap {
        node.layout.gap = gap;
    }
    if let Some(padding) = &style.padding {
        let p = match padding.len() {
            4 => (padding[0], padding[1], padding[2], padding[3]),
            2 => (padding[0], padding[1], padding[0], padding[1]),
            1 => (padding[0], padding[0], padding[0], padding[0]),
            _ => (0.0, 0.0, 0.0, 0.0),
        };
        node.style.padding = p;
        node.layout.padding = p;
    }
    if let Some(w) = style.width {
        node.layout.fixed_width = Some(w);
    }
    if let Some(h) = style.height {
        node.layout.fixed_height = Some(h);
    }
    if let Some(w) = style.min_width {
        node.layout.min_width = Some(w);
    }
    if let Some(w) = style.max_width {
        node.layout.max_width = Some(w);
    }
    if let Some(h) = style.min_height {
        node.layout.min_height = Some(h);
    }
    if let Some(h) = style.max_height {
        node.layout.max_height = Some(h);
    }
    if let Some(margin) = &style.margin {
        let m = match margin.len() {
            4 => (margin[0], margin[1], margin[2], margin[3]),
            2 => (margin[0], margin[1], margin[0], margin[1]),
            1 => (margin[0], margin[0], margin[0], margin[0]),
            _ => (0.0, 0.0, 0.0, 0.0),
        };
        node.style.margin = m;
    }
}
