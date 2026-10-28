//! Wyrd Engine Library (Facade Crate)
//!
//! Re-exports the modular Wyrd subcrates (`wyrd-graphics`, `wyrd-widgets`,
//! `wyrd-settings`, `wyrd-wayland`, `wyrd-script`, `wyrd-wasm`, `wyrd-state`,
//! `wyrd-compositor`) under a unified namespace for desktop shells, greeters,
//! and wallpaper daemons.

#[cfg(feature = "compositor")]
pub mod compositor_ext;

#[cfg(feature = "compositor")]
pub mod compositor {
    pub use crate::compositor_ext::{resolve_cursor_output_id, sync_keybinds};
    pub use wyrd_compositor::compositor::*;
}
pub use wyrd_graphics::{animator, event_bus, icons};
#[cfg(not(feature = "config"))]
pub use wyrd_script::config;
#[cfg(feature = "config")]
pub use wyrd_settings::config;
pub use wyrd_state::state_files;
#[cfg(feature = "wasm")]
pub use wyrd_wasm::modules;
#[cfg(feature = "wayland")]
pub use wyrd_wayland::{
    input, surface_manager, OutputRuntime, OutputUserData, SurfaceBackend, SurfaceBackendError,
    SurfaceBackendKind, SurfaceCapabilities,
};
pub use wyrd_widgets::widgets;

#[cfg(all(feature = "wayland", feature = "config"))]
pub type ShellState = wyrd_wayland::ShellState<config::ShellConfig, widgets::WidgetTree>;
#[cfg(all(feature = "wayland", not(feature = "config")))]
pub type ShellState = wyrd_wayland::ShellState<config::ScriptConfig, widgets::WidgetTree>;

#[cfg(feature = "wayland")]
pub type BarState = ShellState;
#[cfg(feature = "wayland")]
pub type EngineState = ShellState;

/// Wayland protocol layer and backend specialized for [`ShellState`].
#[cfg(feature = "wayland")]
pub mod wayland {
    pub use wyrd_wayland::wayland::*;

    pub mod backend {
        pub use wyrd_wayland::wayland::backend::WaylandFd;

        #[cfg(feature = "config")]
        pub type WaylandBackend = wyrd_wayland::wayland::backend::WaylandBackend<
            super::super::config::ShellConfig,
            super::super::widgets::WidgetTree,
        >;

        #[cfg(not(feature = "config"))]
        pub type WaylandBackend = wyrd_wayland::wayland::backend::WaylandBackend<
            super::super::config::ScriptConfig,
            super::super::widgets::WidgetTree,
        >;
    }
}

/// Rendering pipeline and widget-to-scene bridge convenience re-exports.
pub mod render {
    pub use wyrd_graphics::render::*;

    /// Re-exports from `wyrd_widgets::render_bridge` for backwards compatibility.
    /// Prefer importing directly from `wyrd_widgets::render_bridge`.
    pub use wyrd_widgets::render_bridge::{
        build_scene_node, build_scene_node_with_anim, render_surface,
    };
}
