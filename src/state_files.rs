//! Cross-process state files: theme.toml and wallpaper.toml.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Theme snapshot written by wyrd-shell and read by wyrd-greet.
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
            accent: "#c72548".to_string(),
            background: "#0d060ff8".to_string(),
            foreground: "#ede2e6".to_string(),
            radius: 8.0,
        }
    }
}

/// Active wallpaper mapping written by wyrd-wallpaper and read by dynamic-color.
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
    "#c72548".to_string()
}

pub fn get_runtime_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join("wyrd")
    } else {
        // SAFETY: `libc::getuid()` is a POSIX system call that reads the real UID of the calling
        // process. It takes no pointers, mutates no memory, and is always safe to invoke.
        let uid = unsafe { libc::getuid() };
        let run_user = PathBuf::from(format!("/run/user/{}", uid));
        if run_user.is_dir() {
            run_user.join("wyrd")
        } else {
            std::env::temp_dir().join("wyrd")
        }
    }
}

pub fn write_theme_state(state: &ThemeState) -> Result<()> {
    let dir = get_runtime_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create runtime dir {:?}", dir))?;
    let content =
        toml::to_string_pretty(state).with_context(|| "failed to serialize theme state to TOML")?;
    std::fs::write(dir.join("theme.toml"), content)
        .with_context(|| format!("failed to write {:?}", dir.join("theme.toml")))?;
    Ok(())
}

pub fn read_theme_state() -> Result<ThemeState> {
    let dir = get_runtime_dir();
    let file = dir.join("theme.toml");
    let content =
        std::fs::read_to_string(&file).with_context(|| format!("failed to read {:?}", file))?;
    let state: ThemeState = toml::from_str(&content)
        .with_context(|| format!("failed to parse TOML from {:?}", file))?;
    Ok(state)
}

pub fn write_wallpaper_state(state: &WallpaperState) -> Result<()> {
    let dir = get_runtime_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create runtime dir {:?}", dir))?;
    let content = toml::to_string_pretty(state)
        .with_context(|| "failed to serialize wallpaper state to TOML")?;
    std::fs::write(dir.join("wallpaper.toml"), content)
        .with_context(|| format!("failed to write {:?}", dir.join("wallpaper.toml")))?;
    Ok(())
}

pub fn read_wallpaper_state() -> Result<WallpaperState> {
    let dir = get_runtime_dir();
    let file = dir.join("wallpaper.toml");
    let content =
        std::fs::read_to_string(&file).with_context(|| format!("failed to read {:?}", file))?;
    let state: WallpaperState = toml::from_str(&content)
        .with_context(|| format!("failed to parse TOML from {:?}", file))?;
    Ok(state)
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
