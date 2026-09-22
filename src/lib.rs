//! Wyrd Engine Library

pub mod animator;
pub mod render;
pub mod style;
pub mod widgets;

pub mod compositor;
pub mod compositor_ext;
pub mod config;
pub mod event_bus;
pub mod icons;
pub mod input;
pub mod modules;
pub mod state_files;
pub mod surface_manager;
pub mod wayland;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use wayland_client::globals::GlobalListContents;
use wayland_client::{protocol::wl_registry, Connection, Dispatch, QueueHandle};

#[derive(Debug, Clone, Copy)]
pub struct OutputUserData {
    pub global_name: u32,
}

#[derive(Debug, Clone)]
pub struct OutputRuntime {
    pub global_name: u32,
    pub output: wayland_client::protocol::wl_output::WlOutput,
    pub name: String,
    pub logical_x: i32,
    pub logical_y: i32,
    pub logical_width: i32,
    pub logical_height: i32,
    pub scale: i32,
    pub refresh_rate_mhz: i32,
    pub current_mode: Option<(i32, i32)>,
}

/// Shared application state.
#[derive(Clone)]
pub struct ShellState {
    pub config: Arc<RwLock<config::ShellConfig>>,
    pub event_bus: Arc<event_bus::EventBus>,
    pub surface_manager: Arc<RwLock<surface_manager::SurfaceManager>>,
    pub widget_tree: Arc<RwLock<widgets::WidgetTree>>,
    pub input_state: Arc<RwLock<input::InputState>>,
    pub style: Arc<RwLock<style::StyleEngine>>,
    pub outputs: HashMap<u32, OutputRuntime>,
    pub xdg_output_bound: HashSet<u32>,
    pub fractional_scales: HashMap<wayland_client::backend::ObjectId, f64>,
    pub layer_surface_sizes: HashMap<wayland_client::backend::ObjectId, (u32, u32)>,
    pub pending_input_events: Vec<input::InputEvent>,
    pub frame_ready: bool,
    pub released_buffers: HashSet<wayland_client::backend::ObjectId>,
    pub pointer_surface_id: Option<wayland_client::backend::ObjectId>,
    pub pointer_enter_serial: u32,
    pub surface_trees:
        Arc<RwLock<HashMap<wayland_client::backend::ObjectId, Arc<widgets::WidgetTree>>>>,
    pub module_data: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    pub text_input_committed: Arc<std::sync::Mutex<Vec<String>>>,
}

pub type BarState = ShellState;
pub type EngineState = ShellState;

impl Default for ShellState {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellState {
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(config::ShellConfig::default())),
            event_bus: Arc::new(event_bus::EventBus::new()),
            surface_manager: Arc::new(RwLock::new(surface_manager::SurfaceManager::new())),
            widget_tree: Arc::new(RwLock::new(widgets::WidgetTree::new())),
            input_state: Arc::new(RwLock::new(input::InputState::new())),
            style: Arc::new(RwLock::new(style::StyleEngine::new())),
            outputs: HashMap::new(),
            xdg_output_bound: HashSet::new(),
            layer_surface_sizes: HashMap::new(),
            pending_input_events: Vec::new(),
            frame_ready: true,
            released_buffers: HashSet::new(),
            pointer_surface_id: None,
            pointer_enter_serial: 0,
            fractional_scales: HashMap::new(),
            surface_trees: Arc::new(RwLock::new(HashMap::new())),
            module_data: Arc::new(RwLock::new(HashMap::new())),
            text_input_committed: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}

wayland_client::delegate_noop!(ShellState: ignore wayland_client::protocol::wl_surface::WlSurface);
wayland_client::delegate_noop!(ShellState: ignore wayland_client::protocol::wl_region::WlRegion);
wayland_client::delegate_noop!(ShellState: ignore wayland_client::protocol::wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(ShellState: wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1);
wayland_client::delegate_noop!(ShellState: wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);
wayland_client::delegate_noop!(ShellState: wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);
wayland_client::delegate_noop!(ShellState: wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter);
wayland_client::delegate_noop!(ShellState: ignore wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport);
wayland_client::delegate_noop!(ShellState: wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
wayland_client::delegate_noop!(ShellState: wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1);
wayland_client::delegate_noop!(ShellState: wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1);

// --- text-input-v3 ---
impl wayland_client::Dispatch<wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3, ()> for ShellState {
    fn event(
        _state: &mut Self,
        _proxy: &wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3,
        _event: wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {}
}

impl
    wayland_client::Dispatch<
        wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::ZwpTextInputV3,
        (),
    > for ShellState
{
    fn event(
        state: &mut Self,
        _proxy: &wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::ZwpTextInputV3,
        event: wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::Event::CommitString {
            text: Some(t),
        } = event
        {
            if !t.is_empty() {
                if let Ok(mut committed) = state.text_input_committed.lock() {
                    committed.push(t);
                }
            }
        }
    }
}

//

impl Dispatch<wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1, wayland_client::backend::ObjectId> for ShellState {
    fn event(
        state: &mut Self,
        _proxy: &wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::Event,
        surface_id: &wayland_client::backend::ObjectId,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.fractional_scales.insert(surface_id.clone(), scale as f64 / 120.0);
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ShellState {
    fn event(
        state: &mut Self,
        proxy: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => {
                let output = proxy.bind::<wayland_client::protocol::wl_output::WlOutput, _, _>(
                    name,
                    version.min(4),
                    qhandle,
                    OutputUserData { global_name: name },
                );
                state.outputs.insert(
                    name,
                    OutputRuntime {
                        global_name: name,
                        output,
                        name: format!("output-{name}"),
                        logical_x: 0,
                        logical_y: 0,
                        logical_width: 0,
                        logical_height: 0,
                        scale: 1,
                        refresh_rate_mhz: 0,
                        current_mode: None,
                    },
                );
            }
            wl_registry::Event::GlobalRemove { name } => {
                state.outputs.remove(&name);
                state.xdg_output_bound.remove(&name);
            }
            _ => {}
        }
    }
}
