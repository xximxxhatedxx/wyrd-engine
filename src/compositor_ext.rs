use crate::compositor::CompositorIntegration;
use crate::BarState;
use std::sync::{Arc, Mutex};
use wayland_client::backend::ObjectId;
use wayland_client::Proxy;

pub fn sync_keybinds(
    compositor: &dyn CompositorIntegration,
    keybinds: &[crate::config::KeybindConfig],
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

pub fn resolve_cursor_output_id(
    compositor: &dyn CompositorIntegration,
    state: &Arc<Mutex<BarState>>,
    explicit_output: Option<&str>,
) -> Option<ObjectId> {
    let outputs = state
        .lock()
        .unwrap()
        .outputs
        .values()
        .cloned()
        .collect::<Vec<_>>();
    if outputs.is_empty() {
        return None;
    }

    if let Some(target_name) = explicit_output {
        if target_name != "all" && !target_name.is_empty() {
            if let Some(out) = outputs.iter().find(|o| o.name == target_name) {
                return Some(out.output.id());
            }
        }
    }

    if let Some((cur_gx, cur_gy)) = compositor.cursor_position() {
        if let Some(out) = outputs.iter().find(|o| {
            let w = if o.logical_width > 0 {
                o.logical_width
            } else if let Some((mw, _)) = o.current_mode {
                mw
            } else {
                return false;
            };
            let h = if o.logical_height > 0 {
                o.logical_height
            } else if let Some((_, mh)) = o.current_mode {
                mh
            } else {
                return false;
            };
            cur_gx >= o.logical_x
                && cur_gx < (o.logical_x + w)
                && cur_gy >= o.logical_y
                && cur_gy < (o.logical_y + h)
        }) {
            return Some(out.output.id());
        }
    }

    if let Some(focused_name) = compositor.focused_monitor() {
        if let Some(out) = outputs.iter().find(|o| o.name == focused_name) {
            return Some(out.output.id());
        }
    }

    if let Some(ptr_surf_id) = state.lock().unwrap().pointer_surface_id.clone() {
        if let Some(out) = outputs.iter().find(|o| o.output.id() == ptr_surf_id) {
            return Some(out.output.id());
        }
    }

    outputs.first().map(|o| o.output.id())
}
