//! Surface & Output Manager: multi-monitor, HiDPI, surface lifecycle.

use crate::wayland::output::OutputState;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceBinding {
    pub name: String,
    pub output_name: Option<String>,
    pub dirty: bool,
}

/// Owns all bar surfaces and their output bindings.
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

    /// Create or update surfaces when outputs change.
    pub fn on_outputs_changed(
        &mut self,
        outputs: Vec<OutputState>,
        config_surfaces: &[crate::config::SurfaceConfig],
    ) {
        self.outputs = outputs;

        for cfg in config_surfaces {
            if let Some(ref output_name) = cfg.output {
                if self.outputs.iter().any(|o| &o.name == output_name) {
                    self.ensure_surface(cfg, Some(output_name));
                }
            } else {
                let output_names: Vec<String> =
                    self.outputs.iter().map(|o| o.name.clone()).collect();
                for name in output_names {
                    self.ensure_surface(cfg, Some(&name));
                }
            }
        }

        self.remove_missing_outputs();
    }

    fn ensure_surface(&mut self, cfg: &crate::config::SurfaceConfig, output_name: Option<&str>) {
        let key = format!("{}@{:?}", cfg.name, output_name);
        self.surfaces.entry(key).or_insert_with_key(|k| {
            log::info!("Creating surface binding: {}", k);
            SurfaceBinding {
                name: cfg.name.clone(),
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
