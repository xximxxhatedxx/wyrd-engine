//! Configuration: typed shell config, surface descriptors, and Lua config parsers.

#[cfg(feature = "lua")]
pub mod lua;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use crate::animator::{AnimationConfig, AnimationFrom};
pub use crate::widgets::{
    LayoutConfig, MarginConfig, ShadowConfig, StyleConfig, StyleStateConfig, WidgetConfig,
    WidgetKind,
};
pub use wyrd_script::config::{DebugConfig, KeybindConfig, ModuleConfig};

/// Kind of Wayland surface managed by Wyrd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    #[default]
    Bar,
    Panel,
    Popup,
    #[serde(alias = "wallpaper")]
    Background,
}

impl std::fmt::Display for SurfaceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Bar => "bar",
            Self::Panel => "panel",
            Self::Popup => "popup",
            Self::Background => "background",
        })
    }
}

impl TryFrom<&str> for SurfaceKind {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bar" => Ok(Self::Bar),
            "panel" => Ok(Self::Panel),
            "popup" => Ok(Self::Popup),
            "background" | "wallpaper" => Ok(Self::Background),
            other => Err(format!(
                "invalid SurfaceKind '{}': expected one of \"bar\", \"panel\", \"popup\", \"background\"",
                other
            )),
        }
    }
}

impl TryFrom<String> for SurfaceKind {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

/// Wayland `zwlr_layer_shell_v1` stacking layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Background,
    Bottom,
    #[default]
    Top,
    Overlay,
}

impl std::fmt::Display for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Background => "background",
            Self::Bottom => "bottom",
            Self::Top => "top",
            Self::Overlay => "overlay",
        })
    }
}

impl TryFrom<&str> for Layer {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.trim().to_ascii_lowercase().as_str() {
            "background" => Ok(Self::Background),
            "bottom" => Ok(Self::Bottom),
            "top" => Ok(Self::Top),
            "overlay" => Ok(Self::Overlay),
            other => Err(format!(
                "invalid Layer '{}': expected one of \"background\", \"bottom\", \"top\", \"overlay\"",
                other
            )),
        }
    }
}

impl TryFrom<String> for Layer {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

/// Wayland `zwlr_layer_surface_v1` keyboard interactivity mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardInteractivity {
    #[default]
    None,
    #[serde(alias = "on-demand", alias = "ondemand")]
    OnDemand,
    Exclusive,
}

impl std::fmt::Display for KeyboardInteractivity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "none",
            Self::OnDemand => "on_demand",
            Self::Exclusive => "exclusive",
        })
    }
}

impl TryFrom<&str> for KeyboardInteractivity {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" | "false" => Ok(Self::None),
            "on_demand" | "on-demand" | "ondemand" => Ok(Self::OnDemand),
            "exclusive" | "true" => Ok(Self::Exclusive),
            other => Err(format!(
                "invalid KeyboardInteractivity '{}': expected one of \"none\", \"on_demand\", \"exclusive\"",
                other
            )),
        }
    }
}

impl TryFrom<String> for KeyboardInteractivity {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

fn default_settings() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Complete typed shell configuration parsed from `init.lua`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellConfig {
    pub surfaces: Vec<SurfaceConfig>,
    pub styles: HashMap<String, StyleConfig>,
    pub modules: Vec<ModuleConfig>,
    #[serde(default)]
    pub animations: HashMap<String, AnimationConfig>,
    #[serde(default)]
    pub keybinds: Vec<KeybindConfig>,
    pub debug: DebugConfig,
    #[serde(default = "default_settings")]
    pub settings: serde_json::Value,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            surfaces: Vec::new(),
            styles: HashMap::new(),
            modules: Vec::new(),
            animations: HashMap::new(),
            keybinds: Vec::new(),
            debug: DebugConfig::default(),
            settings: default_settings(),
        }
    }
}

impl ShellConfig {
    /// Reads a `u64` setting from `self.settings[key]`, returning `default` if missing or non-integer.
    pub fn setting_u64(&self, key: &str, default: u64) -> u64 {
        self.settings
            .get(key)
            .and_then(|v| v.as_u64())
            .unwrap_or(default)
    }

    /// Reads an `f32` setting from `self.settings[key]`, returning `default` if missing or non-numeric.
    pub fn setting_f32(&self, key: &str, default: f32) -> f32 {
        self.settings
            .get(key)
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(default)
    }

    /// Reads a `&str` setting from `self.settings[key]`, returning `default` if missing or non-string.
    pub fn setting_str<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.settings
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or(default)
    }
}

pub type BarConfig = ShellConfig;

/// Configuration for a single Wayland surface and its child widgets.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SurfaceConfig {
    pub name: String,
    #[serde(default, alias = "type")]
    pub ty: SurfaceKind,
    #[serde(default)]
    pub layer: Layer,
    pub output: Option<String>, // None = auto-follow all outputs
    pub anchor: Vec<String>,
    pub margin: MarginConfig,
    pub height: u32,
    pub width: Option<u32>,
    pub exclusive_zone: i32,
    #[serde(default)]
    pub keyboard: KeyboardInteractivity,
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

impl wyrd_graphics::SurfaceSpec for SurfaceConfig {
    fn surface_name(&self) -> &str {
        &self.name
    }

    fn target_output(&self) -> Option<&str> {
        self.output.as_deref()
    }
}
