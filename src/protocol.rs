use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Wire protocol between shim and server.
/// Line-delimited JSON over TCP — each line is one ProtocolMessage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProtocolMessage {
    // Shim → Server
    Hello {
        session_name: String,
    },
    McpRequest {
        id: Value,
        method: String,
        #[serde(default)]
        params: Value,
    },
    Goodbye,

    // Server → Shim
    McpResponse {
        id: Value,
        result: Value,
    },
    McpError {
        id: Value,
        code: i64,
        message: String,
    },
    Notification {
        method: String,
        params: Value,
    },
    ServerError {
        message: String,
    },
}
