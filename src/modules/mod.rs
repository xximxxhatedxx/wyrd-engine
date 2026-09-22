pub mod manifest;
pub mod supervisor;
pub mod wasm;

pub use manifest::{find_manifest, ModuleLevel, ModuleManifest};
use serde::{Deserialize, Serialize};
pub use wasm::{WasmEngine, WasmModuleRunner};

/// Core → Module message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CoreMessage {
    Init {
        version: u32,
        config: serde_json::Value,
    },
    Event {
        widget_id: String,
        event: String,
    },
    PopupEvent {
        popup_id: String,
        event: String,
    },
    TopicEvent {
        topic: String,
        value: serde_json::Value,
    },
    Shutdown,
}

/// Module → Core message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ModuleMessage {
    Ready,
    Update {
        widget_id: String,
        payload: serde_json::Value,
    },
    Surface {
        action: String,
        params: serde_json::Value,
    },
    Publish {
        topic: String,
        value: serde_json::Value,
    },
    RequestCapability {
        capability: String,
        action: String,
        params: serde_json::Value,
    },
    FocusRequest {
        popup_id: String,
    },
    Ping,
}
