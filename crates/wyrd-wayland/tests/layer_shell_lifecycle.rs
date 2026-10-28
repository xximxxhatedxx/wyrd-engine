use std::env;
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use wayland_client::protocol::{wl_compositor, wl_output, wl_registry, wl_shm, wl_surface};
use wayland_client::{
    globals::{registry_queue_init, GlobalListContents},
    Connection, Dispatch, EventQueue, QueueHandle,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

struct LifecycleState {
    configured: Option<(u32, u32)>,
    closed: bool,
}

macro_rules! delegate_noop {
    ($proxy:ty, $userdata:ty) => {
        impl Dispatch<$proxy, $userdata> for LifecycleState {
            fn event(
                _state: &mut Self,
                _proxy: &$proxy,
                _event: <$proxy as wayland_client::Proxy>::Event,
                _data: &$userdata,
                _conn: &Connection,
                _qhandle: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

delegate_noop!(wl_registry::WlRegistry, GlobalListContents);
delegate_noop!(wl_compositor::WlCompositor, ());
delegate_noop!(wl_shm::WlShm, ());
delegate_noop!(wl_output::WlOutput, ());
delegate_noop!(wl_surface::WlSurface, ());
delegate_noop!(zwlr_layer_shell_v1::ZwlrLayerShellV1, ());

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for LifecycleState {
    fn event(
        state: &mut Self,
        proxy: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                state.configured = Some((width, height));
                proxy.ack_configure(serial);
            }
            zwlr_layer_surface_v1::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}

struct LifecycleController {
    conn: Connection,
    event_queue: EventQueue<LifecycleState>,
    state: LifecycleState,
    layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
}

impl LifecycleController {
    fn connect() -> Result<Self> {
        let conn = Connection::connect_to_env()
            .context("WAYLAND_DISPLAY not set; cannot connect to compositor")?;
        let (globals, event_queue) = registry_queue_init::<LifecycleState>(&conn)
            .context("failed to initialize Wayland registry")?;
        let qh = event_queue.handle();

        let shell = globals
            .bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(&qh, 1..=4, ())
            .context("zwlr_layer_shell_v1 is unavailable")?;
        let compositor: wl_compositor::WlCompositor = globals
            .bind(&qh, 1..=6, ())
            .context("wl_compositor is unavailable")?;
        let _shm: wl_shm::WlShm = globals
            .bind(&qh, 1..=2, ())
            .context("wl_shm is unavailable")?;
        let output: wl_output::WlOutput = globals
            .bind(&qh, 1..=4, ())
            .context("wl_output is unavailable")?;

        let surface = compositor.create_surface(&qh, ());
        let layer_surface = shell.get_layer_surface(
            &surface,
            Some(&output),
            zwlr_layer_shell_v1::Layer::Overlay,
            "wyrd-layer-shell-lifecycle-test".to_string(),
            &qh,
            (),
        );
        layer_surface.set_size(256, 64);
        layer_surface.set_anchor(
            zwlr_layer_surface_v1::Anchor::Top
                | zwlr_layer_surface_v1::Anchor::Left
                | zwlr_layer_surface_v1::Anchor::Right
                | zwlr_layer_surface_v1::Anchor::Bottom,
        );
        layer_surface.set_exclusive_zone(-1);
        layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand);
        layer_surface.set_margin(0, 0, 0, 0);
        surface.commit();
        conn.flush()?;

        Ok(Self {
            conn,
            event_queue,
            state: LifecycleState {
                configured: None,
                closed: false,
            },
            layer_surface,
        })
    }

    fn wait_for_configure(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if self.state.configured.is_some() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(anyhow::anyhow!(
                    "layer surface Configure event was not received within 5 seconds"
                ));
            }

            let Some(guard) = self.conn.prepare_read() else {
                self.event_queue.dispatch_pending(&mut self.state)?;
                continue;
            };
            let mut pollfd = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: `pollfd` is a valid stack-allocated structure pointing to an open socket fd.
            let result = unsafe { libc::poll(&mut pollfd, 1, 100) };
            if result < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if result > 0 {
                guard.read()?;
                self.event_queue.dispatch_pending(&mut self.state)?;
            }
        }
    }
}

#[test]
fn layer_shell_surface_lifecycle_is_bounded() -> Result<()> {
    let Some(display) = env::var_os("WAYLAND_DISPLAY") else {
        eprintln!("skipping: WAYLAND_DISPLAY not set");
        return Ok(());
    };
    if display.is_empty() {
        eprintln!("skipping: WAYLAND_DISPLAY is empty");
        return Ok(());
    }

    let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/user/1000"));
    let socket = runtime_dir.join(display);
    let socket_exists =
        fs::symlink_metadata(&socket).is_ok_and(|metadata| metadata.file_type().is_socket());
    if !socket_exists {
        eprintln!("skipping: compositor socket not found at {socket:?}");
        return Ok(());
    }

    let mut controller = LifecycleController::connect()?;
    controller.wait_for_configure()?;
    controller.layer_surface.destroy();
    controller.conn.flush()?;
    controller.conn.roundtrip()?;
    Ok(())
}

#[cfg(feature = "gpu-render")]
#[tokio::test]
async fn layer_shell_surface_presents_a_vulkan_gpu_frame_when_requested() -> Result<()> {
    if env::var_os("WYRD_TEST_GPU").is_none() {
        eprintln!("skipping Vulkan Wayland present: set WYRD_TEST_GPU=1 to enable");
        return Ok(());
    }
    let Some(display) = env::var_os("WAYLAND_DISPLAY") else {
        eprintln!("skipping Vulkan Wayland present: WAYLAND_DISPLAY not set");
        return Ok(());
    };
    if display.is_empty() {
        eprintln!("skipping Vulkan Wayland present: WAYLAND_DISPLAY is empty");
        return Ok(());
    }

    let (mut backend, state) =
        match wyrd_wayland::wayland::backend::WaylandBackend::<(), ()>::connect().await {
            Ok(value) => value,
            Err(error) => {
                eprintln!("skipping Vulkan Wayland present: backend unavailable: {error:#}");
                return Ok(());
            }
        };
    let handle = backend.surface_manager.create_surface_with_backend(
        &backend.compositor,
        &backend.qh,
        wyrd_wayland::wayland::surface::SurfaceType::Bar,
        "gpu-present-test",
        false,
        None,
        256,
        64,
        "top",
        &["top".to_string(), "left".to_string(), "right".to_string()],
        (0, 0, 0, 0),
        0,
        "none",
        1.0,
        Some("zwlr-layer-shell"),
    );
    backend.conn.flush()?;
    {
        let mut state = state.lock().unwrap();
        backend.event_queue.roundtrip(&mut *state)?;
    }
    let surface = backend
        .surface_manager
        .get_mut(handle)
        .expect("created LayerShellSurface must remain in the manager");
    let mut renderer = match surface.initialize_gpu_renderer(&backend.conn).await {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("skipping Vulkan Wayland present: GPU surface unavailable: {error:#}");
            return Ok(());
        }
    };
    let adapter = renderer.adapter.get_info();
    println!(
        "Wayland GPU present adapter={} backend={:?} renderer={}",
        adapter.name,
        adapter.backend,
        env::var("WLR_RENDERER").unwrap_or_else(|_| "unknown".to_string())
    );

    let mut scene = wyrd_graphics::render::scene::SceneGraph::new();
    scene.insert(
        scene.root(),
        wyrd_graphics::render::scene::SceneNodeKind::Rect {
            fill: Some(wyrd_graphics::render::scene::Fill::Solid(
                tiny_skia::Color::from_rgba8(70, 150, 230, 255),
            )),
            radius: 4.0,
        },
        (0.0, 0.0, 256.0, 64.0),
        None,
        wyrd_graphics::render::scene::SceneTransform::default(),
    );
    let damage = wyrd_graphics::render::damage::DamageTracker::default();
    let frame = renderer.render_scene(
        &scene,
        surface.gpu_surface().expect("GPU target initialized"),
        &damage,
    )?;
    surface.submit_frame_gpu(&backend.qh, &renderer.queue, frame, &damage.regions)?;
    backend.conn.flush()?;
    {
        let mut state = state.lock().unwrap();
        backend.event_queue.roundtrip(&mut *state)?;
    }
    assert!(
        wyrd_graphics::render::gpu::GpuRenderTarget::preserves_contents(
            surface.gpu_surface().unwrap()
        ),
        "GPU backbuffer was not marked as rendered"
    );
    surface.backend.destroy();
    surface.surface.destroy();
    backend.conn.flush()?;
    {
        let mut state = state.lock().unwrap();
        backend.event_queue.roundtrip(&mut *state)?;
    }
    Ok(())
}
