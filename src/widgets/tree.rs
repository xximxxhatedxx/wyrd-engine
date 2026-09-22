//! Tree operations: measure, layout, draw dispatch with panic isolation.

use crate::render::context::RenderContext;
use crate::widgets::{WidgetId, WidgetTree};
use log::error;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Measure pass: top-down size negotiation.
pub fn measure_tree(
    tree: &mut WidgetTree,
    ctx: &mut RenderContext,
    available_width: f32,
    available_height: f32,
) {
    if let Some(root) = tree.root() {
        measure_node(tree, root, ctx, available_width, available_height);
    }
}

fn measure_node(
    tree: &mut WidgetTree,
    id: WidgetId,
    ctx: &mut RenderContext,
    w: f32,
    h: f32,
) -> (f32, f32) {
    if tree.get(id).is_some_and(|node| node.failed) {
        return (0.0, 0.0);
    }
    let child_count = tree.get(id).map_or(0, |node| node.children.len());
    for i in 0..child_count {
        if let Some(child) = tree.get(id).and_then(|node| node.children.get(i).copied()) {
            measure_node(tree, child, ctx, w, h);
        }
    }

    let result = catch_unwind(AssertUnwindSafe(|| {
        let node = tree.get(id).expect("widget disappeared during measure");
        let (pt, pr, pb, pl) = if node.layout.padding != (0.0, 0.0, 0.0, 0.0) {
            node.layout.padding
        } else {
            node.style.padding
        };
        let pad_h = pr + pl;
        let pad_v = pt + pb;
        let font_family = ctx.resolve_font_family(&node.style.font_family);
        let font_size = node.style.font_size.max(8.0);
        let children = &node.children;

        let size = if !children.is_empty() {
            match node.layout.mode {
                crate::widgets::layout::LayoutMode::Flex(
                    crate::widgets::layout::FlexDirection::Horizontal,
                ) => {
                    let total_gap = node.layout.gap * (children.len().saturating_sub(1) as f32);
                    let mut total_w = total_gap;
                    let mut max_h: f32 = 0.0;
                    for &child_id in children {
                        if let Some(child) = tree.get(child_id) {
                            total_w += child.layout.fixed_width.unwrap_or(child.measured_size.0);
                            max_h = max_h
                                .max(child.layout.fixed_height.unwrap_or(child.measured_size.1));
                        }
                    }
                    (
                        (total_w + pad_h).min(w),
                        (max_h + pad_v).max(font_size * 1.25 + pad_v).min(h),
                    )
                }
                crate::widgets::layout::LayoutMode::Flex(
                    crate::widgets::layout::FlexDirection::Vertical,
                ) => {
                    let total_gap = node.layout.gap * (children.len().saturating_sub(1) as f32);
                    let mut total_h = total_gap;
                    let mut max_w: f32 = 0.0;
                    for &child_id in children {
                        if let Some(child) = tree.get(child_id) {
                            max_w = max_w
                                .max(child.layout.fixed_width.unwrap_or(child.measured_size.0));
                            total_h += child.layout.fixed_height.unwrap_or(child.measured_size.1);
                        }
                    }
                    ((max_w + pad_h).min(w), (total_h + pad_v).min(h))
                }
                crate::widgets::layout::LayoutMode::Grid(ref grid) => {
                    let num_cols = if grid.columns.is_empty() {
                        1
                    } else {
                        grid.columns.len()
                    };
                    let num_rows = if !grid.rows.is_empty() {
                        grid.rows.len()
                    } else {
                        children.len().div_ceil(num_cols)
                    }
                    .max(1);
                    let col_gap = grid.gap.0 * (num_cols.saturating_sub(1) as f32);
                    let row_gap = grid.gap.1 * (num_rows.saturating_sub(1) as f32);
                    let mut max_cell_w: f32 = 40.0;
                    let mut max_cell_h: f32 = 30.0;
                    for &child_id in children {
                        if let Some(child) = tree.get(child_id) {
                            max_cell_w = max_cell_w.max(child.measured_size.0);
                            max_cell_h = max_cell_h.max(child.measured_size.1);
                        }
                    }
                    let total_w = max_cell_w * (num_cols as f32) + col_gap + pad_h;
                    let total_h = max_cell_h * (num_rows as f32) + row_gap + pad_v;
                    (total_w.min(w), total_h.min(h))
                }
                crate::widgets::layout::LayoutMode::Absolute => {
                    let mut max_x: f32 = 0.0;
                    let mut max_y: f32 = 0.0;
                    for &child_id in children {
                        if let Some(child) = tree.get(child_id) {
                            let (cx, cy, cw, ch) = child.layout.absolute;
                            let child_w = cw
                                .or(child.layout.fixed_width)
                                .unwrap_or(child.measured_size.0);
                            let child_h = ch
                                .or(child.layout.fixed_height)
                                .unwrap_or(child.measured_size.1);
                            let x_pos = cx.unwrap_or(0.0) + child_w;
                            let y_pos = cy.unwrap_or(0.0) + child_h;
                            max_x = max_x.max(x_pos);
                            max_y = max_y.max(y_pos);
                        }
                    }
                    let width = if max_x > 0.0 { max_x + pad_h } else { w };
                    let height = if max_y > 0.0 { max_y + pad_v } else { h };
                    (width.min(w), height.min(h))
                }
                _ => {
                    let mut max_w: f32 = 0.0;
                    let mut max_h: f32 = 0.0;
                    for &child_id in children {
                        if let Some(child) = tree.get(child_id) {
                            max_w = max_w.max(child.measured_size.0);
                            max_h = max_h.max(child.measured_size.1);
                        }
                    }
                    ((max_w + pad_h).min(w), (max_h + pad_v).min(h))
                }
            }
        } else {
            match &node.content {
                crate::widgets::WidgetContent::Text { text } => {
                    let avail_w = (w - pad_h).max(1.0);
                    let (tw, th) = crate::render::text::TextRenderer::measure_constrained(
                        ctx,
                        text,
                        font_family,
                        font_size,
                        Some(avail_w),
                    );
                    (
                        (tw + pad_h).min(w),
                        (th + pad_v).max(font_size * 1.25 + pad_v).min(h),
                    )
                }
                crate::widgets::WidgetContent::Button { label, .. } => {
                    let (tw, th) = crate::render::text::TextRenderer::measure(
                        ctx,
                        label,
                        font_family,
                        font_size,
                    );
                    (
                        (tw + pad_h + 12.0).min(w),
                        (th + pad_v).max(font_size * 1.25 + pad_v).min(h),
                    )
                }
                crate::widgets::WidgetContent::TextInput {
                    text, placeholder, ..
                } => {
                    let display = if text.is_empty() { placeholder } else { text };
                    let (tw, th) = crate::render::text::TextRenderer::measure(
                        ctx,
                        display,
                        font_family,
                        font_size,
                    );
                    (
                        (tw + pad_h + 16.0).max(140.0).min(w),
                        (th + pad_v).max(font_size * 1.5 + pad_v).min(h),
                    )
                }
                crate::widgets::WidgetContent::Progress { .. }
                | crate::widgets::WidgetContent::Slider { .. } => (
                    node.layout.fixed_width.unwrap_or(160.0).min(w),
                    node.layout.fixed_height.unwrap_or(20.0).min(h),
                ),
                crate::widgets::WidgetContent::ProgressRing { .. } => {
                    let size = node
                        .layout
                        .fixed_width
                        .or(node.layout.fixed_height)
                        .unwrap_or(80.0);
                    (
                        node.layout.fixed_width.unwrap_or(size).min(w),
                        node.layout.fixed_height.unwrap_or(size).min(h),
                    )
                }
                crate::widgets::WidgetContent::Module { payload, .. } => {
                    if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
                        let (tw, th) = crate::render::text::TextRenderer::measure(
                            ctx,
                            text,
                            font_family,
                            font_size,
                        );
                        (
                            (tw + pad_h).min(w),
                            (th + pad_v).max(font_size * 1.25 + pad_v).min(h),
                        )
                    } else {
                        (
                            node.layout.fixed_width.unwrap_or(120.0).min(w),
                            node.layout.fixed_height.unwrap_or(h.min(30.0)),
                        )
                    }
                }
                crate::widgets::WidgetContent::Image { path } => {
                    let trimmed = path.trim();
                    if trimmed.is_empty() {
                        (0.0, 0.0)
                    } else {
                        let fallback_w = node.layout.fixed_width.unwrap_or(w.min(120.0));
                        let fallback_h = node.layout.fixed_height.unwrap_or(h.min(30.0));
                        image::image_dimensions(trimmed)
                            .map(|(width, height)| {
                                let iw = node
                                    .layout
                                    .fixed_width
                                    .unwrap_or((width as f32 + pad_h).min(w));
                                let ih = node
                                    .layout
                                    .fixed_height
                                    .unwrap_or((height as f32 + pad_v).min(h));
                                (iw, ih)
                            })
                            .unwrap_or((fallback_w, fallback_h))
                    }
                }
                crate::widgets::WidgetContent::Svg { path } => {
                    let trimmed = path.trim();
                    if trimmed.is_empty() {
                        (0.0, 0.0)
                    } else {
                        let fallback_w = node.layout.fixed_width.unwrap_or(w.min(120.0));
                        let fallback_h = node.layout.fixed_height.unwrap_or(h.min(30.0));
                        std::fs::read(trimmed)
                            .ok()
                            .and_then(|data| {
                                resvg::usvg::Tree::from_data(
                                    &data,
                                    &resvg::usvg::Options::default(),
                                )
                                .ok()
                            })
                            .map(|tree| {
                                let iw = node
                                    .layout
                                    .fixed_width
                                    .unwrap_or((tree.size().width() + pad_h).min(w));
                                let ih = node
                                    .layout
                                    .fixed_height
                                    .unwrap_or((tree.size().height() + pad_v).min(h));
                                (iw, ih)
                            })
                            .unwrap_or((fallback_w, fallback_h))
                    }
                }
                crate::widgets::WidgetContent::Container => (
                    node.layout.fixed_width.unwrap_or(w),
                    node.layout.fixed_height.unwrap_or(h),
                ),
            }
        };

        let mut width = node.layout.fixed_width.unwrap_or(size.0);
        if let Some(min) = node.layout.min_width {
            width = width.max(min);
        }
        if let Some(max) = node.layout.max_width {
            width = width.min(max);
        }

        let mut height = node.layout.fixed_height.unwrap_or(size.1);
        if let Some(min) = node.layout.min_height {
            height = height.max(min);
        }
        if let Some(max) = node.layout.max_height {
            height = height.min(max);
        }
        (width, height)
    }));

    match result {
        Ok(size) => {
            if let Some(node) = tree.get_mut(id) {
                node.measured_size = size;
            }
            size
        }
        Err(_) => {
            error!("Widget {:?} panicked during measure; marking as failed", id);
            if let Some(node) = tree.get_mut(id) {
                node.measured_size = (0.0, 0.0);
                node.failed = true;
            }
            (0.0, 0.0)
        }
    }
}

