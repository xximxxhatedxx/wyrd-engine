pub mod manifest;
pub mod supervisor;
pub mod wasm;

pub use manifest::{find_manifest, ModuleLevel, ModuleManifest};
use serde::{Deserialize, Serialize};
pub use wasm::{WasmEngine, WasmModuleRunner};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CoreMessage {
    Init {
        contract_version: u32,
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
    Down {
        module: String,
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_core_message_init_serde() {
        let init = CoreMessage::Init {
            contract_version: 1,
            config: json!({"theme": "dark"}),
        };
        let serialized = serde_json::to_string(&init).unwrap();
        assert!(serialized.contains(r#""contract_version":1"#));
        assert!(!serialized.contains(r#""version":"#));

        let deserialized: CoreMessage = serde_json::from_str(&serialized).unwrap();
        match deserialized {
            CoreMessage::Init {
                contract_version,
                config,
            } => {
                assert_eq!(contract_version, 1);
                assert_eq!(config, json!({"theme": "dark"}));
            }
            _ => panic!("unexpected message variant"),
        }
    }

    #[test]
    fn test_module_message_down_serde() {
        let down = ModuleMessage::Down {
            module: "brightness".to_string(),
            reason: "task terminated".to_string(),
        };
        let serialized = serde_json::to_string(&down).unwrap();
        assert!(serialized.contains(r#""type":"Down""#));
        assert!(serialized.contains(r#""module":"brightness""#));
        assert!(serialized.contains(r#""reason":"task terminated""#));

        let deserialized: ModuleMessage = serde_json::from_str(&serialized).unwrap();
        match deserialized {
            ModuleMessage::Down { module, reason } => {
                assert_eq!(module, "brightness");
                assert_eq!(reason, "task terminated");
            }
            _ => panic!("unexpected message variant"),
        }
    }
}
