//! Wayland protocol layer, wlr-layer-shell surfaces, seat/pointer/keyboard input, and generic surface registry for Wyrd.

pub use wyrd_graphics::{animator, event_bus, icons, render, sync_helpers};
pub use wyrd_state::state_files;

pub mod input;
pub mod surface_manager;
pub mod wayland;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use wayland_client::backend::ObjectId;
use wayland_client::globals::GlobalListContents;
use wayland_client::{protocol::wl_registry, Connection, Dispatch, Proxy, QueueHandle};

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

/// Generic Wayland runtime state and surface registry parameterized by configuration `C`
/// and per-surface scene/tree payload `T`.
///
/// Consumers that do not use `wyrd-config` or `wyrd-widgets` can instantiate `ShellState<(), ()>`
/// (the default) or substitute their own configuration and surface state types.
pub struct ShellState<C = (), T = ()> {
    pub config: Arc<RwLock<C>>,
    pub event_bus: Arc<event_bus::EventBus>,
    pub surface_manager: Arc<RwLock<surface_manager::SurfaceManager>>,
    pub widget_tree: Arc<RwLock<T>>,
    pub input_state: Arc<RwLock<input::InputState>>,
    pub outputs: HashMap<u32, OutputRuntime>,
    pub xdg_output_bound: HashSet<u32>,
    pub fractional_scales: HashMap<ObjectId, f64>,
    pub layer_surface_sizes: HashMap<ObjectId, (u32, u32)>,
    pub pending_input_events: Vec<input::InputEvent>,
    pub frame_ready: bool,
    pub released_buffers: HashSet<ObjectId>,
    pub pointer_surface_id: Option<ObjectId>,
    pub pointer_enter_serial: u32,
    pub surface_trees: Arc<RwLock<HashMap<ObjectId, Arc<T>>>>,
    pub module_data: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    pub text_input_committed: Arc<Mutex<Vec<String>>>,
}

impl<C, T> Clone for ShellState<C, T> {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            event_bus: self.event_bus.clone(),
            surface_manager: self.surface_manager.clone(),
            widget_tree: self.widget_tree.clone(),
            input_state: self.input_state.clone(),
            outputs: self.outputs.clone(),
            xdg_output_bound: self.xdg_output_bound.clone(),
            fractional_scales: self.fractional_scales.clone(),
            layer_surface_sizes: self.layer_surface_sizes.clone(),
            pending_input_events: self.pending_input_events.clone(),
            frame_ready: self.frame_ready,
            released_buffers: self.released_buffers.clone(),
            pointer_surface_id: self.pointer_surface_id.clone(),
            pointer_enter_serial: self.pointer_enter_serial,
            surface_trees: self.surface_trees.clone(),
            module_data: self.module_data.clone(),
            text_input_committed: self.text_input_committed.clone(),
        }
    }
}

pub type BarState<C = (), T = ()> = ShellState<C, T>;
pub type EngineState<C = (), T = ()> = ShellState<C, T>;

