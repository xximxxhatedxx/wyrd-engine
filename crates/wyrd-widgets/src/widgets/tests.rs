use super::*;
use serde_json::json;
use std::collections::HashMap;
use tiny_skia::Color;

#[test]
fn test_ids_match_does_not_bleed_across_modules() {
    assert!(!ids_match("launcher_btn", "power-menu", "power-menu"));
    assert!(!ids_match("cpu_badge", "power-menu", "power-menu"));
    assert!(!ids_match("clock_badge", "power-menu", "power-menu"));
    assert!(!ids_match("audio", "microphone", "audio"));
    assert!(!ids_match("microphone", "audio", "audio"));
    assert!(!ids_match("workspaces", "power-menu", "power-menu"));
    assert!(ids_match("power-menu", "power-menu", "power-menu"));
    assert!(ids_match("system", "system", "system"));
    assert!(ids_match("clock", "clock", "clock"));
    assert!(ids_match("network", "network", "network"));
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
fn popup_update_targets_inner_module_and_preserves_gap() {
    let mut tree = WidgetTree::new();
    let mut root_node = WidgetNode::new(WidgetContent::Container);
    root_node.id = Some("popup_root:audio".into());
    root_node.layout.gap = 0.0;
    root_node.style.padding = (16.0, 16.0, 16.0, 16.0);
    let root_id = tree.insert(root_node);
    tree.root = Some(root_id);

    let mut mod_node = WidgetNode::new(WidgetContent::Module {
        module: "audio".into(),
        payload: serde_json::Value::Null,
    });
    mod_node.id = Some("popup:audio".into());
    mod_node.layout.gap = 6.0;
    let mod_id = tree.insert(mod_node);
    tree.append_child(root_id, mod_id);

    let initial_children = vec![
        serde_json::json!({ "type": "container", "children": [{ "type": "text", "text": "Card 1" }] }),
        serde_json::json!({ "type": "container", "children": [{ "type": "slider", "value": 50.0, "min": 0.0, "max": 100.0 }] }),
    ];
    tree.sync_dynamic_children(mod_id, &initial_children);

    let updated_payload = serde_json::json!({
        "children": [
            { "type": "container", "children": [{ "type": "text", "text": "Card 1" }] },
            { "type": "container", "children": [{ "type": "slider", "value": 75.0, "min": 0.0, "max": 100.0 }] }
        ]
    });
    assert!(tree.update_module_for_output("audio", "popup:audio", updated_payload, None));

    // popup_root:audio must still have gap = 0.0 and single child mod_id
    let root_after = tree.get(root_id).expect("root_id must still exist");
    assert_eq!(root_after.layout.gap, 0.0);
    assert_eq!(root_after.children, vec![mod_id]);

    // popup:audio must still exist with gap = 6.0 and 2 card children
    let mod_after = tree.get(mod_id).expect("mod_id must not be destroyed");
    assert_eq!(mod_after.layout.gap, 6.0);
    assert_eq!(mod_after.children.len(), 2);
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
        crate::widgets::layout::LayoutMode::Flex(crate::widgets::layout::FlexDirection::Horizontal)
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
    ) = base.for_state(WidgetState::Hover);
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
        parse_color("orange"),
        Some(Color::from_rgba8(255, 165, 0, 255))
    );
    assert_eq!(
        parse_color("purple"),
        Some(Color::from_rgba8(128, 0, 128, 255))
    );
    assert_eq!(parse_color("rebeccapurple"), None);
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
        ty: WidgetKind::ProgressRing,
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

#[test]
fn test_update_module_updates_image_and_svg_path() {
    let mut tree = WidgetTree::new();
    let mut img_node = WidgetNode::new(WidgetContent::Image {
        path: String::new(),
    });
    img_node.id = Some("mpris_cover".to_string());
    let img_id = tree.insert(img_node);

    let mut svg_node = WidgetNode::new(WidgetContent::Svg {
        path: String::new(),
    });
    svg_node.id = Some("mpris_icon".to_string());
    let svg_id = tree.insert(svg_node);

    let changed = tree.update_module_for_output(
        "mpris",
        "mpris_cover",
        json!({ "path": "https://example.com/cover.jpg", "text": "Song" }),
        None,
    );
    assert!(changed);
    assert_eq!(
        tree.get(img_id).map(|n| &n.content),
        Some(&WidgetContent::Image {
            path: "https://example.com/cover.jpg".to_string()
        })
    );

    let changed_svg = tree.update_module_for_output(
        "mpris",
        "mpris_icon",
        json!({ "art_url": "/tmp/icon.svg" }),
        None,
    );
    assert!(changed_svg);
    assert_eq!(
        tree.get(svg_id).map(|n| &n.content),
        Some(&WidgetContent::Svg {
            path: "/tmp/icon.svg".to_string()
        })
    );
}
