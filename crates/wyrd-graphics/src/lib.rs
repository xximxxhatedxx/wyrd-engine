//! Graphics, rendering pipeline, scene graph, text shaping, icon resolution, event bus, and spring animations for Wyrd.

pub mod animator;
pub mod event_bus;
pub mod icons;
pub mod render;
pub mod sync_helpers;

use render::scene::WidgetId;

/// Trait for resolving focusable items and spatial hit-testing without coupling
/// input/protocol crates (`wyrd-wayland`) to widget-tree implementations (`wyrd-widgets`).
pub trait FocusableScene {
    /// Returns the logical identifier/name of the item with the given `id`, if present.
    fn node_name(&self, id: WidgetId) -> Option<&str>;

    /// Performs a spatial hit-test at `(x, y)` in surface-local coordinates.
    fn hit_test_node(&self, x: f64, y: f64) -> Option<WidgetId>;
}

/// Trait for describing a surface's name and optional target output without coupling
/// surface lifecycle management (`wyrd-wayland`) to configuration parsers (`wyrd-config`).
pub trait SurfaceSpec {
    /// Logical surface name (e.g., `"bar"`, `"launcher"`).
    fn surface_name(&self) -> &str;

    /// Target output connector name (e.g., `Some("DP-1")`), or `None` for all outputs.
    fn target_output(&self) -> Option<&str>;
}
