//! Widget Core
//!
//! slotmap-дерево, layout engine (Flex/Absolute), стилизация.

pub mod binding;
pub mod layout;
pub mod tree;

pub use crate::animator::{default_animation_presets, AnimationConfig, AnimationFrom};
pub use crate::render::scene::Fill;
use serde::{Deserialize, Serialize};
use slotmap::{DefaultKey, SlotMap};
use tiny_skia::Color;
pub use tree::{apply_diff, diff_trees, layout_tree, measure_tree, TreeDiff};

use std::sync::Arc;

/// Unique widget handle.
pub type WidgetId = DefaultKey;

/// Widget tree using generational arena.
#[derive(Clone, Default)]
pub struct WidgetTree {
    pub(crate) nodes: SlotMap<WidgetId, WidgetNode>,
    pub(crate) root: Option<WidgetId>,
    pub styles: Arc<std::collections::HashMap<String, StyleConfig>>,
}

impl WidgetTree {
    pub fn new() -> Self {
        Self {
            nodes: SlotMap::new(),
            root: None,
            styles: Arc::new(std::collections::HashMap::new()),
        }
    }

    pub fn root(&self) -> Option<WidgetId> {
        self.root
    }

    pub fn insert(&mut self, node: WidgetNode) -> WidgetId {
        let id = self.nodes.insert(node);
        if self.root.is_none() {
            self.root = Some(id);
        }
        id
    }

    pub fn append_child(&mut self, parent: WidgetId, child: WidgetId) {
        if let Some(node) = self.nodes.get_mut(child) {
            node.parent = Some(parent);
        }
        if let Some(node) = self.nodes.get_mut(parent) {
            node.children.push(child);
            node.dirty = true;
        }
    }

    pub fn get(&self, id: WidgetId) -> Option<&WidgetNode> {
        self.nodes.get(id)
    }

    pub fn get_mut(&mut self, id: WidgetId) -> Option<&mut WidgetNode> {
        self.nodes.get_mut(id)
    }

    pub fn find_by_id(&self, id: &str) -> Option<WidgetId> {
        self.nodes
            .iter()
            .find_map(|(widget_id, node)| (node.id.as_deref() == Some(id)).then_some(widget_id))
    }

    pub fn first_child(&self) -> Option<WidgetId> {
        self.root.and_then(|root| {
            self.get(root)
                .and_then(|node| node.children.first().copied())
        })
    }

