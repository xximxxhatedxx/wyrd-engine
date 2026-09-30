use super::{layout, WidgetId};
use crate::render::scene::Fill;
use tiny_skia::Color;

#[derive(Clone)]
pub struct WidgetNode {
    pub id: Option<String>,
    pub parent: Option<WidgetId>,
    pub children: Vec<WidgetId>,
    pub layout: layout::LayoutParams,
    pub style: WidgetStyle,
    pub content: WidgetContent,
    pub measured_size: (f32, f32),
    pub final_rect: (f32, f32, f32, f32), // x, y, w, h
    pub dirty: bool,
    pub failed: bool,
    pub on_click: Option<String>,
    pub on_right_click: Option<String>,
    pub on_scroll: Option<String>,
    pub on_change: Option<String>,
    pub tooltip: Option<String>,
}

impl WidgetNode {
    pub fn new(content: WidgetContent) -> Self {
        let on_click = match &content {
            WidgetContent::Button { on_click, .. } => on_click.clone(),
            _ => None,
        };
        Self {
            id: None,
            parent: None,
            children: Vec::new(),
            layout: layout::LayoutParams::default(),
            style: WidgetStyle::default(),
            content,
            measured_size: (0.0, 0.0),
            final_rect: (0.0, 0.0, 0.0, 0.0),
            dirty: true,
            failed: false,
            on_click,
            on_right_click: None,
            on_scroll: None,
            on_change: None,
            tooltip: None,
        }
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

pub type ResolvedStateStyle = (
    Option<Fill>,
    Option<Color>,
    Option<Color>,
    f32,
    Option<Color>,
    f32,
    crate::render::scene::OutlineStyle,
    Option<(f32, f32, f32, f32)>,
    Option<Color>,
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetState {
    Normal,
    Hover,
    Active,
    Disabled,
    Focus,
}

impl WidgetStyle {
    pub fn for_state(&self, state: WidgetState) -> ResolvedStateStyle {
        let override_style = match state {
            WidgetState::Hover => self.hover.as_ref(),
            WidgetState::Active => self.active.as_ref(),
            WidgetState::Disabled => self.disabled.as_ref(),
            WidgetState::Focus => self.focus.as_ref().or(self.hover.as_ref()),
            WidgetState::Normal => None,
        };
        let Some(override_style) = override_style else {
            return (
                self.background.clone(),
                self.foreground,
                self.accent,
                self.opacity,
                self.outline_color,
                self.outline_width,
                self.outline_style,
                self.shadow,
                self.shadow_color,
            );
        };
        (
            override_style
                .background
                .clone()
                .or_else(|| self.background.clone()),
            override_style.foreground.or(self.foreground),
            override_style.accent.or(self.accent),
            override_style.opacity.unwrap_or(self.opacity),
            override_style.outline_color.or(self.outline_color),
            override_style.outline_width.unwrap_or(self.outline_width),
            override_style.outline_style.unwrap_or(self.outline_style),
            override_style.shadow.or(self.shadow),
            override_style.shadow_color.or(self.shadow_color),
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WidgetStyle {
    pub background: Option<Fill>,
    pub foreground: Option<Color>,
    pub accent: Option<Color>,
    pub outline_color: Option<Color>,
    pub outline_width: f32,
    pub outline_style: crate::render::scene::OutlineStyle,
    pub radius: f32,
    pub padding: (f32, f32, f32, f32), // top, right, bottom, left
    pub margin: (f32, f32, f32, f32),
    pub font_family: String,
    pub font_size: f32,
    pub opacity: f32,
    pub shadow: Option<(f32, f32, f32, f32)>, // radius, opacity, offset_x, offset_y
    pub shadow_color: Option<Color>,
    pub hover: Option<WidgetStateStyle>,
    pub active: Option<WidgetStateStyle>,
    pub disabled: Option<WidgetStateStyle>,
    pub focus: Option<WidgetStateStyle>,
}

impl Default for WidgetStyle {
    fn default() -> Self {
        Self {
            background: None,
            foreground: None,
            accent: None,
            outline_color: None,
            outline_width: 0.0,
            outline_style: crate::render::scene::OutlineStyle::Solid,
            radius: 0.0,
            padding: (0.0, 0.0, 0.0, 0.0),
            margin: (0.0, 0.0, 0.0, 0.0),
            font_family: String::new(),
            font_size: 13.0,
            opacity: 1.0,
            shadow: None,
            shadow_color: None,
            hover: None,
            active: None,
            disabled: None,
            focus: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WidgetStateStyle {
    pub background: Option<Fill>,
    pub foreground: Option<Color>,
    pub accent: Option<Color>,
    pub opacity: Option<f32>,
    pub outline_color: Option<Color>,
    pub outline_width: Option<f32>,
    pub outline_style: Option<crate::render::scene::OutlineStyle>,
    pub shadow: Option<(f32, f32, f32, f32)>,
    pub shadow_color: Option<Color>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WidgetContent {
    Container, // Flex/Absolute layout container
    Text {
        text: String,
    },
    Image {
        path: String,
    },
    Svg {
        path: String,
    },
    Button {
        label: String,
        on_click: Option<String>,
    },
    Progress {
        value: f32,
        max: f32,
        props: serde_json::Value,
    },
    ProgressRing {
        value: f32,
        max: f32,
        stroke_width: Option<f32>,
        text: Option<String>,
        props: serde_json::Value,
    },
    Slider {
        value: f32,
        min: f32,
        max: f32,
        props: serde_json::Value,
    },
    TextInput {
        text: String,
        placeholder: String,
        focused: bool,
        cursor_pos: usize,
        selection: Option<(usize, usize)>,
    },
    // Module widgets hold opaque data from IPC
    Module {
        module: String,
        payload: serde_json::Value,
    },
}

impl WidgetContent {
    pub fn kind(&self) -> &'static str {
        match self {
            WidgetContent::Container => "container",
            WidgetContent::Text { .. } => "text",
            WidgetContent::Image { .. } => "image",
            WidgetContent::Svg { .. } => "svg",
            WidgetContent::Button { .. } => "button",
            WidgetContent::Progress { .. } => "progress",
            WidgetContent::ProgressRing { .. } => "progress_ring",
            WidgetContent::Slider { .. } => "slider",
            WidgetContent::TextInput { .. } => "text_input",
            WidgetContent::Module { .. } => "module",
        }
    }
}
