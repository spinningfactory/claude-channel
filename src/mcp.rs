use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, Write as _};

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub params: Value,
}

fn send_stdout(msg: &Value) {
    let s = serde_json::to_string(msg).unwrap();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    out.write_all(s.as_bytes()).unwrap();
    out.write_all(b"\n").unwrap();
    out.flush().unwrap();
}

fn send_response(id: &Value, result: Value) {
    send_stdout(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }));
}

pub fn send_channel_notification(content: &str, meta: HashMap<String, String>) {
    send_stdout(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/claude/channel",
        "params": {
            "content": content,
            "meta": meta,
        },
    }));
}

pub fn handle_request(req: JsonRpcRequest, server_name: &str, instructions: &str) {
    let id = req.id.unwrap_or(Value::Null);

    match req.method.as_str() {
        "initialize" => {
            send_response(&id, json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": server_name,
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "experimental": {
                        "claude/channel": {}
                    }
                },
                "instructions": instructions,
            }));
        }

        "notifications/initialized" | "initialized" => {}

        "ping" => {
            send_response(&id, json!({}));
        }

        other => {
            eprintln!("[channel] Unhandled method: {}", other);
            if id != Value::Null {
                send_response(&id, json!({
                    "error": { "code": -32601, "message": format!("method not found: {}", other) }
                }));
            }
        }
    }
}
