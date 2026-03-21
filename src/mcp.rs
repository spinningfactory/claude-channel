use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, Write as _};

#[cfg(feature = "redis")]
use crate::coordination::Publisher;

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
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

/// Whether the MCP server should advertise tools.
pub struct McpContext {
    pub server_name: String,
    pub instructions: String,
    #[cfg(feature = "redis")]
    pub publisher: Option<Publisher>,
}

impl McpContext {
    pub fn has_tools(&self) -> bool {
        #[cfg(feature = "redis")]
        { self.publisher.is_some() }
        #[cfg(not(feature = "redis"))]
        { false }
    }

    pub fn handle_request(&self, req: JsonRpcRequest) {
        let id = req.id.unwrap_or(Value::Null);

        match req.method.as_str() {
            "initialize" => {
                let mut capabilities = json!({
                    "experimental": {
                        "claude/channel": {}
                    }
                });

                if self.has_tools() {
                    capabilities.as_object_mut().unwrap()
                        .insert("tools".to_string(), json!({}));
                }

                send_response(&id, json!({
                    "protocolVersion": "2024-11-05",
                    "serverInfo": {
                        "name": self.server_name,
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "capabilities": capabilities,
                    "instructions": self.instructions,
                }));
            }

            "notifications/initialized" | "initialized" => {}

            "tools/list" if self.has_tools() => {
                send_response(&id, json!({
                    "tools": [
                        {
                            "name": "publish",
                            "description": "Publish a message to a Redis channel for other Claude sessions to see",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "channel": {
                                        "type": "string",
                                        "description": "Redis channel: 'claude:questions' for help requests, 'claude:lobby' for announcements, or 'claude:session:<name>' to DM a specific session"
                                    },
                                    "message": {
                                        "type": "string",
                                        "description": "The message to send (will be wrapped in JSON with your session name)"
                                    }
                                },
                                "required": ["channel", "message"]
                            }
                        },
                        {
                            "name": "list_sessions",
                            "description": "List all active Claude Code sessions and their goals",
                            "inputSchema": {
                                "type": "object",
                                "properties": {},
                                "required": []
                            }
                        }
                    ]
                }));
            }

            "tools/call" if self.has_tools() => {
                let tool_name = req.params.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                #[cfg(feature = "redis")]
                if let Some(ref publisher) = self.publisher {
                    match tool_name {
                        "publish" => {
                            let args = req.params.get("arguments").cloned().unwrap_or(json!({}));
                            let channel = args.get("channel")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let message = args.get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();

                            let envelope = json!({
                                "from": publisher.session_name,
                                "body": message,
                            });

                            let result = publisher.publish(&channel, &envelope.to_string());

                            match result {
                                Ok(_) => send_response(&id, json!({
                                    "content": [{ "type": "text", "text": format!("Published to {}", channel) }]
                                })),
                                Err(e) => send_response(&id, json!({
                                    "content": [{ "type": "text", "text": format!("Error: {}", e) }],
                                    "isError": true
                                })),
                            }
                        }

                        "list_sessions" => {
                            let result = publisher.list_sessions();

                            match result {
                                Ok(sessions) => {
                                    let mut text = String::from("Active sessions:\n");
                                    if sessions.is_empty() {
                                        text.push_str("  (none)\n");
                                    }
                                    for (name, data) in &sessions {
                                        let goal = serde_json::from_str::<Value>(data)
                                            .ok()
                                            .and_then(|v| v.get("goal").and_then(|g| g.as_str()).map(String::from))
                                            .unwrap_or_else(|| data.clone());
                                        text.push_str(&format!("  {} — {}\n", name, goal));
                                    }
                                    send_response(&id, json!({
                                        "content": [{ "type": "text", "text": text }]
                                    }));
                                }
                                Err(e) => send_response(&id, json!({
                                    "content": [{ "type": "text", "text": format!("Error: {}", e) }],
                                    "isError": true
                                })),
                            }
                        }

                        _ => {
                            send_response(&id, json!({
                                "content": [{ "type": "text", "text": format!("Unknown tool: {}", tool_name) }],
                                "isError": true
                            }));
                        }
                    }
                    return;
                }

                send_response(&id, json!({
                    "error": { "code": -32601, "message": "tools not available" }
                }));
            }

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
}