    pub fn iter_nodes(&self) -> slotmap::basic::Iter<'_, WidgetId, WidgetNode> {
        self.nodes.iter()
    }

    pub fn iter_nodes_mut(&mut self) -> slotmap::basic::IterMut<'_, WidgetId, WidgetNode> {
        self.nodes.iter_mut()
    }

    pub fn apply_diff(&mut self, new_tree: &WidgetTree, diff: &TreeDiff) {
        tree::apply_diff(self, new_tree, diff);
    }

    pub fn shadow_extents(&self, scale: f32) -> (f32, f32, f32, f32) {
        self.nodes
            .values()
            .fold((0.0, 0.0, 0.0, 0.0), |extents, node| {
                let Some((radius, opacity, offset_x, offset_y)) = node.style.shadow else {
                    return extents;
                };
                if radius <= 0.0 || opacity <= 0.0 {
                    return extents;
                }
                let blur_pad = (radius * 1.5 + 4.0 / scale.max(1.0)).ceil();
                (
                    extents.0.max((blur_pad - offset_x).max(0.0)),
                    extents.1.max((blur_pad + offset_x).max(0.0)),
                    extents.2.max((blur_pad - offset_y).max(0.0)),
                    extents.3.max((blur_pad + offset_y).max(0.0)),
                )
            })
    }

    pub fn is_dirty(&self) -> bool {
        self.nodes.values().any(|n| n.dirty)
    }

    pub fn remove(&mut self, id: WidgetId) -> Option<WidgetNode> {
        self.nodes.remove(id)
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.root = None;
    }

    pub fn update_module(
        &mut self,
        module: &str,
        widget_id: &str,
        payload: serde_json::Value,
    ) -> bool {
        self.update_module_for_output(module, widget_id, payload, None)
    }

    pub fn update_module_for_output(
        &mut self,
        module: &str,
        widget_id: &str,
        payload: serde_json::Value,
        _output_name: Option<&str>,
    ) -> bool {
        let ids: Vec<WidgetId> = self.nodes.keys().collect();
        let mut changed = false;
        for id in ids {
            let is_match = if let Some(node) = self.nodes.get(id) {
                let nid = node.id.as_deref().unwrap_or("");
                let clean_target = widget_id
                    .strip_prefix("popup:")
                    .or_else(|| widget_id.strip_suffix(":popup"))
                    .unwrap_or(widget_id);
                let is_popup = widget_id.starts_with("popup:") || widget_id.ends_with(":popup");
                let is_generic = !is_popup && (widget_id.is_empty() || widget_id == module);

                if let WidgetContent::Module { module: name, .. } = &node.content {
                    if is_generic {
                        // Generic update: match any Module node for this module that is NOT a popup.
                        name == module && !nid.starts_with("popup:")
                    } else {
                        // Targeted update: the Module node must be the right recipient.
                        let clean_nid = nid
                            .strip_prefix("popup:")
                            .or_else(|| nid.strip_suffix(":popup"))
                            .unwrap_or(nid);
                        name == widget_id
                            || (!nid.is_empty() && (nid == widget_id || clean_nid == clean_target))
                    }
                } else if !nid.is_empty() {
                    let clean_nid = nid
                        .strip_prefix("popup:")
                        .or_else(|| nid.strip_suffix(":popup"))
                        .unwrap_or(nid);
                    if is_popup {
                        nid == widget_id
                            || clean_nid == clean_target
                            || (nid == "popup_root"
                                && (clean_target == module || self.root() == Some(id)))
                    } else {
                        ids_match(nid, widget_id, module)
                    }
                } else {
                    false
                }
            } else {
                false
            };

            if is_match {
                // 1. Dynamic children replacement (containers or module placeholders)
                if let Some(children) = payload.get("children").and_then(|v| v.as_array()) {
                    let should_sync = if let Some(node) = self.nodes.get(id) {
                        matches!(
                            node.content,
                            WidgetContent::Container | WidgetContent::Module { .. }
                        )
                    } else {
                        false
                    };
                    if should_sync {
                        if let Some(cur_out) = _output_name {
                            let has_monitor_specs =
                                children.iter().any(|c| c.get("monitor").is_some());
                            if has_monitor_specs {
                                let filtered: Vec<serde_json::Value> = children
                                    .iter()
                                    .filter(|c| {
                                        c.get("monitor")
                                            .and_then(|v| v.as_str())
                                            .is_none_or(|m| !m.is_empty() && m == cur_out)
                                    })
                                    .cloned()
                                    .collect();
                                self.sync_dynamic_children(id, &filtered);
                            } else {
                                self.sync_dynamic_children(id, children);
                            }
                        } else {
                            self.sync_dynamic_children(id, children);
                        }
                        changed = true;
                    }
                }

                // 2. Generic property updates
                if let Some(node) = self.nodes.get_mut(id) {
                    if let WidgetContent::Module {
                        payload: current, ..
                    } = &mut node.content
                    {
                        *current = payload.clone();
                    }
                    let text_candidate = payload.get("text").or_else(|| payload.get("label"));
                    if let Some(text_val) = text_candidate.and_then(|v| v.as_str()) {
                        match &mut node.content {
                            WidgetContent::Text { text } => *text = text_val.to_string(),
                            WidgetContent::Button { label, .. } => *label = text_val.to_string(),
                            _ => {}
                        }
                    }
                    if let Some(val) = payload.get("value").and_then(|v| v.as_f64()) {
                        match &mut node.content {
                            WidgetContent::Slider { value, .. } => *value = val as f32,
                            WidgetContent::Progress { value, .. } => *value = val as f32,
                            WidgetContent::ProgressRing { value, .. } => *value = val as f32,
                            _ => {}
                        }
                    }
                    if let Some(min_val) = payload.get("min").and_then(|v| v.as_f64()) {
                        if let WidgetContent::Slider { min, .. } = &mut node.content {
                            *min = min_val as f32;
                        }
                    }
                    if let Some(max_val) = payload.get("max").and_then(|v| v.as_f64()) {
                        match &mut node.content {
                            WidgetContent::Slider { max, .. } => *max = max_val as f32,
                            WidgetContent::Progress { max, .. } => *max = max_val as f32,
                            WidgetContent::ProgressRing { max, .. } => *max = max_val as f32,
                            _ => {}
                        }
                    }
                    if let Some(props_val) = payload.get("props") {
                        match &mut node.content {
                            WidgetContent::Slider { props, .. } => *props = props_val.clone(),
                            WidgetContent::Progress { props, .. } => *props = props_val.clone(),
                            WidgetContent::ProgressRing { props, .. } => *props = props_val.clone(),
                            _ => {}
                        }
                    }
                    if let Some(act) = payload.get("on_click").and_then(|v| v.as_str()) {
                        node.on_click = Some(act.to_string());
                    }
                    if let Some(tt) = payload.get("tooltip").and_then(|v| v.as_str()) {
                        node.tooltip = Some(tt.to_string());
                    }
                    node.dirty = true;
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn sync_dynamic_children(
        &mut self,
        parent_id: WidgetId,
        children_json: &[serde_json::Value],
    ) {
        if self.update_dynamic_children_in_place(parent_id, children_json) {
            return;
        }
        let parent_node = match self.nodes.get_mut(parent_id) {
            Some(parent) => {
                let nid = parent.id.as_deref().unwrap_or("");
                if matches!(
                    parent.content,
                    WidgetContent::Module { .. } | WidgetContent::Container
                ) {
                    if nid == "tray" || nid.starts_with("tray_") {
                        parent.layout.mode =
                            layout::LayoutMode::Flex(layout::FlexDirection::Horizontal);
                        parent.layout.align = layout::Align::Center;
                        parent.layout.justify = layout::JustifyContent::Center;
                        if parent.layout.gap == 0.0 {
                            parent.layout.gap = 6.0;
                        }
                    } else if parent.layout.mode == layout::LayoutMode::default() {
                        if nid.contains("popup") || nid.contains("root") {
                            parent.layout.mode =
                                layout::LayoutMode::Flex(layout::FlexDirection::Vertical);
                            parent.layout.align = layout::Align::Stretch;
                            parent.layout.justify = layout::JustifyContent::Start;
                            if parent.layout.gap == 0.0 {
                                parent.layout.gap = 8.0;
                            }
                        } else {
                            parent.layout.mode =
                                layout::LayoutMode::Flex(layout::FlexDirection::Horizontal);
                            parent.layout.align = layout::Align::Center;
                            parent.layout.justify = layout::JustifyContent::Center;
                            if parent.layout.gap == 0.0 {
                                parent.layout.gap = 4.0;
                            }
                        }
                    }
                }
                parent.clone()
            }
            None => return,
        };

        // Remove old children
        let old_children: Vec<WidgetId> = if let Some(parent) = self.nodes.get_mut(parent_id) {
            std::mem::take(&mut parent.children)
        } else {
            return;
        };
        for child in old_children {
            self.remove_subtree(child);
        }

        let styles = Arc::clone(&self.styles);
        for item in children_json {
            let child_cfg: WidgetConfig = match serde_json::from_value(item.clone()) {
                Ok(cfg) => cfg,
                Err(_) => {
                    let text = item
                        .get("text")
                        .or_else(|| item.get("label"))
                        .or_else(|| item.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let id = item
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string);
                    let action = item
                        .get("action")
                        .or_else(|| item.get("on_click"))
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string);
                    let mut node = WidgetNode::new(WidgetContent::Text {
                        text: text.to_string(),
                    });
                    node.id = id;
                    node.on_click = action;
                    node.style = parent_node.style.clone();
                    let cid = self.insert(node);
                    self.append_child(parent_id, cid);
                    continue;
                }
            };

            let _ = insert_configured(self, Some(parent_id), &child_cfg, &styles);
        }
    }

    fn update_dynamic_children_in_place(
        &mut self,
        parent_id: WidgetId,
        children_json: &[serde_json::Value],
    ) -> bool {
        let old_children = match self.nodes.get(parent_id) {
            Some(node) if node.children.len() == children_json.len() => node.children.clone(),
            _ => return false,
        };

        let styles = Arc::clone(&self.styles);
        let mut updates = Vec::with_capacity(children_json.len());
        for (child_id, value) in old_children.iter().zip(children_json) {
            let config: WidgetConfig = match serde_json::from_value(value.clone()) {
                Ok(config) => config,
                Err(_) => return false,
            };
            let mut fresh = WidgetTree::new();
            let fresh_id = insert_configured(&mut fresh, None, &config, &styles);
            let Some(fresh_node) = fresh.get(fresh_id).cloned() else {
                return false;
            };
            let Some(existing) = self.nodes.get(*child_id) else {
                return false;
            };
            if existing.children.len() != config.children.len() {
                return false;
            }
            updates.push((*child_id, fresh_node, config.children));
        }

        for (child_id, fresh_node, nested_children) in updates {
            let existing_children = self
                .nodes
                .get(child_id)
                .map(|node| node.children.clone())
                .unwrap_or_default();
            if let Some(node) = self.nodes.get_mut(child_id) {
                node.id = fresh_node.id;
                node.layout = fresh_node.layout;
                node.style = fresh_node.style;
                node.content = fresh_node.content;
                node.on_click = fresh_node.on_click;
                node.on_right_click = fresh_node.on_right_click;
                node.on_scroll = fresh_node.on_scroll;
                node.on_change = fresh_node.on_change;
                node.tooltip = fresh_node.tooltip;
                node.dirty = true;
            }
            if !nested_children.is_empty()
                && !self
                    .update_dynamic_children_in_place_for_ids(&existing_children, &nested_children)
            {
                return false;
            }
        }
        true
    }

    fn update_dynamic_children_in_place_for_ids(
        &mut self,
        old_children: &[WidgetId],
        children_json: &[WidgetConfig],
    ) -> bool {
        if old_children.len() != children_json.len() {
            return false;
        }
        let styles = Arc::clone(&self.styles);
        for (child_id, config) in old_children.iter().zip(children_json) {
            let mut fresh = WidgetTree::new();
            let fresh_id = insert_configured(&mut fresh, None, config, &styles);
            let fresh_node = match fresh.get(fresh_id).cloned() {
                Some(node) => node,
                None => return false,
            };
            let existing_children = match self.nodes.get(*child_id) {
                Some(node) if node.children.len() == config.children.len() => node.children.clone(),
                _ => return false,
            };
            if let Some(node) = self.nodes.get_mut(*child_id) {
                node.id = fresh_node.id;
                node.layout = fresh_node.layout;
                node.style = fresh_node.style;
                node.content = fresh_node.content;
                node.on_click = fresh_node.on_click;
                node.on_right_click = fresh_node.on_right_click;
                node.on_scroll = fresh_node.on_scroll;
                node.on_change = fresh_node.on_change;
                node.tooltip = fresh_node.tooltip;
                node.dirty = true;
            }
            if !config.children.is_empty()
                && !self
                    .update_dynamic_children_in_place_for_ids(&existing_children, &config.children)
            {
                return false;
            }
        }
        true
    }

    pub fn remove_subtree(&mut self, id: WidgetId) {
        if let Some(node) = self.nodes.remove(id) {
            for child in node.children {
                self.remove_subtree(child);
            }
        }
    }

    pub fn hit_test(&self, x: f64, y: f64) -> Option<WidgetId> {
        fn visit(tree: &WidgetTree, id: WidgetId, x: f64, y: f64) -> Option<WidgetId> {
            let node = tree.get(id)?;
            let (left, top, width, height) = node.final_rect;
            let inside = x >= left as f64
                && y >= top as f64
                && x < (left + width) as f64
                && y < (top + height) as f64;

            if (node.layout.clip || node.layout.scroll_y) && !inside {
                return None;
            }

            for child in node.children.iter().rev() {
                if let Some(hit) = visit(tree, *child, x, y) {
                    return Some(hit);
                }
            }
            if inside && !node.failed {
                Some(id)
            } else {
                None
            }
        }

        self.root.and_then(|root| visit(self, root, x, y))
    }
}

fn ids_match(nid: &str, target: &str, module: &str) -> bool {
    if nid.is_empty() || target.is_empty() {
        return false;
    }
    if nid == target {
        return true;
    }

    let clean_nid = nid.strip_prefix("popup:").unwrap_or(nid);
    let clean_target = target.strip_prefix("popup:").unwrap_or(target);

    if clean_nid == clean_target {
        return true;
    }

    // Popup root match: when a module sends to popup_root or root
    if (clean_target == "popup_root" || clean_target == "root")
        && !module.is_empty()
        && (clean_nid == module || nid == module)
    {
        return true;
    }

    // Direct match if target is the module and node ID matches the module
    if !module.is_empty()
        && (target == module || clean_target == module)
        && (clean_nid == module || nid == module)
    {
        return true;
    }

    // Direct matches with common widget suffixes without hardcoded module tables
    for suffix in &["_badge", "_btn", "_item", "_label", "_root"] {
        if let Some(prefix) = clean_nid.strip_suffix(suffix) {
            if prefix == target
                || prefix == clean_target
                || (!module.is_empty() && (prefix == module || module.starts_with(prefix)))
            {
                return true;
            }
        }
        if let Some(prefix) = clean_target.strip_suffix(suffix) {
            if prefix == nid || prefix == clean_nid {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_ids_match_does_not_bleed_across_modules() {
        assert!(!ids_match("launcher_btn", "power-menu", "power-menu"));
        assert!(!ids_match("cpu_badge", "power-menu", "power-menu"));
        assert!(!ids_match("clock_badge", "power-menu", "power-menu"));
        assert!(!ids_match("audio", "microphone", "audio"));
        assert!(!ids_match("microphone", "audio", "audio"));
        assert!(!ids_match("workspaces", "power-menu", "power-menu"));
        assert!(ids_match("power_btn", "power-menu", "power-menu"));
        assert!(ids_match("power_btn", "power", "power-menu"));
        assert!(ids_match("cpu_badge", "cpu", "system"));
        assert!(ids_match("memory_badge", "memory", "system"));
        assert!(ids_match("clock_badge", "clock", "clock"));
        assert!(ids_match("network_badge", "network", "network"));
        assert!(ids_match("audio", "audio", "audio"));
        assert!(ids_match("microphone", "microphone", "audio"));
    }

    #[test]
    fn targeted_audio_popup_update_does_not_replace_parent_popup() {
        let mut tree = WidgetTree::new();
        let root = tree.insert(WidgetNode::new(WidgetContent::Container));

        let mut parent = WidgetNode::new(WidgetContent::Module {
            module: "audio".into(),
            payload: serde_json::Value::Null,
        });
        parent.id = Some("popup:audio".into());
        let parent_id = tree.insert(parent);
        tree.append_child(root, parent_id);

        let mut selector = WidgetNode::new(WidgetContent::Module {
            module: "audio".into(),
            payload: serde_json::Value::Null,
        });
        selector.id = Some("popup:audio-sinks".into());
        let selector_id = tree.insert(selector);
        tree.append_child(root, selector_id);

        let payload = serde_json::json!({
            "children": [{ "type": "text", "text": "Output devices" }]
        });
        assert!(tree.update_module_for_output("audio", "popup:audio-sinks", payload, None));
        assert!(tree.get(parent_id).unwrap().children.is_empty());
        assert_eq!(tree.get(selector_id).unwrap().children.len(), 1);
    }

    #[test]
    fn generic_tray_update_does_not_contaminate_tray_menu_popup() {
        let mut tree = WidgetTree::new();
        let root = tree.insert(WidgetNode::new(WidgetContent::Container));

        // Bar tray widget
        let mut bar_tray = WidgetNode::new(WidgetContent::Module {
            module: "tray".into(),
            payload: serde_json::Value::Null,
        });
        bar_tray.id = Some("tray".into());
        let bar_tray_id = tree.insert(bar_tray);
        tree.append_child(root, bar_tray_id);

        // Tray menu popup widget
        let mut tray_popup = WidgetNode::new(WidgetContent::Module {
            module: "tray".into(),
            payload: serde_json::Value::Null,
        });
        tray_popup.id = Some("popup:tray_menu".into());
        let tray_popup_id = tree.insert(tray_popup);
        tree.append_child(root, tray_popup_id);

        // Generic tray bar update
        let bar_payload = serde_json::json!({
            "children": [
                { "type": "button", "id": "tray_app1", "text": "App1" },
                { "type": "button", "id": "tray_app2", "text": "App2" }
            ]
        });
        assert!(tree.update_module_for_output("tray", "tray", bar_payload, None));

        // Verify bar tray received the 2 buttons, but popup:tray_menu did NOT receive them
        assert_eq!(tree.get(bar_tray_id).unwrap().children.len(), 2);
        assert!(tree.get(tray_popup_id).unwrap().children.is_empty());

        // Targeted popup update
        let popup_payload = serde_json::json!({
            "children": [
                { "type": "button", "text": "Disconnect" }
            ]
        });
        assert!(tree.update_module_for_output("tray", "popup:tray_menu", popup_payload, None));

        // Verify popup received its menu item and bar tray remained untouched (still 2 items)
        assert_eq!(tree.get(bar_tray_id).unwrap().children.len(), 2);
        assert_eq!(tree.get(tray_popup_id).unwrap().children.len(), 1);

        // Verify bar tray layout is Flex Horizontal
        assert_eq!(
            tree.get(bar_tray_id).unwrap().layout.mode,
            crate::widgets::layout::LayoutMode::Flex(
                crate::widgets::layout::FlexDirection::Horizontal
            )
        );
    }

    #[test]
    fn finds_widget_by_declarative_id() {
        let mut tree = WidgetTree::new();
        let mut node = WidgetNode::new(WidgetContent::Text {
            text: String::new(),
        });
        node.id = Some("launcher-input".into());
        let widget = tree.insert(node);

        assert_eq!(tree.find_by_id("launcher-input"), Some(widget));
        assert_eq!(tree.find_by_id("missing"), None);
    }

    #[test]
    fn hit_test_rejects_points_outside_widget_bounds() {
        let mut tree = WidgetTree::new();
        let widget = tree.insert(WidgetNode::new(WidgetContent::Text {
            text: String::new(),
        }));
        tree.get_mut(widget).unwrap().final_rect = (8.0, 12.0, 120.0, 40.0);

        assert_eq!(tree.hit_test(8.0, 12.0), Some(widget));
        assert_eq!(tree.hit_test(128.0, 52.0), None);
        assert_eq!(tree.hit_test(7.99, 20.0), None);
    }

    #[test]
    fn state_style_overrides_only_defined_tokens() {
        let red = Color::from_rgba8(255, 0, 0, 255);
        let base = WidgetStyle {
            background: Some(Fill::Solid(Color::BLACK)),
            foreground: Some(Color::WHITE),
            outline_color: Some(Color::BLACK),
            outline_width: 1.0,
            shadow: Some((10.0, 0.5, 0.0, 2.0)),
            shadow_color: Some(Color::BLACK),
            opacity: 1.0,
            hover: Some(WidgetStateStyle {
                background: Some(Fill::Solid(red)),
                outline_color: Some(Color::from_rgba8(0, 255, 0, 255)),
                outline_width: Some(2.5),
                shadow: Some((20.0, 0.8, 0.0, 4.0)),
                shadow_color: Some(red),
                ..WidgetStateStyle::default()
            }),
            ..WidgetStyle::default()
        };

        let (
            background,
            foreground,
            _,
            opacity,
            outline_color,
            outline_width,
            outline_style,
            shadow,
            shadow_color,
        ) = base.for_state(crate::style::WidgetState::Hover);
        assert_eq!(background, Some(Fill::Solid(red)));
        assert_eq!(foreground, Some(Color::WHITE));
        assert_eq!(opacity, 1.0);
        assert_eq!(outline_color, Some(Color::from_rgba8(0, 255, 0, 255)));
        assert_eq!(outline_width, 2.5);
        assert_eq!(outline_style, crate::render::scene::OutlineStyle::Solid);
        assert_eq!(shadow, Some((20.0, 0.8, 0.0, 4.0)));
        assert_eq!(shadow_color, Some(red));
    }

    #[test]
    fn parse_linear_gradient_supports_angles_and_stops() {
        let fill = parse_fill("linear-gradient(135deg, #7cf2ce, #b7a2ff)").unwrap();
        match fill {
            Fill::LinearGradient { angle_deg, stops } => {
                assert_eq!(angle_deg, 135.0);
                assert_eq!(stops.len(), 2);
                assert_eq!(stops[0].0, 0.0);
                assert_eq!(stops[0].1, parse_color("#7cf2ce").unwrap());
                assert_eq!(stops[1].0, 1.0);
                assert_eq!(stops[1].1, parse_color("#b7a2ff").unwrap());
            }
            _ => panic!("Expected LinearGradient"),
        }

        let fill_rgba = parse_fill(
            "linear-gradient(to right, rgba(167, 139, 250, 0.3), rgba(103, 232, 249, 0.15))",
        )
        .unwrap();
        match fill_rgba {
            Fill::LinearGradient { angle_deg, stops } => {
                assert_eq!(angle_deg, 90.0);
                assert_eq!(stops.len(), 2);
                assert_eq!(stops[0].0, 0.0);
                assert_eq!(stops[1].0, 1.0);
            }
            _ => panic!("Expected LinearGradient"),
        }
    }

    #[test]
    fn parse_color_supports_all_formats() {
        assert_eq!(parse_color("#f00"), Some(Color::from_rgba8(255, 0, 0, 255)));
        assert_eq!(
            parse_color("#f008"),
            Some(Color::from_rgba8(255, 0, 0, 136))
        );
        assert_eq!(
            parse_color("#ff0000"),
            Some(Color::from_rgba8(255, 0, 0, 255))
        );
        assert_eq!(
            parse_color("#ff000080"),
            Some(Color::from_rgba8(255, 0, 0, 128))
        );
        assert_eq!(
            parse_color("rgb(10, 20, 30)"),
            Some(Color::from_rgba8(10, 20, 30, 255))
        );
        assert_eq!(
            parse_color("rgba(10, 20, 30, 0.5)"),
            Some(Color::from_rgba8(10, 20, 30, 128))
        );
        assert_eq!(
            parse_color("rgba(18, 12, 31, 0.88)"),
            Some(Color::from_rgba8(18, 12, 31, 224))
        );
        assert_eq!(
            parse_color("transparent"),
            Some(Color::from_rgba8(0, 0, 0, 0))
        );
        assert_eq!(
            parse_color("white"),
            Some(Color::from_rgba8(255, 255, 255, 255))
        );
        assert_eq!(
            parse_color("crimson"),
            Some(Color::from_rgba8(220, 20, 60, 255))
        );
        assert_eq!(
            parse_color("rebeccapurple"),
            Some(Color::from_rgba8(102, 51, 153, 255))
        );
        assert_eq!(
            parse_color("hsl(120, 50%, 50%)"),
            Some(Color::from_rgba8(64, 191, 64, 255))
        );
        assert_eq!(
            parse_color("hsla(120, 50%, 50%, 0.5)"),
            Some(Color::from_rgba8(64, 191, 64, 128))
        );
        assert_eq!(
            parse_color("radial-gradient(circle, #fff, #000)"),
            Some(Color::from_rgba8(255, 255, 255, 255))
        );
    }

    #[test]
    fn test_parse_fill_named_literals_and_case_insensitivity() {
        assert_eq!(
            parse_fill("transparent"),
            Some(Fill::Solid(Color::from_rgba8(0, 0, 0, 0)))
        );
        assert_eq!(
            parse_fill("none"),
            Some(Fill::Solid(Color::from_rgba8(0, 0, 0, 0)))
        );
        assert_eq!(
            parse_fill("TRANSPARENT"),
            Some(Fill::Solid(Color::from_rgba8(0, 0, 0, 0)))
        );
        assert_eq!(
            parse_fill("None"),
            Some(Fill::Solid(Color::from_rgba8(0, 0, 0, 0)))
        );
        assert_eq!(
            parse_fill("white"),
            Some(Fill::Solid(Color::from_rgba8(255, 255, 255, 255)))
        );
        assert_eq!(
            parse_fill("WHITE"),
            Some(Fill::Solid(Color::from_rgba8(255, 255, 255, 255)))
        );
        assert_eq!(
            parse_fill("black"),
            Some(Fill::Solid(Color::from_rgba8(0, 0, 0, 255)))
        );
        assert_eq!(
            parse_fill("BLACK"),
            Some(Fill::Solid(Color::from_rgba8(0, 0, 0, 255)))
        );
        assert_eq!(
            parse_fill("radial-gradient(circle, #fff, #000)"),
            Some(Fill::RadialGradient {
                cx: 0.5,
                cy: 0.5,
                radius: 0.0,
                stops: vec![
                    (0.0, Color::from_rgba8(255, 255, 255, 255)),
                    (1.0, Color::from_rgba8(0, 0, 0, 255)),
                ],
            })
        );
    }

    #[test]
    fn update_module_creates_dynamic_children() {
        let mut tree = WidgetTree::new();
        let mut ws_node = WidgetNode::new(WidgetContent::Module {
            module: "workspaces".into(),
            payload: serde_json::Value::Null,
        });
        ws_node.id = Some("workspaces".into());
        let ws_id = tree.insert(ws_node);

        let payload = serde_json::json!({
            "text": "1 2",
            "children": [
                { "type": "button", "id": "ws-1", "text": "1  󰞷", "on_click": "hyprctl dispatch workspace 1" },
                { "type": "button", "id": "ws-2", "text": "2  󰈹", "on_click": "hyprctl dispatch workspace 2" }
            ]
        });

        let changed = tree.update_module("workspaces", "workspaces", payload);
        assert!(changed);

        let parent = tree.get(ws_id).unwrap();
        assert_eq!(parent.children.len(), 2);

        let child1 = tree.get(parent.children[0]).unwrap();
        assert_eq!(child1.id.as_deref(), Some("ws-1"));
        if let WidgetContent::Button { label, on_click } = &child1.content {
            assert_eq!(label, "1  󰞷");
            assert_eq!(on_click.as_deref(), Some("hyprctl dispatch workspace 1"));
        } else {
            panic!("Expected Button widget");
        }
    }

    #[test]
    fn test_progress_ring_parsing() {
        let mut tree = WidgetTree::new();
        let config = WidgetConfig {
            ty: "progress_ring".to_string(),
            id: Some("cpu_ring".to_string()),
            text: Some("42%".to_string()),
            max: Some(100.0),
            value: Some(42.0),
            stroke_width: Some(8.0),
            ..Default::default()
        };
        let styles = HashMap::new();
        let id = insert_configured(&mut tree, None, &config, &styles);
        let node = tree.get(id).unwrap();
        assert!(
            matches!(node.content, WidgetContent::ProgressRing { value, max, stroke_width, ref text, .. }
            if value == 42.0 && max == 100.0 && stroke_width == Some(8.0) && text.as_deref() == Some("42%"))
        );
    }

    #[test]
    fn test_get_style_has_zero_hardcoded_fallbacks() {
        let styles = HashMap::new();
        assert!(get_style("card", &styles).is_none());
        assert!(get_style("chip", &styles).is_none());
        assert!(get_style("chip_accent", &styles).is_none());
        assert!(get_style("clean_accent", &styles).is_none());
        assert!(get_style("clean_item", &styles).is_none());
        assert!(get_style("muted", &styles).is_none());
        assert!(get_style("separator", &styles).is_none());
        assert!(get_style("audio_banner", &styles).is_none());
    }

    #[test]
    fn test_from_popup_config_no_hardcoded_styles() {
        let styles = HashMap::new();
        let tree = from_popup_config(&[], &styles);
        let root = tree.root().unwrap();
        let node = tree.get(root).unwrap();
        assert_eq!(node.style.background, None);
        assert_eq!(node.style.outline_color, None);
        assert_eq!(node.style.shadow, None);
    }

    #[test]
    fn test_style_inheritance_single_parent() {
        let mut styles = HashMap::new();
        styles.insert(
            "base".to_string(),
            StyleConfig {
                background: Some("#111111".to_string()),
                foreground: Some("#ffffff".to_string()),
                radius: Some(8.0),
                ..Default::default()
            },
        );
        styles.insert(
            "child".to_string(),
            StyleConfig {
                extends: Some(vec!["base".to_string()]),
                foreground: Some("#ff0000".to_string()),
                ..Default::default()
            },
        );

        assert!(resolve_style_inheritance(&mut styles).is_ok());

        let child = styles.get("child").unwrap();
        assert_eq!(child.background.as_deref(), Some("#111111"));
        assert_eq!(child.foreground.as_deref(), Some("#ff0000"));
        assert_eq!(child.radius, Some(8.0));
        assert_eq!(child.extends, None);
    }

    #[test]
    fn test_style_inheritance_multi_parent_override() {
        let mut styles = HashMap::new();
        styles.insert(
            "p1".to_string(),
            StyleConfig {
                background: Some("#111111".to_string()),
                foreground: Some("#ffffff".to_string()),
                radius: Some(4.0),
                ..Default::default()
            },
        );
        styles.insert(
            "p2".to_string(),
            StyleConfig {
                foreground: Some("#00ff00".to_string()),
                radius: Some(12.0),
                opacity: Some(0.9),
                ..Default::default()
            },
        );
        styles.insert(
            "child".to_string(),
            StyleConfig {
                extends: Some(vec!["p1".to_string(), "p2".to_string()]),
                radius: Some(16.0),
                ..Default::default()
            },
        );

        assert!(resolve_style_inheritance(&mut styles).is_ok());

        let child = styles.get("child").unwrap();
        assert_eq!(child.background.as_deref(), Some("#111111"));
        assert_eq!(child.foreground.as_deref(), Some("#00ff00"));
        assert_eq!(child.opacity, Some(0.9));
        assert_eq!(child.radius, Some(16.0));
    }

    #[test]
    fn test_style_inheritance_cyclic_detection() {
        let mut styles = HashMap::new();
        styles.insert(
            "a".to_string(),
            StyleConfig {
                extends: Some(vec!["b".to_string()]),
                ..Default::default()
            },
        );
        styles.insert(
            "b".to_string(),
            StyleConfig {
                extends: Some(vec!["a".to_string()]),
                ..Default::default()
            },
        );

        let err = resolve_style_inheritance(&mut styles).unwrap_err();
        assert!(err.starts_with("Cyclic style inheritance detected:"));
    }

    #[test]
    fn test_style_inheritance_missing_parent() {
        let mut styles = HashMap::new();
        styles.insert(
            "child".to_string(),
            StyleConfig {
                extends: Some(vec!["nonexistent".to_string()]),
                ..Default::default()
            },
        );

        let err = resolve_style_inheritance(&mut styles).unwrap_err();
        assert!(err.contains("Parent style 'nonexistent' referenced by 'child' does not exist"));
    }

    #[test]
    fn test_style_inheritance_nested_hover_merge() {
        let mut styles = HashMap::new();
        styles.insert(
            "base".to_string(),
            StyleConfig {
                background: Some("#111111".to_string()),
                hover: Some(Box::new(StyleStateConfig {
                    background: Some("#222222".to_string()),
                    foreground: Some("#ffffff".to_string()),
                    opacity: Some(0.8),
                    ..Default::default()
                })),
                ..Default::default()
            },
        );
        styles.insert(
            "accented".to_string(),
            StyleConfig {
                extends: Some(vec!["base".to_string()]),
                hover: Some(Box::new(StyleStateConfig {
                    foreground: Some("#ff007c".to_string()),
                    ..Default::default()
                })),
                ..Default::default()
            },
        );

        assert!(resolve_style_inheritance(&mut styles).is_ok());

        let accented = styles.get("accented").unwrap();
        assert_eq!(accented.background.as_deref(), Some("#111111"));
        let hover = accented.hover.as_ref().unwrap();
        assert_eq!(hover.background.as_deref(), Some("#222222"));
        assert_eq!(hover.foreground.as_deref(), Some("#ff007c"));
        assert_eq!(hover.opacity, Some(0.8));
    }

    #[test]
    fn test_diff_trees_identical() {
        let mut t1 = WidgetTree::new();
        let r1 = t1.insert(WidgetNode::new(WidgetContent::Container));
        let c1 = t1.insert(WidgetNode::new(WidgetContent::Text {
            text: "Hello".into(),
        }));
        t1.append_child(r1, c1);

        let mut t2 = WidgetTree::new();
        let r2 = t2.insert(WidgetNode::new(WidgetContent::Container));
        let c2 = t2.insert(WidgetNode::new(WidgetContent::Text {
            text: "Hello".into(),
        }));
        t2.append_child(r2, c2);

        let diff = diff_trees(&t1, &t2);
        assert_eq!(diff.preserved.len(), 2);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert!(diff.changed.is_empty());
    }

    #[test]
    fn test_diff_trees_child_appended() {
        let mut t1 = WidgetTree::new();
        let r1 = t1.insert(WidgetNode::new(WidgetContent::Container));
        let c1 = t1.insert(WidgetNode::new(WidgetContent::Text {
            text: "Item 1".into(),
        }));
        t1.append_child(r1, c1);

        let mut t2 = WidgetTree::new();
        let r2 = t2.insert(WidgetNode::new(WidgetContent::Container));
        let c2_1 = t2.insert(WidgetNode::new(WidgetContent::Text {
            text: "Item 1".into(),
        }));
        let c2_2 = t2.insert(WidgetNode::new(WidgetContent::Text {
            text: "Item 2".into(),
        }));
        t2.append_child(r2, c2_1);
        t2.append_child(r2, c2_2);

        let diff = diff_trees(&t1, &t2);
        assert_eq!(diff.preserved.len(), 2); // r1 <-> r2, c1 <-> c2_1
        assert_eq!(diff.added.len(), 1); // c2_2 added
        assert!(diff.removed.is_empty());
        assert_eq!(diff.added[0], c2_2);
    }

    #[test]
    fn test_diff_trees_child_removed() {
        let mut t1 = WidgetTree::new();
        let r1 = t1.insert(WidgetNode::new(WidgetContent::Container));
        let c1 = t1.insert(WidgetNode::new(WidgetContent::Text {
            text: "Item 1".into(),
        }));
        let c2 = t1.insert(WidgetNode::new(WidgetContent::Text {
            text: "Item 2".into(),
        }));
        t1.append_child(r1, c1);
        t1.append_child(r1, c2);

        let mut t2 = WidgetTree::new();
        let r2 = t2.insert(WidgetNode::new(WidgetContent::Container));
        let c2_1 = t2.insert(WidgetNode::new(WidgetContent::Text {
            text: "Item 1".into(),
        }));
        t2.append_child(r2, c2_1);

        let diff = diff_trees(&t1, &t2);
        assert_eq!(diff.preserved.len(), 2); // r1 <-> r2, c1 <-> c2_1
        assert_eq!(diff.removed.len(), 1); // c2 removed
        assert!(diff.added.is_empty());
        assert_eq!(diff.removed[0], c2);
    }

    #[test]
    fn test_diff_trees_root_kind_changed_fallback() {
        let mut t1 = WidgetTree::new();
        let r1 = t1.insert(WidgetNode::new(WidgetContent::Container));
        let c1 = t1.insert(WidgetNode::new(WidgetContent::Text {
            text: "Hello".into(),
        }));
        t1.append_child(r1, c1);

        let mut t2 = WidgetTree::new();
        let _r2 = t2.insert(WidgetNode::new(WidgetContent::Button {
            label: "Click".into(),
            on_click: None,
        }));

        let diff = diff_trees(&t1, &t2);
        assert!(
            diff.preserved.is_empty(),
            "Fallback diff must have empty preserved list"
        );
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.removed.len(), 2);
    }

    #[test]
    fn test_apply_diff_preserves_widget_ids() {
        let mut t1 = WidgetTree::new();
        let r1 = t1.insert(WidgetNode::new(WidgetContent::Container));
        let mut n1 = WidgetNode::new(WidgetContent::Text { text: "Old".into() });
        n1.id = Some("my_text".into());
        let c1 = t1.insert(n1);
        t1.append_child(r1, c1);

        let mut t2 = WidgetTree::new();
        let r2 = t2.insert(WidgetNode::new(WidgetContent::Container));
        let mut n2 = WidgetNode::new(WidgetContent::Text { text: "New".into() });
        n2.id = Some("my_text".into());
        let c2 = t2.insert(n2);
        let mut n3 = WidgetNode::new(WidgetContent::Button {
            label: "Add".into(),
            on_click: None,
        });
        n3.id = Some("my_btn".into());
        let c3 = t2.insert(n3);
        t2.append_child(r2, c2);
        t2.append_child(r2, c3);

        let diff = diff_trees(&t1, &t2);
        assert_eq!(diff.preserved.len(), 2);
        assert_eq!(diff.added.len(), 1);

        t1.apply_diff(&t2, &diff);

        // Verify r1 and c1 retained their exact WidgetId
        assert_eq!(t1.root(), Some(r1));
        let node_c1 = t1
            .get(c1)
            .expect("c1 must still exist in t1 at the same WidgetId");
        assert_eq!(node_c1.id.as_deref(), Some("my_text"));
        match &node_c1.content {
            WidgetContent::Text { text } => assert_eq!(text, "New"),
            _ => panic!("Expected text content"),
        }

        // Verify new node was added
        let btn_id = t1.find_by_id("my_btn").expect("my_btn must exist in t1");
        assert_ne!(btn_id, c1);
        assert_ne!(btn_id, r1);
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarginConfig {
    pub top: i32,
    pub left: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetConfig {
    #[serde(default = "default_widget_type", alias = "type")]
    pub ty: String,
    pub id: Option<String>,
    pub module: Option<String>,
    #[serde(alias = "label", alias = "name")]
    pub text: Option<String>,
    #[serde(alias = "src", alias = "image", alias = "icon")]
    pub path: Option<String>,
    pub style: Option<String>,
    #[serde(default)]
    pub children: Vec<WidgetConfig>,
    pub layout: Option<LayoutConfig>,
    #[serde(alias = "action")]
    pub on_click: Option<String>,
    #[serde(alias = "context_action")]
    pub on_right_click: Option<String>,
    pub on_scroll: Option<String>,
    pub on_change: Option<String>,
    #[serde(default)]
    pub tooltip: Option<String>,
    pub placeholder: Option<String>,
    pub repeat_over: Option<String>,
    pub bind: Option<String>,
    pub format: Option<String>,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub value: Option<f32>,
    pub stroke_width: Option<f32>,
    #[serde(alias = "fixed_width")]
    pub width: Option<f32>,
    #[serde(alias = "fixed_height")]
    pub height: Option<f32>,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    pub min_height: Option<f32>,
    pub max_height: Option<f32>,
    pub opacity: Option<f32>,
    #[serde(default)]
    pub props: Option<serde_json::Value>,
    #[serde(default)]
    pub hover_animation: Option<AnimationConfig>,
    pub scroll_y: Option<bool>,
    pub scrollable: Option<bool>,
    pub clip: Option<bool>,
}

impl Default for WidgetConfig {
    fn default() -> Self {
        Self {
            ty: default_widget_type(),
            id: None,
            module: None,
            text: None,
            path: None,
            style: None,
            children: Vec::new(),
            layout: None,
            on_click: None,
            on_right_click: None,
            on_scroll: None,
            on_change: None,
            tooltip: None,
            placeholder: None,
            repeat_over: None,
            bind: None,
            format: None,
            min: None,
            max: None,
            value: None,
            stroke_width: None,
            width: None,
            height: None,
            min_width: None,
            max_width: None,
            min_height: None,
            max_height: None,
            opacity: None,
            props: None,
            hover_animation: None,
            scroll_y: None,
            scrollable: None,
            clip: None,
        }
    }
}

fn default_widget_type() -> String {
    "item".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AbsolutePositionConfig {
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LayoutConfig {
    pub mode: Option<String>,      // "flex", "absolute", "stack", "grid"
    pub direction: Option<String>, // "horizontal", "vertical"
    pub gap: Option<f32>,
    pub padding: Option<Vec<f32>>,
    pub align: Option<String>,   // "start", "center", "end", "stretch"
    pub justify: Option<String>, // "start", "center", "end", "space-between", "space-around", "space-evenly"
    pub x: Option<f32>,
    pub y: Option<f32>,
    #[serde(alias = "fixed_width")]
    pub width: Option<f32>,
    #[serde(alias = "fixed_height")]
    pub height: Option<f32>,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    pub min_height: Option<f32>,
    pub max_height: Option<f32>,
    pub weight: Option<f32>,
    pub z_index: Option<i32>,
    pub transform: Option<layout::Transform>,
    pub format: Option<String>,
    pub columns: Option<Vec<String>>,
    pub rows: Option<Vec<String>>,
    pub grid_gap: Option<Vec<f32>>,
    pub absolute: Option<AbsolutePositionConfig>,
    pub scroll_y: Option<bool>,
    pub scrollable: Option<bool>,
    pub clip: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct StyleConfig {
    #[serde(default)]
    pub extends: Option<Vec<String>>,
    pub background: Option<String>,
    pub surface: Option<String>,
    pub foreground: Option<String>,
    pub accent: Option<String>,
    pub outline: Option<String>,
    pub opacity: Option<f32>,
    pub shadow: Option<ShadowConfig>,
    pub radius: Option<f32>,
    pub font: Option<String>,
    pub font_size: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    pub min_height: Option<f32>,
    pub max_height: Option<f32>,
    pub padding: Option<Vec<f32>>,
    pub margin: Option<Vec<f32>>,
    pub hover: Option<Box<StyleStateConfig>>,
    pub active: Option<Box<StyleStateConfig>>,
    pub disabled: Option<Box<StyleStateConfig>>,
    pub focus: Option<Box<StyleStateConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct StyleStateConfig {
    pub background: Option<String>,
    pub foreground: Option<String>,
    pub accent: Option<String>,
    pub opacity: Option<f32>,
    pub outline: Option<String>,
    pub shadow: Option<ShadowConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShadowConfig {
    pub radius: f32,
    pub opacity: f32,
    pub offset_x: Option<f32>,
    pub offset_y: Option<f32>,
    pub color: Option<String>,
}

pub fn merge_style_state(base: &StyleStateConfig, child: &StyleStateConfig) -> StyleStateConfig {
    StyleStateConfig {
        background: child.background.clone().or_else(|| base.background.clone()),
        foreground: child.foreground.clone().or_else(|| base.foreground.clone()),
        accent: child.accent.clone().or_else(|| base.accent.clone()),
        opacity: child.opacity.or(base.opacity),
        outline: child.outline.clone().or_else(|| base.outline.clone()),
        shadow: child.shadow.clone().or_else(|| base.shadow.clone()),
    }
}

pub fn merge_style(base: &StyleConfig, child: &StyleConfig) -> StyleConfig {
    let merge_state = |b: Option<&StyleStateConfig>, c: Option<&StyleStateConfig>| match (b, c) {
        (Some(b_st), Some(c_st)) => Some(Box::new(merge_style_state(b_st, c_st))),
        (Some(b_st), None) => Some(Box::new(b_st.clone())),
        (None, Some(c_st)) => Some(Box::new(c_st.clone())),
        (None, None) => None,
    };

    StyleConfig {
        extends: None,
        background: child.background.clone().or_else(|| base.background.clone()),
        surface: child.surface.clone().or_else(|| base.surface.clone()),
        foreground: child.foreground.clone().or_else(|| base.foreground.clone()),
        accent: child.accent.clone().or_else(|| base.accent.clone()),
        outline: child.outline.clone().or_else(|| base.outline.clone()),
        opacity: child.opacity.or(base.opacity),
        shadow: child.shadow.clone().or_else(|| base.shadow.clone()),
        radius: child.radius.or(base.radius),
        font: child.font.clone().or_else(|| base.font.clone()),
        font_size: child.font_size.or(base.font_size),
        width: child.width.or(base.width),
        height: child.height.or(base.height),
        min_width: child.min_width.or(base.min_width),
        max_width: child.max_width.or(base.max_width),
        min_height: child.min_height.or(base.min_height),
        max_height: child.max_height.or(base.max_height),
        padding: child.padding.clone().or_else(|| base.padding.clone()),
        margin: child.margin.clone().or_else(|| base.margin.clone()),
        hover: merge_state(base.hover.as_deref(), child.hover.as_deref()),
        active: merge_state(base.active.as_deref(), child.active.as_deref()),
        disabled: merge_state(base.disabled.as_deref(), child.disabled.as_deref()),
        focus: merge_state(base.focus.as_deref(), child.focus.as_deref()),
    }
}

pub fn resolve_style_inheritance(
    styles: &mut std::collections::HashMap<String, StyleConfig>,
) -> Result<(), String> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Unvisited,
        Visiting,
        Visited,
    }

    let mut states: std::collections::HashMap<String, State> = styles
        .keys()
        .map(|k| (k.clone(), State::Unvisited))
        .collect();

    let mut path: Vec<String> = Vec::new();

    fn dfs(
        node: &str,
        styles: &mut std::collections::HashMap<String, StyleConfig>,
        states: &mut std::collections::HashMap<String, State>,
        path: &mut Vec<String>,
    ) -> Result<(), String> {
        states.insert(node.to_string(), State::Visiting);
        path.push(node.to_string());

        let parents = styles.get(node).and_then(|s| s.extends.clone());
        if let Some(parents) = parents {
            for parent in &parents {
                if !styles.contains_key(parent) {
                    return Err(format!(
                        "Parent style '{}' referenced by '{}' does not exist",
                        parent, node
                    ));
                }

                match states.get(parent).copied().unwrap_or(State::Unvisited) {
                    State::Visiting => {
                        path.push(parent.clone());
                        let cycle = path.join(" -> ");
                        return Err(format!("Cyclic style inheritance detected: {}", cycle));
                    }
                    State::Unvisited => {
                        dfs(parent, styles, states, path)?;
                    }
                    State::Visited => {}
                }
            }

            // Merge parents in order (rightmost parent overrides leftmost)
            let mut merged_parent: Option<StyleConfig> = None;
            for parent in &parents {
                let p_style = match styles.get(parent) {
                    Some(s) => s.clone(),
                    None => return Err(format!("Parent style '{}' not found", parent)),
                };
                merged_parent = match merged_parent {
                    None => Some(p_style),
                    Some(prev) => Some(merge_style(&prev, &p_style)),
                };
            }

            let child_style = match styles.get(node) {
                Some(s) => s.clone(),
                None => return Err(format!("Style '{}' not found", node)),
            };
            let fully_resolved = if let Some(p) = merged_parent {
                merge_style(&p, &child_style)
            } else {
                let mut s = child_style;
                s.extends = None;
                s
            };

            styles.insert(node.to_string(), fully_resolved);
        }

        states.insert(node.to_string(), State::Visited);
        path.pop();
        Ok(())
    }

    let names: Vec<String> = styles.keys().cloned().collect();
    for name in names {
        if states.get(&name).copied() == Some(State::Unvisited) {
            dfs(&name, styles, &mut states, &mut path)?;
        }
    }

    Ok(())
}

pub fn get_style<'a>(
    name: &str,
    styles: &'a std::collections::HashMap<String, StyleConfig>,
) -> Option<std::borrow::Cow<'a, StyleConfig>> {
    styles.get(name).map(std::borrow::Cow::Borrowed)
}

pub fn parse_outline(
    outline_str: &str,
) -> (f32, crate::render::scene::OutlineStyle, Option<Color>) {
    let trimmed = outline_str.trim();
    if trimmed == "0px transparent" || trimmed == "none" || trimmed == "0" || trimmed == "0px" {
        return (0.0, crate::render::scene::OutlineStyle::Solid, None);
    }
    if let Some(color) = parse_color(trimmed) {
        return (1.0, crate::render::scene::OutlineStyle::Solid, Some(color));
    }
    let mut width = 1.0;
    let mut style = crate::render::scene::OutlineStyle::Solid;
    let mut rest = trimmed;
    if let Some(first_space) = trimmed.find(' ') {
        let first_tok = &trimmed[..first_space];
        if let Ok(w) = first_tok.trim_end_matches("px").parse::<f32>() {
            width = w;
            rest = trimmed[first_space..].trim();
        }
    }
    if let Some(after_solid) = rest.strip_prefix("solid") {
        style = crate::render::scene::OutlineStyle::Solid;
        rest = after_solid.trim();
    } else if let Some(after_dashed) = rest.strip_prefix("dashed") {
        style = crate::render::scene::OutlineStyle::Dashed;
        rest = after_dashed.trim();
    } else if let Some(after_dotted) = rest.strip_prefix("dotted") {
        style = crate::render::scene::OutlineStyle::Dotted;
        rest = after_dotted.trim();
    }
    if let Some(color) = parse_color(rest) {
        (width.max(0.5), style, Some(color))
    } else {
        (0.0, crate::render::scene::OutlineStyle::Solid, None)
    }
}

pub fn parse_shadow(shadow: &ShadowConfig) -> ((f32, f32, f32, f32), Option<Color>) {
    (
        (
            shadow.radius,
            shadow.opacity,
            shadow.offset_x.unwrap_or(0.0),
            shadow.offset_y.unwrap_or(0.0),
        ),
        shadow.color.as_deref().and_then(parse_color),
    )
}

pub fn apply_style_to_node(node: &mut WidgetNode, style: &StyleConfig) {
    if let Some(bg) = style.background.as_deref().and_then(parse_fill) {
        node.style.background = Some(bg);
    } else if let Some(surf) = style.surface.as_deref().and_then(parse_fill) {
        node.style.background = Some(surf);
    }
    if let Some(fg) = style.foreground.as_deref().and_then(parse_color) {
        node.style.foreground = Some(fg);
    }
    if let Some(accent) = style.accent.as_deref().and_then(parse_color) {
        node.style.accent = Some(accent);
    }
    if let Some(outline_str) = style.outline.as_deref() {
        let (w, s, c) = parse_outline(outline_str);
        node.style.outline_width = w;
        node.style.outline_style = s;
        node.style.outline_color = c;
    }

    let parse_state = |state: Option<&Box<StyleStateConfig>>| {
        state.as_ref().map(|state| {
            let (outline_width, outline_style, outline_color) =
                if let Some(outline_str) = state.outline.as_deref() {
                    let (w, s, c) = parse_outline(outline_str);
                    (Some(w), Some(s), c)
                } else {
                    (None, None, None)
                };
            let (shadow, shadow_color) = if let Some(s) = state.shadow.as_ref() {
                let (sh, c) = parse_shadow(s);
                (Some(sh), c)
            } else {
                (None, None)
            };
            WidgetStateStyle {
                background: state.background.as_deref().and_then(parse_fill),
                foreground: state.foreground.as_deref().and_then(parse_color),
                accent: state.accent.as_deref().and_then(parse_color),
                opacity: state.opacity.map(|value| value.clamp(0.0, 1.0)),
                outline_color,
                outline_width,
                outline_style,
                shadow,
                shadow_color,
            }
        })
    };
    node.style.hover = parse_state(style.hover.as_ref());
    node.style.active = parse_state(style.active.as_ref());
    node.style.disabled = parse_state(style.disabled.as_ref());
    node.style.focus = parse_state(style.focus.as_ref());
    if let Some(op) = style.opacity {
        node.style.opacity = op.clamp(0.0, 1.0);
    }
    if let Some(shadow) = style.shadow.as_ref() {
        let (sh, c) = parse_shadow(shadow);
        node.style.shadow = Some(sh);
        node.style.shadow_color = c;
    }
    if let Some(radius) = style.radius {
        node.style.radius = radius;
    }
    if let Some(font) = &style.font {
        node.style.font_family = font.clone();
    }
    if let Some(font_size) = style.font_size {
        node.style.font_size = font_size;
    }
    if let Some(padding) = &style.padding {
        let p = match padding.len() {
            4 => (padding[0], padding[1], padding[2], padding[3]),
            2 => (padding[0], padding[1], padding[0], padding[1]),
            1 => (padding[0], padding[0], padding[0], padding[0]),
            _ => (0.0, 0.0, 0.0, 0.0),
        };
        node.style.padding = p;
        node.layout.padding = p;
    }
    if let Some(w) = style.width {
        node.layout.fixed_width = Some(w);
    }
    if let Some(h) = style.height {
        node.layout.fixed_height = Some(h);
    }
    if let Some(w) = style.min_width {
        node.layout.min_width = Some(w);
    }
    if let Some(w) = style.max_width {
        node.layout.max_width = Some(w);
    }
    if let Some(h) = style.min_height {
        node.layout.min_height = Some(h);
    }
    if let Some(h) = style.max_height {
        node.layout.max_height = Some(h);
    }
    if let Some(margin) = &style.margin {
        let m = match margin.len() {
            4 => (margin[0], margin[1], margin[2], margin[3]),
            2 => (margin[0], margin[1], margin[0], margin[1]),
            1 => (margin[0], margin[0], margin[0], margin[0]),
            _ => (0.0, 0.0, 0.0, 0.0),
        };
        node.style.margin = m;
    }
}

fn apply_container_to_root(
    root_node: &mut WidgetNode,
    top_cfg: &WidgetConfig,
    styles: &std::collections::HashMap<String, StyleConfig>,
) {
    if let Some(id) = &top_cfg.id {
        root_node.id = Some(id.clone());
    }
    if let Some(style_name) = &top_cfg.style {
        if let Some(style) = styles.get(style_name) {
            apply_style_to_node(root_node, style);
        }
    }
    if let Some(layout) = &top_cfg.layout {
        if let Some(mode) = layout.mode.as_deref() {
            if mode == "absolute" || mode == "abs" {
                root_node.layout.mode = layout::LayoutMode::Absolute;
            } else if mode == "grid" {
                let cols = layout
                    .columns
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|s| layout::GridSize::parse(s))
                    .collect();
                let rows = layout
                    .rows
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|s| layout::GridSize::parse(s))
                    .collect();
                let gg = layout
                    .grid_gap
                    .as_ref()
                    .map(|g| {
                        if g.len() >= 2 {
                            (g[0], g[1])
                        } else if !g.is_empty() {
                            (g[0], g[0])
                        } else {
                            (layout.gap.unwrap_or(0.0), layout.gap.unwrap_or(0.0))
                        }
                    })
                    .unwrap_or((layout.gap.unwrap_or(0.0), layout.gap.unwrap_or(0.0)));
                root_node.layout.mode = layout::LayoutMode::Grid(layout::GridLayout {
                    columns: cols,
                    rows,
                    gap: gg,
                });
            } else if mode == "stack" {
                root_node.layout.mode = layout::LayoutMode::Stack;
            } else if mode == "flex_row" || mode == "row" || mode == "horizontal" {
                root_node.layout.mode = layout::LayoutMode::Flex(layout::FlexDirection::Horizontal);
            } else if mode == "flex_col" || mode == "col" || mode == "column" || mode == "vertical"
            {
                root_node.layout.mode = layout::LayoutMode::Flex(layout::FlexDirection::Vertical);
            }
        }
        if let Some(w) = layout.width {
            root_node.layout.fixed_width = Some(w);
        }
        if let Some(h) = layout.height {
            root_node.layout.fixed_height = Some(h);
        }
        if let Some(w) = layout.min_width {
            root_node.layout.min_width = Some(w);
        }
        if let Some(w) = layout.max_width {
            root_node.layout.max_width = Some(w);
        }
        if let Some(h) = layout.min_height {
            root_node.layout.min_height = Some(h);
        }
        if let Some(h) = layout.max_height {
            root_node.layout.max_height = Some(h);
        }
        if let Some(gap) = layout.gap {
            root_node.layout.gap = gap;
        }
        if let Some(align) = layout.align.as_deref() {
            root_node.layout.align = match align {
                "center" => layout::Align::Center,
                "end" => layout::Align::End,
                "stretch" => layout::Align::Stretch,
                _ => layout::Align::Start,
            };
        }
        if let Some(justify) = layout.justify.as_deref() {
            root_node.layout.justify = match justify {
                "center" => layout::JustifyContent::Center,
                "end" | "right" => layout::JustifyContent::End,
                "space-between" => layout::JustifyContent::SpaceBetween,
                "space-around" => layout::JustifyContent::SpaceAround,
                "space-evenly" => layout::JustifyContent::SpaceEvenly,
                _ => layout::JustifyContent::Start,
            };
        }
        if layout.direction.as_deref() == Some("horizontal")
            || layout.direction.as_deref() == Some("row")
        {
            root_node.layout.mode = layout::LayoutMode::Flex(layout::FlexDirection::Horizontal);
        }
        if let Some(padding) = &layout.padding {
            let p = match padding.len() {
                4 => (padding[0], padding[1], padding[2], padding[3]),
                2 => (padding[0], padding[1], padding[0], padding[1]),
                1 => (padding[0], padding[0], padding[0], padding[0]),
                _ => (0.0, 0.0, 0.0, 0.0),
            };
            root_node.layout.padding = p;
            root_node.style.padding = p;
        }
    }
}

pub fn from_config(
    config: &[WidgetConfig],
    styles: &std::collections::HashMap<String, StyleConfig>,
) -> WidgetTree {
    let mut tree = WidgetTree::new();
    tree.styles = Arc::new(styles.clone());
    let mut root_node = WidgetNode::new(WidgetContent::Container);
    root_node.layout.mode = layout::LayoutMode::Flex(layout::FlexDirection::Horizontal);
    root_node.layout.justify = layout::JustifyContent::SpaceBetween;
    root_node.layout.align = layout::Align::Center;
    root_node.layout.padding = (3.0, 10.0, 3.0, 10.0);
    root_node.style.padding = (3.0, 10.0, 3.0, 10.0);

    if let Some(style) = styles.get("bar").or_else(|| styles.get("global")) {
        apply_style_to_node(&mut root_node, style);
    }

    let root = tree.insert(root_node);

    if config.len() == 1 && (config[0].ty == "container" || config[0].ty == "box") {
        let top_cfg = &config[0];
        if let Some(root_node) = tree.get_mut(root) {
            apply_container_to_root(root_node, top_cfg, styles);
        }
        for child in &top_cfg.children {
            insert_configured(&mut tree, Some(root), child, styles);
        }
    } else {
        for widget in config {
            insert_configured(&mut tree, Some(root), widget, styles);
        }
    }
    tree
}

/// Builds a popup widget tree ensuring the root is fully styled with the rich popup backdrop/card.
pub fn from_popup_config(
    config: &[WidgetConfig],
    styles: &std::collections::HashMap<String, StyleConfig>,
) -> WidgetTree {
    let mut tree = WidgetTree::new();
    tree.styles = Arc::new(styles.clone());
    let mut root_node = WidgetNode::new(WidgetContent::Container);
    root_node.id = Some("popup_root".to_string());
    root_node.layout.mode = layout::LayoutMode::Flex(layout::FlexDirection::Vertical);
    root_node.layout.justify = layout::JustifyContent::Start;
    root_node.layout.align = layout::Align::Stretch;
    root_node.layout.gap = 10.0;
    root_node.layout.padding = (14.0, 16.0, 14.0, 16.0);
    root_node.style.padding = (14.0, 16.0, 14.0, 16.0);

    if let Some(style) = styles.get("popup").or_else(|| styles.get("global")) {
        apply_style_to_node(&mut root_node, style);
    }

    let root = tree.insert(root_node);

    if config.len() == 1 && (config[0].ty == "container" || config[0].ty == "box") {
        let top_cfg = &config[0];
        if let Some(root_node) = tree.get_mut(root) {
            apply_container_to_root(root_node, top_cfg, styles);
        }
        for child in &top_cfg.children {
            insert_configured(&mut tree, Some(root), child, styles);
        }
    } else {
        for widget in config {
            insert_configured(&mut tree, Some(root), widget, styles);
        }
    }

    tree
}

/// Builds a widget tree using live module data bindings.
pub fn from_config_with_store(
    config: &[WidgetConfig],
    styles: &std::collections::HashMap<String, StyleConfig>,
    data_store: &std::collections::HashMap<String, serde_json::Value>,
) -> WidgetTree {
    let expanded = binding::expand_widget_configs(config, None, data_store);
    from_config(&expanded, styles)
}

/// Builds a popup widget tree using live module data bindings.
pub fn from_popup_config_with_store(
    config: &[WidgetConfig],
    styles: &std::collections::HashMap<String, StyleConfig>,
    data_store: &std::collections::HashMap<String, serde_json::Value>,
) -> WidgetTree {
    let expanded = binding::expand_widget_configs(config, None, data_store);
    from_popup_config(&expanded, styles)
}

fn insert_configured(
    tree: &mut WidgetTree,
    parent: Option<WidgetId>,
    config: &WidgetConfig,
    styles: &std::collections::HashMap<String, StyleConfig>,
) -> WidgetId {
    let content = match config.ty.as_str() {
        "text" | "label" => WidgetContent::Text {
            text: config.text.clone().unwrap_or_default(),
        },
        "image" => WidgetContent::Image {
            path: config
                .path
                .clone()
                .or_else(|| config.text.clone())
                .unwrap_or_default(),
        },
        "svg" => WidgetContent::Svg {
            path: config
                .path
                .clone()
                .or_else(|| config.text.clone())
                .unwrap_or_default(),
        },
        "button" => {
            if let Some(path) = config.path.as_deref().filter(|p| !p.trim().is_empty()) {
                if path.ends_with(".svg") {
                    WidgetContent::Svg {
                        path: path.to_string(),
                    }
                } else {
                    WidgetContent::Image {
                        path: path.to_string(),
                    }
                }
            } else {
                WidgetContent::Button {
                    label: config.text.clone().unwrap_or_default(),
                    on_click: config.on_click.clone(),
                }
            }
        }
        "item" => {
            if let Some(lbl) = &config.text {
                WidgetContent::Text { text: lbl.clone() }
            } else if let Some(m) = &config.module {
                WidgetContent::Module {
                    module: m.clone(),
                    payload: serde_json::Value::Null,
                }
            } else {
                WidgetContent::Container
            }
        }
        "progress" => WidgetContent::Progress {
            value: config.value.unwrap_or(0.0),
            max: config.max.unwrap_or(100.0),
            props: config.props.clone().unwrap_or(serde_json::Value::Null),
        },
        "ring" | "progress_ring" | "circle_progress" => WidgetContent::ProgressRing {
            value: config.value.unwrap_or(0.0),
            max: config.max.unwrap_or(100.0),
            stroke_width: config.stroke_width,
            text: config.text.clone(),
            props: config.props.clone().unwrap_or(serde_json::Value::Null),
        },
        "slider" => WidgetContent::Slider {
            value: config.value.unwrap_or(50.0),
            min: config.min.unwrap_or(0.0),
            max: config.max.unwrap_or(100.0),
            props: config.props.clone().unwrap_or(serde_json::Value::Null),
        },
        "input" | "text_input" => {
            let t = config.text.clone().unwrap_or_default();
            let c_pos = t.len();
            WidgetContent::TextInput {
                text: t,
                placeholder: config.placeholder.clone().unwrap_or_default(),
                focused: false,
                cursor_pos: c_pos,
                selection: None,
            }
        }
        "module" | "workspaces" | "tray" => WidgetContent::Module {
            module: config
                .module
                .clone()
                .unwrap_or_else(|| config.id.clone().unwrap_or_else(|| config.ty.clone())),
            payload: serde_json::Value::Null,
        },
        _ => {
            if let Some(mod_name) = &config.module {
                WidgetContent::Module {
                    module: mod_name.clone(),
                    payload: serde_json::Value::Null,
                }
            } else if let Some(lbl) = &config.text {
                WidgetContent::Text { text: lbl.clone() }
            } else {
                WidgetContent::Container
            }
        }
    };
    let mut node = WidgetNode::new(content);
    node.id = config.id.clone();
    node.on_click = config.on_click.clone();
    node.on_right_click = config.on_right_click.clone();
    node.on_scroll = config.on_scroll.clone();
    node.on_change = config.on_change.clone().or_else(|| {
        if config.ty == "slider" {
            config.on_click.clone()
        } else {
            None
        }
    });
    node.tooltip = config.tooltip.clone();
    if let Some(style_name) = config.style.as_deref() {
        if let Some(style) = styles.get(style_name) {
            apply_style_to_node(&mut node, style);
        }
    } else if let Some(global) = styles.get("global") {
        apply_style_to_node(&mut node, global);
    }
    if let Some(op) = config.opacity {
        node.style.opacity = op;
    }
    if let Some(layout) = &config.layout {
        node.layout.gap = layout.gap.unwrap_or(0.0);
        let is_col = layout.direction.as_deref() == Some("vertical")
            || layout.direction.as_deref() == Some("column")
            || layout.mode.as_deref() == Some("flex_col")
            || layout.mode.as_deref() == Some("col")
            || layout.mode.as_deref() == Some("column")
            || layout.mode.as_deref() == Some("vertical");
        node.layout.align = match layout.align.as_deref() {
            Some("center") => layout::Align::Center,
            Some("end") => layout::Align::End,
            Some("stretch") => layout::Align::Stretch,
            Some("start") => layout::Align::Start,
            _ => {
                if is_col {
                    layout::Align::Stretch
                } else {
                    layout::Align::Start
                }
            }
        };
        node.layout.justify = match layout.justify.as_deref() {
            Some("center") => layout::JustifyContent::Center,
            Some("end") | Some("right") => layout::JustifyContent::End,
            Some("space-between") | Some("space_between") => layout::JustifyContent::SpaceBetween,
            Some("space-around") | Some("space_around") => layout::JustifyContent::SpaceAround,
            Some("space-evenly") | Some("space_evenly") => layout::JustifyContent::SpaceEvenly,
            _ => layout::JustifyContent::Start,
        };
        if layout.direction.as_deref() == Some("vertical")
            || layout.direction.as_deref() == Some("column")
        {
            node.layout.mode = layout::LayoutMode::Flex(layout::FlexDirection::Vertical);
        }
        if let Some(padding) = &layout.padding {
            let p = match padding.len() {
                4 => (padding[0], padding[1], padding[2], padding[3]),
                2 => (padding[0], padding[1], padding[0], padding[1]),
                1 => (padding[0], padding[0], padding[0], padding[0]),
                _ => (0.0, 0.0, 0.0, 0.0),
            };
            node.layout.padding = p;
        }
        if let Some(mode) = layout.mode.as_deref() {
            if mode == "absolute" {
                node.layout.mode = layout::LayoutMode::Absolute;
            } else if mode == "stack" {
                node.layout.mode = layout::LayoutMode::Stack;
            } else if mode == "grid" {
                let cols = layout
                    .columns
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|s| layout::GridSize::parse(s))
                    .collect();
                let rows = layout
                    .rows
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|s| layout::GridSize::parse(s))
                    .collect();
                let gg = layout
                    .grid_gap
                    .as_ref()
                    .map(|g| {
                        if g.len() >= 2 {
                            (g[0], g[1])
                        } else if !g.is_empty() {
                            (g[0], g[0])
                        } else {
                            (layout.gap.unwrap_or(0.0), layout.gap.unwrap_or(0.0))
                        }
                    })
                    .unwrap_or((layout.gap.unwrap_or(0.0), layout.gap.unwrap_or(0.0)));
                node.layout.mode = layout::LayoutMode::Grid(layout::GridLayout {
                    columns: cols,
                    rows,
                    gap: gg,
                });
            } else if mode == "flex_row" || mode == "row" || mode == "horizontal" {
                node.layout.mode = layout::LayoutMode::Flex(layout::FlexDirection::Horizontal);
            } else if mode == "flex_col" || mode == "col" || mode == "column" || mode == "vertical"
            {
                node.layout.mode = layout::LayoutMode::Flex(layout::FlexDirection::Vertical);
            }
        }
        node.layout.transform = layout.transform.clone();
        node.layout.z_index = layout.z_index;
        node.layout.format = layout.format.clone().or_else(|| config.format.clone());
        let (abs_x, abs_y, abs_w, abs_h) = if let Some(abs) = &layout.absolute {
            node.layout.mode = layout::LayoutMode::Absolute;
            (
                abs.x.or(layout.x),
                abs.y.or(layout.y),
                abs.width.or(layout.width),
                abs.height.or(layout.height),
            )
        } else {
            (layout.x, layout.y, layout.width, layout.height)
        };
        node.layout.absolute = (abs_x, abs_y, abs_w, abs_h);
        if let Some(w) = abs_w {
            node.layout.fixed_width = Some(w);
        }
        if let Some(h) = abs_h {
            node.layout.fixed_height = Some(h);
        }
        if let Some(w) = layout.min_width {
            node.layout.min_width = Some(w);
        }
        if let Some(w) = layout.max_width {
            node.layout.max_width = Some(w);
        }
        if let Some(h) = layout.min_height {
            node.layout.min_height = Some(h);
        }
        if let Some(h) = layout.max_height {
            node.layout.max_height = Some(h);
        }
        if let Some(weight) = layout.weight {
            node.layout.weight = weight;
        }
        if let Some(sy) = layout.scroll_y.or(layout.scrollable) {
            node.layout.scroll_y = sy;
            node.layout.clip = true;
        }
        if let Some(clip) = layout.clip {
            node.layout.clip = clip;
        }
    } else {
        let is_popup_item = node
            .id
            .as_deref()
            .is_some_and(|id| id.contains("popup") || id.contains("root"));
        if is_popup_item {
            node.layout.mode = layout::LayoutMode::Flex(layout::FlexDirection::Vertical);
            node.layout.align = layout::Align::Stretch;
            node.layout.justify = layout::JustifyContent::Start;
            node.layout.gap = 8.0;
        }
    }
    if let Some(w) = config.width {
        node.layout.fixed_width = Some(w);
    }
    if let Some(h) = config.height {
        node.layout.fixed_height = Some(h);
    }
    if let Some(w) = config.min_width {
        node.layout.min_width = Some(w);
    }
    if let Some(w) = config.max_width {
        node.layout.max_width = Some(w);
    }
    if let Some(h) = config.min_height {
        node.layout.min_height = Some(h);
    }
    if let Some(h) = config.max_height {
        node.layout.max_height = Some(h);
    }
    if let Some(sy) = config.scroll_y.or(config.scrollable) {
        node.layout.scroll_y = sy;
        node.layout.clip = true;
    }
    if let Some(clip) = config.clip {
        node.layout.clip = clip;
    }
    let id = tree.insert(node);
    if let Some(parent) = parent {
        tree.append_child(parent, id);
    }
    for child in &config.children {
        insert_configured(tree, Some(id), child, styles);
    }
    id
}

pub fn parse_named_color(value: &str) -> Option<Color> {
    let rgb = match value.to_ascii_lowercase().as_str() {
        "aliceblue" => (240, 248, 255),
        "antiquewhite" => (250, 235, 215),
        "aqua" | "cyan" => (0, 255, 255),
        "aquamarine" => (127, 255, 212),
        "azure" => (240, 255, 255),
        "beige" => (245, 245, 220),
        "bisque" => (255, 228, 196),
        "black" => (0, 0, 0),
        "blanchedalmond" => (255, 235, 205),
        "blue" => (0, 0, 255),
        "blueviolet" => (138, 43, 226),
        "brown" => (165, 42, 42),
        "burlywood" => (222, 184, 135),
        "cadetblue" => (95, 158, 160),
        "chartreuse" => (127, 255, 0),
        "chocolate" => (210, 105, 30),
        "coral" => (255, 127, 80),
        "cornflowerblue" => (100, 149, 237),
        "cornsilk" => (255, 248, 220),
        "crimson" => (220, 20, 60),
        "darkblue" => (0, 0, 139),
        "darkcyan" => (0, 139, 139),
        "darkgoldenrod" => (184, 134, 11),
        "darkgray" | "darkgrey" => (169, 169, 169),
        "darkgreen" => (0, 100, 0),
        "darkkhaki" => (189, 183, 107),
        "darkmagenta" => (139, 0, 139),
        "darkolivegreen" => (85, 107, 47),
        "darkorange" => (255, 140, 0),
        "darkorchid" => (153, 50, 204),
        "darkred" => (139, 0, 0),
        "darksalmon" => (233, 150, 122),
        "darkseagreen" => (143, 188, 143),
        "darkslateblue" => (72, 61, 139),
        "darkslategray" | "darkslategrey" => (47, 79, 79),
        "darkturquoise" => (0, 206, 209),
        "darkviolet" => (148, 0, 211),
        "deeppink" => (255, 20, 147),
        "deepskyblue" => (0, 191, 255),
        "dimgray" | "dimgrey" => (105, 105, 105),
        "dodgerblue" => (30, 144, 255),
        "firebrick" => (178, 34, 34),
        "floralwhite" => (255, 250, 240),
        "forestgreen" => (34, 139, 34),
        "fuchsia" | "magenta" => (255, 0, 255),
        "gainsboro" => (220, 220, 220),
        "ghostwhite" => (248, 248, 255),
        "gold" => (255, 215, 0),
        "goldenrod" => (218, 165, 32),
        "gray" | "grey" => (128, 128, 128),
        "green" => (0, 128, 0),
        "greenyellow" => (173, 255, 47),
        "honeydew" => (240, 255, 240),
        "hotpink" => (255, 105, 180),
        "indianred" => (205, 92, 92),
        "indigo" => (75, 0, 130),
        "ivory" => (255, 255, 240),
        "khaki" => (240, 230, 140),
        "lavender" => (230, 230, 250),
        "lavenderblush" => (255, 240, 245),
        "lawngreen" => (124, 252, 0),
        "lemonchiffon" => (255, 250, 205),
        "lightblue" => (173, 216, 230),
        "lightcoral" => (240, 128, 128),
        "lightcyan" => (224, 255, 255),
        "lightgoldenrodyellow" => (250, 250, 210),
        "lightgray" | "lightgrey" => (211, 211, 211),
        "lightgreen" => (144, 238, 144),
        "lightpink" => (255, 182, 193),
        "lightsalmon" => (255, 160, 122),
        "lightseagreen" => (32, 178, 170),
        "lightskyblue" => (135, 206, 250),
        "lightslategray" | "lightslategrey" => (119, 136, 153),
        "lightsteelblue" => (176, 196, 222),
        "lightyellow" => (255, 255, 224),
        "lime" => (0, 255, 0),
        "limegreen" => (50, 205, 50),
        "linen" => (250, 240, 230),
        "maroon" => (128, 0, 0),
        "mediumaquamarine" => (102, 205, 170),
        "mediumblue" => (0, 0, 205),
        "mediumorchid" => (186, 85, 211),
        "mediumpurple" => (147, 112, 219),
        "mediumseagreen" => (60, 179, 113),
        "mediumslateblue" => (123, 104, 238),
        "mediumspringgreen" => (0, 250, 154),
        "mediumturquoise" => (72, 209, 204),
        "mediumvioletred" => (199, 21, 133),
        "midnightblue" => (25, 25, 112),
        "mintcream" => (245, 255, 250),
        "mistyrose" => (255, 228, 225),
        "moccasin" => (255, 228, 181),
        "navajowhite" => (255, 222, 173),
        "navy" => (0, 0, 128),
        "oldlace" => (253, 245, 230),
        "olive" => (128, 128, 0),
        "olivedrab" => (107, 142, 35),
        "orange" => (255, 165, 0),
        "orangered" => (255, 69, 0),
        "orchid" => (218, 112, 214),
        "palegoldenrod" => (238, 232, 170),
        "palegreen" => (152, 251, 152),
        "paleturquoise" => (175, 238, 238),
        "palevioletred" => (219, 112, 147),
        "papayawhip" => (255, 239, 213),
        "peachpuff" => (255, 218, 185),
        "peru" => (205, 133, 63),
        "pink" => (255, 192, 203),
        "plum" => (221, 160, 221),
        "powderblue" => (176, 224, 230),
        "purple" => (128, 0, 128),
        "rebeccapurple" => (102, 51, 153),
        "red" => (255, 0, 0),
        "rosybrown" => (188, 143, 143),
        "royalblue" => (65, 105, 225),
        "saddlebrown" => (139, 69, 19),
        "salmon" => (250, 128, 114),
        "sandybrown" => (244, 164, 96),
        "seagreen" => (46, 139, 87),
        "seashell" => (255, 245, 238),
        "sienna" => (160, 82, 45),
        "silver" => (192, 192, 192),
        "skyblue" => (135, 206, 235),
        "slateblue" => (106, 90, 205),
        "slategray" | "slategrey" => (112, 128, 144),
        "snow" => (255, 250, 250),
        "springgreen" => (0, 255, 127),
        "steelblue" => (70, 130, 180),
        "tan" => (210, 180, 140),
        "teal" => (0, 128, 128),
        "thistle" => (216, 191, 216),
        "tomato" => (255, 99, 71),
        "turquoise" => (64, 224, 208),
        "violet" => (238, 130, 238),
        "wheat" => (245, 222, 179),
        "white" => (255, 255, 255),
        "whitesmoke" => (245, 245, 245),
        "yellow" => (255, 255, 0),
        "yellowgreen" => (154, 205, 50),
        "transparent" | "none" => return Some(Color::from_rgba8(0, 0, 0, 0)),
        _ => return None,
    };
    Some(Color::from_rgba8(rgb.0, rgb.1, rgb.2, 255))
}

fn hsl_to_rgb(h_deg: f32, s: f32, l: f32, a: f32) -> Color {
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);
    let a = a.clamp(0.0, 1.0);
    let h = h_deg.rem_euclid(360.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - ((h_prime % 2.0) - 1.0).abs());
    let (r1, g1, b1) = if (0.0..1.0).contains(&h_prime) {
        (c, x, 0.0)
    } else if (1.0..2.0).contains(&h_prime) {
        (x, c, 0.0)
    } else if (2.0..3.0).contains(&h_prime) {
        (0.0, c, x)
    } else if (3.0..4.0).contains(&h_prime) {
        (0.0, x, c)
    } else if (4.0..5.0).contains(&h_prime) {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let m = l - c / 2.0;
    let r = ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    let g = ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    let b = ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    let alpha = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::from_rgba8(r, g, b, alpha)
}

fn parse_hsl(trimmed: &str) -> Option<Color> {
    let is_hsl = trimmed.starts_with("hsl(") && trimmed.ends_with(')');
    let is_hsla = trimmed.starts_with("hsla(") && trimmed.ends_with(')');
    if !is_hsl && !is_hsla {
        return None;
    }
    let prefix_len = if is_hsl { 4 } else { 5 };
    let inside = &trimmed[prefix_len..trimmed.len() - 1];
    let norm = inside.replace('/', ",");
    let parts: Vec<&str> = if norm.contains(',') {
        norm.split(',').map(str::trim).collect()
    } else {
        norm.split_whitespace().collect()
    };

    if parts.len() < 3 {
        return None;
    }

    let h_str = parts[0].trim();
    let h: f32 = if let Some(deg) = h_str.strip_suffix("deg") {
        deg.trim().parse().ok()?
    } else if let Some(rad) = h_str.strip_suffix("rad") {
        rad.trim().parse::<f32>().ok()?.to_degrees()
    } else if let Some(turn) = h_str.strip_suffix("turn") {
        turn.trim().parse::<f32>().ok()? * 360.0
    } else {
        h_str.parse().ok()?
    };

    let s_str = parts[1].trim().strip_suffix('%')?;
    let s: f32 = s_str.trim().parse::<f32>().ok()? / 100.0;

    let l_str = parts[2].trim().strip_suffix('%')?;
    let l: f32 = l_str.trim().parse::<f32>().ok()? / 100.0;

    let a: f32 = if parts.len() >= 4 {
        let a_str = parts[3].trim();
        if let Some(pct) = a_str.strip_suffix('%') {
            (pct.trim().parse::<f32>().ok()? / 100.0).clamp(0.0, 1.0)
        } else {
            a_str.parse::<f32>().ok()?.clamp(0.0, 1.0)
        }
    } else {
        1.0
    };

    Some(hsl_to_rgb(h, s, l, a))
}

pub fn parse_color(value: &str) -> Option<Color> {
    let trimmed = value.trim();
    if let Some(named) = parse_named_color(trimmed) {
        return Some(named);
    }

    if let Some(hex) = trimmed.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                Some(Color::from_rgba8(r, g, b, 255))
            }
            4 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                let a = u8::from_str_radix(&hex[3..4], 16).ok()? * 17;
                Some(Color::from_rgba8(r, g, b, a))
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Color::from_rgba8(r, g, b, 255))
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some(Color::from_rgba8(r, g, b, a))
            }
            _ => None,
        };
    }

    if trimmed.starts_with("rgb(") && trimmed.ends_with(')') {
        let inside = &trimmed[4..trimmed.len() - 1];
        let parts: Vec<&str> = inside.split(',').map(str::trim).collect();
        if parts.len() == 3 {
            let r = parts[0].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
            let g = parts[1].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
            let b = parts[2].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
            return Some(Color::from_rgba8(r, g, b, 255));
        }
    }

    if trimmed.starts_with("rgba(") && trimmed.ends_with(')') {
        let inside = &trimmed[5..trimmed.len() - 1];
        let parts: Vec<&str> = inside.split(',').map(str::trim).collect();
        if parts.len() == 4 {
            let r = parts[0].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
            let g = parts[1].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
            let b = parts[2].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
            let a_str = parts[3].trim_end_matches('%');
            let a = if let Ok(val) = a_str.parse::<f32>() {
                if parts[3].ends_with('%') {
                    (val.clamp(0.0, 100.0) * 2.55).round() as u8
                } else if val <= 1.0 {
                    (val.clamp(0.0, 1.0) * 255.0).round() as u8
                } else {
                    val.clamp(0.0, 255.0).round() as u8
                }
            } else {
                255
            };
            return Some(Color::from_rgba8(r, g, b, a));
        }
    }

    if let Some(hsl_col) = parse_hsl(trimmed) {
        return Some(hsl_col);
    }

    if trimmed.starts_with("linear-gradient") || trimmed.starts_with("radial-gradient") {
        if let Some(start) = trimmed.find("rgba(") {
            if let Some(end) = trimmed[start..].find(')') {
                return parse_color(&trimmed[start..start + end + 1]);
            }
        } else if let Some(start) = trimmed.find("rgb(") {
            if let Some(end) = trimmed[start..].find(')') {
                return parse_color(&trimmed[start..start + end + 1]);
            }
        } else if let Some(start) = trimmed.find("hsla(") {
            if let Some(end) = trimmed[start..].find(')') {
                return parse_color(&trimmed[start..start + end + 1]);
            }
        } else if let Some(start) = trimmed.find("hsl(") {
            if let Some(end) = trimmed[start..].find(')') {
                return parse_color(&trimmed[start..start + end + 1]);
            }
        } else if let Some(start) = trimmed.find('#') {
            let hex_cand = &trimmed[start..];
            let end = hex_cand
                .find(|c: char| !c.is_ascii_hexdigit() && c != '#')
                .unwrap_or(hex_cand.len());
            return parse_color(&hex_cand[..end]);
        }
    }

    None
}

