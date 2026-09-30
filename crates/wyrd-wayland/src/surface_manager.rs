//! Surface & Output Manager: multi-monitor, HiDPI, surface lifecycle.

use crate::wayland::output::OutputState;
use std::collections::HashMap;
use wyrd_graphics::SurfaceSpec;

/// Descriptor for a surface binding when managing surfaces without `wyrd-config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceDescriptor {
    pub name: String,
    pub output: Option<String>,
}

impl SurfaceSpec for SurfaceDescriptor {
    fn surface_name(&self) -> &str {
        &self.name
    }

    fn target_output(&self) -> Option<&str> {
        self.output.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceBinding {
    pub name: String,
    pub output_name: Option<String>,
    pub dirty: bool,
}

/// Manages active Wayland surface bindings across connected outputs.
pub struct SurfaceManager {
    pub surfaces: HashMap<String, SurfaceBinding>,
    pub outputs: Vec<OutputState>,
}

impl Default for SurfaceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceManager {
    pub fn new() -> Self {
        Self {
            surfaces: HashMap::new(),
            outputs: Vec::new(),
        }
    }

    /// Reconciles surface bindings with the current connected `outputs` and `config_surfaces`.
    pub fn on_outputs_changed<S: SurfaceSpec>(
        &mut self,
        outputs: Vec<OutputState>,
        config_surfaces: &[S],
    ) {
        self.outputs = outputs;

        for cfg in config_surfaces {
            if let Some(output_name) = cfg.target_output() {
                if self.outputs.iter().any(|o| o.name == output_name) {
                    self.ensure_surface(cfg.surface_name(), Some(output_name));
                }
            } else {
                let output_names: Vec<String> =
                    self.outputs.iter().map(|o| o.name.clone()).collect();
                for name in output_names {
                    self.ensure_surface(cfg.surface_name(), Some(&name));
                }
            }
        }

        self.remove_missing_outputs();
    }

    fn ensure_surface(&mut self, surface_name: &str, output_name: Option<&str>) {
        let key = format!("{}@{:?}", surface_name, output_name);
        self.surfaces.entry(key).or_insert_with_key(|k| {
            log::info!("Creating surface binding: {}", k);
            SurfaceBinding {
                name: surface_name.to_owned(),
                output_name: output_name.map(str::to_owned),
                dirty: true,
            }
        });
    }

    pub fn remove_missing_outputs(&mut self) {
        let output_names: std::collections::HashSet<&str> = self
            .outputs
            .iter()
            .map(|output| output.name.as_str())
            .collect();
        self.surfaces.retain(|_, surface| {
            surface
                .output_name
                .as_ref()
                .is_none_or(|name| output_names.contains(name.as_str()))
        });
    }

    pub fn mark_all_dirty(&mut self) {
        for s in self.surfaces.values_mut() {
            s.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SurfaceManager;

    #[test]
    fn removes_bindings_for_disconnected_outputs() {
        let mut manager = SurfaceManager::new();
        manager.surfaces.insert(
            "bar@DP-1".into(),
            super::SurfaceBinding {
                name: "bar".into(),
                output_name: Some("DP-1".into()),
                dirty: false,
            },
        );
        manager.surfaces.insert(
            "bar@HDMI-A-1".into(),
            super::SurfaceBinding {
                name: "bar".into(),
                output_name: Some("HDMI-A-1".into()),
                dirty: false,
            },
        );

        manager.outputs.clear();
        manager.remove_missing_outputs();

        assert!(manager.surfaces.is_empty());
    }
}
