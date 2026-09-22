//! Wayland Backend + Surface/Output

pub mod backend;
pub mod output;
pub mod seat;
pub mod surface;

use wayland_client::protocol::wl_output;

/// Identifies a compositor output.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OutputId {
    pub name: String,
    pub wl_output: wl_output::WlOutput,
}

/// Negotiated protocol support.
#[derive(Debug, Clone, Default)]
pub struct ProtocolSupport {
    pub ext_layer_shell: bool,
    pub zwlr_layer_shell: bool,
    pub fractional_scale: bool,
    pub viewporter: bool,
    pub cursor_shape: bool,
    pub text_input_v3: bool,
    pub xdg_activation: bool,
    pub data_control: bool,
}
