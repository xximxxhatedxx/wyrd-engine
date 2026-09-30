//! Typed shell configuration (`ShellConfig`, `SurfaceConfig`, `SurfaceKind`, `Layer`,
//! `KeyboardInteractivity`) and Lua widget/surface/layout/style parsers (`parse_widget`,
//! `parse_surface_params`, `parse_layout`, `parse_style_params`) for Wyrd.

pub use wyrd_graphics::{animator, sync_helpers};
pub use wyrd_widgets::widgets;

pub mod config;
pub use config::*;

#[cfg(feature = "lua")]
pub use config::lua::parsers::{
    parse_animation_config_table, parse_layout, parse_style_params, parse_surface_params,
    parse_widget, parse_widgets_array,
};
