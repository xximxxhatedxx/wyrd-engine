//! Layout engine: Flex, Absolute, Stack, and Grid.

use crate::widgets::{WidgetId, WidgetTree};

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Transform {
    pub translate_x: Option<f32>,
    pub translate_y: Option<f32>,
    pub rotate: Option<f32>, // degrees
    pub scale_x: Option<f32>,
    pub scale_y: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum GridSize {
    Fixed(f32),
    Fraction(f32),
    Auto,
}

impl GridSize {
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s == "auto" {
            GridSize::Auto
        } else if let Some(fr) = s.strip_suffix("fr") {
            GridSize::Fraction(fr.parse().unwrap_or(1.0))
        } else if let Some(px) = s.strip_suffix("px") {
            GridSize::Fixed(px.parse().unwrap_or(0.0))
        } else if let Ok(num) = s.parse::<f32>() {
            GridSize::Fixed(num)
        } else {
            GridSize::Auto
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GridLayout {
    pub columns: Vec<GridSize>,
    pub rows: Vec<GridSize>,
    pub gap: (f32, f32), // (column_gap, row_gap)
}

#[derive(Debug, Clone, PartialEq)]
pub enum LayoutMode {
    Flex(FlexDirection),
    Absolute,
    Stack,
    Grid(GridLayout),
}

impl Default for LayoutMode {
    fn default() -> Self {
        LayoutMode::Flex(FlexDirection::Horizontal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    Start,
    Center,
    End,
    #[default]
    Stretch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JustifyContent {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutParams {
    pub mode: LayoutMode,
    pub gap: f32,
    pub padding: (f32, f32, f32, f32),
    pub align: Align,
    pub justify: JustifyContent,
    pub valign: Align,
    pub fixed_width: Option<f32>,
    pub fixed_height: Option<f32>,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
    pub min_height: Option<f32>,
    pub max_height: Option<f32>,
    pub weight: f32,
    pub absolute: (Option<f32>, Option<f32>, Option<f32>, Option<f32>),
    pub transform: Option<Transform>,
    pub z_index: Option<i32>,
    pub format: Option<String>,
    pub scroll_y: bool,
    pub scroll_offset_y: f32,
    pub clip: bool,
}

pub fn grid_layout(
    tree: &mut WidgetTree,
    _parent: WidgetId,
    parent_rect: (f32, f32, f32, f32),
    grid: &GridLayout,
    padding: (f32, f32, f32, f32),
    children: &[WidgetId],
) {
    if children.is_empty() {
        return;
    }
    let (px, py, pw, ph) = parent_rect;
    let content_x = px + padding.3;
    let content_y = py + padding.0;
    let content_w = (pw - padding.1 - padding.3).max(0.0);
    let content_h = (ph - padding.0 - padding.2).max(0.0);

    let num_cols = if grid.columns.is_empty() {
        1
    } else {
        grid.columns.len()
    };
    let col_gap = grid.gap.0;
    let row_gap = grid.gap.1;

    let total_col_gap = col_gap * (num_cols.saturating_sub(1) as f32);
    let mut total_fixed_w = 0.0;
    let mut total_fr_w = 0.0;
    for col in &grid.columns {
        match col {
            GridSize::Fixed(w) => total_fixed_w += w,
            GridSize::Fraction(fr) => total_fr_w += fr,
            GridSize::Auto => total_fr_w += 1.0,
        }
    }
    if grid.columns.is_empty() {
        total_fr_w = 1.0;
    }
    let rem_w = (content_w - total_col_gap - total_fixed_w).max(0.0);

    let mut col_widths = Vec::with_capacity(num_cols);
    for col in &grid.columns {
        let w = match col {
            GridSize::Fixed(w) => *w,
            GridSize::Fraction(fr) => {
                if total_fr_w > 0.0 {
                    rem_w * (fr / total_fr_w)
                } else {
                    0.0
                }
            }
            GridSize::Auto => {
                if total_fr_w > 0.0 {
                    rem_w * (1.0 / total_fr_w)
                } else {
                    0.0
                }
            }
        };
        col_widths.push(w);
    }
    if grid.columns.is_empty() {
        col_widths.push(content_w);
    }

    let num_rows = if !grid.rows.is_empty() {
        grid.rows.len()
    } else {
        children.len().div_ceil(num_cols)
    };
    let num_rows = num_rows.max(1);
    let total_row_gap = row_gap * (num_rows.saturating_sub(1) as f32);
    let mut total_fixed_h = 0.0;
    let mut total_fr_h = 0.0;
    for row in &grid.rows {
        match row {
            GridSize::Fixed(h) => total_fixed_h += h,
            GridSize::Fraction(fr) => total_fr_h += fr,
            GridSize::Auto => total_fr_h += 1.0,
        }
    }
    if grid.rows.is_empty() {
        total_fr_h = num_rows as f32;
    }
    let rem_h = (content_h - total_row_gap - total_fixed_h).max(0.0);

    let mut row_heights = Vec::with_capacity(num_rows);
    if !grid.rows.is_empty() {
        for row in &grid.rows {
            let h = match row {
                GridSize::Fixed(h) => *h,
                GridSize::Fraction(fr) => {
                    if total_fr_h > 0.0 {
                        rem_h * (fr / total_fr_h)
                    } else {
                        0.0
                    }
                }
                GridSize::Auto => {
                    if total_fr_h > 0.0 {
                        rem_h * (1.0 / total_fr_h)
                    } else {
                        0.0
                    }
                }
            };
            row_heights.push(h);
        }
    } else {
        let h = if num_rows > 0 {
            (content_h - total_row_gap) / num_rows as f32
        } else {
            content_h
        };
        for _ in 0..num_rows {
            row_heights.push(h);
        }
    }

    let mut col_offsets = Vec::with_capacity(num_cols);
    let mut cur_x = 0.0;
    for &w in &col_widths {
        col_offsets.push(cur_x);
        cur_x += w + col_gap;
    }

    let mut row_offsets = Vec::with_capacity(num_rows);
    let mut cur_y = 0.0;
    for &h in &row_heights {
        row_offsets.push(cur_y);
        cur_y += h + row_gap;
    }

    for (i, &child_id) in children.iter().enumerate() {
        let col_idx = i % num_cols;
        let row_idx = i / num_cols;
        let cell_x = content_x + col_offsets.get(col_idx).copied().unwrap_or(0.0);
        let cell_y = content_y + row_offsets.get(row_idx).copied().unwrap_or(0.0);
        let cell_w = col_widths.get(col_idx).copied().unwrap_or(content_w);
        let cell_h = row_heights.get(row_idx).copied().unwrap_or(30.0);

        if let Some(child) = tree.get_mut(child_id) {
            let w = child.layout.fixed_width.unwrap_or(cell_w);
            let h = child.layout.fixed_height.unwrap_or(cell_h);
            child.final_rect = (cell_x, cell_y, w.min(cell_w), h.min(cell_h));
        }
    }
}

pub fn absolute_layout(
    tree: &mut WidgetTree,
    _parent: WidgetId,
    parent_rect: (f32, f32, f32, f32),
    children: &[WidgetId],
) {
    let (px, py, _pw, _ph) = parent_rect;
    for &child_id in children {
        let Some(child) = tree.get(child_id) else {
            continue;
        };
        let (x, y, width, height) = child.layout.absolute;
        let rect = (
            px + x.unwrap_or(0.0),
            py + y.unwrap_or(0.0),
            width
                .or(child.layout.fixed_width)
                .unwrap_or(child.measured_size.0),
            height
                .or(child.layout.fixed_height)
                .unwrap_or(child.measured_size.1),
        );
        if let Some(child) = tree.get_mut(child_id) {
            child.final_rect = rect;
        }
    }
}

pub fn stack_layout(
    tree: &mut WidgetTree,
    _parent: WidgetId,
    parent_rect: (f32, f32, f32, f32),
    children: &[WidgetId],
) {
    for &child_id in children {
        if let Some(child) = tree.get_mut(child_id) {
            child.final_rect = parent_rect;
        }
    }
}

/// Flex layout algorithm supporting both cross-axis (align) and main-axis (justify).
pub fn flex_layout(
    tree: &mut WidgetTree,
    parent: WidgetId,
    parent_rect: (f32, f32, f32, f32),
    direction: FlexDirection,
    gap: f32,
    padding: (f32, f32, f32, f32),
    children: &[WidgetId],
) {
    if children.is_empty() {
        return;
    }

    let (px, py, pw, ph) = parent_rect;
    let content_x = px + padding.3;
    let content_y = py + padding.0;
    let content_w = (pw - padding.1 - padding.3).max(0.0);
    let content_h = (ph - padding.0 - padding.2).max(0.0);

    let (align, justify) = tree
        .get(parent)
        .map(|node| (node.layout.align, node.layout.justify))
        .unwrap_or((Align::Start, JustifyContent::Start));

    let total_gap = gap * (children.len().saturating_sub(1) as f32);

    match direction {
        FlexDirection::Horizontal => {
            let total_weight: f32 = children
                .iter()
                .filter_map(|id| tree.get(*id))
                .map(|c| c.layout.weight)
                .sum();

            let fixed_width: f32 = children
                .iter()
                .filter_map(|id| tree.get(*id))
                .filter(|c| c.layout.weight == 0.0)
                .map(|c| {
                    let mut w = c.layout.fixed_width.unwrap_or(c.measured_size.0);
                    if let Some(min) = c.layout.min_width {
                        w = w.max(min);
                    }
                    if let Some(max) = c.layout.max_width {
                        w = w.min(max);
                    }
                    w
                })
                .sum();

            let (mut cursor, gap_extra, remaining) = if total_weight > 0.0 {
                let rem = (content_w - total_gap - fixed_width).max(0.0);
                (content_x, 0.0, rem)
            } else {
                let free_space = (content_w - total_gap - fixed_width).max(0.0);
                match justify {
                    JustifyContent::Start => (content_x, 0.0, 0.0),
                    JustifyContent::Center => (content_x + free_space / 2.0, 0.0, 0.0),
                    JustifyContent::End => (content_x + free_space, 0.0, 0.0),
                    JustifyContent::SpaceBetween => {
                        if children.len() > 1 {
                            (content_x, free_space / (children.len() - 1) as f32, 0.0)
                        } else {
                            (content_x, 0.0, 0.0)
                        }
                    }
                    JustifyContent::SpaceAround => {
                        let step = free_space / children.len() as f32;
                        (content_x + step / 2.0, step, 0.0)
                    }
                    JustifyContent::SpaceEvenly => {
                        let step = free_space / (children.len() + 1) as f32;
                        (content_x + step, step, 0.0)
                    }
                }
            };

            for &child_id in children {
                let child = match tree.get(child_id) {
                    Some(c) => c,
                    None => continue,
                };
                let mut child_w = if let Some(width) = child.layout.fixed_width {
                    width
                } else if child.layout.weight > 0.0 && total_weight > 0.0 {
                    remaining * (child.layout.weight / total_weight)
                } else {
                    child.measured_size.0
                };
                if let Some(min) = child.layout.min_width {
                    child_w = child_w.max(min);
                }
                if let Some(max) = child.layout.max_width {
                    child_w = child_w.min(max);
                }
                child_w = child_w.min(content_w);

                let mut child_h = child.layout.fixed_height.unwrap_or(child.measured_size.1);
                if let Some(min) = child.layout.min_height {
                    child_h = child_h.max(min);
                }
                if let Some(max) = child.layout.max_height {
                    child_h = child_h.min(max);
                }
                child_h = child_h.min(content_h);

                let child_y = match align {
                    Align::Center => content_y + (content_h - child_h) / 2.0,
                    Align::End => content_y + content_h - child_h,
                    Align::Stretch => content_y,
                    Align::Start => content_y,
                };
                let mut child_h = if align == Align::Stretch {
                    content_h
                } else {
                    child_h
                };
                if let Some(min) = child.layout.min_height {
                    child_h = child_h.max(min);
                }
                if let Some(max) = child.layout.max_height {
                    child_h = child_h.min(max);
                }

                if let Some(node) = tree.get_mut(child_id) {
                    node.final_rect = (cursor, child_y, child_w, child_h);
                }
                cursor += child_w + gap + gap_extra;
            }
        }
        FlexDirection::Vertical => {
            let (parent_scroll_y, parent_scroll_offset) = tree
                .get(parent)
                .map(|p| (p.layout.scroll_y, p.layout.scroll_offset_y))
                .unwrap_or((false, 0.0));

            let total_weight: f32 = children
                .iter()
                .filter_map(|id| tree.get(*id))
                .map(|c| c.layout.weight)
                .sum();

            let fixed_height: f32 = children
                .iter()
                .filter_map(|id| tree.get(*id))
                .filter(|c| c.layout.weight == 0.0)
                .map(|c| {
                    let mut h = c.layout.fixed_height.unwrap_or(c.measured_size.1);
                    if let Some(min) = c.layout.min_height {
                        h = h.max(min);
                    }
                    if let Some(max) = c.layout.max_height {
                        h = h.min(max);
                    }
                    h
                })
                .sum();

            let (mut cursor, gap_extra, remaining) = if total_weight > 0.0 {
                let rem = (content_h - total_gap - fixed_height).max(0.0);
                (content_y, 0.0, rem)
            } else {
                let free_space = (content_h - total_gap - fixed_height).max(0.0);
                match justify {
                    JustifyContent::Start => (content_y, 0.0, 0.0),
                    JustifyContent::Center => (content_y + free_space / 2.0, 0.0, 0.0),
                    JustifyContent::End => (content_y + free_space, 0.0, 0.0),
                    JustifyContent::SpaceBetween => {
                        if children.len() > 1 {
                            (content_y, free_space / (children.len() - 1) as f32, 0.0)
                        } else {
                            (content_y, 0.0, 0.0)
                        }
                    }
                    JustifyContent::SpaceAround => {
                        let step = free_space / children.len() as f32;
                        (content_y + step / 2.0, step, 0.0)
                    }
                    JustifyContent::SpaceEvenly => {
                        let step = free_space / (children.len() + 1) as f32;
                        (content_y + step, step, 0.0)
                    }
                }
            };

            if parent_scroll_y {
                cursor -= parent_scroll_offset;
            }

            for &child_id in children {
                let child = match tree.get(child_id) {
                    Some(c) => c,
                    None => continue,
                };
                let mut child_w = child.layout.fixed_width.unwrap_or(child.measured_size.0);
                if let Some(min) = child.layout.min_width {
                    child_w = child_w.max(min);
                }
                if let Some(max) = child.layout.max_width {
                    child_w = child_w.min(max);
                }
                child_w = child_w.min(content_w);

                let mut child_h = if let Some(height) = child.layout.fixed_height {
                    height
                } else if child.layout.weight > 0.0 && total_weight > 0.0 {
                    remaining * (child.layout.weight / total_weight)
                } else {
                    child.measured_size.1
                };
                if let Some(min) = child.layout.min_height {
                    child_h = child_h.max(min);
                }
                if let Some(max) = child.layout.max_height {
                    child_h = child_h.min(max);
                }
                if !parent_scroll_y {
                    child_h = child_h.min(content_h);
                }

                let child_x = match align {
                    Align::Center => content_x + (content_w - child_w) / 2.0,
                    Align::End => content_x + content_w - child_w,
                    Align::Stretch => content_x,
                    Align::Start => content_x,
                };
                let mut child_w = if align == Align::Stretch {
                    content_w
                } else {
                    child_w
                };
                if let Some(min) = child.layout.min_width {
                    child_w = child_w.max(min);
                }
                if let Some(max) = child.layout.max_width {
                    child_w = child_w.min(max);
                }

                if let Some(node) = tree.get_mut(child_id) {
                    node.final_rect = (child_x, cursor, child_w, child_h);
                }
                cursor += child_h + gap + gap_extra;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::{WidgetContent, WidgetNode};

    #[test]
    fn flex_horizontal_justify_center() {
        let mut tree = WidgetTree::new();
        let mut parent = WidgetNode::new(WidgetContent::Container);
        parent.layout.mode = LayoutMode::Flex(FlexDirection::Horizontal);
        parent.layout.justify = JustifyContent::Center;
        let parent_id = tree.insert(parent);

        let mut c1 = WidgetNode::new(WidgetContent::Text { text: "A".into() });
        c1.measured_size = (40.0, 20.0);
        let c1_id = tree.insert(c1);
        tree.append_child(parent_id, c1_id);

        let mut c2 = WidgetNode::new(WidgetContent::Text { text: "B".into() });
        c2.measured_size = (60.0, 20.0);
        let c2_id = tree.insert(c2);
        tree.append_child(parent_id, c2_id);

        flex_layout(
            &mut tree,
            parent_id,
            (0.0, 0.0, 200.0, 30.0),
            FlexDirection::Horizontal,
            10.0,
            (0.0, 0.0, 0.0, 0.0),
            &[c1_id, c2_id],
        );

        // Total content width = 40 + 10 + 60 = 110. Free space = 90. Centered start = 45.
        let rect1 = tree.get(c1_id).unwrap().final_rect;
        let rect2 = tree.get(c2_id).unwrap().final_rect;

        assert_eq!(rect1.0, 45.0);
        assert_eq!(rect1.2, 40.0);
        assert_eq!(rect2.0, 95.0);
        assert_eq!(rect2.2, 60.0);
    }

    #[test]
    fn flex_horizontal_justify_end() {
        let mut tree = WidgetTree::new();
        let mut parent = WidgetNode::new(WidgetContent::Container);
        parent.layout.mode = LayoutMode::Flex(FlexDirection::Horizontal);
        parent.layout.justify = JustifyContent::End;
        let parent_id = tree.insert(parent);

        let mut c1 = WidgetNode::new(WidgetContent::Text { text: "A".into() });
        c1.measured_size = (50.0, 20.0);
        let c1_id = tree.insert(c1);
        tree.append_child(parent_id, c1_id);

        flex_layout(
            &mut tree,
            parent_id,
            (0.0, 0.0, 200.0, 30.0),
            FlexDirection::Horizontal,
            0.0,
            (0.0, 0.0, 0.0, 0.0),
            &[c1_id],
        );

        // Free space = 150. End start = 150.
        let rect1 = tree.get(c1_id).unwrap().final_rect;
        assert_eq!(rect1.0, 150.0);
        assert_eq!(rect1.2, 50.0);
    }

    #[test]
    fn flex_horizontal_justify_space_between() {
        let mut tree = WidgetTree::new();
        let mut parent = WidgetNode::new(WidgetContent::Container);
        parent.layout.mode = LayoutMode::Flex(FlexDirection::Horizontal);
        parent.layout.justify = JustifyContent::SpaceBetween;
        let parent_id = tree.insert(parent);

        let mut c1 = WidgetNode::new(WidgetContent::Text {
            text: "Left".into(),
        });
        c1.measured_size = (50.0, 20.0);
        let c1_id = tree.insert(c1);
        tree.append_child(parent_id, c1_id);

        let mut c2 = WidgetNode::new(WidgetContent::Text {
            text: "Right".into(),
        });
        c2.measured_size = (50.0, 20.0);
        let c2_id = tree.insert(c2);
        tree.append_child(parent_id, c2_id);

        flex_layout(
            &mut tree,
            parent_id,
            (0.0, 0.0, 300.0, 30.0),
            FlexDirection::Horizontal,
            0.0,
            (0.0, 0.0, 0.0, 0.0),
            &[c1_id, c2_id],
        );

        let rect1 = tree.get(c1_id).unwrap().final_rect;
        let rect2 = tree.get(c2_id).unwrap().final_rect;

        assert_eq!(rect1.0, 0.0);
        assert_eq!(rect2.0, 250.0);
    }

    #[test]
    fn flex_horizontal_justify_space_between_three_children() {
        let mut tree = WidgetTree::new();
        let mut parent = WidgetNode::new(WidgetContent::Container);
        parent.layout.mode = LayoutMode::Flex(FlexDirection::Horizontal);
        parent.layout.justify = JustifyContent::SpaceBetween;
        let parent_id = tree.insert(parent);

        let mut c1 = WidgetNode::new(WidgetContent::Text {
            text: "Left".into(),
        });
        c1.measured_size = (50.0, 20.0);
        let c1_id = tree.insert(c1);
        tree.append_child(parent_id, c1_id);

        let mut c2 = WidgetNode::new(WidgetContent::Text {
            text: "Center".into(),
        });
        c2.measured_size = (60.0, 20.0);
        let c2_id = tree.insert(c2);
        tree.append_child(parent_id, c2_id);

        let mut c3 = WidgetNode::new(WidgetContent::Text {
            text: "Right".into(),
        });
        c3.measured_size = (50.0, 20.0);
        let c3_id = tree.insert(c3);
        tree.append_child(parent_id, c3_id);

        // Content width = 300. Used = 50 + 60 + 50 = 160. Free space = 140.
        // N = 3 => gap_extra = 140 / 2 = 70.
        // c1: 0..50
        // c2: 50 + 70 = 120 .. 180
        // c3: 180 + 70 = 250 .. 300
        flex_layout(
            &mut tree,
            parent_id,
            (0.0, 0.0, 300.0, 30.0),
            FlexDirection::Horizontal,
            0.0,
            (0.0, 0.0, 0.0, 0.0),
            &[c1_id, c2_id, c3_id],
        );

        let rect1 = tree.get(c1_id).unwrap().final_rect;
        let rect2 = tree.get(c2_id).unwrap().final_rect;
        let rect3 = tree.get(c3_id).unwrap().final_rect;

        assert_eq!(rect1.0, 0.0);
        assert_eq!(rect1.2, 50.0);
        assert_eq!(rect2.0, 120.0);
        assert_eq!(rect2.2, 60.0);
        assert_eq!(rect3.0, 250.0);
        assert_eq!(rect3.2, 50.0);
    }

    #[test]
    fn flex_min_max_constraints_clamping() {
        let mut tree = WidgetTree::new();
        let mut parent = WidgetNode::new(WidgetContent::Container);
        parent.layout.mode = LayoutMode::Flex(FlexDirection::Horizontal);
        let parent_id = tree.insert(parent);

        let mut c1 = WidgetNode::new(WidgetContent::Text { text: "A".into() });
        c1.measured_size = (20.0, 20.0);
        c1.layout.min_width = Some(50.0); // should clamp up to 50
        c1.layout.max_height = Some(15.0); // should clamp down to 15
        let c1_id = tree.insert(c1);
        tree.append_child(parent_id, c1_id);

        let mut c2 = WidgetNode::new(WidgetContent::Text { text: "B".into() });
        c2.measured_size = (100.0, 20.0);
        c2.layout.max_width = Some(60.0); // should clamp down to 60
        let c2_id = tree.insert(c2);
        tree.append_child(parent_id, c2_id);

        flex_layout(
            &mut tree,
            parent_id,
            (0.0, 0.0, 200.0, 30.0),
            FlexDirection::Horizontal,
            10.0,
            (0.0, 0.0, 0.0, 0.0),
            &[c1_id, c2_id],
        );

        let rect1 = tree.get(c1_id).unwrap().final_rect;
        let rect2 = tree.get(c2_id).unwrap().final_rect;

        assert_eq!(rect1.2, 50.0);
        assert_eq!(rect1.3, 15.0);
        assert_eq!(rect2.2, 60.0);
    }

    #[test]
    fn test_grid_layout_columns_and_rows() {
        let mut tree = WidgetTree::new();
        let mut parent = WidgetNode::new(WidgetContent::Container);
        let grid = GridLayout {
            columns: vec![GridSize::Fixed(100.0), GridSize::Fraction(1.0)],
            rows: vec![GridSize::Fixed(40.0), GridSize::Fixed(40.0)],
            gap: (10.0, 10.0),
        };
        parent.layout.mode = LayoutMode::Grid(grid.clone());
        let parent_id = tree.insert(parent);

        let c1_id = tree.insert(WidgetNode::new(WidgetContent::Text { text: "1".into() }));
        let c2_id = tree.insert(WidgetNode::new(WidgetContent::Text { text: "2".into() }));
        let c3_id = tree.insert(WidgetNode::new(WidgetContent::Text { text: "3".into() }));
        let c4_id = tree.insert(WidgetNode::new(WidgetContent::Text { text: "4".into() }));

        grid_layout(
            &mut tree,
            parent_id,
            (0.0, 0.0, 310.0, 90.0),
            &grid,
            (0.0, 0.0, 0.0, 0.0),
            &[c1_id, c2_id, c3_id, c4_id],
        );

        // Total width = 310. Col gap = 10. Fixed col0 = 100. Rem fr col1 = 310 - 10 - 100 = 200.
        // Row0: y = 0, h = 40. Row1: y = 50, h = 40.
        let r1 = tree.get(c1_id).unwrap().final_rect;
        let r2 = tree.get(c2_id).unwrap().final_rect;
        let r3 = tree.get(c3_id).unwrap().final_rect;
        let r4 = tree.get(c4_id).unwrap().final_rect;

        assert_eq!(r1, (0.0, 0.0, 100.0, 40.0));
        assert_eq!(r2, (110.0, 0.0, 200.0, 40.0));
        assert_eq!(r3, (0.0, 50.0, 100.0, 40.0));
        assert_eq!(r4, (110.0, 50.0, 200.0, 40.0));
    }

    #[test]
    fn test_absolute_layout_positions() {
        let mut tree = WidgetTree::new();
        let mut parent = WidgetNode::new(WidgetContent::Container);
        parent.layout.mode = LayoutMode::Absolute;
        let parent_id = tree.insert(parent);

        let mut c1 = WidgetNode::new(WidgetContent::Text {
            text: "TopLeft".into(),
        });
        c1.layout.absolute = (Some(0.0), Some(0.0), Some(50.0), Some(30.0));
        let c1_id = tree.insert(c1);

        let mut c2 = WidgetNode::new(WidgetContent::Button {
            label: "SpiralDay".into(),
            on_click: None,
        });
        c2.layout.absolute = (Some(250.0), Some(180.0), Some(34.0), Some(34.0));
        let c2_id = tree.insert(c2);

        absolute_layout(
            &mut tree,
            parent_id,
            (10.0, 20.0, 400.0, 400.0),
            &[c1_id, c2_id],
        );

        let r1 = tree.get(c1_id).unwrap().final_rect;
        let r2 = tree.get(c2_id).unwrap().final_rect;

        assert_eq!(r1, (10.0, 20.0, 50.0, 30.0));
        assert_eq!(r2, (260.0, 200.0, 34.0, 34.0));
    }
}
