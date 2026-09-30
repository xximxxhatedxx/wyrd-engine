//! Generic Lua 5.4 runtime (`LuaRuntime<A>`), `LuaRuntimeAdapter` trait, host extensions,
//! and hot-reload file watcher via `mlua`.

pub mod watch;

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use mlua::{Lua, Table, Value};

use super::{resolve_json_style_inheritance, KeybindConfig, ModuleConfig, ScriptConfig};
use crate::sync_helpers::{lock_unpoisoned, read_unpoisoned};

/// Adapter trait allowing [`LuaRuntime`] to be shared without code duplication between:
/// - `wyrd-script` (`GenericScriptAdapter` producing `ScriptConfig` with zero widget/graphics deps), and
/// - `wyrd-config` (`TypedShellAdapter` producing `ShellConfig` and `Vec<WidgetConfig>`).
pub trait LuaRuntimeAdapter: Send + Sync + 'static {
    type Config: Clone + Default + Send + Sync + 'static;
    type StyleMap: Clone + Default + Send + Sync + 'static;
    type SurfaceOutput: Send + Sync + 'static;

    /// Registers adapter-specific host functions (`create`, `style`, `resolve_styles`, `animation`,
    /// `animate`, `Style.global`, and common `load_module`/`Module.load`/`keybind`/`settings`).
    fn register_adapter_functions(
        lua: &Lua,
        bar_table: &Table,
        config: &Arc<RwLock<Self::Config>>,
        builders_map: &Arc<Mutex<HashMap<String, mlua::RegistryKey>>>,
        last_error_time: &Arc<Mutex<HashMap<String, Instant>>>,
        store_snapshot: &HashMap<String, serde_json::Value>,
    ) -> Result<()>;

    /// Resolves style inheritance after script execution and returns `(styles_snapshot, extra_state)`.
    fn finalize_config(
        config: &mut Self::Config,
    ) -> Result<(Self::StyleMap, Arc<dyn Any + Send + Sync>)>;

    /// Updates accent tokens across styles and re-resolves inheritance.
    fn apply_accent(config: &mut Self::Config, styles: &RwLock<Self::StyleMap>, accent: &str);

    /// Updates primary/accent tokens from a theme palette and re-resolves inheritance.
    fn apply_palette_primary(
        config: &mut Self::Config,
        styles: &RwLock<Self::StyleMap>,
        primary: Option<&str>,
    );

    /// Appends a keybind registered via `register_keybind_extension`.
    fn push_keybind(config: &mut Self::Config, keybind: KeybindConfig);

    /// Parses the Lua table returned by a surface's `build(state)` callback.
    fn parse_rebuilt_surface(
        lua: &Lua,
        table: Table,
        styles: &RwLock<Self::StyleMap>,
        extra: &Arc<dyn Any + Send + Sync>,
    ) -> Result<Self::SurfaceOutput, String>;
}

/// Trait implemented by host extensions that register custom Lua functions on the
/// `wyrd` / `bar` / `shell` global tables when [`LuaRuntime`] initializes or reloads.
pub trait LuaHostExtension<C = ScriptConfig>: Send + Sync + 'static {
    fn register(&self, lua: &Lua, wyrd_table: &Table, config: &Arc<RwLock<C>>) -> mlua::Result<()>;
}

impl<C: 'static, F> LuaHostExtension<C> for F
where
    F: Fn(&Lua, &Table, &Arc<RwLock<C>>) -> mlua::Result<()> + Send + Sync + 'static,
{
    fn register(&self, lua: &Lua, wyrd_table: &Table, config: &Arc<RwLock<C>>) -> mlua::Result<()> {
        (self)(lua, wyrd_table, config)
    }
}

pub type LuaExtensionFn<C = ScriptConfig> =
    dyn Fn(&Lua, &Table, &Arc<RwLock<C>>) -> mlua::Result<()> + Send + Sync + 'static;