fn split_gradient_args(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    for ch in s.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                let tok = current.trim().to_string();
                if !tok.is_empty() {
                    tokens.push(tok);
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    let tok = current.trim().to_string();
    if !tok.is_empty() {
        tokens.push(tok);
    }
    tokens
}

fn parse_angle_or_direction(s: &str) -> Option<f32> {
    let lower = s.to_ascii_lowercase();
    let trimmed = lower.trim();
    if let Some(num_str) = trimmed.strip_suffix("deg") {
        return num_str.trim().parse::<f32>().ok();
    }
    if let Some(num_str) = trimmed.strip_suffix("rad") {
        return num_str.trim().parse::<f32>().ok().map(|r| r.to_degrees());
    }
    if let Some(num_str) = trimmed.strip_suffix("turn") {
        return num_str.trim().parse::<f32>().ok().map(|t| t * 360.0);
    }
    if let Some(dir) = trimmed.strip_prefix("to ") {
        let dir = dir.trim();
        return match dir {
            "top" => Some(0.0),
            "right" => Some(90.0),
            "bottom" => Some(180.0),
            "left" => Some(270.0),
            "top right" | "right top" => Some(45.0),
            "bottom right" | "right bottom" => Some(135.0),
            "bottom left" | "left bottom" => Some(225.0),
            "top left" | "left top" => Some(315.0),
            _ => None,
        };
    }
    None
}

fn parse_color_stop(s: &str) -> Option<(Option<f32>, Color)> {
    let trimmed = s.trim();
    let mut last_space = None;
    let mut depth = 0;
    for (idx, ch) in trimmed.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ' ' | '\t' if depth == 0 => last_space = Some(idx),
            _ => {}
        }
    }

    if let Some(idx) = last_space {
        let (color_part, offset_part) = trimmed.split_at(idx);
        let offset_trimmed = offset_part.trim();
        let offset_val = if let Some(pct) = offset_trimmed.strip_suffix('%') {
            pct.trim()
                .parse::<f32>()
                .ok()
                .map(|p| (p / 100.0).clamp(0.0, 1.0))
        } else {
            offset_trimmed
                .parse::<f32>()
                .ok()
                .map(|v| v.clamp(0.0, 1.0))
        };
        if let Some(color) = parse_color(color_part.trim()) {
            return Some((offset_val, color));
        }
    }

    if let Some(color) = parse_color(trimmed) {
        return Some((None, color));
    }

    None
}

