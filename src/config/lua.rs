//! Lua 5.4 runtime via mlua.

use anyhow::{Context, Result};
use log::info;
use mlua::{Lua, Table, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

use super::{
    AnimationConfig, AnimationFrom, BarConfig, LayoutConfig, ModuleConfig, ShadowConfig,
    StyleConfig, StyleStateConfig, SurfaceConfig, WidgetConfig,
};

fn expand_path(path: &std::path::Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with("~/") || s == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(s.strip_prefix("~/").unwrap_or(""));
        }
    }
    path.to_path_buf()
}

struct LuaRuntimeInner {
    lua: Lua,
    builders: std::collections::HashMap<String, mlua::RegistryKey>,
    styles: Arc<RwLock<std::collections::HashMap<String, StyleConfig>>>,
    animations: Arc<RwLock<std::collections::HashMap<String, AnimationConfig>>>,
    config: Arc<RwLock<BarConfig>>,
}

#[derive(Clone)]
pub struct LuaRuntime {
    config_path: PathBuf,
    inner: Arc<std::sync::Mutex<Option<LuaRuntimeInner>>>,
    pending_rebuilds: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    last_error_time: Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
}

fn json_to_lua(lua: &Lua, val: &serde_json::Value) -> mlua::Result<Value> {
    match val {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(b) => Ok(Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::Number(f))
            } else {
                Ok(Value::Nil)
            }
        }
        serde_json::Value::String(s) => Ok(Value::String(lua.create_string(s)?)),
        serde_json::Value::Array(arr) => {
            let table = lua.create_table()?;
            for (idx, item) in arr.iter().enumerate() {
                table.set(idx + 1, json_to_lua(lua, item)?)?;
            }
            Ok(Value::Table(table))
        }
        serde_json::Value::Object(obj) => {
            let table = lua.create_table()?;
            for (k, v) in obj {
                table.set(k.as_str(), json_to_lua(lua, v)?)?;
            }
            Ok(Value::Table(table))
        }
    }
}

pub fn create_lua_state_snapshot(
    lua: &Lua,
    data_store: &std::collections::HashMap<String, serde_json::Value>,
) -> mlua::Result<Table> {
    let state = lua.create_table()?;
    let module_table = lua.create_table()?;

    for (key, value) in data_store {
        let lua_val = json_to_lua(lua, value)?;
        module_table.set(key.as_str(), lua_val.clone())?;

        if key.contains('.') {
            let parts: Vec<&str> = key.split('.').collect();
            let mut curr = module_table.clone();
            for (i, &part) in parts.iter().enumerate() {
                if i == parts.len() - 1 {
                    curr.set(part, lua_val.clone())?;
                } else {
                    let next = match curr.get::<Table>(part) {
                        Ok(t) => t,
                        Err(_) => {
                            let new_t = lua.create_table()?;
                            curr.set(part, new_t.clone())?;
                            new_t
                        }
                    };
                    curr = next;
                }
            }
        }

        state.set(key.as_str(), lua_val)?;
    }

    state.set("module", module_table)?;
    Ok(state)
}

#[inline]
fn lock_unpoisoned<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[inline]
fn rw_read_unpoisoned<T>(rw: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    rw.read().unwrap_or_else(|p| p.into_inner())
}

impl LuaRuntime {
    pub fn new(config_path: PathBuf) -> Result<Self> {
        let config_path = expand_path(&config_path);
        Ok(Self {
            config_path,
            inner: Arc::new(std::sync::Mutex::new(None)),
            pending_rebuilds: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            last_error_time: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        })
    }

    pub fn request_rebuild(&self, name: &str) {
        lock_unpoisoned(&self.pending_rebuilds).insert(name.to_string());
    }

    pub fn request_rebuild_all(&self) {
        lock_unpoisoned(&self.pending_rebuilds).insert("*".to_string());
    }

    pub fn has_pending_rebuilds(&self) -> bool {
        !lock_unpoisoned(&self.pending_rebuilds).is_empty()
    }

    pub fn drain_pending_rebuilds(&self) -> Vec<String> {
        let mut set = lock_unpoisoned(&self.pending_rebuilds);
        set.drain().collect()
    }

    pub fn update_accent(&self, accent: &str) -> Result<()> {
        let inner_guard = lock_unpoisoned(&self.inner);
        let inner = match inner_guard.as_ref() {
            Some(i) => i,
            None => return Ok(()),
        };

        let lua = &inner.lua;
        let globals = lua.globals();

        // 1. Invoke theme hook if registered in Lua
        if let Ok(wyrd_tbl) = globals.get::<Table>("wyrd") {
            if let Ok(hook) = wyrd_tbl.get::<mlua::Function>("_on_theme_accent") {
                if let Err(e) = hook.call::<()>(accent.to_string()) {
                    log::warn!("wyrd._on_theme_accent hook error: {}", e);
                }
            } else if let Ok(hook) = wyrd_tbl.get::<mlua::Function>("on_theme_accent") {
                if let Err(e) = hook.call::<()>(accent.to_string()) {
                    log::warn!("wyrd.on_theme_accent hook error: {}", e);
                }
            }
        }

        // 2. Update styles in config and inner.styles
        let mut cfg = inner.config.write().expect("Lua config lock poisoned");
        for (name, style) in cfg.styles.iter_mut() {
            if style.accent.is_some()
                || name == "global"
                || name == "primary"
                || name == "chip_accent"
                || name.contains("accent")
            {
                style.accent = Some(accent.to_string());
            }
        }
        let _ = crate::widgets::resolve_style_inheritance(&mut cfg.styles);
        *inner.styles.write().expect("styles lock poisoned") = cfg.styles.clone();

        drop(cfg);
        drop(inner_guard);
        self.request_rebuild_all();
        Ok(())
    }

