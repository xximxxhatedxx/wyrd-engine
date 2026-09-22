use super::hyprland::{hyprland_socket_path, HyprlandIntegration};
use super::niri::{niri_socket_path, NiriIntegration};
use super::none::NoneIntegration;
use super::sway::{sway_socket_path, SwayIntegration};
use super::types::{KeyboardInfo, ToplevelInfo, WindowStatus, WorkspaceInfo};
use super::CompositorIntegration;
use std::sync::{Arc, RwLock};
use std::time::Duration;

fn detect_backend() -> Option<Arc<dyn CompositorIntegration>> {
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() || hyprland_socket_path().is_some() {
        Some(Arc::new(HyprlandIntegration::new()))
    } else if std::env::var("NIRI_SOCKET").is_ok() || niri_socket_path().is_some() {
        Some(Arc::new(NiriIntegration::new()))
    } else if std::env::var("SWAYSOCK").is_ok() || sway_socket_path().is_some() {
        Some(Arc::new(SwayIntegration::new()))
    } else {
        None
    }
}

pub struct AutoIntegration {
    current: RwLock<Arc<dyn CompositorIntegration>>,
}

impl Default for AutoIntegration {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoIntegration {
    pub fn new() -> Self {
        let initial = detect_backend().unwrap_or_else(|| {
            log::info!("Generic Wayland layer-shell protocol active (River, Labwc, Wayfire, etc.)");
            Arc::new(NoneIntegration::new())
        });
        if initial.name() != "none" {
            log::info!("Detected compositor: {}", initial.name());
        }
        Self {
            current: RwLock::new(initial),
        }
    }

    fn backend(&self) -> Arc<dyn CompositorIntegration> {
        let cur = self.current.read().unwrap().clone();
        if cur.name() == "none" {
            if let Some(new_comp) = detect_backend() {
                log::info!("Compositor dynamically detected: {}", new_comp.name());
                *self.current.write().unwrap() = new_comp.clone();
                return new_comp;
            }
        }
        cur
    }
}

impl CompositorIntegration for AutoIntegration {
    fn name(&self) -> &'static str {
        match self.backend().name() {
            "hyprland" => "hyprland",
            "niri" => "niri",
            "sway" => "sway",
            _ => "none",
        }
    }

    fn register_keybind(&self, mods: &str, key: &str, action: &str) -> anyhow::Result<()> {
        self.backend().register_keybind(mods, key, action)
    }

    fn cursor_position(&self) -> Option<(i32, i32)> {
        self.backend().cursor_position()
    }

    fn focused_monitor(&self) -> Option<String> {
        self.backend().focused_monitor()
    }

    fn switch_workspace(&self, id: &str) -> anyhow::Result<()> {
        self.backend().switch_workspace(id)
    }

    fn list_windows(&self) -> Vec<ToplevelInfo> {
        self.backend().list_windows()
    }

    fn active_window(&self) -> Option<ToplevelInfo> {
        self.backend().active_window()
    }

    fn list_workspaces(&self) -> Vec<WorkspaceInfo> {
        self.backend().list_workspaces()
    }

    fn active_workspace(&self) -> Option<WorkspaceInfo> {
        self.backend().active_workspace()
    }

    fn keyboard_layout(&self) -> Option<KeyboardInfo> {
        self.backend().keyboard_layout()
    }

    fn switch_keyboard_layout(&self, target: &str) -> anyhow::Result<()> {
        self.backend().switch_keyboard_layout(target)
    }

    fn exit(&self) -> anyhow::Result<()> {
        self.backend().exit()
    }

    fn query_status(&self) -> WindowStatus {
        self.backend().query_status()
    }

    fn run_event_loop(&self, on_change: &dyn Fn()) {
        loop {
            let comp = self.backend();
            if comp.name() != "none" {
                on_change();
                comp.run_event_loop(on_change);
                *self.current.write().unwrap() = Arc::new(NoneIntegration::new());
                on_change();
            } else {
                std::thread::sleep(Duration::from_millis(500));
                if let Some(new_comp) = detect_backend() {
                    log::info!(
                        "Compositor dynamically detected in event loop: {}",
                        new_comp.name()
                    );
                    *self.current.write().unwrap() = new_comp;
                    on_change();
                }
            }
        }
    }
}
