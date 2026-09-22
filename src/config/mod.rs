//! Configuration: Lua runtime, hot-reload.

pub mod lua;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use crate::animator::{default_animation_presets, AnimationConfig, AnimationFrom};
pub use crate::widgets::{
    LayoutConfig, MarginConfig, ShadowConfig, StyleConfig, StyleStateConfig, WidgetConfig,
};

/// Keybinding configuration for compositor global bindings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeybindConfig {
    pub modifiers: String,
    pub key: String,
    pub action: String,
}

/// Parsed shell configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellConfig {
    pub surfaces: Vec<SurfaceConfig>,
    pub styles: HashMap<String, StyleConfig>,
    pub modules: Vec<ModuleConfig>,
    #[serde(default = "default_animation_presets")]
    pub animations: HashMap<String, AnimationConfig>,
    #[serde(default)]
    pub keybinds: Vec<KeybindConfig>,
    pub debug: DebugConfig,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            surfaces: Vec::new(),
            styles: HashMap::new(),
            modules: Vec::new(),
            animations: default_animation_presets(),
            keybinds: Vec::new(),
            debug: DebugConfig::default(),
        }
    }
}

pub type BarConfig = ShellConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SurfaceConfig {
    pub name: String,
    pub ty: String,             // "bar", "panel", "popup", "background"
    pub layer: String,          // "top", "overlay", "background"
    pub output: Option<String>, // None = auto-follow all outputs
    pub anchor: Vec<String>,
    pub margin: MarginConfig,
    pub height: u32,
    pub width: Option<u32>,
    pub exclusive_zone: i32,
    pub keyboard: String, // "none", "on_demand", "exclusive"
    pub visible: bool,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub module_channel: Option<String>,
    #[serde(default)]
    pub anchor_to: Option<String>,
    #[serde(default)]
    pub open_animation: Option<AnimationConfig>,
    #[serde(default)]
    pub close_animation: Option<AnimationConfig>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub has_build_fn: bool,
    #[serde(default)]
    pub style: Option<String>,
    #[serde(default)]
    pub drag_strip_height: Option<f32>,
    pub widgets: Vec<WidgetConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModuleConfig {
    pub name: String,
    pub enabled: bool,
    pub options: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DebugConfig {
    pub overlay: bool,
    pub log_level: String,
}