pub fn parse_gradient(value: &str) -> Option<Fill> {
    let trimmed = value.trim();
    if !trimmed.starts_with("linear-gradient(") || !trimmed.ends_with(')') {
        return None;
    }
    let inside = &trimmed[16..trimmed.len() - 1];
    let raw_args = split_gradient_args(inside);
    if raw_args.is_empty() {
        return None;
    }

    let mut angle_deg = 180.0;
    let mut color_args = &raw_args[..];

    if let Some(angle) = parse_angle_or_direction(&raw_args[0]) {
        angle_deg = angle;
        color_args = &raw_args[1..];
    }

    if color_args.len() < 2 {
        return None;
    }

    let mut parsed_stops = Vec::new();
    for arg in color_args {
        let (opt_offset, color) = parse_color_stop(arg)?;
        parsed_stops.push((opt_offset, color));
    }

    let total = parsed_stops.len();
    let mut final_stops = Vec::with_capacity(total);
    for (i, (opt_offset, color)) in parsed_stops.into_iter().enumerate() {
        let offset = opt_offset.unwrap_or_else(|| {
            if total <= 1 {
                0.0
            } else {
                i as f32 / (total - 1) as f32
            }
        });
        final_stops.push((offset, color));
    }

    Some(Fill::LinearGradient {
        angle_deg,
        stops: final_stops,
    })
}

