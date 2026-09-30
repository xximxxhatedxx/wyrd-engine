//! Typed shell configuration adapter (`TypedShellAdapter`) over [`wyrd_script::config::lua::LuaRuntime`].
//!
//! `wyrd-config` does not duplicate [`LuaRuntime`]; instead, it implements [`LuaRuntimeAdapter`]
//! to plug its typed widget/surface/layout/style parsers (`parsers.rs`) into `wyrd-script`'s runtime.

pub mod parsers;

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use anyhow::Result;
use mlua::{Lua, Table, Value};

use super::{AnimationConfig, BarConfig, KeybindConfig, ModuleConfig, StyleConfig, WidgetConfig};
use parsers::{
    parse_animation_config_table, parse_style_params, parse_surface_params, parse_widgets_array,
};
pub use wyrd_script::config::lua::{
    create_lua_state_snapshot, lua_table_to_json_non_mutating, register_keybind_extension,
    LuaExtensionFn, LuaHostExtension, LuaRuntimeAdapter,
};

/// Adapter plugging `wyrd-config`'s typed [`BarConfig`] (`ShellConfig`), [`StyleConfig`],
/// [`AnimationConfig`], and [`WidgetConfig`] parsers into [`wyrd_script::config::lua::LuaRuntime`].
pub struct TypedShellAdapter;

impl LuaRuntimeAdapter for TypedShellAdapter {
    type Config = BarConfig;
    type StyleMap = HashMap<String, StyleConfig>;
    type SurfaceOutput = Vec<WidgetConfig>;

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
        let last_error_time_clone = last_error_time.clone();
        let store_snapshot_clone = store_snapshot.clone();