    pub fn update_palette(&self, palette: &serde_json::Value) -> Result<()> {
        let inner_guard = lock_unpoisoned(&self.inner);
        let inner = match inner_guard.as_ref() {
            Some(i) => i,
            None => return Ok(()),
        };

        let map = if let Some(inner_map) = palette.get("palette").and_then(|v| v.as_object()) {
            inner_map
        } else if let Some(obj) = palette.as_object() {
            obj
        } else {
            return Ok(());
        };

        let lua = &inner.lua;
        let globals = lua.globals();

        // 1. Create Lua table from palette map
        if let Ok(lua_table) = lua.create_table() {
            for (k, v) in map {
                if let Some(s) = v.as_str() {
                    let _ = lua_table.set(k.as_str(), s);
                }
            }

            // 2. Invoke theme hook if registered in Lua
            if let Ok(wyrd_tbl) = globals.get::<Table>("wyrd") {
                if let Ok(hook) = wyrd_tbl.get::<mlua::Function>("_on_theme_palette") {
                    if let Err(e) = hook.call::<()>(lua_table.clone()) {
                        log::warn!("wyrd._on_theme_palette hook error: {}", e);
                    }
                } else if let Ok(hook) = wyrd_tbl.get::<mlua::Function>("on_theme_palette") {
                    if let Err(e) = hook.call::<()>(lua_table.clone()) {
                        log::warn!("wyrd.on_theme_palette hook error: {}", e);
                    }
                }
            }
        }

        // 3. Update styles in config and inner.styles
        let primary = map
            .get("primary")
            .or_else(|| map.get("accent"))
            .and_then(|v| v.as_str());
        let surface = map.get("surface").and_then(|v| v.as_str());
        let surface_container = map.get("surface_container").and_then(|v| v.as_str());
        let background = map.get("background").and_then(|v| v.as_str());
        let on_surface = map.get("on_surface").and_then(|v| v.as_str());
        let outline = map.get("outline").and_then(|v| v.as_str());
        let outline_variant = map.get("outline_variant").and_then(|v| v.as_str());

        let mut cfg = inner.config.write().expect("Lua config lock poisoned");
        for (name, style) in cfg.styles.iter_mut() {
            if let Some(p) = primary {
                if style.accent.is_some()
                    || name == "global"
                    || name == "primary"
                    || name == "chip_accent"
                    || name.contains("accent")
                {
                    style.accent = Some(p.to_string());
                }
            }

            if name.starts_with("card") || name.starts_with("chip") || name.contains("surface") {
                if let Some(surf) = surface_container.or(surface) {
                    style.background = Some(format!("{}d9", surf));
                }
                if let Some(out) = outline_variant.or(outline) {
                    style.outline = Some(format!("1px solid {}40", out));
                }
                if let Some(fg) = on_surface {
                    style.foreground = Some(fg.to_string());
                }
            } else if name == "bar" || name.starts_with("bar_") || name == "panel" {
                if let Some(bg) = background {
                    style.background = Some(format!("{}d9", bg));
                }
                if let Some(out) = outline_variant.or(outline) {
                    style.outline = Some(format!("1px solid {}33", out));
                }
            } else if name == "popup" || name.starts_with("popup_") {
                if let Some(bg) = background.or(surface) {
                    style.background = Some(format!("{}f0", bg));
                }
                if let Some(out) = outline.or(outline_variant) {
                    style.outline = Some(format!("1px solid {}59", out));
                }
            }
        }
        let _ = crate::widgets::resolve_style_inheritance(&mut cfg.styles);
        *inner.styles.write().expect("styles lock poisoned") = cfg.styles.clone();

        drop(cfg);
        drop(inner_guard);
        self.request_rebuild_all();
        Ok(())
    }

    pub fn get_styles(&self) -> Option<std::collections::HashMap<String, StyleConfig>> {
        lock_unpoisoned(&self.inner)
            .as_ref()
            .map(|i| rw_read_unpoisoned(&i.styles).clone())
    }

    pub async fn load_config(&self) -> Result<BarConfig> {
        let script = tokio::fs::read_to_string(&self.config_path)
            .await
            .with_context(|| format!("cannot read {:?}", self.config_path))?;
        self.load_config_from_str(&script)
    }

    pub fn load_config_from_str(&self, script: &str) -> Result<BarConfig> {
        self.load_config_from_str_with_store(script, &std::collections::HashMap::new())
    }

