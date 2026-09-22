use std::time::{Duration, Instant};
use wyrd_engine::animator::{Animator, EasingCurve};
use wyrd_engine::compositor::{create_compositor_integration, CompositorChoice};
use wyrd_engine::config::lua::LuaRuntime;
use wyrd_engine::render::context::RenderContext;
use wyrd_engine::render::damage::DamageTracker;
use wyrd_engine::render::render_surface;
use wyrd_engine::widgets::tree::{layout_tree, measure_tree};
use wyrd_engine::widgets::{WidgetContent, WidgetNode, WidgetTree};

#[test]
fn test_widget_tree_layout_and_render() {
    let mut tree = WidgetTree::new();
    let container = WidgetNode::new(WidgetContent::Container);
    let root_id = tree.insert(container);

    let text_child = WidgetNode::new(WidgetContent::Text {
        text: "Hello, Wyrd Engine!".to_string(),
    });
    let child_id = tree.insert(text_child);
    tree.append_child(root_id, child_id);

    let mut ctx = RenderContext::new(1.0);

    // 1. Negotiation pass (measure)
    measure_tree(&mut tree, &mut ctx, 800.0, 600.0);

    // 2. Geometry positioning pass (layout)
    layout_tree(&mut tree, 0.0, 0.0, 800.0, 600.0);

    let root_node = tree.get(root_id).expect("Root node must exist");
    assert_eq!(root_node.final_rect.2, 800.0);
    assert_eq!(root_node.final_rect.3, 600.0);

    // 3. Render surface to pixmap
    let mut damage = DamageTracker::default();
    let rendered = render_surface(
        0,
        800,
        600,
        1.0,
        true,
        &tree,
        &mut ctx,
        &mut damage,
        None,
        None,
        1.0,
    );
    assert!(rendered.is_some(), "render_surface should produce a pixmap");
}

#[test]
fn test_animator_lifecycle_and_tick() {
    let mut animator = Animator::new();
    let handle = animator.start_easing(100.0, Duration::from_millis(100), EasingCurve::Linear);

    assert!(animator.has_active());
    assert!(animator.is_active(handle));

    // Tick forward 50ms
    animator.tick(Instant::now() + Duration::from_millis(50));
    assert!(animator.has_active());

    // Tick forward past duration
    animator.tick(Instant::now() + Duration::from_millis(150));
    assert!(!animator.has_active());
}

#[test]
fn test_compositor_choices_and_query() {
    let comp = create_compositor_integration(CompositorChoice::None);
    assert_eq!(comp.name(), "none");
    let status = comp.query_status();
    assert!(status.list.is_empty());
    assert!(status.workspaces.is_empty());
}

#[tokio::test]
async fn test_lua_runtime_eval() {
    let temp_dir = tempfile::tempdir().expect("Failed to create tempdir");
    let config_path = temp_dir.path().join("init.lua");
    std::fs::write(&config_path, "").unwrap();

    let runtime = LuaRuntime::new(config_path).expect("Failed to create LuaRuntime");
    let lua_code = r#"
        local x = 10 + 20
        assert(x == 30)
    "#;
    let res = runtime.load_config_from_str(lua_code);
    assert!(res.is_ok(), "Lua execution failed: {:?}", res.err());
}
