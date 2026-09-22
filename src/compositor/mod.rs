use std::sync::Arc;

pub mod auto;
pub mod hyprland;
pub mod niri;
pub mod none;
pub mod sway;
pub mod types;

pub use crate::compositor_ext::{resolve_cursor_output_id, sync_keybinds};
pub use auto::AutoIntegration;
pub use hyprland::{hyprland_event_socket_path, hyprland_socket_path, HyprlandIntegration};
pub use niri::{niri_socket_path, NiriIntegration};
pub use none::NoneIntegration;
pub use sway::{sway_socket_path, SwayIntegration};
pub use types::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CompositorChoice {
    #[default]
    Auto,
    Hyprland,
    Niri,
    Sway,
    None,
}

pub trait CompositorIntegration: Send + Sync {
    fn name(&self) -> &'static str;
    fn register_keybind(&self, mods: &str, key: &str, action: &str) -> anyhow::Result<()>;
    fn cursor_position(&self) -> Option<(i32, i32)>;
    fn focused_monitor(&self) -> Option<String>;
    fn switch_workspace(&self, id: &str) -> anyhow::Result<()>;

    fn list_windows(&self) -> Vec<ToplevelInfo> {
        Vec::new()
    }

    fn active_window(&self) -> Option<ToplevelInfo> {
        self.list_windows().into_iter().find(|w| w.active)
    }

    fn list_workspaces(&self) -> Vec<WorkspaceInfo> {
        Vec::new()
    }

    fn active_workspace(&self) -> Option<WorkspaceInfo> {
        self.list_workspaces().into_iter().find(|w| w.active)
    }

    fn keyboard_layout(&self) -> Option<KeyboardInfo> {
        None
    }

    fn switch_keyboard_layout(&self, _target: &str) -> anyhow::Result<()> {
        anyhow::bail!("unsupported by generic compositor backend")
    }

    fn exit(&self) -> anyhow::Result<()> {
        anyhow::bail!("exit unsupported by generic compositor backend")
    }

    fn run_event_loop(&self, on_change: &dyn Fn()) {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            on_change();
        }
    }

    fn query_status(&self) -> WindowStatus {
        let list = self.list_windows();
        let active = self
            .active_window()
            .or_else(|| list.iter().find(|w| w.active).cloned());
        let workspaces = self.list_workspaces();
        let keyboard = self.keyboard_layout();
        WindowStatus {
            active,
            list,
            workspaces,
            keyboard,
        }
    }
}

pub fn create_compositor_integration(
    choice: impl Into<CompositorChoice>,
) -> Arc<dyn CompositorIntegration> {
    match choice.into() {
        CompositorChoice::Hyprland => Arc::new(HyprlandIntegration::new()),
        CompositorChoice::Niri => Arc::new(NiriIntegration::new()),
        CompositorChoice::Sway => Arc::new(SwayIntegration::new()),
        CompositorChoice::None => Arc::new(NoneIntegration::new()),
        CompositorChoice::Auto => Arc::new(AutoIntegration::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compositor_names() {
        assert_eq!(HyprlandIntegration::new().name(), "hyprland");
        assert_eq!(NiriIntegration::new().name(), "niri");
        assert_eq!(SwayIntegration::new().name(), "sway");
        assert_eq!(NoneIntegration::new().name(), "none");
    }

    #[test]
    fn test_unsupported_keybinds_log_and_succeed() {
        let niri = NiriIntegration::new();
        assert!(niri
            .register_keybind("Super", "Space", "popup:launcher")
            .is_ok());

        let sway = SwayIntegration::new();
        assert!(sway
            .register_keybind("Super", "Space", "popup:launcher")
            .is_ok());

        let none = NoneIntegration::new();
        assert!(none
            .register_keybind("Super", "Space", "popup:launcher")
            .is_ok());
    }

    #[test]
    fn test_create_compositor_explicit_choices() {
        let c_hypr = create_compositor_integration(CompositorChoice::Hyprland);
        assert_eq!(c_hypr.name(), "hyprland");

        let c_niri = create_compositor_integration(CompositorChoice::Niri);
        assert_eq!(c_niri.name(), "niri");

        let c_sway = create_compositor_integration(CompositorChoice::Sway);
        assert_eq!(c_sway.name(), "sway");

        let c_none = create_compositor_integration(CompositorChoice::None);
        assert_eq!(c_none.name(), "none");
    }

    #[test]
    fn test_none_integration_defaults() {
        let none = NoneIntegration::new();
        assert!(none.list_windows().is_empty());
        assert!(none.list_workspaces().is_empty());
        assert!(none.active_window().is_none());
        assert!(none.active_workspace().is_none());
        // keyboard_layout() returns None when headless, or Some if connected to live Wayland session
        let _ = none.keyboard_layout();
        assert!(none.switch_workspace("1").is_err());
        assert!(none.switch_keyboard_layout("next").is_err());
    }
}