impl<C: Default, T: Default> Default for ShellState<C, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Default, T: Default> ShellState<C, T> {
    /// Creates a new [`ShellState`] initialized with `C::default()` and `T::default()`.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(C::default())),
            event_bus: Arc::new(event_bus::EventBus::new()),
            surface_manager: Arc::new(RwLock::new(surface_manager::SurfaceManager::new())),
            widget_tree: Arc::new(RwLock::new(T::default())),
            input_state: Arc::new(RwLock::new(input::InputState::new())),
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
            text_input_committed: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

/// Resolves the active Wayland output [`ObjectId`] for a surface placement request,
/// checking explicit output name first, then global cursor coordinates, then focused monitor
/// name, then the pointer's currently hovered surface output, and finally falling back to
/// the first connected output.
pub fn resolve_cursor_output_id<C, T>(
    cursor_pos: Option<(i32, i32)>,
    focused_monitor: Option<&str>,
    state: &Arc<Mutex<ShellState<C, T>>>,
    explicit_output: Option<&str>,
) -> Option<ObjectId> {
    let outputs = sync_helpers::lock_unpoisoned(state)
        .outputs
        .values()
        .cloned()
        .collect::<Vec<_>>();
    if outputs.is_empty() {
        return None;
    }

    if let Some(target_name) = explicit_output {
        if target_name != "all" && !target_name.is_empty() {
            if let Some(out) = outputs.iter().find(|o| o.name == target_name) {
                return Some(out.output.id());
            }
        }
    }

    if let Some((cur_gx, cur_gy)) = cursor_pos {
        if let Some(out) = outputs.iter().find(|o| {
            let w = if o.logical_width > 0 {
                o.logical_width
            } else if let Some((mw, _)) = o.current_mode {
                mw
            } else {
                return false;
            };
            let h = if o.logical_height > 0 {
                o.logical_height
            } else if let Some((_, mh)) = o.current_mode {
                mh
            } else {
                return false;
            };
            cur_gx >= o.logical_x
                && cur_gx < (o.logical_x + w)
                && cur_gy >= o.logical_y
                && cur_gy < (o.logical_y + h)
        }) {
            return Some(out.output.id());
        }
    }

    if let Some(focused_name) = focused_monitor {
        if let Some(out) = outputs.iter().find(|o| o.name == focused_name) {
            return Some(out.output.id());
        }
    }

    if let Some(ptr_surf_id) = sync_helpers::lock_unpoisoned(state)
        .pointer_surface_id
        .clone()
    {
        if let Some(out) = outputs.iter().find(|o| o.output.id() == ptr_surf_id) {
            return Some(out.output.id());
        }
    }

    outputs.first().map(|o| o.output.id())
}

macro_rules! delegate_generic_noop {
    ($iface:ty) => {
        impl<C: 'static, T: 'static> Dispatch<$iface, ()> for ShellState<C, T> {
            fn event(
                _state: &mut Self,
                _proxy: &$iface,
                _event: <$iface as Proxy>::Event,
                _data: &(),
                _conn: &Connection,
                _qhandle: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

delegate_generic_noop!(wayland_client::protocol::wl_surface::WlSurface);
delegate_generic_noop!(wayland_client::protocol::wl_region::WlRegion);
delegate_generic_noop!(wayland_client::protocol::wl_shm_pool::WlShmPool);
delegate_generic_noop!(
    wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1
);
delegate_generic_noop!(
    wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1
);
delegate_generic_noop!(
    wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1
);
delegate_generic_noop!(wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter);
delegate_generic_noop!(wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport);
delegate_generic_noop!(
    wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1
);
delegate_generic_noop!(
    wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1
);
delegate_generic_noop!(
    wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1
);
delegate_generic_noop!(
    wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3
);

impl<C: 'static, T: 'static>
    Dispatch<wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::ZwpTextInputV3, ()>
    for ShellState<C, T>
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
                sync_helpers::lock_unpoisoned(&state.text_input_committed).push(t);
            }
        }
    }
}

impl<C: 'static, T: 'static>
    Dispatch<
        wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1,
        ObjectId,
    > for ShellState<C, T>
{
    fn event(
        state: &mut Self,
        _proxy: &wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::Event,
        surface_id: &ObjectId,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.fractional_scales.insert(surface_id.clone(), scale as f64 / 120.0);
        }
    }
}

/// Required by `wayland_client::globals::registry_queue_init`.
/// Initial `wl_output` globals are bound during `WaylandBackend::connect`; this
/// handler processes subsequent dynamic monitor hotplug (`Global`) and removal
/// (`GlobalRemove`) events arriving on the `EventQueue` at runtime.
impl<C: 'static, T: 'static> Dispatch<wl_registry::WlRegistry, GlobalListContents>
    for ShellState<C, T>
{
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
