//! Cross-process state files: theme.toml and wallpaper.toml.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThemeState {
    pub accent: String,
    pub background: String,
    pub foreground: String,
    pub radius: f32,
}

impl Default for ThemeState {
    fn default() -> Self {
        Self {
            accent: "#7aa2f7".to_string(),
            background: "#00000000".to_string(),
            foreground: "#ffffffff".to_string(),
            radius: 8.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct WallpaperState {
    #[serde(default = "default_color_mode")]
    pub color_mode: String,
    #[serde(default = "default_accent")]
    pub accent: String,
    #[serde(default = "default_accent")]
    pub auto_accent: String,
    pub primary: Option<String>,
    #[serde(default)]
    pub outputs: HashMap<String, String>,
    #[serde(default)]
    pub palette: Option<HashMap<String, String>>,
}

fn default_color_mode() -> String {
    "auto".to_string()
}

fn default_accent() -> String {
    "#7aa2f7".to_string()
}

pub fn get_runtime_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join("wyrd")
    } else {
        // SAFETY: getuid is a stateless POSIX syscall.
        let uid = unsafe { libc::getuid() };
        let run_user = PathBuf::from(format!("/run/user/{}", uid));
        if run_user.is_dir() {
            run_user.join("wyrd")
        } else {
            std::env::temp_dir().join("wyrd")
        }
    }
}

pub fn get_persistent_state_dir() -> PathBuf {
    if let Ok(state_home) = std::env::var("XDG_STATE_HOME") {
        PathBuf::from(state_home).join("wyrd")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("wyrd")
    } else {
        std::env::temp_dir().join("wyrd-state")
    }
}

pub fn get_config_dir() -> PathBuf {
    if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(config_home).join("wyrd")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("wyrd")
    } else {
        std::env::temp_dir().join("wyrd-config")
    }
}

pub fn runtime_wallpaper_file() -> PathBuf {
    get_runtime_dir().join("wallpaper.toml")
}

pub fn persistent_wallpaper_file() -> PathBuf {
    get_persistent_state_dir().join("wallpaper.toml")
}

pub fn config_wallpaper_file() -> PathBuf {
    get_config_dir().join("wallpaper.toml")
}

fn write_atomic(file: &std::path::Path, content: &str) -> Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {:?}", parent))?;
    }
    let tmp = file.with_extension("toml.tmp");
    std::fs::write(&tmp, content)
        .with_context(|| format!("failed to write temporary file {:?}", tmp))?;
    std::fs::rename(&tmp, file)
        .with_context(|| format!("failed to atomically rename {:?} to {:?}", tmp, file))?;
    Ok(())
}

pub fn write_theme_state(state: &ThemeState) -> Result<()> {
    let content =
        toml::to_string_pretty(state).with_context(|| "failed to serialize theme state to TOML")?;
    let runtime_file = get_runtime_dir().join("theme.toml");
    write_atomic(&runtime_file, &content)?;
    let _ = write_atomic(&get_persistent_state_dir().join("theme.toml"), &content);
    Ok(())
}

pub fn read_theme_state() -> Result<ThemeState> {
    let candidates = [
        get_runtime_dir().join("theme.toml"),
        get_persistent_state_dir().join("theme.toml"),
        get_config_dir().join("theme.toml"),
    ];
    for file in &candidates {
        if let Ok(content) = std::fs::read_to_string(file) {
            if let Ok(state) = toml::from_str::<ThemeState>(&content) {
                return Ok(state);
            }
        }
    }
    let file = &candidates[0];
    let content =
        std::fs::read_to_string(file).with_context(|| format!("failed to read {:?}", file))?;
    let state: ThemeState = toml::from_str(&content)
        .with_context(|| format!("failed to parse TOML from {:?}", file))?;
    Ok(state)
}

pub fn write_wallpaper_state(state: &WallpaperState) -> Result<()> {
    let content = toml::to_string_pretty(state)
        .with_context(|| "failed to serialize wallpaper state to TOML")?;

    let _ = write_atomic(&runtime_wallpaper_file(), &content);
    write_atomic(&persistent_wallpaper_file(), &content)?;
    let _ = write_atomic(&config_wallpaper_file(), &content);

    let bg = state
        .palette
        .as_ref()
        .and_then(|p| p.get("surface").or_else(|| p.get("background")).cloned())
        .unwrap_or_else(|| "#00000000".to_string());
    let fg = state
        .palette
        .as_ref()
        .and_then(|p| p.get("on_surface").or_else(|| p.get("foreground")).cloned())
        .unwrap_or_else(|| "#ffffffff".to_string());
    let theme = ThemeState {
        accent: state.accent.clone(),
        background: bg,
        foreground: fg,
        radius: 8.0,
    };
    let _ = write_theme_state(&theme);

    Ok(())
}

pub fn read_wallpaper_state() -> Result<WallpaperState> {
    let candidates = [
        runtime_wallpaper_file(),
        persistent_wallpaper_file(),
        config_wallpaper_file(),
    ];
    for file in &candidates {
        if let Ok(content) = std::fs::read_to_string(file) {
            if let Ok(state) = toml::from_str::<WallpaperState>(&content) {
                return Ok(state);
            }
        }
    }
    let file = &candidates[0];
    let content =
        std::fs::read_to_string(file).with_context(|| format!("failed to read {:?}", file))?;
    let state: WallpaperState = toml::from_str(&content)
        .with_context(|| format!("failed to parse TOML from {:?}", file))?;
    Ok(state)
}

pub fn read_persisted_wallpaper_state() -> Option<WallpaperState> {
    let candidates = [
        persistent_wallpaper_file(),
        config_wallpaper_file(),
        runtime_wallpaper_file(),
    ];
    for file in candidates {
        if let Ok(content) = std::fs::read_to_string(&file) {
            if let Ok(state) = toml::from_str::<WallpaperState>(&content) {
                if !state.outputs.is_empty() || !state.color_mode.is_empty() {
                    return Some(state);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_state_toml_roundtrip() {
        let theme = ThemeState {
            accent: "#c72548".to_string(),
            background: "#0d060ff8".to_string(),
            foreground: "#ede2e6".to_string(),
            radius: 12.0,
        };
        let encoded = toml::to_string_pretty(&theme).unwrap();
        let decoded: ThemeState = toml::from_str(&encoded).unwrap();
        assert_eq!(theme, decoded);
    }

    #[test]
    fn test_wallpaper_state_toml_roundtrip() {
        let mut outputs = HashMap::new();
        outputs.insert("eDP-1".to_string(), "/home/user/wall.png".to_string());
        let wall = WallpaperState {
            color_mode: "auto".to_string(),
            accent: "#c72548".to_string(),
            auto_accent: "#c72548".to_string(),
            primary: Some("eDP-1".to_string()),
            outputs,
            palette: Some(HashMap::from([(
                "primary".to_string(),
                "#7dd3fc".to_string(),
            )])),
        };
        let encoded = toml::to_string_pretty(&wall).unwrap();
        let decoded: WallpaperState = toml::from_str(&encoded).unwrap();
        assert_eq!(wall, decoded);
    }
}