/// Converts a Lua value to `serde_json::Value`, skipping `Value::Function` fields cleanly
/// without mutating any input `mlua::Table`.
pub fn lua_value_to_json_non_mutating(value: &Value) -> serde_json::Value {
    match value {
        Value::Nil
        | Value::Function(_)
        | Value::Thread(_)
        | Value::UserData(_)
        | Value::LightUserData(_)
        | Value::Error(_)
        | Value::Other(_) => serde_json::Value::Null,
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::Integer(i) => serde_json::Value::Number((*i).into()),
        Value::Number(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => {
            serde_json::Value::String(s.to_str().ok().map(|b| b.to_string()).unwrap_or_default())
        }
        Value::Table(tbl) => lua_table_to_json_non_mutating(tbl),
    }
}

/// Converts a Lua table to `serde_json::Value` without mutating the source table,
/// omitting function entries (such as `build = function(state) ... end`).
pub fn lua_table_to_json_non_mutating(table: &Table) -> serde_json::Value {
    let mut max_idx = 0i64;
    let mut count = 0i64;
    let mut is_seq = true;

    for pair in table.pairs::<Value, Value>() {
        let Ok((k, v)) = pair else { continue };
        if matches!(v, Value::Function(_)) {
            continue;
        }
        match k {
            Value::Integer(i) if i >= 1 => {
                count += 1;
                if i > max_idx {
                    max_idx = i;
                }
            }
            _ => {
                is_seq = false;
                break;
            }
        }
    }

    if is_seq && count > 0 && max_idx == count {
        let mut arr = Vec::with_capacity(count as usize);
        for i in 1..=count {
            if let Ok(v) = table.get::<Value>(i) {
                arr.push(lua_value_to_json_non_mutating(&v));
            }
        }
        serde_json::Value::Array(arr)
    } else if is_seq && count == 0 {
        // Empty table: represent as empty object by default
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        let mut map = serde_json::Map::new();
        for pair in table.pairs::<Value, Value>() {
            let Ok((k, v)) = pair else { continue };
            if matches!(v, Value::Function(_)) {
                continue;
            }
            if let Value::String(s) = k {
                if let Ok(key_str) = s.to_str() {
                    map.insert(key_str.to_string(), lua_value_to_json_non_mutating(&v));
                }
            }
        }
        serde_json::Value::Object(map)
    }
}

/// Default adapter for `wyrd-script` that stores generic JSON tables in [`ScriptConfig`].
pub struct GenericScriptAdapter;

impl LuaRuntimeAdapter for GenericScriptAdapter {
    type Config = ScriptConfig;
    type StyleMap = HashMap<String, serde_json::Value>;
    type SurfaceOutput = serde_json::Value;

    fn register_adapter_functions(
        lua: &Lua,
        bar_table: &Table,
        config: &Arc<RwLock<Self::Config>>,
        builders_map: &Arc<Mutex<HashMap<String, mlua::RegistryKey>>>,
        last_error_time: &Arc<Mutex<HashMap<String, Instant>>>,
        store_snapshot: &HashMap<String, serde_json::Value>,
    ) -> Result<()> {
        let globals = lua.globals();
        let cfg_clone = config.clone();
        let builders_clone = builders_map.clone();
        let last_error_clone = last_error_time.clone();
        let store_clone = store_snapshot.clone();

        let create_fn = lua
            .create_function(move |lua, params: Table| {
                let mut cfg = cfg_clone.write().unwrap_or_else(|p| p.into_inner());
                let name: String = params.get("name").unwrap_or_default();
                let mut has_build_fn = false;
                let mut built_widgets: Option<serde_json::Value> = None;

                if let Ok(build_fn) = params.get::<mlua::Function>("build") {
                    has_build_fn = true;
                    if let Ok(key) = lua.create_registry_value(build_fn.clone()) {
                        lock_unpoisoned(&builders_clone).insert(name.clone(), key);
                    }
                    if let Ok(state_tbl) = create_lua_state_snapshot(lua, &store_clone) {
                        match build_fn.call::<Value>(state_tbl) {
                            Ok(Value::Table(tbl)) => {
                                built_widgets = Some(lua_table_to_json_non_mutating(&tbl));
                            }
                            _ => {
                                lock_unpoisoned(&last_error_clone)
                                    .insert(name.clone(), Instant::now());
                                built_widgets = Some(serde_json::Value::Array(Vec::new()));
                            }
                        }
                    }
                }

                let mut json_val = lua_table_to_json_non_mutating(&params);
                if let Some(obj) = json_val.as_object_mut() {
                    obj.insert(
                        "has_build_fn".to_string(),
                        serde_json::Value::Bool(has_build_fn),
                    );
                    if let Some(widgets) = built_widgets {
                        obj.insert("widgets".to_string(), widgets);
                    }
                }
                cfg.surfaces.push(json_val);
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua create: {}", e))?;
        bar_table
            .set("create", create_fn)
            .map_err(|e| anyhow::anyhow!("lua set create: {}", e))?;

        let cfg_style = config.clone();
        let style_fn = lua
            .create_function(move |_lua, args: mlua::MultiValue| {
                let mut cfg = cfg_style.write().unwrap_or_else(|p| p.into_inner());
                let mut args_vec = args.into_vec();
                if args_vec.len() >= 2 {
                    let name = match args_vec.remove(0) {
                        Value::String(s) => {
                            s.to_str().ok().map(|b| b.to_string()).unwrap_or_default()
                        }
                        _ => return Ok(()),
                    };
                    if let Value::Table(tbl) = args_vec.remove(0) {
                        cfg.styles
                            .insert(name, lua_table_to_json_non_mutating(&tbl));
                    }
                } else if args_vec.len() == 1 {
                    if let Value::Table(table) = args_vec.remove(0) {
                        for pair in table.pairs::<Value, Value>() {
                            if let Ok((Value::String(k), Value::Table(tbl))) = pair {
                                let name =
                                    k.to_str().ok().map(|b| b.to_string()).unwrap_or_default();
                                cfg.styles
                                    .insert(name, lua_table_to_json_non_mutating(&tbl));
                            }
                        }
                    }
                }
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua style: {}", e))?;
        bar_table
            .set("style", style_fn)
            .map_err(|e| anyhow::anyhow!("lua set style: {}", e))?;

        let cfg_resolve = config.clone();
        let resolve_styles_fn = lua
            .create_function(move |_lua, ()| {
                let mut cfg = cfg_resolve.write().unwrap_or_else(|p| p.into_inner());
                resolve_json_style_inheritance(&mut cfg.styles)
                    .map_err(mlua::Error::RuntimeError)?;
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua resolve_styles: {}", e))?;
        bar_table
            .set("resolve_styles", resolve_styles_fn)
            .map_err(|e| anyhow::anyhow!("lua set resolve_styles: {}", e))?;

        let cfg_load = config.clone();
        let load_fn = lua
            .create_function(move |_lua, (name, opts): (String, Value)| {
                let mut cfg = cfg_load.write().unwrap_or_else(|p| p.into_inner());
                let options = match opts {
                    Value::Table(ref t) => lua_table_to_json_non_mutating(t),
                    _ => serde_json::Value::Null,
                };
                cfg.modules.push(ModuleConfig {
                    name,
                    enabled: true,
                    options,
                });
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua load_module: {}", e))?;
        bar_table
            .set("load_module", load_fn.clone())
            .map_err(|e| anyhow::anyhow!("lua set load_module: {}", e))?;

        let module_table = lua
            .create_table()
            .map_err(|e| anyhow::anyhow!("lua create Module table: {}", e))?;
        module_table
            .set("load", load_fn)
            .map_err(|e| anyhow::anyhow!("lua set Module.load: {}", e))?;
        globals
            .set("Module", module_table)
            .map_err(|e| anyhow::anyhow!("lua set Module: {}", e))?;

        let cfg_anim = config.clone();
        let animation_fn = lua
            .create_function(move |_lua, args: mlua::MultiValue| {
                let mut cfg = cfg_anim.write().unwrap_or_else(|p| p.into_inner());
                let mut args_vec = args.into_vec();
                if args_vec.len() >= 2 {
                    let name = match args_vec.remove(0) {
                        Value::String(s) => {
                            s.to_str().ok().map(|b| b.to_string()).unwrap_or_default()
                        }
                        _ => return Ok(()),
                    };
                    if let Value::Table(tbl) = args_vec.remove(0) {
                        cfg.animations
                            .insert(name, lua_table_to_json_non_mutating(&tbl));
                    }
                } else if args_vec.len() == 1 {
                    if let Value::Table(table) = args_vec.remove(0) {
                        for pair in table.pairs::<Value, Value>() {
                            if let Ok((Value::String(k), Value::Table(tbl))) = pair {
                                let name =
                                    k.to_str().ok().map(|b| b.to_string()).unwrap_or_default();
                                cfg.animations
                                    .insert(name, lua_table_to_json_non_mutating(&tbl));
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
                let mut cfg = cfg_keybind.write().unwrap_or_else(|p| p.into_inner());
                cfg.keybinds.push(KeybindConfig {
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

        let cfg_settings = config.clone();
        let settings_fn = lua
            .create_function(move |_lua, params: Table| {
                let mut cfg = cfg_settings.write().unwrap_or_else(|p| p.into_inner());
                if let serde_json::Value::Object(mut incoming) =
                    lua_table_to_json_non_mutating(&params)
                {
                    if let Some(val) = incoming.remove("toast_duration_ms") {
                        incoming
                            .entry("notification_duration_ms".to_string())
                            .or_insert(val);
                    }
                    if let Some(val) = incoming.remove("toast_duration_max_ms") {
                        incoming
                            .entry("notification_max_duration_ms".to_string())
                            .or_insert(val);
                    }
                    if !cfg.settings.is_object() {
                        cfg.settings = serde_json::Value::Object(serde_json::Map::new());
                    }
                    if let Some(target_map) = cfg.settings.as_object_mut() {
                        for (k, v) in incoming {
                            target_map.insert(k, v);
                        }
                    }
                }
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua settings: {}", e))?;
        bar_table
            .set("settings", settings_fn)
            .map_err(|e| anyhow::anyhow!("lua set settings: {}", e))?;

        let style_table = lua
            .create_table()
            .map_err(|e| anyhow::anyhow!("lua create Style table: {}", e))?;
        let cfg_global_style = config.clone();
        let global_style_fn = lua
            .create_function(move |_lua, params: Table| {
                let mut cfg = cfg_global_style.write().unwrap_or_else(|p| p.into_inner());
                let mut style = lua_table_to_json_non_mutating(&params);
                if let Some(obj) = style.as_object_mut() {
                    obj.insert("default".to_string(), serde_json::Value::Bool(true));
                }
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

        Ok(())
    }

    fn finalize_config(
        config: &mut Self::Config,
    ) -> Result<(Self::StyleMap, Arc<dyn Any + Send + Sync>)> {
        resolve_json_style_inheritance(&mut config.styles)
            .map_err(|e| anyhow::anyhow!("init.lua execution failed: {}", e))?;
        Ok((config.styles.clone(), Arc::new(config.animations.clone())))
    }

    fn apply_accent(config: &mut Self::Config, styles: &RwLock<Self::StyleMap>, accent: &str) {
        for style in config.styles.values_mut() {
            if let Some(obj) = style.as_object_mut() {
                if obj.contains_key("accent")
                    || obj.get("role").and_then(|v| v.as_str()) == Some("accent")
                {
                    obj.insert(
                        "accent".to_string(),
                        serde_json::Value::String(accent.to_string()),
                    );
                }
            }
        }
        let _ = resolve_json_style_inheritance(&mut config.styles);
        *styles.write().unwrap_or_else(|p| p.into_inner()) = config.styles.clone();
    }

    fn apply_palette_primary(
        config: &mut Self::Config,
        styles: &RwLock<Self::StyleMap>,
        primary: Option<&str>,
    ) {
        if let Some(p) = primary {
            for style in config.styles.values_mut() {
                if let Some(obj) = style.as_object_mut() {
                    if obj.contains_key("accent")
                        || obj.get("role").and_then(|v| v.as_str()) == Some("accent")
                    {
                        obj.insert(
                            "accent".to_string(),
                            serde_json::Value::String(p.to_string()),
                        );
                    }
                }
            }
        }
        let _ = resolve_json_style_inheritance(&mut config.styles);
        *styles.write().unwrap_or_else(|p| p.into_inner()) = config.styles.clone();
    }

    fn push_keybind(config: &mut Self::Config, keybind: KeybindConfig) {
        config.keybinds.push(keybind);
    }

    fn parse_rebuilt_surface(
        _lua: &Lua,
        table: Table,
        _styles: &RwLock<Self::StyleMap>,
        _extra: &Arc<dyn Any + Send + Sync>,
    ) -> Result<Self::SurfaceOutput, String> {
        Ok(lua_table_to_json_non_mutating(&table))
    }
}

fn expand_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s.starts_with("~/") || s == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(s.strip_prefix("~/").unwrap_or(""));
        }
    }
    path.to_path_buf()
}

struct LuaRuntimeInner<A: LuaRuntimeAdapter> {
    lua: Lua,
    builders: HashMap<String, mlua::RegistryKey>,
    styles: Arc<RwLock<A::StyleMap>>,
    extra: Arc<dyn Any + Send + Sync>,
    config: Arc<RwLock<A::Config>>,
}

type ExtensionList<C> = Arc<Mutex<Vec<Arc<dyn LuaHostExtension<C>>>>>;

/// Unified Lua 5.4 runtime shared across `wyrd-script` (`LuaRuntime<GenericScriptAdapter>`)
/// and `wyrd-config` (`LuaRuntime<TypedShellAdapter>`).
pub struct LuaRuntime<A: LuaRuntimeAdapter = GenericScriptAdapter> {
    pub(crate) config_path: PathBuf,
    inner: Arc<Mutex<Option<LuaRuntimeInner<A>>>>,
    pending_rebuilds: Arc<Mutex<HashSet<String>>>,
    last_error_time: Arc<Mutex<HashMap<String, Instant>>>,
    extensions: ExtensionList<A::Config>,
}

impl<A: LuaRuntimeAdapter> Clone for LuaRuntime<A> {
    fn clone(&self) -> Self {
        Self {
            config_path: self.config_path.clone(),
            inner: self.inner.clone(),
            pending_rebuilds: self.pending_rebuilds.clone(),
            last_error_time: self.last_error_time.clone(),
            extensions: self.extensions.clone(),
        }
    }
}

pub fn json_to_lua(lua: &Lua, val: &serde_json::Value) -> mlua::Result<Value> {
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

/// Converts a store of module JSON values into a nested Lua state table for `build(state)` callbacks.
pub fn create_lua_state_snapshot(
    lua: &Lua,
    data_store: &HashMap<String, serde_json::Value>,
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

impl<A: LuaRuntimeAdapter> LuaRuntime<A> {
    /// Creates a new [`LuaRuntime`] rooted at `config_path` (expanding leading `~/`).
    pub fn new(config_path: PathBuf) -> Result<Self> {
        let config_path = expand_path(&config_path);
        Ok(Self {
            config_path,
            inner: Arc::new(Mutex::new(None)),
            pending_rebuilds: Arc::new(Mutex::new(HashSet::new())),
            last_error_time: Arc::new(Mutex::new(HashMap::new())),
            extensions: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Registers a custom host extension callback or [`LuaHostExtension`] implementation.
    pub fn register_lua_extension<E: LuaHostExtension<A::Config>>(&self, ext: E) {
        lock_unpoisoned(&self.extensions).push(Arc::new(ext));
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
        lock_unpoisoned(&self.pending_rebuilds).drain().collect()
    }

    pub fn get_styles(&self) -> Option<A::StyleMap> {
        lock_unpoisoned(&self.inner)
            .as_ref()
            .map(|i| read_unpoisoned(&i.styles).clone())
    }

    pub async fn load_config(&self) -> Result<A::Config> {
        let script = tokio::fs::read_to_string(&self.config_path)
            .await
            .with_context(|| format!("cannot read {:?}", self.config_path))?;
        self.load_config_from_str(&script)
    }

    pub fn load_config_from_str(&self, script: &str) -> Result<A::Config> {
        self.load_config_from_str_with_store(script, &HashMap::new())
    }

    pub fn load_config_from_str_with_store(
        &self,
        script: &str,
        initial_store: &HashMap<String, serde_json::Value>,
    ) -> Result<A::Config> {
        let lua = Lua::new();
        let bar_table = lua
            .create_table()
            .map_err(|e| anyhow::anyhow!("lua create_table: {}", e))?;

        let config = Arc::new(RwLock::new(A::Config::default()));
        let builders_map = Arc::new(Mutex::new(HashMap::new()));
        let extensions = lock_unpoisoned(&self.extensions).clone();

        self.register_common_and_adapter_functions(
            &lua,
            &bar_table,
            &config,
            &builders_map,
            initial_store,
            &extensions,
        )?;

        lua.load(script)
            .exec()
            .map_err(|e| anyhow::anyhow!("init.lua execution failed: {}", e))?;

        let mut cfg = config.write().unwrap_or_else(|p| p.into_inner());
        let (styles_snapshot, extra) = A::finalize_config(&mut cfg)?;

        let builders = std::mem::take(&mut *lock_unpoisoned(&builders_map));
        let inner = LuaRuntimeInner {
            lua,
            builders,
            styles: Arc::new(RwLock::new(styles_snapshot)),
            extra,
            config: config.clone(),
        };
        *lock_unpoisoned(&self.inner) = Some(inner);

        Ok(cfg.clone())
    }

    pub fn update_accent(&self, accent: &str) -> Result<()> {
        let inner_guard = lock_unpoisoned(&self.inner);
        let Some(inner) = inner_guard.as_ref() else {
            return Ok(());
        };

        let globals = inner.lua.globals();
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

        let mut cfg = inner.config.write().unwrap_or_else(|p| p.into_inner());
        A::apply_accent(&mut cfg, &inner.styles, accent);

        drop(cfg);
        drop(inner_guard);
        self.request_rebuild_all();
        Ok(())
    }

    pub fn update_palette(&self, palette: &serde_json::Value) -> Result<()> {
        let inner_guard = lock_unpoisoned(&self.inner);
        let Some(inner) = inner_guard.as_ref() else {
            return Ok(());
        };

        let map = if let Some(inner_map) = palette.get("palette").and_then(|v| v.as_object()) {
            inner_map
        } else if let Some(obj) = palette.as_object() {
            obj
        } else {
            return Ok(());
        };

        let globals = inner.lua.globals();
        if let Ok(lua_table) = inner.lua.create_table() {
            for (k, v) in map {
                if let Some(s) = v.as_str() {
                    let _ = lua_table.set(k.as_str(), s);
                }
            }
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

        let primary = map
            .get("primary")
            .or_else(|| map.get("accent"))
            .and_then(|v| v.as_str());

        let mut cfg = inner.config.write().unwrap_or_else(|p| p.into_inner());
        A::apply_palette_primary(&mut cfg, &inner.styles, primary);

        drop(cfg);
        drop(inner_guard);
        self.request_rebuild_all();
        Ok(())
    }

    pub fn build_surface_with_store(
        &self,
        name: &str,
        store: &HashMap<String, serde_json::Value>,
    ) -> Result<Option<A::SurfaceOutput>> {
        if let Some(err_time) = lock_unpoisoned(&self.last_error_time).get(name) {
            if err_time.elapsed() < Duration::from_secs(5) {
                log::debug!(
                    "Rebuild for surface '{}' suppressed by 5s error backoff",
                    name
                );
                return Ok(None);
            }
        }

        let mut inner_guard = lock_unpoisoned(&self.inner);
        let Some(inner) = inner_guard.as_mut() else {
            return Ok(None);
        };
        let Some(key) = inner.builders.get(name) else {
            return Ok(None);
        };
        let Ok(builder_fn) = inner.lua.registry_value::<mlua::Function>(key) else {
            return Ok(None);
        };
        let Ok(state_table) = create_lua_state_snapshot(&inner.lua, store) else {
            return Ok(None);
        };

        match builder_fn.call::<Value>(state_table) {
            Ok(Value::Table(tbl)) => {
                match A::parse_rebuilt_surface(&inner.lua, tbl, &inner.styles, &inner.extra) {
                    Ok(output) => Ok(Some(output)),
                    Err(e) => {
                        log::error!(
                            "Lua build() error parsing widgets for surface '{}':\n{}",
                            name,
                            e
                        );
                        lock_unpoisoned(&self.last_error_time)
                            .insert(name.to_string(), Instant::now());
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
                lock_unpoisoned(&self.last_error_time).insert(name.to_string(), Instant::now());
                Ok(None)
            }
            Err(e) => {
                log::error!("Lua build() error for surface '{}':\n{}", name, e);
                lock_unpoisoned(&self.last_error_time).insert(name.to_string(), Instant::now());
                Ok(None)
            }
        }
    }

    fn register_common_and_adapter_functions(
        &self,
        lua: &Lua,
        bar_table: &Table,
        config: &Arc<RwLock<A::Config>>,
        builders_map: &Arc<Mutex<HashMap<String, mlua::RegistryKey>>>,
        store_snapshot: &HashMap<String, serde_json::Value>,
        extensions: &[Arc<dyn LuaHostExtension<A::Config>>],
    ) -> Result<()> {
        let globals = lua.globals();

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

        A::register_adapter_functions(
            lua,
            bar_table,
            config,
            builders_map,
            &self.last_error_time,
            store_snapshot,
        )?;

        for ext in extensions {
            ext.register(lua, bar_table, config)
                .map_err(|e| anyhow::anyhow!("lua extension registration failed: {}", e))?;
        }

        globals
            .set("bar", bar_table.clone())
            .map_err(|e| anyhow::anyhow!("lua set bar: {}", e))?;
        globals
            .set("shell", bar_table.clone())
            .map_err(|e| anyhow::anyhow!("lua set shell: {}", e))?;
        globals
            .set("wyrd", bar_table.clone())
            .map_err(|e| anyhow::anyhow!("lua set wyrd: {}", e))?;

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

        Ok(())
    }
}

/// Registers the `wyrd.keybind(modifiers, key, action)` extension on any [`LuaRuntime<A>`].
pub fn register_keybind_extension<A: LuaRuntimeAdapter>(runtime: &LuaRuntime<A>) {
    runtime.register_lua_extension(
        |lua: &Lua, wyrd_table: &Table, config: &Arc<RwLock<A::Config>>| {
            let cfg_keybind = config.clone();
            let keybind_fn =
                lua.create_function(move |_lua, (mods, key, action): (String, String, String)| {
                    let mut cfg = cfg_keybind.write().unwrap_or_else(|p| p.into_inner());
                    A::push_keybind(
                        &mut cfg,
                        KeybindConfig {
                            modifiers: mods,
                            key,
                            action,
                        },
                    );
                    Ok(())
                })?;
            wyrd_table.set("keybind", keybind_fn)?;
            Ok(())
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generic_lua_runtime_and_extension() {
        let runtime: LuaRuntime = LuaRuntime::new(PathBuf::from("/tmp/init.lua")).unwrap();
        runtime.register_lua_extension(
            |lua: &Lua, wyrd_table: &Table, config: &Arc<RwLock<ScriptConfig>>| {
                let cfg_clone = config.clone();
                let f = lua.create_function(move |_lua, msg: String| {
                    let mut cfg = cfg_clone.write().unwrap_or_else(|p| p.into_inner());
                    if let Some(obj) = cfg.settings.as_object_mut() {
                        obj.insert("wallpaper".to_string(), serde_json::Value::String(msg));
                    }
                    Ok(())
                })?;
                wyrd_table.set("set_wallpaper", f)?;
                Ok(())
            },
        );

        let cfg = runtime
            .load_config_from_str(
                r##"
                wyrd.style("base", { background = "#11111b", accent = "#89b4fa" })
                wyrd.style("derived", { extends = "base", radius = 8 })
                wyrd.set_wallpaper("/usr/share/backgrounds/forest.jpg")
                "##,
            )
            .unwrap();

        assert_eq!(
            cfg.setting_str("wallpaper", ""),
            "/usr/share/backgrounds/forest.jpg"
        );
        let derived = cfg.styles.get("derived").unwrap();
        assert_eq!(derived.get("background").unwrap().as_str(), Some("#11111b"));
        assert_eq!(derived.get("radius").unwrap().as_i64(), Some(8));
    }

    #[test]
    fn test_create_does_not_mutate_caller_table_build_field() {
        let runtime: LuaRuntime = LuaRuntime::new(PathBuf::from("/tmp/init.lua")).unwrap();
        let cfg = runtime
            .load_config_from_str(
                r#"
                local spec = {
                    name = "non_mutated_surface",
                    type = "bar",
                    build = function(state)
                        return { { type = "label", text = "ok" } }
                    end,
                }
                wyrd.create(spec)
                assert(type(spec.build) == "function", "wyrd.create must not mutate spec.build to nil")
                "#,
            )
            .unwrap();

        assert_eq!(cfg.surfaces.len(), 1);
        assert_eq!(
            cfg.surfaces[0]
                .get("has_build_fn")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }
}
