#![cfg(feature = "gpu-render")]

use wyrd_engine::render::backend::RenderMode;
use wyrd_engine::render::context::RenderContext;
use wyrd_engine::render::damage::DamageTracker;
use wyrd_engine::render::gpu::{GpuRenderFrame, GpuRenderTarget, GpuRenderer, OffscreenGpuTarget};
use wyrd_engine::widgets::{
    parse_fill, render_surface_present, PresentedSurface, WidgetContent, WidgetNode, WidgetTree,
};

// Guideline for environment variable tests:
// Any test reading or mutating process-level environment variables (such as WYRD_FORCE_CPU)
// must call `common::env_lock::acquire_env_lock()` to prevent race conditions during parallel test runs.
mod common;

use common::env_lock::acquire_env_lock;

#[test]
#[ignore = "requires a usable GPU adapter"]
fn full_pipeline_widget_tree_to_gpu_frame() {
    let _guard = acquire_env_lock();
    std::env::remove_var("WYRD_FORCE_CPU");
    let renderer =
        pollster::block_on(GpuRenderer::new()).expect("a usable GPU adapter is required");
    let mut target = OffscreenGpuTarget::new(&renderer, 128, 64).unwrap();
    let mut context = RenderContext::with_mode(1.0, RenderMode::Gpu);
    context.gpu = Some(renderer);

    let mut tree = WidgetTree::new();
    let mut root = WidgetNode::new(WidgetContent::Container);
    root.final_rect = (0.0, 0.0, 128.0, 64.0);
    root.style.background = parse_fill("rgba(20, 40, 80, 255)");
    tree.insert(root);

    let mut damage = DamageTracker::default();
    let presented = render_surface_present(
        0,
        128,
        64,
        1.0,
        true,
        &tree,
        &mut context,
        &mut damage,
        None,
        None,
        1.0,
        Some(&mut target),
    )
    .expect("full pipeline should produce a frame");

    let PresentedSurface::Gpu(frame) = presented else {
        panic!("expected PresentedSurface::Gpu frame, got CPU fallback");
    };
    let queue = &context.gpu.as_ref().unwrap().queue;
    let frame_id = frame.present(queue);
    assert_eq!(frame_id, 1);
    assert!(
        !damage.regions.is_empty(),
        "damage regions should be recorded"
    );
}

struct FailingRenderTarget;

impl GpuRenderTarget for FailingRenderTarget {
    fn acquire_frame(&mut self) -> anyhow::Result<GpuRenderFrame> {
        anyhow::bail!("simulated GPU swapchain failure")
    }
}

#[test]
#[ignore = "requires a usable GPU adapter"]
fn gpu_render_failure_falls_back_to_cpu() {
    let renderer =
        pollster::block_on(GpuRenderer::new()).expect("a usable GPU adapter is required");
    let mut context = RenderContext::with_mode(1.0, RenderMode::Gpu);
    context.gpu = Some(renderer);

    let mut tree = WidgetTree::new();
    let mut root = WidgetNode::new(WidgetContent::Container);
    root.final_rect = (0.0, 0.0, 64.0, 64.0);
    root.style.background = parse_fill("red");
    tree.insert(root);

    let mut damage = DamageTracker::default();
    let mut failing_target = FailingRenderTarget;

    let presented = render_surface_present(
        0,
        64,
        64,
        1.0,
        true,
        &tree,
        &mut context,
        &mut damage,
        None,
        None,
        1.0,
        Some(&mut failing_target),
    )
    .expect("render_surface_present should succeed via CPU fallback");

    match presented {
        PresentedSurface::Cpu { pixmap, damage, .. } => {
            assert_eq!(pixmap.width(), 64);
            assert_eq!(pixmap.height(), 64);
            // Solid red pixel at (32, 32)
            let offset = (32 * 64 + 32) * 4;
            assert_eq!(&pixmap.data()[offset..offset + 4], &[255, 0, 0, 255]);
            assert!(!damage.regions.is_empty());
        }
        PresentedSurface::Gpu(_) => {
            panic!("expected CPU fallback when GPU render fails, but got GPU frame");
        }
        _ => panic!("unexpected future PresentedSurface variant"),
    }
}

#[test]
#[ignore = "requires a usable GPU adapter"]
fn force_cpu_env_var_triggers_cpu_fallback() {
    let _guard = acquire_env_lock();
    let renderer =
        pollster::block_on(GpuRenderer::new()).expect("a usable GPU adapter is required");
    let mut target = OffscreenGpuTarget::new(&renderer, 64, 64).unwrap();
    let mut context = RenderContext::with_mode(1.0, RenderMode::Gpu);
    context.gpu = Some(renderer);

    let mut tree = WidgetTree::new();
    let mut root = WidgetNode::new(WidgetContent::Container);
    root.final_rect = (0.0, 0.0, 64.0, 64.0);
    root.style.background = parse_fill("blue");
    tree.insert(root);

    let mut damage = DamageTracker::default();

    // Set WYRD_FORCE_CPU=1
    std::env::set_var("WYRD_FORCE_CPU", "1");
    let presented = render_surface_present(
        0,
        64,
        64,
        1.0,
        true,
        &tree,
        &mut context,
        &mut damage,
        None,
        None,
        1.0,
        Some(&mut target),
    )
    .expect("render_surface_present should succeed");
    std::env::remove_var("WYRD_FORCE_CPU");

    match presented {
        PresentedSurface::Cpu { pixmap, .. } => {
            let offset = (32 * 64 + 32) * 4;
            // Blue pixel: RGBA = (0, 0, 255, 255)
            assert_eq!(&pixmap.data()[offset..offset + 4], &[0, 0, 255, 255]);
        }
        PresentedSurface::Gpu(_) => {
            panic!("WYRD_FORCE_CPU=1 was set, but GPU frame was returned instead of CPU fallback");
        }
        _ => panic!("unexpected future PresentedSurface variant"),
    }
}