pub fn parse_radial_gradient(value: &str) -> Option<Fill> {
    let trimmed = value.trim();
    if !trimmed.starts_with("radial-gradient(") || !trimmed.ends_with(')') {
        return None;
    }
    let inside = &trimmed[16..trimmed.len() - 1];
    let raw_args = split_gradient_args(inside);
    if raw_args.is_empty() {
        return None;
    }

    let mut cx = 0.5;
    let mut cy = 0.5;
    let radius = 0.0;
    let mut color_args = &raw_args[..];

    let first = raw_args[0].trim().to_ascii_lowercase();
    if first.starts_with("circle") || first.contains("at ") {
        color_args = &raw_args[1..];
        if let Some(at_idx) = first.find("at ") {
            let pos_str = first[at_idx + 3..].trim();
            let pos_parts: Vec<&str> = pos_str.split_whitespace().collect();
            if pos_parts.len() == 1 {
                match pos_parts[0] {
                    "center" => {
                        cx = 0.5;
                        cy = 0.5;
                    }
                    "top" => {
                        cx = 0.5;
                        cy = 0.0;
                    }
                    "bottom" => {
                        cx = 0.5;
                        cy = 1.0;
                    }
                    "left" => {
                        cx = 0.0;
                        cy = 0.5;
                    }
                    "right" => {
                        cx = 1.0;
                        cy = 0.5;
                    }
                    _ => {}
                }
            } else if pos_parts.len() >= 2 {
                if let Some(x_pct) = pos_parts[0]
                    .strip_suffix('%')
                    .and_then(|p| p.parse::<f32>().ok())
                {
                    cx = (x_pct / 100.0).clamp(0.0, 1.0);
                }
                if let Some(y_pct) = pos_parts[1]
                    .strip_suffix('%')
                    .and_then(|p| p.parse::<f32>().ok())
                {
                    cy = (y_pct / 100.0).clamp(0.0, 1.0);
                }
            }
        }
    }

    if color_args.len() < 2 {
        return None;
    }

    let mut parsed_stops = Vec::new();
    for arg in color_args {
        let (opt_offset, color) = parse_color_stop(arg)?;
        parsed_stops.push((opt_offset, color));
    }

    let total = parsed_stops.len();
    let mut final_stops = Vec::with_capacity(total);
    for (i, (opt_offset, color)) in parsed_stops.into_iter().enumerate() {
        let offset = opt_offset.unwrap_or_else(|| {
            if total <= 1 {
                0.0
            } else {
                i as f32 / (total - 1) as f32
            }
        });
        final_stops.push((offset, color));
    }

    Some(Fill::RadialGradient {
        cx,
        cy,
        radius,
        stops: final_stops,
    })
}