        let create_fn = lua
            .create_function(move |lua, params: Table| {
                let mut cfg = cfg_clone.write().unwrap_or_else(|p| p.into_inner());
                let cfg_guard = &mut *cfg;
                let surface = parse_surface_params(
                    lua,
                    params,
                    &mut cfg_guard.styles,
                    &cfg_guard.animations,
                    &builders_clone,
                    &last_error_time_clone,
                    &store_snapshot_clone,
                )
                .map_err(|e| mlua::Error::RuntimeError(format!("{}", e)))?;
                cfg_guard.surfaces.push(surface);
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("lua create_function: {}", e))?;
        bar_table
            .set("create", create_fn)
            .map_err(|e| anyhow::anyhow!("lua set create: {}", e))?;

        let cfg_clone = config.clone();
        let style_fn = lua
            .create_function(move |lua, args: mlua::MultiValue| {
                let mut cfg = cfg_clone.write().unwrap_or_else(|p| p.into_inner());
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
                let mut cfg = cfg_clone_resolve.write().unwrap_or_else(|p| p.into_inner());
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
                let mut cfg = cfg_clone.write().unwrap_or_else(|p| p.into_inner());
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
            .map_err(|e| anyhow::anyhow!("lua create_function: {}", e))?;
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

        let cfg_clone_anim = config.clone();
        let animation_fn = lua
            .create_function(move |_lua, args: mlua::MultiValue| {
                let mut cfg = cfg_clone_anim.write().unwrap_or_else(|p| p.into_inner());
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
                        log::warn!(
                            "setting 'toast_duration_ms' is deprecated, use 'notification_duration_ms'"
                        );
                        incoming
                            .entry("notification_duration_ms".to_string())
                            .or_insert(val);
                    }
                    if let Some(val) = incoming.remove("toast_duration_max_ms") {
                        log::warn!(
                            "setting 'toast_duration_max_ms' is deprecated, use 'notification_max_duration_ms'"
                        );
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
        let cfg_clone = config.clone();
        let global_style_fn = lua
            .create_function(move |lua, params: Table| {
                let mut cfg = cfg_clone.write().unwrap_or_else(|p| p.into_inner());
                let mut style = parse_style_params(lua, params)
                    .map_err(|e| mlua::Error::RuntimeError(format!("{}", e)))?;
                style.default = true;
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
        crate::widgets::resolve_style_inheritance(&mut config.styles)
            .map_err(|e| anyhow::anyhow!("init.lua execution failed: {}", e))?;
        Ok((config.styles.clone(), Arc::new(config.animations.clone())))
    }

    fn apply_accent(config: &mut Self::Config, styles: &RwLock<Self::StyleMap>, accent: &str) {
        for style in config.styles.values_mut() {
            if style.accent.is_some() || style.role.as_deref() == Some("accent") {
                style.accent = Some(accent.to_string());
            }
        }
        let _ = crate::widgets::resolve_style_inheritance(&mut config.styles);
        *styles.write().unwrap_or_else(|p| p.into_inner()) = config.styles.clone();
    }

    fn apply_palette_primary(
        config: &mut Self::Config,
        styles: &RwLock<Self::StyleMap>,
        primary: Option<&str>,
    ) {
        if let Some(p) = primary {
            for style in config.styles.values_mut() {
                if style.accent.is_some() || style.role.as_deref() == Some("accent") {
                    style.accent = Some(p.to_string());
                }
            }
        }
        let _ = crate::widgets::resolve_style_inheritance(&mut config.styles);
        *styles.write().unwrap_or_else(|p| p.into_inner()) = config.styles.clone();
    }

    fn push_keybind(config: &mut Self::Config, keybind: KeybindConfig) {
        config.keybinds.push(keybind);
    }

    fn parse_rebuilt_surface(
        lua: &Lua,
        table: Table,
        styles: &RwLock<Self::StyleMap>,
        extra: &Arc<dyn Any + Send + Sync>,
    ) -> Result<Self::SurfaceOutput, String> {
        let mut styles_guard = styles.write().unwrap_or_else(|p| p.into_inner());
        let empty_anims = HashMap::<String, AnimationConfig>::new();
        let animations = extra
            .downcast_ref::<HashMap<String, AnimationConfig>>()
            .unwrap_or(&empty_anims);
        parse_widgets_array(lua, table, &mut styles_guard, animations).map_err(|e| e.to_string())
    }
}

/// Typed [`LuaRuntime`] specialization for `wyrd-config` backed directly by
/// [`wyrd_script::config::lua::LuaRuntime<TypedShellAdapter>`].
pub type LuaRuntime = wyrd_script::config::lua::LuaRuntime<TypedShellAdapter>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SurfaceKind;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_load_aetheria_init_lua() {
        let runtime = LuaRuntime::new(PathBuf::from("/tmp/init.lua")).unwrap();
        let script = r##"
            wyrd.style("bar", { background = "#11111b", foreground = "#cdd6f4", radius = 8 })
            wyrd.style("popup", { background = "#1e1e2e", foreground = "#cdd6f4", radius = 12 })
            wyrd.style("chip", { background = "#313244", foreground = "#cdd6f4", radius = 6 })
            wyrd.style("quicksettings_popup", { extends = "popup", radius = 14 })
            for _, name in ipairs({"launcher", "audio", "network", "bluetooth", "battery", "calendar", "system", "notifications", "power", "quicksettings"}) do
                wyrd.create {
                    name = name,
                    type = "popup",
                    style = name == "quicksettings" and "quicksettings_popup" or "popup",
                    widgets = { { type = "text", text = name } },
                }
            end
        "##;
        let cfg = runtime.load_config_from_str(script).unwrap();
        assert!(!cfg.surfaces.is_empty(), "Expected surfaces to be defined");
        assert!(cfg.styles.contains_key("bar"));
        assert!(cfg.styles.contains_key("popup"));
        assert!(cfg.styles.contains_key("chip"));
        let popup_names: Vec<String> = cfg
            .surfaces
            .iter()
            .filter(|s| s.ty == SurfaceKind::Popup)
            .map(|s| s.name.clone())
            .collect();
        assert!(popup_names.contains(&"launcher".to_string()));
        assert!(popup_names.contains(&"quicksettings".to_string()));
        assert!(cfg.styles.contains_key("quicksettings_popup"));
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
                role = "bar",
                background = "#000000",
            })
            wyrd.style("card", {
                role = "surface",
                background = "#222222",
            })

            local hook_called = false
            wyrd._on_theme_palette = function(palette)
                hook_called = true
                wyrd.style("primary", {
                    accent = palette.primary,
                })
                wyrd.style("bar", {
                    background = palette.background .. "d9",
                })
                wyrd.style("card", {
                    background = palette.surface_container .. "d9",
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
        for (theme_name, accent) in &[
            ("nord", "#88c0d0"),
            ("gruvbox", "#fabd2f"),
            ("tokyo-night", "#7aa2f7"),
        ] {
            let script = format!(
                r##"
                wyrd.style("bar", {{ background = "#1a1b26", foreground = "#c0caf5", accent = "{accent}" }})
                wyrd.style("popup", {{ extends = "bar", radius = 12 }})
                wyrd.style("quicksettings_popup", {{ extends = "popup", radius = 14 }})
                "##
            );
            let runtime = LuaRuntime::new(PathBuf::from("/tmp/init.lua")).unwrap();
            let cfg = runtime
                .load_config_from_str(&script)
                .unwrap_or_else(|e| panic!("Failed to load theme '{theme_name}': {e}"));
            assert!(cfg.styles.contains_key("bar"));
            assert!(cfg.styles.contains_key("popup"));
            assert!(cfg.styles.contains_key("quicksettings_popup"));
        }
    }

    #[tokio::test]
    async fn test_lua_bar_settings() {
        let script = r#"
            bar.settings {
                slider_release_window_ms = 1200,
                slider_dispatch_throttle_ms = 45,
                memory_trim_interval_s = 60,
                module_init_grace_ms = 80,
                popup_resize_spring_stiffness = 420.0,
                popup_resize_spring_damping = 35.0,
                notification_duration_ms = 4000,
                notification_max_duration_ms = 7000,
            }
        "#;
        let runtime = LuaRuntime::new(std::path::PathBuf::from("/tmp/init.lua")).unwrap();
        let cfg = runtime.load_config_from_str(script).unwrap();
        assert_eq!(cfg.setting_u64("slider_release_window_ms", 0), 1200);
        assert_eq!(cfg.setting_u64("slider_dispatch_throttle_ms", 0), 45);
        assert_eq!(cfg.setting_u64("memory_trim_interval_s", 0), 60);
        assert_eq!(cfg.setting_u64("module_init_grace_ms", 0), 80);
        assert_eq!(cfg.setting_f32("popup_resize_spring_stiffness", 0.0), 420.0);
        assert_eq!(cfg.setting_f32("popup_resize_spring_damping", 0.0), 35.0);
        assert_eq!(cfg.setting_u64("notification_duration_ms", 0), 4000);
        assert_eq!(cfg.setting_u64("notification_max_duration_ms", 0), 7000);
    }

    #[test]
    fn test_register_lua_extension() {
        let runtime = LuaRuntime::new(std::path::PathBuf::from("/tmp/init.lua")).unwrap();
        runtime.register_lua_extension(
            |lua: &Lua, wyrd_table: &Table, config: &Arc<RwLock<BarConfig>>| {
                let cfg_clone = config.clone();
                let custom_fn = lua.create_function(move |_lua, greeting: String| {
                    let mut cfg = cfg_clone.write().unwrap_or_else(|p| p.into_inner());
                    if let Some(map) = cfg.settings.as_object_mut() {
                        map.insert(
                            "custom_greeting".to_string(),
                            serde_json::Value::String(greeting),
                        );
                    }
                    Ok(())
                })?;
                wyrd_table.set("greet", custom_fn)?;
                Ok(())
            },
        );

        let cfg = runtime
            .load_config_from_str(r#"wyrd.greet("hello from extension")"#)
            .unwrap();
        assert_eq!(
            cfg.setting_str("custom_greeting", ""),
            "hello from extension"
        );
    }
}