/// Layout pass: assign final positions.
pub fn layout_tree(tree: &mut WidgetTree, x: f32, y: f32, width: f32, height: f32) {
    if let Some(root) = tree.root() {
        layout_node(tree, root, x, y, width, height);
    }
}

fn layout_node(tree: &mut WidgetTree, id: WidgetId, x: f32, y: f32, w: f32, h: f32) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(node) = tree.get_mut(id) {
            node.final_rect = (x, y, w, h);
        }
    }));

    let mut stack_children = [WidgetId::default(); 64];
    let (child_count, mode, gap, padding) = match tree.get(id) {
        Some(node) => {
            let count = node.children.len();
            let pad = if node.layout.padding != (0.0, 0.0, 0.0, 0.0) {
                node.layout.padding
            } else {
                node.style.padding
            };
            if count <= stack_children.len() {
                stack_children[..count].copy_from_slice(&node.children);
            }
            (count, node.layout.mode.clone(), node.layout.gap, pad)
        }
        None => return,
    };

    if child_count > 0 {
        if child_count <= stack_children.len() {
            let children_slice = &stack_children[..child_count];
            match mode {
                crate::widgets::layout::LayoutMode::Flex(direction) => {
                    crate::widgets::layout::flex_layout(
                        tree,
                        id,
                        (x, y, w, h),
                        direction,
                        gap,
                        padding,
                        children_slice,
                    );
                }
                crate::widgets::layout::LayoutMode::Absolute => {
                    crate::widgets::layout::absolute_layout(tree, id, (x, y, w, h), children_slice);
                }
                crate::widgets::layout::LayoutMode::Stack => {
                    crate::widgets::layout::stack_layout(tree, id, (x, y, w, h), children_slice);
                }
                crate::widgets::layout::LayoutMode::Grid(ref grid) => {
                    crate::widgets::layout::grid_layout(
                        tree,
                        id,
                        (x, y, w, h),
                        grid,
                        padding,
                        children_slice,
                    );
                }
            }
            for &child in children_slice {
                if let Some(rect) = tree.get(child).map(|node| node.final_rect) {
                    layout_node(tree, child, rect.0, rect.1, rect.2, rect.3);
                }
            }
        } else {
            let heap_children = tree.get(id).map(|n| n.children.clone()).unwrap_or_default();
            match mode {
                crate::widgets::layout::LayoutMode::Flex(direction) => {
                    crate::widgets::layout::flex_layout(
                        tree,
                        id,
                        (x, y, w, h),
                        direction,
                        gap,
                        padding,
                        &heap_children,
                    );
                }
                crate::widgets::layout::LayoutMode::Absolute => {
                    crate::widgets::layout::absolute_layout(tree, id, (x, y, w, h), &heap_children);
                }
                crate::widgets::layout::LayoutMode::Stack => {
                    crate::widgets::layout::stack_layout(tree, id, (x, y, w, h), &heap_children);
                }
                crate::widgets::layout::LayoutMode::Grid(ref grid) => {
                    crate::widgets::layout::grid_layout(
                        tree,
                        id,
                        (x, y, w, h),
                        grid,
                        padding,
                        &heap_children,
                    );
                }
            }
            for child in heap_children {
                if let Some(rect) = tree.get(child).map(|node| node.final_rect) {
                    layout_node(tree, child, rect.0, rect.1, rect.2, rect.3);
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeDiff {
    pub preserved: Vec<(WidgetId, WidgetId)>, // (old_id, new_id)
    pub added: Vec<WidgetId>,
    pub removed: Vec<WidgetId>,
    pub changed: Vec<WidgetId>,
}

fn node_semantically_equal(a: &crate::widgets::WidgetNode, b: &crate::widgets::WidgetNode) -> bool {
    a.id == b.id
        && a.content == b.content
        && a.style == b.style
        && a.layout == b.layout
        && a.on_click == b.on_click
        && a.on_right_click == b.on_right_click
        && a.on_scroll == b.on_scroll
        && a.on_change == b.on_change
        && a.tooltip == b.tooltip
}

fn collect_subtree(tree: &WidgetTree, root: WidgetId, list: &mut Vec<WidgetId>) {
    list.push(root);
    if let Some(node) = tree.get(root) {
        for &child in &node.children {
            collect_subtree(tree, child, list);
        }
    }
}

pub fn diff_trees(old: &WidgetTree, new: &WidgetTree) -> TreeDiff {
    let old_root = old.root();
    let new_root = new.root();

    match (old_root, new_root) {
        (None, None) => TreeDiff {
            preserved: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
            changed: Vec::new(),
        },
        (None, Some(_)) => TreeDiff {
            preserved: Vec::new(),
            added: new.iter_nodes().map(|(id, _)| id).collect(),
            removed: Vec::new(),
            changed: Vec::new(),
        },
        (Some(_), None) => TreeDiff {
            preserved: Vec::new(),
            added: Vec::new(),
            removed: old.iter_nodes().map(|(id, _)| id).collect(),
            changed: Vec::new(),
        },
        (Some(old_r), Some(new_r)) => {
            let old_r_node = match old.get(old_r) {
                Some(n) => n,
                None => {
                    return TreeDiff {
                        preserved: Vec::new(),
                        added: new.iter_nodes().map(|(id, _)| id).collect(),
                        removed: old.iter_nodes().map(|(id, _)| id).collect(),
                        changed: Vec::new(),
                    };
                }
            };
            let new_r_node = match new.get(new_r) {
                Some(n) => n,
                None => {
                    return TreeDiff {
                        preserved: Vec::new(),
                        added: new.iter_nodes().map(|(id, _)| id).collect(),
                        removed: old.iter_nodes().map(|(id, _)| id).collect(),
                        changed: Vec::new(),
                    };
                }
            };

            // Check if root's kind changed: fallback (empty preserved list)
            if old_r_node.content.kind() != new_r_node.content.kind() {
                return TreeDiff {
                    preserved: Vec::new(),
                    added: new.iter_nodes().map(|(id, _)| id).collect(),
                    removed: old.iter_nodes().map(|(id, _)| id).collect(),
                    changed: Vec::new(),
                };
            }

            let mut preserved = Vec::new();
            let mut added = Vec::new();
            let mut removed = Vec::new();
            let mut changed = Vec::new();

            diff_nodes_recursive(
                old,
                new,
                old_r,
                new_r,
                &mut preserved,
                &mut added,
                &mut removed,
                &mut changed,
            );

            TreeDiff {
                preserved,
                added,
                removed,
                changed,
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn diff_nodes_recursive(
    old: &WidgetTree,
    new: &WidgetTree,
    old_id: WidgetId,
    new_id: WidgetId,
    preserved: &mut Vec<(WidgetId, WidgetId)>,
    added: &mut Vec<WidgetId>,
    removed: &mut Vec<WidgetId>,
    changed: &mut Vec<WidgetId>,
) {
    let old_node = match old.get(old_id) {
        Some(n) => n,
        None => return,
    };
    let new_node = match new.get(new_id) {
        Some(n) => n,
        None => return,
    };

    preserved.push((old_id, new_id));

    if !node_semantically_equal(old_node, new_node) {
        changed.push(new_id);
    }

    let old_children = &old_node.children;
    let new_children = &new_node.children;

    let mut matched_old = vec![false; old_children.len()];
    let mut matched_new = vec![false; new_children.len()];

    // Pass 1: match children with identical explicit IDs and same kind
    for (ni, &nc_id) in new_children.iter().enumerate() {
        if let Some(nc_node) = new.get(nc_id) {
            if let Some(ref target_id) = nc_node.id {
                for (oi, &oc_id) in old_children.iter().enumerate() {
                    if !matched_old[oi] {
                        if let Some(oc_node) = old.get(oc_id) {
                            if oc_node.id.as_deref() == Some(target_id)
                                && oc_node.content.kind() == nc_node.content.kind()
                            {
                                matched_old[oi] = true;
                                matched_new[ni] = true;
                                diff_nodes_recursive(
                                    old, new, oc_id, nc_id, preserved, added, removed, changed,
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    // Pass 2: match remaining children in relative order by matching kind
    let mut oi = 0;
    for (ni, &nc_id) in new_children.iter().enumerate() {
        if matched_new[ni] {
            continue;
        }
        let nc_node = match new.get(nc_id) {
            Some(n) => n,
            None => continue,
        };

        while oi < old_children.len() {
            if !matched_old[oi] {
                if let Some(oc_node) = old.get(old_children[oi]) {
                    let id_compatible = match (&oc_node.id, &nc_node.id) {
                        (Some(a), Some(b)) => a == b,
                        (None, None) => true,
                        _ => false,
                    };
                    if id_compatible && oc_node.content.kind() == nc_node.content.kind() {
                        matched_old[oi] = true;
                        matched_new[ni] = true;
                        diff_nodes_recursive(
                            old,
                            new,
                            old_children[oi],
                            nc_id,
                            preserved,
                            added,
                            removed,
                            changed,
                        );
                        oi += 1;
                        break;
                    }
                }
            }
            oi += 1;
        }
    }

    // Pass 3: any unmatched new children and their entire subtrees are added
    for (ni, &nc_id) in new_children.iter().enumerate() {
        if !matched_new[ni] {
            collect_subtree(new, nc_id, added);
        }
    }

    // Pass 4: any unmatched old children and their entire subtrees are removed
    for (oi, &oc_id) in old_children.iter().enumerate() {
        if !matched_old[oi] {
            collect_subtree(old, oc_id, removed);
        }
    }
}

pub fn apply_diff(old_tree: &mut WidgetTree, new_tree: &WidgetTree, diff: &TreeDiff) {
    if diff.preserved.is_empty() && new_tree.root().is_some() {
        log::debug!("Tree diff fallback: replacing entire widget tree");
        *old_tree = new_tree.clone();
        return;
    }

    let mut mapping: std::collections::HashMap<WidgetId, WidgetId> =
        std::collections::HashMap::new();
    for &(old_id, new_id) in &diff.preserved {
        mapping.insert(new_id, old_id);
    }

    for &old_id in &diff.removed {
        old_tree.remove(old_id);
    }

    for &new_id in &diff.added {
        if let Some(new_node) = new_tree.get(new_id) {
            let mut node_to_insert = new_node.clone();
            node_to_insert.parent = None;
            node_to_insert.children.clear();
            let inserted_id = old_tree.insert(node_to_insert);
            mapping.insert(new_id, inserted_id);
        }
    }

    for &(old_id, new_id) in &diff.preserved {
        if let Some(new_node) = new_tree.get(new_id) {
            if let Some(old_node) = old_tree.get_mut(old_id) {
                old_node.id = new_node.id.clone();
                old_node.layout = new_node.layout.clone();
                old_node.style = new_node.style.clone();
                old_node.content = new_node.content.clone();
                old_node.on_click = new_node.on_click.clone();
                old_node.on_right_click = new_node.on_right_click.clone();
                old_node.on_scroll = new_node.on_scroll.clone();
                old_node.on_change = new_node.on_change.clone();
                old_node.tooltip = new_node.tooltip.clone();
                old_node.dirty = true;
            }
        }
    }

    for (&new_id, &target_id) in &mapping {
        if let Some(new_node) = new_tree.get(new_id) {
            let mapped_parent = new_node.parent.and_then(|p| mapping.get(&p).copied());
            let mapped_children: Vec<WidgetId> = new_node
                .children
                .iter()
                .filter_map(|c| mapping.get(c).copied())
                .collect();
            if let Some(target_node) = old_tree.get_mut(target_id) {
                target_node.parent = mapped_parent;
                target_node.children = mapped_children;
            }
        }
    }

    if let Some(new_root) = new_tree.root() {
        old_tree.root = mapping.get(&new_root).copied();
    } else {
        old_tree.root = None;
    }
    old_tree.styles = new_tree.styles.clone();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_widget_stays_quarantined_after_measure() {
        let mut tree = WidgetTree::new();
        let widget = tree.insert(crate::widgets::WidgetNode::new(
            crate::widgets::WidgetContent::Text {
                text: "hidden".into(),
            },
        ));
        tree.get_mut(widget).unwrap().failed = true;

        let mut context = RenderContext::new(1.0);
        measure_tree(&mut tree, &mut context, 100.0, 32.0);

        assert!(tree.get(widget).unwrap().failed);
    }
}
