//! Compositor extension helpers bridging `wyrd-compositor`, `wyrd-wayland`, and `wyrd-config`.

use crate::compositor::CompositorIntegration;
use std::sync::{Arc, Mutex};
use wayland_client::backend::ObjectId;

/// Registers each keybind in `keybinds` with the active compositor backend.
pub fn sync_keybinds(
    compositor: &dyn CompositorIntegration,
    keybinds: &[wyrd_script::config::KeybindConfig],
) {
    if keybinds.is_empty() {
        return;
    }
    for kb in keybinds {
        if let Err(e) = compositor.register_keybind(&kb.modifiers, &kb.key, &kb.action) {
            log::warn!(
                "Failed to register keybind {} + {} -> {}: {}",
                kb.modifiers,
                kb.key,
                kb.action,
                e
            );
        }
    }
}

/// Resolves the Wayland output [`ObjectId`] corresponding to `explicit_output`,
/// the compositor's cursor position, or the focused monitor.
pub fn resolve_cursor_output_id<C, T>(
    compositor: &dyn CompositorIntegration,
    state: &Arc<Mutex<wyrd_wayland::ShellState<C, T>>>,
    explicit_output: Option<&str>,
) -> Option<ObjectId> {
    let cursor_pos = compositor.cursor_position();
    let focused_monitor = compositor.focused_monitor();
    wyrd_wayland::resolve_cursor_output_id(
        cursor_pos,
        focused_monitor.as_deref(),
        state,
        explicit_output,
    )
}
