//! Widget Core
//!
//! SlotMap-backed widget tree, Flexbox/Grid/Absolute layout engine, and styling.

pub mod binding;
pub mod layout;
pub mod render_bridge;
pub mod tree;

pub mod builder;
pub mod color;
pub mod node;
pub mod style;
#[cfg(test)]
mod tests;

pub use crate::animator::{AnimationConfig, AnimationFrom};
pub use crate::render::scene::Fill;
pub use builder::*;
pub use color::*;
pub use node::*;
pub use render_bridge::{build_scene_node, build_scene_node_with_anim, render_surface};
use slotmap::{DefaultKey, SlotMap};
pub use style::*;
pub use tree::{apply_diff, diff_trees, layout_tree, measure_tree, TreeDiff};

use std::sync::Arc;

pub type WidgetId = DefaultKey;

#[derive(Clone, Default)]
pub struct WidgetTree {
    pub(crate) nodes: SlotMap<WidgetId, WidgetNode>,
    pub(crate) root: Option<WidgetId>,
    pub styles: Arc<std::collections::HashMap<String, StyleConfig>>,
    pub default_vertical_style: Option<String>,
}

enum UpdateTarget<'a> {
    Popup {
        raw: &'a str,
        clean: &'a str,
        has_explicit_node: bool,
    },
    ModuleBroadcast {
        module: &'a str,
    },
    Widget {
        module: &'a str,
        target: &'a str,
        clean: &'a str,
    },
}

