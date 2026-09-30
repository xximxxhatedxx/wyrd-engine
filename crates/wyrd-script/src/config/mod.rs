//! Generic Lua runtime configuration structures and extension types.

#[cfg(feature = "lua")]
pub mod lua;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeybindConfig {
    pub modifiers: String,
    pub key: String,
    pub action: String,
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

fn default_settings() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Generic script configuration produced by `wyrd-script` before widget-specific
/// parsing (`wyrd-config`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptConfig {
    pub surfaces: Vec<serde_json::Value>,
    pub styles: HashMap<String, serde_json::Value>,
    pub modules: Vec<ModuleConfig>,
    #[serde(default)]
    pub animations: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub keybinds: Vec<KeybindConfig>,
    pub debug: DebugConfig,
    #[serde(default = "default_settings")]
    pub settings: serde_json::Value,
}

impl Default for ScriptConfig {
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

impl ScriptConfig {
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

/// Resolves single or multi-parent `extends` inheritance across generic JSON style maps,
/// returning an error if a cycle is detected.
pub fn resolve_json_style_inheritance(
    styles: &mut HashMap<String, serde_json::Value>,
) -> Result<(), String> {
    fn visit(
        name: &str,
        raw: &HashMap<String, serde_json::Value>,
        resolved: &mut HashMap<String, serde_json::Value>,
        visiting: &mut Vec<String>,
    ) -> Result<serde_json::Value, String> {
        if let Some(done) = resolved.get(name) {
            return Ok(done.clone());
        }
        if visiting.contains(&name.to_owned()) {
            visiting.push(name.to_owned());
            return Err(format!(
                "Cyclic style inheritance detected: {}",
                visiting.join(" -> ")
            ));
        }
        let Some(current) = raw.get(name) else {
            return Ok(serde_json::Value::Object(serde_json::Map::new()));
        };
        visiting.push(name.to_owned());

        let mut merged = serde_json::Map::new();
        if let Some(obj) = current.as_object() {
            if let Some(ext) = obj.get("extends") {
                let parents: Vec<String> = match ext {
                    serde_json::Value::String(s) => vec![s.clone()],
                    serde_json::Value::Array(arr) => arr
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect(),
                    _ => Vec::new(),
                };
                for p in parents {
                    if let serde_json::Value::Object(parent_obj) =
                        visit(&p, raw, resolved, visiting)?
                    {
                        for (k, v) in parent_obj {
                            merged.insert(k, v);
                        }
                    }
                }
            }
            for (k, v) in obj {
                if let (
                    Some(serde_json::Value::Object(existing_sub)),
                    serde_json::Value::Object(new_sub),
                ) = (merged.get_mut(k), v)
                {
                    for (sk, sv) in new_sub {
                        existing_sub.insert(sk.clone(), sv.clone());
                    }
                } else {
                    merged.insert(k.clone(), v.clone());
                }
            }
        }

        visiting.pop();
        let val = serde_json::Value::Object(merged);
        resolved.insert(name.to_owned(), val.clone());
        Ok(val)
    }

    let snapshot = styles.clone();
    let mut resolved = HashMap::new();
    let mut visiting = Vec::new();
    for key in snapshot.keys() {
        visit(key, &snapshot, &mut resolved, &mut visiting)?;
    }
    *styles = resolved;
    Ok(())
}
