use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ToplevelInfo {
    pub title: String,
    pub app_id: String,
    pub active: bool,
    pub maximized: bool,
    pub minimized: bool,
    pub fullscreen: bool,
}

pub type WindowInfo = ToplevelInfo;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WorkspaceApp {
    pub app_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub id: String,
    pub name: String,
    pub monitor: String,
    pub active: bool,
    pub apps: Vec<WorkspaceApp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct KeyboardInfo {
    pub layout: String,
    pub short_name: String,
    #[serde(default)]
    pub variant: String,
    pub index: u32,
    #[serde(default)]
    pub layouts: Vec<String>,
    #[serde(default)]
    pub short_layouts: Vec<String>,
    #[serde(default)]
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct WindowStatus {
    pub active: Option<ToplevelInfo>,
    pub list: Vec<ToplevelInfo>,
    pub workspaces: Vec<WorkspaceInfo>,
    #[serde(default)]
    pub keyboard: Option<KeyboardInfo>,
}

pub fn deduce_short_layout_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "EN".to_string();
    }
    if let Some(start) = trimmed.find('(') {
        if let Some(end) = trimmed[start..].find(')') {
            let inner = trimmed[start + 1..start + end].trim();
            if !inner.is_empty() {
                return inner.to_uppercase();
            }
        }
    }
    match trimmed.to_lowercase().as_str() {
        "english" | "english (us)" | "us" => "US".to_string(),
        "russian" | "ru" => "RU".to_string(),
        "ukrainian" | "ua" => "UA".to_string(),
        "german" | "de" | "deutsch" => "DE".to_string(),
        "french" | "fr" | "français" => "FR".to_string(),
        "spanish" | "es" | "español" => "ES".to_string(),
        "italian" | "it" | "italiano" => "IT".to_string(),
        "polish" | "pl" | "polski" => "PL".to_string(),
        "portuguese" | "pt" | "português" => "PT".to_string(),
        "japanese" | "ja" | "jp" => "JP".to_string(),
        "chinese" | "zh" | "cn" => "CN".to_string(),
        "korean" | "ko" | "kr" => "KR".to_string(),
        "turkish" | "tr" | "türkçe" => "TR".to_string(),
        "swedish" | "sv" | "svenska" => "SE".to_string(),
        "norwegian" | "no" | "norsk" => "NO".to_string(),
        "danish" | "da" | "dansk" => "DK".to_string(),
        "finnish" | "fi" | "suomi" => "FI".to_string(),
        "czech" | "cs" | "čeština" => "CZ".to_string(),
        s if s.len() <= 3 => s.to_uppercase(),
        s => s.chars().take(2).collect::<String>().to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deduce_short_layout_name() {
        assert_eq!(deduce_short_layout_name("English (US)"), "US");
        assert_eq!(deduce_short_layout_name("Russian"), "RU");
        assert_eq!(deduce_short_layout_name("Ukrainian"), "UA");
        assert_eq!(deduce_short_layout_name("German"), "DE");
        assert_eq!(deduce_short_layout_name("us"), "US");
        assert_eq!(deduce_short_layout_name(""), "EN");
    }
}
