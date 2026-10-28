#![cfg(feature = "gpu-render")]

use wyrd_engine::render::backend::RenderMode;
use wyrd_engine::render::context::RenderContext;
use wyrd_engine::render::damage::DamageTracker;
use wyrd_engine::render::gpu::{GpuRenderer, OffscreenGpuTarget};
use wyrd_engine::widgets::{
    parse_fill, render_surface_present, PresentedSurface, WidgetContent, WidgetNode, WidgetTree,
};

#[test]
#[ignore = "requires a usable GPU adapter"]
fn engine_widget_bridge_submits_and_presents_offscreen_gpu_frame() {
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
    .expect("widget bridge should produce a frame");
    let PresentedSurface::Gpu(frame) = presented else {
        panic!("GPU mode unexpectedly fell back to CPU");
    };
    let queue = &context.gpu.as_ref().unwrap().queue;
    assert_eq!(frame.present(queue), 1);
    assert!(!damage.regions.is_empty());
}

#[test]
#[ignore = "10-second idle CPU measurement; requires a usable GPU adapter"]
fn offscreen_gpu_idle_cpu_stays_below_five_percent() {
    use std::fs;
    use std::thread;
    use std::time::{Duration, Instant};
    use wyrd_graphics::render::gpu::{GpuRenderer, OffscreenGpuTarget};
    use wyrd_graphics::render::scene::SceneGraph;

    fn process_cpu_ticks() -> u64 {
        let stat = fs::read_to_string("/proc/self/stat").expect("read process CPU stats");
        let fields = stat
            .rsplit_once(')')
            .expect("process stat has a command field")
            .1
            .split_whitespace()
            .collect::<Vec<_>>();
        fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()
    }

    let mut renderer = pollster::block_on(GpuRenderer::new())
        .expect("a usable GPU adapter is required for the idle test");
    let mut target = OffscreenGpuTarget::new(&renderer, 64, 64).unwrap();
    let scene = SceneGraph::new();
    let frame = renderer
        .render_scene(
            &scene,
            &mut target,
            &wyrd_graphics::render::damage::DamageTracker::default(),
        )
        .unwrap();
    let _ = frame.present(&renderer.queue);
    renderer.wait_for_idle().unwrap();

    let start_ticks = process_cpu_ticks();
    let start = Instant::now();
    thread::sleep(Duration::from_secs(10));
    let elapsed = start.elapsed().as_secs_f64();
    let cpu_ticks = process_cpu_ticks().saturating_sub(start_ticks);
    let cpu_percent_one_core = cpu_ticks as f64 / 100.0 / elapsed * 100.0;
    println!(
        "gpu_idle_cpu elapsed_s={elapsed:.3} process_cpu_ticks={cpu_ticks} assumed_clock_ticks_per_second=100 cpu_percent_one_core={cpu_percent_one_core:.3}"
    );
    assert!(
        cpu_percent_one_core < 5.0,
        "idle CPU was {cpu_percent_one_core:.3}% of one core; expected <5%"
    );
}
