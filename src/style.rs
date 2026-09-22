//! Style engine: cascading tokens.

use std::collections::HashMap;
use tiny_skia::Color;

pub struct StyleEngine {
    global: StyleTokens,
    module_overrides: HashMap<String, StyleTokens>,
}

impl Default for StyleEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl StyleEngine {
    pub fn new() -> Self {
        Self {
            global: StyleTokens::default_dark(),
            module_overrides: HashMap::new(),
        }
    }

    pub fn resolve(&self, module: Option<&str>, _state: WidgetState) -> ResolvedStyle {
        let base = module
            .and_then(|m| self.module_overrides.get(m))
            .unwrap_or(&self.global);
        ResolvedStyle {
            background: base.background,
            foreground: base.foreground,
            accent: base.accent,
            radius: base.radius,
            font_family: base.font_family.clone(),
            font_size: base.font_size,
        }
    }

    pub fn set_accent(&mut self, accent: Color) {
        self.global.accent = accent;
        for tokens in self.module_overrides.values_mut() {
            tokens.accent = accent;
        }
    }

    pub fn set_global_tokens(&mut self, tokens: StyleTokens) {
        self.global = tokens;
    }
}

#[derive(Debug, Clone)]
pub struct StyleTokens {
    pub background: Color,
    pub foreground: Color,
    pub accent: Color,
    pub radius: f32,
    pub font_family: String,
    pub font_size: f32,
}

impl StyleTokens {
    pub fn default_dark() -> Self {
        Self {
            background: Color::from_rgba8(30, 30, 30, 255),
            foreground: Color::from_rgba8(220, 220, 220, 255),
            accent: Color::from_rgba8(100, 149, 237, 255),
            radius: 4.0,
            font_family: "sans-serif".to_string(),
            font_size: 14.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetState {
    Normal,
    Hover,
    Active,
    Disabled,
    Focus,
}

#[derive(Debug, Clone)]
pub struct ResolvedStyle {
    pub background: Color,
    pub foreground: Color,
    pub accent: Color,
    pub radius: f32,
    pub font_family: String,
    pub font_size: f32,
}