impl WidgetTree {
    pub fn new() -> Self {
        Self {
            nodes: SlotMap::new(),
            root: None,
            styles: Arc::new(std::collections::HashMap::new()),
            default_vertical_style: None,
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

    fn node_matches_update_target(node: &WidgetNode, target: &UpdateTarget<'_>) -> bool {
        let nid = node.id.as_deref().unwrap_or("");
        let clean_nid = nid
            .strip_prefix("popup:")
            .or_else(|| nid.strip_suffix(":popup"))
            .unwrap_or(nid);

        if let WidgetContent::Module { module: name, .. } = &node.content {
            match target {
                UpdateTarget::ModuleBroadcast { module } => {
                    name == *module && !nid.starts_with("popup:")
                }
                UpdateTarget::Popup { raw, clean, .. } => {
                    name == *raw || (!nid.is_empty() && (nid == *raw || clean_nid == *clean))
                }
                UpdateTarget::Widget {
                    target: tgt, clean, ..
                } => name == *tgt || (!nid.is_empty() && (nid == *tgt || clean_nid == *clean)),
            }
        } else if !nid.is_empty() {
            match target {
                UpdateTarget::Popup {
                    raw,
                    clean,
                    has_explicit_node,
                } => {
                    nid == *raw
                        || (nid.starts_with("popup:") && clean_nid == *clean)
                        || (!has_explicit_node
                            && ((nid.starts_with("popup_root:")
                                && nid.strip_prefix("popup_root:") == Some(*clean))
                                || (nid == "popup_root" && *clean == "popup_root")))
                }
                UpdateTarget::ModuleBroadcast { module } => {
                    !nid.starts_with("popup:")
                        && !nid.starts_with("popup_root")
                        && ids_match(nid, module, module)
                }
                UpdateTarget::Widget {
                    module,
                    target: tgt,
                    ..
                } => {
                    !nid.starts_with("popup:")
                        && !nid.starts_with("popup_root")
                        && ids_match(nid, tgt, module)
                }
            }
        } else {
            false
        }
    }

    pub fn update_module_for_output(
        &mut self,
        module: &str,
        widget_id: &str,
        payload: serde_json::Value,
        _output_name: Option<&str>,
    ) -> bool {
        let clean_target = widget_id
            .strip_prefix("popup:")
            .or_else(|| widget_id.strip_suffix(":popup"))
            .unwrap_or(widget_id);
        let is_popup = widget_id.starts_with("popup:") || widget_id.ends_with(":popup");

        let target_kind = if is_popup {
            let has_explicit_node = self.nodes.values().any(|n| {
                n.id.as_deref().is_some_and(|nid| {
                    let cn = nid
                        .strip_prefix("popup:")
                        .or_else(|| nid.strip_suffix(":popup"))
                        .unwrap_or(nid);
                    nid == widget_id || (nid.starts_with("popup:") && cn == clean_target)
                })
            });
            UpdateTarget::Popup {
                raw: widget_id,
                clean: clean_target,
                has_explicit_node,
            }
        } else if widget_id.is_empty() || widget_id == module {
            UpdateTarget::ModuleBroadcast { module }
        } else {
            UpdateTarget::Widget {
                module,
                target: widget_id,
                clean: clean_target,
            }
        };

        let ids: Vec<WidgetId> = self.nodes.keys().collect();
        let mut changed = false;
        for id in ids {
            let is_match = self
                .nodes
                .get(id)
                .is_some_and(|node| Self::node_matches_update_target(node, &target_kind));

            if is_match {
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
                        if let Some(layout_obj) = payload.get("layout").and_then(|v| v.as_object())
                        {
                            if let Some(node) = self.nodes.get_mut(id) {
                                let is_popup_or_unconfigured = node.layout.mode
                                    == layout::LayoutMode::default()
                                    || node.id.as_deref().is_some_and(|nid| nid.contains("popup"));
                                if let Some(gap) = layout_obj.get("gap").and_then(|v| v.as_f64()) {
                                    if is_popup_or_unconfigured || node.layout.gap == 0.0 {
                                        node.layout.gap = gap as f32;
                                    }
                                }
                                if is_popup_or_unconfigured {
                                    if let Some(mode) =
                                        layout_obj.get("mode").and_then(|v| v.as_str())
                                    {
                                        if mode == "flex_col"
                                            || mode == "col"
                                            || mode == "column"
                                            || mode == "vertical"
                                        {
                                            node.layout.mode = layout::LayoutMode::Flex(
                                                layout::FlexDirection::Vertical,
                                            );
                                            node.layout.align = layout::Align::Stretch;
                                            node.layout.justify = layout::JustifyContent::Start;
                                        } else if mode == "flex_row"
                                            || mode == "row"
                                            || mode == "horizontal"
                                        {
                                            node.layout.mode = layout::LayoutMode::Flex(
                                                layout::FlexDirection::Horizontal,
                                            );
                                            node.layout.align = layout::Align::Center;
                                            node.layout.justify = layout::JustifyContent::Center;
                                        }
                                    }
                                    if let Some(align) =
                                        layout_obj.get("align").and_then(|v| v.as_str())
                                    {
                                        node.layout.align = match align {
                                            "center" => layout::Align::Center,
                                            "end" => layout::Align::End,
                                            "stretch" => layout::Align::Stretch,
                                            _ => layout::Align::Start,
                                        };
                                    }
                                    if let Some(justify) =
                                        layout_obj.get("justify").and_then(|v| v.as_str())
                                    {
                                        node.layout.justify = match justify {
                                            "center" => layout::JustifyContent::Center,
                                            "end" => layout::JustifyContent::End,
                                            "space_between" | "space-between" => {
                                                layout::JustifyContent::SpaceBetween
                                            }
                                            "space_around" | "space-around" => {
                                                layout::JustifyContent::SpaceAround
                                            }
                                            _ => layout::JustifyContent::Start,
                                        };
                                    }
                                }
                                if let Some(pad) =
                                    layout_obj.get("padding").and_then(|v| v.as_array())
                                {
                                    if pad.len() == 4 {
                                        node.layout.padding = (
                                            pad[0].as_f64().unwrap_or(0.0) as f32,
                                            pad[1].as_f64().unwrap_or(0.0) as f32,
                                            pad[2].as_f64().unwrap_or(0.0) as f32,
                                            pad[3].as_f64().unwrap_or(0.0) as f32,
                                        );
                                    }
                                }
                            }
                        }
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

                if let Some(node) = self.nodes.get_mut(id) {
                    if let WidgetContent::Module {
                        payload: current, ..
                    } = &mut node.content
                    {
                        *current = payload.clone();
                    }
                    let text_candidate = payload.get("text").or_else(|| payload.get("label"));
                    if let Some(text_val) = text_candidate.and_then(|v| v.as_str()) {
                        let is_compact_button = node.layout.fixed_width.is_some_and(|w| w <= 48.0);
                        let is_short_code_or_vert_clock = text_val.contains('\n')
                            || (text_val.chars().count() <= 2 && !text_val.contains(' '));
                        let resolved_text = if is_compact_button && !is_short_code_or_vert_clock {
                            if let Some(glyph) = payload
                                .get("icon_glyph")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                            {
                                Some(glyph.to_string())
                            } else {
                                text_val
                                    .chars()
                                    .find(|c| (*c as u32) >= 0x2300)
                                    .map(|icon_char| icon_char.to_string())
                            }
                        } else {
                            Some(text_val.to_string())
                        };
                        if let Some(new_text) = resolved_text {
                            match &mut node.content {
                                WidgetContent::Text { text } => *text = new_text,
                                WidgetContent::Button { label, .. } if node.children.is_empty() => {
                                    *label = new_text;
                                }
                                _ => {}
                            }
                        }
                    }
                    if let Some(path_val) = payload
                        .get("path")
                        .or_else(|| payload.get("art_url"))
                        .or_else(|| payload.get("cover_url"))
                        .or_else(|| payload.get("image"))
                        .or_else(|| payload.get("src"))
                        .or_else(|| payload.get("icon_path"))
                        .or_else(|| payload.get("icon"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        match &mut node.content {
                            WidgetContent::Image { path } | WidgetContent::Svg { path } => {
                                *path = path_val.to_string();
                            }
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
        let default_gap = find_default_style(&self.styles)
            .and_then(|s| s.gap)
            .unwrap_or(0.0);
        let parent_node = match self.nodes.get_mut(parent_id) {
            Some(parent) => {
                let nid = parent.id.as_deref().unwrap_or("");
                if matches!(
                    parent.content,
                    WidgetContent::Module { .. } | WidgetContent::Container
                ) {
                    if nid.contains("popup") || nid.contains("root") {
                        parent.layout.mode =
                            layout::LayoutMode::Flex(layout::FlexDirection::Vertical);
                        parent.layout.align = layout::Align::Stretch;
                        parent.layout.justify = layout::JustifyContent::Start;
                        if parent.layout.gap == 0.0 {
                            parent.layout.gap = default_gap;
                        }
                    } else if parent.layout.mode == layout::LayoutMode::default() {
                        parent.layout.mode =
                            layout::LayoutMode::Flex(layout::FlexDirection::Horizontal);
                        parent.layout.align = layout::Align::Center;
                        parent.layout.justify = layout::JustifyContent::Center;
                        if parent.layout.gap == 0.0 {
                            parent.layout.gap = default_gap;
                        }
                    }
                }
                parent.clone()
            }
            None => return,
        };

        let is_vertical_bar_module = parent_node.layout.mode
            == layout::LayoutMode::Flex(layout::FlexDirection::Vertical)
            && !parent_node.id.as_deref().unwrap_or("").contains("popup")
            && !parent_node.id.as_deref().unwrap_or("").contains("root");

        // Preserve scroll_offset_y by widget id across dynamic subtree rebuilds
        let mut saved_scroll_offsets: std::collections::HashMap<String, f32> =
            std::collections::HashMap::new();
        fn collect_scroll_offsets(
            tree: &WidgetTree,
            id: WidgetId,
            out: &mut std::collections::HashMap<String, f32>,
        ) {
            if let Some(node) = tree.get(id) {
                if node.layout.scroll_offset_y > 0.0 {
                    if let Some(ref wid) = node.id {
                        out.insert(wid.clone(), node.layout.scroll_offset_y);
                    }
                }
                for &child in &node.children {
                    collect_scroll_offsets(tree, child, out);
                }
            }
        }
        fn restore_scroll_offsets(
            tree: &mut WidgetTree,
            id: WidgetId,
            saved: &std::collections::HashMap<String, f32>,
        ) {
            let children = if let Some(node) = tree.get_mut(id) {
                if let Some(ref wid) = node.id {
                    if let Some(&offset) = saved.get(wid) {
                        node.layout.scroll_offset_y = offset;
                    }
                }
                node.children.clone()
            } else {
                return;
            };
            for child in children {
                restore_scroll_offsets(tree, child, saved);
            }
        }

        // Remove old children
        let old_children: Vec<WidgetId> = if let Some(parent) = self.nodes.get_mut(parent_id) {
            std::mem::take(&mut parent.children)
        } else {
            return;
        };
        for &child in &old_children {
            collect_scroll_offsets(self, child, &mut saved_scroll_offsets);
        }
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

            let cid = insert_configured(self, Some(parent_id), &child_cfg, &styles);
            if is_vertical_bar_module {
                Self::adapt_child_for_vertical_bar(self, cid);
            }
            if !saved_scroll_offsets.is_empty() {
                restore_scroll_offsets(self, cid, &saved_scroll_offsets);
            }
        }
    }

    fn adapt_child_for_vertical_bar(tree: &mut WidgetTree, child_id: WidgetId) {
        let vstyle = tree
            .default_vertical_style
            .as_deref()
            .and_then(|s| tree.styles.get(s))
            .cloned();
        if let Some(node) = tree.nodes.get_mut(child_id) {
            if matches!(node.content, WidgetContent::Button { .. }) {
                node.layout.mode = layout::LayoutMode::Flex(layout::FlexDirection::Vertical);
                node.layout.align = layout::Align::Center;
                node.layout.justify = layout::JustifyContent::Center;
                if let Some(ref style) = vstyle {
                    apply_style_to_node(node, style);
                }
            }
        }
    }

    fn update_dynamic_children_in_place(
        &mut self,
        parent_id: WidgetId,
        children_json: &[serde_json::Value],
    ) -> bool {
        let (old_children, is_vertical_bar_module) = match self.nodes.get(parent_id) {
            Some(node) if node.children.len() == children_json.len() => {
                let is_vert = node.layout.mode
                    == layout::LayoutMode::Flex(layout::FlexDirection::Vertical)
                    && !node.id.as_deref().unwrap_or("").contains("popup")
                    && !node.id.as_deref().unwrap_or("").contains("root");
                (node.children.clone(), is_vert)
            }
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
            if is_vertical_bar_module {
                Self::adapt_child_for_vertical_bar(&mut fresh, fresh_id);
            }
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
                let prev_scroll = node.layout.scroll_offset_y;
                node.id = fresh_node.id;
                node.layout = fresh_node.layout;
                node.layout.scroll_offset_y = prev_scroll;
                node.style = fresh_node.style;
                if !matches!(node.content, WidgetContent::TextInput { focused: true, .. }) {
                    node.content = fresh_node.content;
                }
                node.on_click = fresh_node.on_click;
                node.on_right_click = fresh_node.on_right_click;
                node.on_scroll = fresh_node.on_scroll;
                node.on_change = fresh_node.on_change;
                node.tooltip = fresh_node.tooltip;
                node.dirty = true;
            }
            if !nested_children.is_empty()
                && !self.update_dynamic_children_in_place_for_ids(
                    child_id,
                    &existing_children,
                    &nested_children,
                )
            {
                return false;
            }
        }
        true
    }

    fn update_dynamic_children_in_place_for_ids(
        &mut self,
        parent_id: WidgetId,
        old_children: &[WidgetId],
        children_json: &[WidgetConfig],
    ) -> bool {
        if old_children.len() != children_json.len() {
            return false;
        }
        let parent_style = self.nodes.get(parent_id).map(|n| n.style.clone());
        let styles = Arc::clone(&self.styles);
        for (child_id, config) in old_children.iter().zip(children_json) {
            let mut fresh = WidgetTree::new();
            let fresh_id = insert_configured(&mut fresh, None, config, &styles);
            let mut fresh_node = match fresh.get(fresh_id).cloned() {
                Some(node) => node,
                None => return false,
            };
            if config.style.is_none() {
                if let Some(ref pstyle) = parent_style {
                    fresh_node.style.font_family = pstyle.font_family.clone();
                    fresh_node.style.font_size = pstyle.font_size;
                    if pstyle.foreground.is_some() {
                        fresh_node.style.foreground = pstyle.foreground;
                    }
                }
            }
            let existing_children = match self.nodes.get(*child_id) {
                Some(node) if node.children.len() == config.children.len() => node.children.clone(),
                _ => return false,
            };
            if let Some(node) = self.nodes.get_mut(*child_id) {
                let prev_scroll = node.layout.scroll_offset_y;
                node.id = fresh_node.id;
                node.layout = fresh_node.layout;
                node.layout.scroll_offset_y = prev_scroll;
                node.style = fresh_node.style;
                if !matches!(node.content, WidgetContent::TextInput { focused: true, .. }) {
                    node.content = fresh_node.content;
                }
                node.on_click = fresh_node.on_click;
                node.on_right_click = fresh_node.on_right_click;
                node.on_scroll = fresh_node.on_scroll;
                node.on_change = fresh_node.on_change;
                node.tooltip = fresh_node.tooltip;
                node.dirty = true;
            }
            if !config.children.is_empty()
                && !self.update_dynamic_children_in_place_for_ids(
                    *child_id,
                    &existing_children,
                    &config.children,
                )
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
            let (mut left, mut top, mut width, mut height) = node.final_rect;
            if let Some(t) = node.layout.transform.as_ref() {
                let sx = t.scale_x.unwrap_or(1.0);
                let sy = t.scale_y.unwrap_or(1.0);
                let tx = t.translate_x.unwrap_or(0.0);
                let ty = t.translate_y.unwrap_or(0.0);
                let new_w = width * sx;
                let new_h = height * sy;
                left += tx - (new_w - width) * 0.5;
                top += ty - (new_h - height) * 0.5;
                width = new_w;
                height = new_h;
            }
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

    false
}

impl wyrd_graphics::FocusableScene for WidgetTree {
    fn node_name(&self, id: WidgetId) -> Option<&str> {
        self.get(id).and_then(|n| n.id.as_deref())
    }

    fn hit_test_node(&self, x: f64, y: f64) -> Option<WidgetId> {
        self.hit_test(x, y)
    }
}
