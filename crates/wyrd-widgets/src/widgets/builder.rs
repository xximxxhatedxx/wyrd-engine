use super::*;
use std::sync::Arc;

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
                "space-between" | "space_between" => layout::JustifyContent::SpaceBetween,
                "space-around" | "space_around" => layout::JustifyContent::SpaceAround,
                "space-evenly" | "space_evenly" => layout::JustifyContent::SpaceEvenly,
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

    if let Some(style) = find_default_style(styles) {
        apply_style_to_node(&mut root_node, style);
    }

    let root = tree.insert(root_node);

    if config.len() == 1 && config[0].ty == WidgetKind::Container {
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

    if let Some(style) = find_default_style(styles) {
        apply_style_to_node(&mut root_node, style);
    }

    let root = tree.insert(root_node);

    if config.len() == 1 && config[0].ty == WidgetKind::Container {
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

pub fn from_config_with_store(
    config: &[WidgetConfig],
    styles: &std::collections::HashMap<String, StyleConfig>,
    data_store: &std::collections::HashMap<String, serde_json::Value>,
) -> WidgetTree {
    let expanded = binding::expand_widget_configs(config, None, data_store);
    from_config(&expanded, styles)
}

pub fn from_popup_config_with_store(
    config: &[WidgetConfig],
    styles: &std::collections::HashMap<String, StyleConfig>,
    data_store: &std::collections::HashMap<String, serde_json::Value>,
) -> WidgetTree {
    let expanded = binding::expand_widget_configs(config, None, data_store);
    from_popup_config(&expanded, styles)
}

pub(crate) fn insert_configured(
    tree: &mut WidgetTree,
    parent: Option<WidgetId>,
    config: &WidgetConfig,
    styles: &std::collections::HashMap<String, StyleConfig>,
) -> WidgetId {
    let content = match config.ty {
        WidgetKind::Text => WidgetContent::Text {
            text: config.text.clone().unwrap_or_default(),
        },
        WidgetKind::Image => WidgetContent::Image {
            path: config
                .path
                .clone()
                .or_else(|| config.text.clone())
                .unwrap_or_default(),
        },
        WidgetKind::Svg => WidgetContent::Svg {
            path: config
                .path
                .clone()
                .or_else(|| config.text.clone())
                .unwrap_or_default(),
        },
        WidgetKind::Button => {
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
        WidgetKind::Item => {
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
        WidgetKind::Progress => WidgetContent::Progress {
            value: config.value.unwrap_or(0.0),
            max: config.max.unwrap_or(100.0),
            props: config.props.clone().unwrap_or(serde_json::Value::Null),
        },
        WidgetKind::ProgressRing => WidgetContent::ProgressRing {
            value: config.value.unwrap_or(0.0),
            max: config.max.unwrap_or(100.0),
            stroke_width: config.stroke_width,
            text: config.text.clone(),
            props: config.props.clone().unwrap_or(serde_json::Value::Null),
        },
        WidgetKind::Slider => WidgetContent::Slider {
            value: config.value.unwrap_or(50.0),
            min: config.min.unwrap_or(0.0),
            max: config.max.unwrap_or(100.0),
            props: config.props.clone().unwrap_or(serde_json::Value::Null),
        },
        WidgetKind::TextInput => {
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
        WidgetKind::Module => WidgetContent::Module {
            module: config
                .module
                .clone()
                .unwrap_or_else(|| config.id.clone().unwrap_or_else(|| config.ty.to_string())),
            payload: serde_json::Value::Null,
        },
        WidgetKind::Container => {
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
        if config.ty == WidgetKind::Slider {
            config.on_click.clone()
        } else {
            None
        }
    });
    node.tooltip = config.tooltip.clone();
    if let Some(default_style) = find_default_style(styles) {
        if let Some(font) = &default_style.font {
            node.style.font_family = font.clone();
        }
        if let Some(font_size) = default_style.font_size {
            node.style.font_size = font_size;
        }
        if let Some(fg) = default_style.foreground.as_deref().and_then(parse_color) {
            node.style.foreground = Some(fg);
        }
        if let Some(accent) = default_style.accent.as_deref().and_then(parse_color) {
            node.style.accent = Some(accent);
        }
    }
    if let Some(parent_id) = parent {
        if let Some(parent_node) = tree.get(parent_id) {
            node.style.font_family = parent_node.style.font_family.clone();
            node.style.font_size = parent_node.style.font_size;
            if config.style.is_none() && parent_node.style.foreground.is_some() {
                node.style.foreground = parent_node.style.foreground;
            }
        }
    }
    if let Some(style_name) = config.style.as_deref() {
        if let Some(style) = styles.get(style_name) {
            apply_style_to_node(&mut node, style);
        }
    } else if config.ty != WidgetKind::Text {
        if let Some(default_style) = find_default_style(styles) {
            apply_style_to_node(&mut node, default_style);
        }
    }
    if let Some(op) = config.opacity {
        node.style.opacity = op;
    }
    if let Some(layout) = &config.layout {
        if let Some(gap) = layout.gap {
            node.layout.gap = gap;
        }
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
        if let Some(r) = layout.border_radius {
            node.style.radius = r;
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
            if node.layout.gap == 0.0 {
                if let Some(g) = find_default_style(styles).and_then(|s| s.gap) {
                    node.layout.gap = g;
                }
            }
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
