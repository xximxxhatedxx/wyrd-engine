//! Output management + HiDPI

use log::info;
use wayland_client::{
    protocol::wl_output::{self, WlOutput},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};

use crate::{BarState, OutputUserData};

#[derive(Debug, Clone)]
pub struct OutputState {
    pub wl_output: WlOutput,
    pub name: String,
    pub logical_x: i32,
    pub logical_y: i32,
    pub logical_width: i32,
    pub logical_height: i32,
    pub scale: i32,
    pub fractional_scale: Option<f64>,
    pub refresh_rate_mhz: i32,
    pub current_mode: Option<(i32, i32)>,
}

impl OutputState {
    pub fn effective_scale(&self) -> f64 {
        self.fractional_scale.unwrap_or(self.scale as f64)
    }

    pub fn physical_size(&self) -> (i32, i32) {
        let s = self.effective_scale();
        (
            (self.logical_width as f64 * s).round() as i32,
            (self.logical_height as f64 * s).round() as i32,
        )
    }
}

impl Dispatch<wl_output::WlOutput, OutputUserData> for BarState {
    fn event(
        state: &mut Self,
        proxy: &wl_output::WlOutput,
        event: wl_output::Event,
        data: &OutputUserData,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.get_mut(&data.global_name) else {
            return;
        };
        match event {
            wl_output::Event::Done => {
                info!("Output configuration done for {:?}", proxy.id());
            }
            wl_output::Event::Scale { factor } => {
                output.scale = factor;
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                refresh,
            } => {
                output.logical_width = width;
                output.logical_height = height;
                output.current_mode = Some((width, height));
                output.refresh_rate_mhz = refresh;
                if let Ok(mode) = flags.into_result() {
                    if mode.contains(wl_output::Mode::Current) {
                        // Current mode
                    }
                }
            }
            wl_output::Event::Name { name } => {
                output.name = name.clone();
                info!("wl_output name for {:?}: {}", proxy.id(), name);
            }
            wl_output::Event::Description { description } => {
                info!(
                    "wl_output description for {:?}: {}",
                    proxy.id(),
                    description
                );
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, OutputUserData> for BarState {
    fn event(
        state: &mut Self,
        _proxy: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        data: &OutputUserData,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                if let Some(output) = state.outputs.get_mut(&data.global_name) {
                    output.logical_x = x;
                    output.logical_y = y;
                }
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                if let Some(output) = state.outputs.get_mut(&data.global_name) {
                    output.logical_width = width;
                    output.logical_height = height;
                }
            }
            zxdg_output_v1::Event::Name { name } => {
                if let Some(output) = state.outputs.get_mut(&data.global_name) {
                    output.name = name.clone();
                }
                info!("XDG output name: {}", name);
            }
            _ => {}
        }
    }
}
