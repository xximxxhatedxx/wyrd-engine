//! Wyrd Engine Library (Facade Crate)
//!
//! Re-exports the modular Wyrd subcrates (`wyrd-graphics`, `wyrd-widgets`,
//! `wyrd-config`, `wyrd-wayland`, `wyrd-script`, `wyrd-wasm`, `wyrd-state`,
//! `wyrd-compositor`) under a unified namespace for desktop shells, greeters,
//! and wallpaper daemons.

pub mod compositor_ext;

pub mod compositor {
    pub use crate::compositor_ext::{resolve_cursor_output_id, sync_keybinds};
    pub use wyrd_compositor::compositor::*;
}
#[cfg(feature = "config")]
pub use wyrd_config::config;
pub use wyrd_graphics::{animator, event_bus, icons, sync_helpers};
#[cfg(not(feature = "config"))]
pub use wyrd_script::config;
pub use wyrd_state::state_files;
#[cfg(feature = "wasm")]
pub use wyrd_wasm::modules;
pub use wyrd_wayland::{input, surface_manager, OutputRuntime, OutputUserData};
pub use wyrd_widgets::widgets;

#[cfg(feature = "config")]
pub type ShellState = wyrd_wayland::ShellState<config::ShellConfig, widgets::WidgetTree>;
#[cfg(not(feature = "config"))]
pub type ShellState = wyrd_wayland::ShellState<config::ScriptConfig, widgets::WidgetTree>;

pub type BarState = ShellState;
pub type EngineState = ShellState;

/// Wayland protocol layer and backend specialized for [`ShellState`].
pub mod wayland {
    pub use wyrd_wayland::wayland::*;

    pub mod backend {
        pub use wyrd_wayland::wayland::backend::WaylandFd;
        pub type WaylandBackend = wyrd_wayland::wayland::backend::WaylandBackend<
            super::super::config::ShellConfig,
            super::super::widgets::WidgetTree,
        >;
    }
}

/// Rendering pipeline and deprecated widget-to-scene bridge re-exports.
pub mod render {
    pub use wyrd_graphics::render::*;

    #[deprecated(since = "0.2.0", note = "use wyrd_widgets::render_bridge directly")]
    pub use wyrd_widgets::render_bridge::{
        build_scene_node, build_scene_node_with_anim, render_surface,
    };
}