pub fn parse_fill(value: &str) -> Option<Fill> {
    let trimmed = value.trim();
    if trimmed.starts_with("linear-gradient(") {
        if let Some(fill) = parse_gradient(trimmed) {
            return Some(fill);
        }
    }
    if trimmed.starts_with("radial-gradient(") {
        if let Some(fill) = parse_radial_gradient(trimmed) {
            return Some(fill);
        }
    }
    parse_color(trimmed).map(Fill::Solid)
}

/// A single widget node.
#[derive(Clone)]
pub struct WidgetNode {
    pub id: Option<String>,
    pub parent: Option<WidgetId>,
    pub children: Vec<WidgetId>,
    pub layout: layout::LayoutParams,
    pub style: WidgetStyle,
    pub content: WidgetContent,
    pub measured_size: (f32, f32),
    pub final_rect: (f32, f32, f32, f32), // x, y, w, h
    pub dirty: bool,
    pub failed: bool, // §16.4: marked broken after panic
    pub on_click: Option<String>,
    pub on_right_click: Option<String>,
    pub on_scroll: Option<String>,
    pub on_change: Option<String>,
    pub tooltip: Option<String>,
}

impl WidgetNode {
    pub fn new(content: WidgetContent) -> Self {
        let on_click = match &content {
            WidgetContent::Button { on_click, .. } => on_click.clone(),
            _ => None,
        };
        Self {
            id: None,
            parent: None,
            children: Vec::new(),
            layout: layout::LayoutParams::default(),
            style: WidgetStyle::default(),
            content,
            measured_size: (0.0, 0.0),
            final_rect: (0.0, 0.0, 0.0, 0.0),
            dirty: true,
            failed: false,
            on_click,
            on_right_click: None,
            on_scroll: None,
            on_change: None,
            tooltip: None,
        }
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

pub type ResolvedStateStyle = (
    Option<Fill>,
    Option<Color>,
    Option<Color>,
    f32,
    Option<Color>,
    f32,
    crate::render::scene::OutlineStyle,
    Option<(f32, f32, f32, f32)>,
    Option<Color>,
);

impl WidgetStyle {
    pub fn for_state(&self, state: crate::style::WidgetState) -> ResolvedStateStyle {
        let override_style = match state {
            crate::style::WidgetState::Hover => self.hover.as_ref(),
            crate::style::WidgetState::Active => self.active.as_ref(),
            crate::style::WidgetState::Disabled => self.disabled.as_ref(),
            crate::style::WidgetState::Focus => self.focus.as_ref(),
            crate::style::WidgetState::Normal => None,
        };
        let Some(override_style) = override_style else {
            return (
                self.background.clone(),
                self.foreground,
                self.accent,
                self.opacity,
                self.outline_color,
                self.outline_width,
                self.outline_style,
                self.shadow,
                self.shadow_color,
            );
        };
        (
            override_style
                .background
                .clone()
                .or_else(|| self.background.clone()),
            override_style.foreground.or(self.foreground),
            override_style.accent.or(self.accent),
            override_style.opacity.unwrap_or(self.opacity),
            override_style.outline_color.or(self.outline_color),
            override_style.outline_width.unwrap_or(self.outline_width),
            override_style.outline_style.unwrap_or(self.outline_style),
            override_style.shadow.or(self.shadow),
            override_style.shadow_color.or(self.shadow_color),
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WidgetStyle {
    pub background: Option<Fill>,
    pub foreground: Option<Color>,
    pub accent: Option<Color>,
    pub outline_color: Option<Color>,
    pub outline_width: f32,
    pub outline_style: crate::render::scene::OutlineStyle,
    pub radius: f32,
    pub padding: (f32, f32, f32, f32), // top, right, bottom, left
    pub margin: (f32, f32, f32, f32),
    pub font_family: String,
    pub font_size: f32,
    pub opacity: f32,
    pub shadow: Option<(f32, f32, f32, f32)>, // radius, opacity, offset_x, offset_y
    pub shadow_color: Option<Color>,
    pub hover: Option<WidgetStateStyle>,
    pub active: Option<WidgetStateStyle>,
    pub disabled: Option<WidgetStateStyle>,
    pub focus: Option<WidgetStateStyle>,
}

impl Default for WidgetStyle {
    fn default() -> Self {
        Self {
            background: None,
            foreground: None,
            accent: None,
            outline_color: None,
            outline_width: 0.0,
            outline_style: crate::render::scene::OutlineStyle::Solid,
            radius: 0.0,
            padding: (0.0, 0.0, 0.0, 0.0),
            margin: (0.0, 0.0, 0.0, 0.0),
            font_family: String::new(),
            font_size: 13.0,
            opacity: 1.0,
            shadow: None,
            shadow_color: None,
            hover: None,
            active: None,
            disabled: None,
            focus: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WidgetStateStyle {
    pub background: Option<Fill>,
    pub foreground: Option<Color>,
    pub accent: Option<Color>,
    pub opacity: Option<f32>,
    pub outline_color: Option<Color>,
    pub outline_width: Option<f32>,
    pub outline_style: Option<crate::render::scene::OutlineStyle>,
    pub shadow: Option<(f32, f32, f32, f32)>,
    pub shadow_color: Option<Color>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WidgetContent {
    Container, // Flex/Absolute layout container
    Text {
        text: String,
    },
    Image {
        path: String,
    },
    Svg {
        path: String,
    },
    Button {
        label: String,
        on_click: Option<String>,
    },
    Progress {
        value: f32,
        max: f32,
        props: serde_json::Value,
    },
    ProgressRing {
        value: f32,
        max: f32,
        stroke_width: Option<f32>,
        text: Option<String>,
        props: serde_json::Value,
    },
    Slider {
        value: f32,
        min: f32,
        max: f32,
        props: serde_json::Value,
    },
    TextInput {
        text: String,
        placeholder: String,
        focused: bool,
        cursor_pos: usize,
        selection: Option<(usize, usize)>,
    },
    // Module widgets hold opaque data from IPC
    Module {
        module: String,
        payload: serde_json::Value,
    },
}

impl WidgetContent {
    pub fn kind(&self) -> &'static str {
        match self {
            WidgetContent::Container => "container",
            WidgetContent::Text { .. } => "text",
            WidgetContent::Image { .. } => "image",
            WidgetContent::Svg { .. } => "svg",
            WidgetContent::Button { .. } => "button",
            WidgetContent::Progress { .. } => "progress",
            WidgetContent::ProgressRing { .. } => "progress_ring",
            WidgetContent::Slider { .. } => "slider",
            WidgetContent::TextInput { .. } => "text_input",
            WidgetContent::Module { .. } => "module",
        }
    }
}