    pub fn load_config_from_str_with_store(
        &self,
        script: &str,
        initial_store: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<BarConfig> {
        let lua = Lua::new();
        let globals = lua.globals();
        let bar_table = lua
            .create_table()
            .map_err(|e| anyhow::anyhow!("lua create_table: {}", e))?;

        let config = Arc::new(RwLock::new(BarConfig::default()));
        let cfg_clone = config.clone();

        let builders_map = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let builders_clone = builders_map.clone();
        let last_error_time_clone = self.last_error_time.clone();
        let store_snapshot = initial_store.clone();

        let create_fn = lua
            .create_function(move |lua, params: Table| {
                let mut cfg = cfg_clone.write().expect("Lua config lock poisoned");
                let cfg_guard = &mut *cfg;
                let surface = parse_surface_params(
                    lua,
                    params,
                    &mut cfg_guard.styles,
                    &cfg_guard.animations,
                    &builders_clone,
                    &last_error_time_clone,
                    &store_snapshot,
                )
                .map_err(|e| mlua::Error::RuntimeError(format!("{}", e)))?;
                cfg_guard.surfaces.push(surface);
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua create_function: {}", e))?;
        bar_table
            .set("create", create_fn)
            .map_err(|e| anyhow::anyhow!("lua set create: {}", e))?;

        let pending_rebuild = self.pending_rebuilds.clone();
        let rebuild_fn = lua
            .create_function(move |_lua, name: String| {
                lock_unpoisoned(&pending_rebuild).insert(name);
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua rebuild: {}", e))?;
        bar_table
            .set("rebuild", rebuild_fn)
            .map_err(|e| anyhow::anyhow!("lua set rebuild: {}", e))?;

        let pending_all = self.pending_rebuilds.clone();
        let rebuild_all_fn = lua
            .create_function(move |_lua, ()| {
                lock_unpoisoned(&pending_all).insert("*".to_string());
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua rebuild_all: {}", e))?;
        bar_table
            .set("rebuild_all", rebuild_all_fn)
            .map_err(|e| anyhow::anyhow!("lua set rebuild_all: {}", e))?;

        let cfg_clone = config.clone();
        let style_fn = lua
            .create_function(move |lua, args: mlua::MultiValue| {
                let mut cfg = cfg_clone.write().expect("Lua config lock poisoned");
                let mut args_vec = args.into_vec();
                if args_vec.len() >= 2 {
                    let name = match args_vec.remove(0) {
                        Value::String(s) => {
                            s.to_str().ok().map(|b| b.to_string()).unwrap_or_default()
                        }
                        _ => return Ok(()),
                    };
                    if let Value::Table(params) = args_vec.remove(0) {
                        let style = parse_style_params(lua, params)
                            .map_err(|e| mlua::Error::RuntimeError(format!("{}", e)))?;
                        cfg.styles.insert(name, style);
                    }
                } else if args_vec.len() == 1 {
                    if let Value::Table(table) = args_vec.remove(0) {
                        for pair in table.pairs::<Value, Value>() {
                            if let Ok((Value::String(k), Value::Table(v))) = pair {
                                let name =
                                    k.to_str().ok().map(|b| b.to_string()).unwrap_or_default();
                                if let Ok(style) = parse_style_params(lua, v) {
                                    cfg.styles.insert(name, style);
                                }
                            }
                        }
                    }
                }
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua create_function: {}", e))?;
        bar_table
            .set("style", style_fn)
            .map_err(|e| anyhow::anyhow!("lua set style: {}", e))?;

        let cfg_clone_resolve = config.clone();
        let resolve_styles_fn = lua
            .create_function(move |_lua, ()| {
                let mut cfg = cfg_clone_resolve.write().expect("Lua config lock poisoned");
                crate::widgets::resolve_style_inheritance(&mut cfg.styles)
                    .map_err(mlua::Error::RuntimeError)?;
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua resolve_styles: {}", e))?;
        bar_table
            .set("resolve_styles", resolve_styles_fn)
            .map_err(|e| anyhow::anyhow!("lua set resolve_styles: {}", e))?;

        let cfg_clone = config.clone();
        let load_fn = lua
            .create_function(move |_lua, (name, opts): (String, Value)| {
                let mut cfg = cfg_clone.write().expect("Lua config lock poisoned");
                let options = match opts {
                    Value::Table(t) => {
                        let val = mlua::Value::Table(t);
                        serde_json::to_value(val).unwrap_or_default()
                    }
                    _ => serde_json::Value::Null,
                };
                cfg.modules.push(ModuleConfig {
                    name,
                    enabled: true,
                    options,
                });
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua create_function: {}", e))?;
        bar_table
            .set("load_module", load_fn)
            .map_err(|e| anyhow::anyhow!("lua set load_module: {}", e))?;

        let cfg_clone_anim = config.clone();
        let animation_fn = lua
            .create_function(move |_lua, args: mlua::MultiValue| {
                let mut cfg = cfg_clone_anim.write().expect("Lua config lock poisoned");
                let mut args_vec = args.into_vec();
                if args_vec.len() >= 2 {
                    let name = match args_vec.remove(0) {
                        Value::String(s) => {
                            s.to_str().ok().map(|b| b.to_string()).unwrap_or_default()
                        }
                        _ => return Ok(()),
                    };
                    if let Value::Table(params) = args_vec.remove(0) {
                        let anim = parse_animation_config_table(params)
                            .map_err(|e| mlua::Error::RuntimeError(format!("{}", e)))?;
                        cfg.animations.insert(name, anim);
                    }
                } else if args_vec.len() == 1 {
                    if let Value::Table(table) = args_vec.remove(0) {
                        for pair in table.pairs::<Value, Value>() {
                            if let Ok((Value::String(k), Value::Table(v))) = pair {
                                let name =
                                    k.to_str().ok().map(|b| b.to_string()).unwrap_or_default();
                                if let Ok(anim) = parse_animation_config_table(v) {
                                    cfg.animations.insert(name, anim);
                                }
                            }
                        }
                    }
                }
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua animation: {}", e))?;
        bar_table
            .set("animation", animation_fn.clone())
            .map_err(|e| anyhow::anyhow!("lua set animation: {}", e))?;
        bar_table
            .set("animate", animation_fn)
            .map_err(|e| anyhow::anyhow!("lua set animate: {}", e))?;

        let cfg_keybind = config.clone();
        let keybind_fn = lua
            .create_function(move |_lua, (mods, key, action): (String, String, String)| {
                let mut cfg = cfg_keybind.write().expect("Lua config lock poisoned");
                cfg.keybinds.push(super::KeybindConfig {
                    modifiers: mods,
                    key,
                    action,
                });
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua keybind: {}", e))?;
        bar_table
            .set("keybind", keybind_fn)
            .map_err(|e| anyhow::anyhow!("lua set keybind: {}", e))?;

        globals
            .set("bar", bar_table.clone())
            .map_err(|e| anyhow::anyhow!("lua set bar: {}", e))?;
        globals
            .set("shell", bar_table.clone())
            .map_err(|e| anyhow::anyhow!("lua set shell: {}", e))?;
        globals
            .set("wyrd", bar_table.clone())
            .map_err(|e| anyhow::anyhow!("lua set wyrd: {}", e))?;

        let module_table = lua
            .create_table()
            .map_err(|e| anyhow::anyhow!("lua create Module table: {}", e))?;
        let cfg_clone = config.clone();
        let module_load_fn = lua
            .create_function(move |_lua, (name, opts): (String, Value)| {
                let mut cfg = cfg_clone.write().expect("Lua config lock poisoned");
                let options = match opts {
                    Value::Table(table) => {
                        serde_json::to_value(mlua::Value::Table(table)).unwrap_or_default()
                    }
                    _ => serde_json::Value::Null,
                };
                cfg.modules.push(ModuleConfig {
                    name,
                    enabled: true,
                    options,
                });
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua Module.load: {}", e))?;
        module_table
            .set("load", module_load_fn)
            .map_err(|e| anyhow::anyhow!("lua set Module.load: {}", e))?;
        globals
            .set("Module", module_table)
            .map_err(|e| anyhow::anyhow!("lua set Module: {}", e))?;

        let style_table = lua
            .create_table()
            .map_err(|e| anyhow::anyhow!("lua create Style table: {}", e))?;
        let cfg_clone = config.clone();
        let global_style_fn = lua
            .create_function(move |lua, params: Table| {
                let mut cfg = cfg_clone.write().expect("Lua config lock poisoned");
                let style = parse_style_params(lua, params)
                    .map_err(|e| mlua::Error::RuntimeError(format!("{}", e)))?;
                cfg.styles.insert("global".to_owned(), style);
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua Style.global: {}", e))?;
        style_table
            .set("global", global_style_fn)
            .map_err(|e| anyhow::anyhow!("lua set Style.global: {}", e))?;
        globals
            .set("Style", style_table)
            .map_err(|e| anyhow::anyhow!("lua set Style: {}", e))?;

        if let Some(parent) = self.config_path.parent() {
            if let Ok(package) = globals.get::<Table>("package") {
                let current_path: String = package.get("path").unwrap_or_default();
                let mut base_dirs = vec![parent.to_path_buf()];
                if let Some(gp) = parent.parent() {
                    base_dirs.push(gp.to_path_buf());
                    if let Some(ggp) = gp.parent() {
                        base_dirs.push(ggp.to_path_buf());
                    }
                }
                let mut additions = Vec::new();
                for dir in base_dirs {
                    let d = dir.to_string_lossy();
                    additions.push(format!("{}/?.lua;{}/?/init.lua;{}/lua/?.lua", d, d, d));
                }
                let new_path = format!("{};{}", additions.join(";"), current_path);
                let _ = package.set("path", new_path);
            }

            let settings_file = parent.join("settings.toml");
            if let Ok(content) = std::fs::read_to_string(&settings_file) {
                if let Ok(toml_val) = toml::from_str::<toml::Value>(&content) {
                    if let Ok(settings_table) = lua.create_table() {
                        if let toml::Value::Table(map) = toml_val {
                            for (k, v) in map {
                                match v {
                                    toml::Value::String(s) => {
                                        let _ = settings_table.set(k, s);
                                    }
                                    toml::Value::Boolean(b) => {
                                        let _ = settings_table.set(k, b);
                                    }
                                    toml::Value::Integer(i) => {
                                        let _ = settings_table.set(k, i);
                                    }
                                    toml::Value::Float(f) => {
                                        let _ = settings_table.set(k, f);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        let _ = bar_table.set("settings", settings_table);
                    }
                }
            }
        }

        lua.load(script)
            .exec()
            .map_err(|e| anyhow::anyhow!("init.lua execution failed: {}", e))?;

        let mut cfg = config.write().expect("Lua config lock poisoned");
        crate::widgets::resolve_style_inheritance(&mut cfg.styles)
            .map_err(|e| anyhow::anyhow!("init.lua execution failed: {}", e))?;

        let builders = std::mem::take(&mut *lock_unpoisoned(&builders_map));

        let inner = LuaRuntimeInner {
            lua,
            builders,
            styles: Arc::new(RwLock::new(cfg.styles.clone())),
            animations: Arc::new(RwLock::new(cfg.animations.clone())),
            config: config.clone(),
        };
        *lock_unpoisoned(&self.inner) = Some(inner);

        Ok(cfg.clone())
    }

    pub fn build_surface_with_store(
        &self,
        name: &str,
        store: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<Option<Vec<WidgetConfig>>> {
        if let Some(err_time) = lock_unpoisoned(&self.last_error_time).get(name) {
            if err_time.elapsed() < std::time::Duration::from_secs(5) {
                log::debug!(
                    "Rebuild for surface '{}' suppressed by 5s error backoff",
                    name
                );
                return Ok(None);
            }
        }

        let mut inner_guard = lock_unpoisoned(&self.inner);
        let inner = match inner_guard.as_mut() {
            Some(i) => i,
            None => return Ok(None),
        };

        let key = match inner.builders.get(name) {
            Some(k) => k,
            None => return Ok(None),
        };

        let builder_fn: mlua::Function = match inner.lua.registry_value(key) {
            Ok(f) => f,
            Err(e) => {
                log::error!(
                    "Failed to retrieve builder function for surface '{}': {}",
                    name,
                    e
                );
                return Ok(None);
            }
        };

        let state_table = match create_lua_state_snapshot(&inner.lua, store) {
            Ok(t) => t,
            Err(e) => {
                log::error!("Failed to create state snapshot table: {}", e);
                return Ok(None);
            }
        };

        match builder_fn.call::<Value>(state_table) {
            Ok(Value::Table(tbl)) => {
                let mut styles = inner.styles.write().expect("styles lock poisoned");
                let animations = inner.animations.read().expect("animations lock poisoned");
                match parse_widgets_array(&inner.lua, tbl, &mut styles, &animations) {
                    Ok(widgets) => Ok(Some(widgets)),
                    Err(e) => {
                        log::error!(
                            "Lua build() error parsing widgets for surface '{}':\n{}",
                            name,
                            e
                        );
                        lock_unpoisoned(&self.last_error_time)
                            .insert(name.to_string(), std::time::Instant::now());
                        Ok(None)
                    }
                }
            }
            Ok(other) => {
                log::error!(
                    "Lua build() for surface '{}' returned non-table value: {:?}",
                    name,
                    other
                );
                lock_unpoisoned(&self.last_error_time)
                    .insert(name.to_string(), std::time::Instant::now());
                Ok(None)
            }
            Err(e) => {
                log::error!("Lua build() error for surface '{}':\n{}", name, e);
                lock_unpoisoned(&self.last_error_time)
                    .insert(name.to_string(), std::time::Instant::now());
                Ok(None)
            }
        }
    }

    pub async fn watch_config(&self, tx: tokio::sync::mpsc::Sender<BarConfig>) -> Result<()> {
        use notify_debouncer_mini::new_debouncer;
        use std::time::Duration;

        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel(10);
        let mut debouncer = new_debouncer(Duration::from_millis(250), move |res| {
            let _ = notify_tx.blocking_send(res);
        })?;

        let watch_dir = self
            .config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let _ = debouncer
            .watcher()
            .watch(watch_dir, notify::RecursiveMode::Recursive);

        let mut last_mtimes: std::collections::HashMap<PathBuf, std::time::SystemTime> =
            std::collections::HashMap::new();

        fn scan_mtimes(
            dir: &std::path::Path,
            mtimes: &mut std::collections::HashMap<PathBuf, std::time::SystemTime>,
        ) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_dir() {
                        if path.file_name().and_then(|n| n.to_str()) != Some("modules") {
                            scan_mtimes(&path, mtimes);
                        }
                    } else if path.extension().and_then(|ext| ext.to_str()) == Some("lua")
                        || path.extension().and_then(|ext| ext.to_str()) == Some("toml")
                    {
                        if let Ok(meta) = entry.metadata() {
                            if let Ok(mtime) = meta.modified() {
                                mtimes.insert(path, mtime);
                            }
                        }
                    }
                }
            }
        }
        scan_mtimes(watch_dir, &mut last_mtimes);
        if let Ok(meta) = std::fs::metadata(&self.config_path) {
            if let Ok(mtime) = meta.modified() {
                last_mtimes.insert(self.config_path.clone(), mtime);
            }
        }

        while let Some(events) = notify_rx.recv().await {
            match events {
                Ok(events) => {
                    let mut has_config_change = false;
                    for e in events {
                        let path = &e.path;
                        if path.components().any(|c| c.as_os_str() == "modules") {
                            continue;
                        }
                        let is_candidate = path == &self.config_path
                            || path.extension().and_then(|ext| ext.to_str()) == Some("lua")
                            || path.extension().and_then(|ext| ext.to_str()) == Some("toml");
                        if !is_candidate {
                            continue;
                        }
                        if let Ok(meta) = std::fs::metadata(path) {
                            if let Ok(mtime) = meta.modified() {
                                match last_mtimes.get(path) {
                                    Some(&prev_mtime) if prev_mtime == mtime => {
                                        // Unchanged mtime, event was read or access
                                    }
                                    _ => {
                                        last_mtimes.insert(path.clone(), mtime);
                                        has_config_change = true;
                                    }
                                }
                            }
                        } else if last_mtimes.remove(path).is_some() {
                            has_config_change = true;
                        }
                    }
                    if has_config_change {
                        info!(
                            "Config/theme files modified ({:?}), hot-reloading...",
                            self.config_path
                        );
                        match self.load_config().await {
                            Ok(new_cfg) => {
                                if tx.send(new_cfg).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                log::error!("Config hot-reload parse error: {}", e);
                            }
                        }
                    }
                }
                Err(error) => {
                    log::warn!("Config debouncer error: {:?}", error);
                }
            }
        }

        Ok(())
    }
}

fn parse_animation_from_table(table: Table) -> mlua::Result<AnimationFrom> {
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

fn parse_animation_config_table(table: Table) -> mlua::Result<AnimationConfig> {
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

fn resolve_animation_value(
    val: Option<Value>,
    animations: &std::collections::HashMap<String, AnimationConfig>,
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

fn parse_surface_params(
    lua: &Lua,
    params: Table,
    styles: &mut std::collections::HashMap<String, StyleConfig>,
    animations: &std::collections::HashMap<String, AnimationConfig>,
    builders: &Arc<std::sync::Mutex<std::collections::HashMap<String, mlua::RegistryKey>>>,
    last_error_time: &Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    initial_data_store: &std::collections::HashMap<String, serde_json::Value>,
) -> mlua::Result<SurfaceConfig> {
    let ty: String = params.get("type")?;
    let name: String = params.get("name").unwrap_or_else(|_| ty.clone());
    let layer: String = params.get("layer").unwrap_or_else(|_| "top".to_string());
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
    let exclusive_zone: i32 =
        params
            .get("exclusive_zone")
            .unwrap_or(if ty == "bar" { height as i32 } else { 0 });
    let keyboard: String = params
        .get("keyboard")
        .unwrap_or_else(|_| "none".to_string());
    let visible: bool = params.get::<Option<bool>>("visible")?.unwrap_or(true);
    let pinned: bool = params.get::<Option<bool>>("pinned")?.unwrap_or(false);

    let margin = params.get::<Option<Table>>("margin").ok().flatten();
    let margin_cfg = margin
        .map(|m| super::MarginConfig {
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
                    lock_unpoisoned(last_error_time)
                        .insert(name.clone(), std::time::Instant::now());
                    Vec::new()
                }
            },
            Ok(other) => {
                log::error!(
                    "Lua build() for surface '{}' returned non-table value: {:?}",
                    name,
                    other
                );
                lock_unpoisoned(last_error_time).insert(name.clone(), std::time::Instant::now());
                Vec::new()
            }
            Err(e) => {
                log::error!("Lua build() error for surface '{}':\n{}", name, e);
                lock_unpoisoned(last_error_time).insert(name.clone(), std::time::Instant::now());
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

fn parse_widgets_array(
    lua: &Lua,
    table: Table,
    styles: &mut std::collections::HashMap<String, StyleConfig>,
    animations: &std::collections::HashMap<String, AnimationConfig>,
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

fn parse_widget(
    lua: &Lua,
    table: Table,
    styles: &mut std::collections::HashMap<String, StyleConfig>,
    animations: &std::collections::HashMap<String, AnimationConfig>,
) -> mlua::Result<WidgetConfig> {
    let mut ty: String = table
        .get("type")
        .unwrap_or_else(|_| "container".to_string());
    let id: Option<String> = table.get("id").ok();
    if ty == "widget" {
        if let Some(ref widget_id) = id {
            if widget_id == "workspaces" {
                ty = "workspaces".to_string();
            } else if widget_id == "tray" {
                ty = "tray".to_string();
            } else {
                ty = "container".to_string();
            }
        } else {
            ty = "container".to_string();
        }
    }

    let module: Option<String> = table.get("module").ok();
    let text: Option<String> = table.get("text").ok();
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

    let on_click: Option<String> = table.get("on_click").or_else(|_| table.get("action")).ok();
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

fn parse_layout(_lua: &Lua, table: Table) -> mlua::Result<LayoutConfig> {
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
    })
}

fn parse_style_params(_lua: &Lua, table: Table) -> mlua::Result<StyleConfig> {
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
        padding,
        margin,
        hover: parse_state(table.get("hover").ok().flatten()),
        active: parse_state(table.get("active").ok().flatten()),
        disabled: parse_state(table.get("disabled").ok().flatten()),
        focus: parse_state(table.get("focus").ok().flatten()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_load_aetheria_init_lua() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("examples/config/init.lua");
        if path.exists() {
            let loader = LuaRuntime::new(path).unwrap();
            let config = loader.load_config().await;
            assert!(
                config.is_ok(),
                "Failed to load examples/config/init.lua: {:?}",
                config.err()
            );
            let cfg = config.unwrap();
            assert!(!cfg.surfaces.is_empty(), "Expected surfaces to be defined");
            assert!(
                cfg.styles.contains_key("bar"),
                "Expected bar style to be defined"
            );
            assert!(
                cfg.styles.contains_key("popup"),
                "Expected popup style to be defined"
            );
            assert!(
                cfg.styles.contains_key("chip"),
                "Expected chip style to be defined"
            );
            let popup_names: Vec<String> = cfg
                .surfaces
                .iter()
                .filter(|s| s.ty == "popup")
                .map(|s| s.name.clone())
                .collect();
            assert!(popup_names.contains(&"launcher".to_string()));
            assert!(popup_names.contains(&"audio".to_string()));
            assert!(popup_names.contains(&"network".to_string()));
            assert!(popup_names.contains(&"bluetooth".to_string()));
            assert!(popup_names.contains(&"battery".to_string()));
            assert!(popup_names.contains(&"calendar".to_string()));
            assert!(popup_names.contains(&"system".to_string()));
            assert!(popup_names.contains(&"notifications".to_string()));
            assert!(popup_names.contains(&"power".to_string()));
            assert!(popup_names.contains(&"quicksettings".to_string()));

            let qs_popup = cfg
                .surfaces
                .iter()
                .find(|s| s.name == "quicksettings")
                .expect("quicksettings popup");
            assert!(
                !qs_popup.widgets.is_empty(),
                "quicksettings should have populated widgets"
            );
            assert!(cfg.styles.contains_key("quicksettings_popup"));
        }
    }

    #[tokio::test]
    async fn test_lua_style_extends_resolution() {
        let unique_name = format!(
            "wyrd_test_style_extends_{}.lua",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let init_path = std::env::temp_dir().join(unique_name);
        std::fs::write(
            &init_path,
            r##"
            wyrd.style("base", {
                background = "#1a1b26",
                foreground = "#c0caf5",
                radius = 8,
                hover = {
                    background = "#24283b",
                    foreground = "#ffffff",
                },
            })

            wyrd.style("accented", {
                extends = "base",
                foreground = "#7aa2f7",
                hover = {
                    foreground = "#bb9af7",
                },
            })

            wyrd.style("chip_multi", {
                extends = { "base", "accented" },
                radius = 12,
            })
            "##,
        )
        .unwrap();

        let loader = LuaRuntime::new(init_path.clone()).unwrap();
        let cfg = loader
            .load_config()
            .await
            .expect("Failed to load config with style extends");
        let _ = std::fs::remove_file(&init_path);

        let accented = cfg
            .styles
            .get("accented")
            .expect("accented style should exist");
        assert_eq!(accented.background.as_deref(), Some("#1a1b26"));
        assert_eq!(accented.foreground.as_deref(), Some("#7aa2f7"));
        assert_eq!(accented.radius, Some(8.0));
        let acc_hover = accented.hover.as_ref().expect("hover should exist");
        assert_eq!(acc_hover.background.as_deref(), Some("#24283b"));
        assert_eq!(acc_hover.foreground.as_deref(), Some("#bb9af7"));

        let chip = cfg
            .styles
            .get("chip_multi")
            .expect("chip_multi should exist");
        assert_eq!(chip.background.as_deref(), Some("#1a1b26"));
        assert_eq!(chip.foreground.as_deref(), Some("#7aa2f7"));
        assert_eq!(chip.radius, Some(12.0));
    }

    #[tokio::test]
    async fn test_lua_style_extends_cyclic_fails() {
        let unique_name = format!(
            "wyrd_test_style_cyclic_{}.lua",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let init_path = std::env::temp_dir().join(unique_name);
        std::fs::write(
            &init_path,
            r##"
            wyrd.style("a", { extends = "b", background = "#111" })
            wyrd.style("b", { extends = "a", background = "#222" })
            "##,
        )
        .unwrap();

        let loader = LuaRuntime::new(init_path.clone()).unwrap();
        let res = loader.load_config().await;
        let _ = std::fs::remove_file(&init_path);
        assert!(
            res.is_err(),
            "Cyclic style inheritance should produce an error"
        );
        let err_msg = format!("{}", res.unwrap_err());
        assert!(err_msg.contains("Cyclic style inheritance detected:"));
    }

    #[tokio::test]
    async fn test_lua_animation_registration_and_surface_resolution() {
        let unique_name = format!(
            "wyrd_test_anim_{}.lua",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let init_path = std::env::temp_dir().join(unique_name);
        std::fs::write(
            &init_path,
            r##"
            wyrd.animation("@custom_bounce", {
                kind = "spring",
                stiffness = 450,
                damping = 18,
                from = { y = -20, opacity = 0 },
                to = { y = 0, opacity = 1 }
            })

            wyrd.create({
                type = "popup",
                name = "anim_popup",
                open_animation = "@custom_bounce",
                close_animation = {
                    kind = "easing",
                    duration_ms = 200,
                    curve = "ease-out"
                },
                widgets = {
                    {
                        type = "button",
                        id = "btn1",
                        hover_animation = {
                            kind = "spring",
                            stiffness = 500,
                            damping = 22
                        }
                    }
                }
            })
            "##,
        )
        .unwrap();

        let loader = LuaRuntime::new(init_path.clone()).unwrap();
        let cfg = loader
            .load_config()
            .await
            .expect("Failed to load config with animations");
        let _ = std::fs::remove_file(&init_path);

        let custom_bounce = cfg
            .animations
            .get("@custom_bounce")
            .expect("custom_bounce should exist in animations");
        assert_eq!(custom_bounce.kind, "spring");
        assert_eq!(custom_bounce.stiffness, Some(450.0));
        assert_eq!(custom_bounce.damping, Some(18.0));

        let popup = cfg
            .surfaces
            .iter()
            .find(|s| s.name == "anim_popup")
            .expect("anim_popup should exist");
        let open_anim = popup
            .open_animation
            .as_ref()
            .expect("open_animation should be resolved");
        assert_eq!(open_anim.kind, "spring");
        assert_eq!(open_anim.stiffness, Some(450.0));
        assert_eq!(open_anim.from.as_ref().and_then(|f| f.y), Some(-20.0));

        let close_anim = popup
            .close_animation
            .as_ref()
            .expect("close_animation should be resolved");
        assert_eq!(close_anim.kind, "easing");
        assert_eq!(close_anim.duration_ms, Some(200));
        assert_eq!(close_anim.curve.as_deref(), Some("ease-out"));

        let btn = &popup.widgets[0];
        let hover_anim = btn
            .hover_animation
            .as_ref()
            .expect("hover_animation should be parsed");
        assert_eq!(hover_anim.kind, "spring");
        assert_eq!(hover_anim.stiffness, Some(500.0));
        assert_eq!(hover_anim.damping, Some(22.0));
    }

    #[tokio::test]
    async fn test_lua_surface_build_function_loads_and_produces_tree() {
        let runtime = LuaRuntime::new(std::path::PathBuf::from("/dev/null")).unwrap();
        let mut store = std::collections::HashMap::new();
        store.insert("clock".to_string(), serde_json::json!({ "time": "12:34" }));
        let script = r#"
            wyrd.create({
                type = "bar",
                name = "builder_bar",
                build = function(state)
                    local clock = state.clock or {}
                    return {
                        {
                            type = "label",
                            text = "Time: " .. (clock.time or "none"),
                        }
                    }
                end,
            })
        "#;
        let cfg = runtime
            .load_config_from_str_with_store(script, &store)
            .unwrap();
        let surface = cfg
            .surfaces
            .iter()
            .find(|s| s.name == "builder_bar")
            .expect("builder_bar found");
        assert!(surface.has_build_fn);
        assert_eq!(surface.widgets.len(), 1);
        assert_eq!(surface.widgets[0].text.as_deref(), Some("Time: 12:34"));
    }

    #[tokio::test]
    async fn test_lua_surface_build_error_falls_back_without_crashing() {
        let runtime = LuaRuntime::new(std::path::PathBuf::from("/dev/null")).unwrap();
        let script = r#"
            wyrd.create({
                type = "bar",
                name = "error_bar",
                build = function(state)
                    error("intentional build failure")
                end,
            })
        "#;
        let cfg = runtime.load_config_from_str(script).unwrap();
        let surface = cfg
            .surfaces
            .iter()
            .find(|s| s.name == "error_bar")
            .expect("error_bar found");
        assert!(surface.has_build_fn);
        assert!(
            surface.widgets.is_empty(),
            "Widgets should fall back to empty vec on build failure"
        );
    }

    #[tokio::test]
    async fn test_lua_surface_build_precedence_over_widgets() {
        let runtime = LuaRuntime::new(std::path::PathBuf::from("/dev/null")).unwrap();
        let script = r#"
            wyrd.create({
                type = "bar",
                name = "precedence_bar",
                widgets = {
                    { type = "label", text = "Static widget" },
                },
                build = function(state)
                    return {
                        { type = "label", text = "Dynamic widget from build" },
                    }
                end,
            })
        "#;
        let cfg = runtime.load_config_from_str(script).unwrap();
        let surface = cfg
            .surfaces
            .iter()
            .find(|s| s.name == "precedence_bar")
            .expect("precedence_bar found");
        assert!(surface.has_build_fn);
        assert_eq!(surface.widgets.len(), 1);
        assert_eq!(
            surface.widgets[0].text.as_deref(),
            Some("Dynamic widget from build")
        );
    }

    #[tokio::test]
    async fn test_lua_rebuild_pending_and_dynamic_update() {
        let runtime = LuaRuntime::new(std::path::PathBuf::from("/dev/null")).unwrap();
        let mut store = std::collections::HashMap::new();
        store.insert(
            "state_module".to_string(),
            serde_json::json!({ "count": 1 }),
        );
        let script = r#"
            wyrd.create({
                type = "bar",
                name = "counter_bar",
                build = function(state)
                    local mod = state.state_module or {}
                    local c = mod.count or 0
                    if c > 5 then
                        wyrd.rebuild("counter_bar")
                    end
                    return {
                        { type = "label", text = "Count: " .. tostring(c) }
                    }
                end,
            })
        "#;
        let cfg = runtime
            .load_config_from_str_with_store(script, &store)
            .unwrap();
        let surface = cfg
            .surfaces
            .iter()
            .find(|s| s.name == "counter_bar")
            .expect("counter_bar found");
        assert_eq!(surface.widgets[0].text.as_deref(), Some("Count: 1"));
        assert!(!runtime.has_pending_rebuilds());

        // Update state and call build_surface_with_store
        store.insert(
            "state_module".to_string(),
            serde_json::json!({ "count": 10 }),
        );
        let rebuilt_widgets = runtime
            .build_surface_with_store("counter_bar", &store)
            .unwrap()
            .expect("rebuilt widgets");
        assert_eq!(rebuilt_widgets.len(), 1);
        assert_eq!(rebuilt_widgets[0].text.as_deref(), Some("Count: 10"));
        // Since c > 5, wyrd.rebuild("counter_bar") was triggered inside build!
        assert!(runtime.has_pending_rebuilds());
        let pending = runtime.drain_pending_rebuilds();
        assert!(pending.contains(&"counter_bar".to_string()));
    }

    #[tokio::test]
    async fn test_lua_build_error_backoff_suppression() {
        let runtime = LuaRuntime::new(std::path::PathBuf::from("/dev/null")).unwrap();
        let script = r#"
            wyrd.create({
                type = "bar",
                name = "flaky_bar",
                build = function(state)
                    error("runtime build failure")
                end,
            })
        "#;
        let _ = runtime.load_config_from_str(script).unwrap();
        let store = std::collections::HashMap::new();

        // Immediate subsequent call to build_surface_with_store must be suppressed by the 5s error backoff
        let result = runtime
            .build_surface_with_store("flaky_bar", &store)
            .unwrap();
        assert!(
            result.is_none(),
            "Expected rebuild to be suppressed by 5s error backoff"
        );
    }

    #[tokio::test]
    async fn test_dynamic_accent_update_in_lua_runtime() {
        let runtime = LuaRuntime::new(std::path::PathBuf::from("/dev/null")).unwrap();
        let script = r##"
            wyrd.style("primary", {
                background = "#112233",
                accent = "#00FF00",
            })
            wyrd.style("chip", {
                accent = "#00FF00",
            })

            local current_accent = "#00FF00"
            wyrd._on_theme_accent = function(hex)
                current_accent = hex
                wyrd.style("primary", {
                    background = "#112233",
                    accent = hex,
                })
            end
        "##;
        let config = runtime.load_config_from_str(script).unwrap();
        assert_eq!(
            config.styles.get("primary").unwrap().accent.as_deref(),
            Some("#00FF00")
        );

        // Now dynamically update accent
        runtime.update_accent("#FF0077").unwrap();

        let updated_styles = runtime.get_styles().expect("styles available");
        assert_eq!(
            updated_styles.get("primary").unwrap().accent.as_deref(),
            Some("#FF0077")
        );
        assert_eq!(
            updated_styles.get("chip").unwrap().accent.as_deref(),
            Some("#FF0077")
        );

        // All surfaces should be flagged for rebuild
        assert!(runtime.has_pending_rebuilds());
        let pending = runtime.drain_pending_rebuilds();
        assert!(pending.contains(&"*".to_string()));
    }

    #[tokio::test]
    async fn test_dynamic_palette_update_in_lua_runtime() {
        let runtime = LuaRuntime::new(std::path::PathBuf::from("/dev/null")).unwrap();
        let script = r##"
            wyrd.style("primary", {
                background = "#112233",
                accent = "#00FF00",
            })
            wyrd.style("bar", {
                background = "#000000",
            })
            wyrd.style("card", {
                background = "#222222",
            })

            local hook_called = false
            wyrd._on_theme_palette = function(palette)
                hook_called = true
                wyrd.style("primary", {
                    accent = palette.primary,
                })
            end
        "##;
        let config = runtime.load_config_from_str(script).unwrap();
        assert_eq!(
            config.styles.get("primary").unwrap().accent.as_deref(),
            Some("#00FF00")
        );

        let palette = serde_json::json!({
            "primary": "#7dd3fc",
            "surface": "#121722",
            "surface_container": "#182030",
            "background": "#0c0e16",
            "on_surface": "#e2e8f0",
            "outline": "#38455e"
        });

        runtime.update_palette(&palette).unwrap();

        let updated_styles = runtime.get_styles().expect("styles available");
        assert_eq!(
            updated_styles.get("primary").unwrap().accent.as_deref(),
            Some("#7dd3fc")
        );
        assert_eq!(
            updated_styles.get("bar").unwrap().background.as_deref(),
            Some("#0c0e16d9")
        );
        assert_eq!(
            updated_styles.get("card").unwrap().background.as_deref(),
            Some("#182030d9")
        );

        assert!(runtime.has_pending_rebuilds());
        let pending = runtime.drain_pending_rebuilds();
        assert!(pending.contains(&"*".to_string()));
    }

    #[tokio::test]
    async fn test_load_new_themes() {
        let example_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("examples")
            .join("config");
        if !example_dir.exists() {
            return;
        }

        for theme_name in &["nord", "gruvbox", "tokyo-night"] {
            let script = format!(
                r#"
                local settings = {{ theme = "{theme_name}" }}
                local theme_mod = require("themes.{theme_name}")
                theme_mod.apply()
                "#,
                theme_name = theme_name
            );
            let init_path = example_dir.join("init.lua");
            let runtime = LuaRuntime::new(init_path).unwrap();
            let config = runtime.load_config_from_str(&script);
            assert!(
                config.is_ok(),
                "Failed to load theme '{}': {:?}",
                theme_name,
                config.err()
            );
            let cfg = config.unwrap();
            assert!(
                cfg.styles.contains_key("bar"),
                "Theme '{}' missing 'bar' style",
                theme_name
            );
            assert!(
                cfg.styles.contains_key("popup"),
                "Theme '{}' missing 'popup' style",
                theme_name
            );
            assert!(
                cfg.styles.contains_key("quicksettings_popup"),
                "Theme '{}' missing 'quicksettings_popup' style",
                theme_name
            );
        }
    }
}
