use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, RwLock};

mod protocol;
use protocol::ProtocolMessage;

// ---------------------------------------------------------------------------
// SessionHandle — one per connected shim
// ---------------------------------------------------------------------------

struct SessionHandle {
    name: String,
    status: String,
    tx: tokio::sync::mpsc::UnboundedSender<String>, // write lines to this shim's TCP
}

// ---------------------------------------------------------------------------
// SessionManager — in-memory registry + broadcast
// ---------------------------------------------------------------------------

struct SessionManager {
    sessions: RwLock<HashMap<String, SessionHandle>>,
    event_tx: broadcast::Sender<Event>,
    redis: Option<redis::Client>,
}

#[derive(Clone, Debug)]
struct Event {
    timestamp: String,
    from: String,
    body: String,
    event_type: String, // "message", "join", "leave", "status"
}

impl SessionManager {
    fn new(redis_url: Option<&str>) -> Result<Arc<Self>> {
        let (event_tx, _) = broadcast::channel(1024);
        let redis = redis_url
            .map(|url| redis::Client::open(url))
            .transpose()?;
        Ok(Arc::new(Self {
            sessions: RwLock::new(HashMap::new()),
            event_tx,
            redis,
        }))
    }

    async fn register(&self, name: String, tx: tokio::sync::mpsc::UnboundedSender<String>) {
        let event = Event {
            timestamp: chrono::Utc::now().to_rfc3339(),
            from: name.clone(),
            body: format!("{} joined", name),
            event_type: "join".to_string(),
        };

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(
                name.clone(),
                SessionHandle {
                    name: name.clone(),
                    status: "just connected".to_string(),
                    tx,
                },
            );
        }

        // Persist to Redis
        if let Some(ref client) = self.redis {
            if let Ok(mut con) = client.get_multiplexed_async_connection().await {
                let meta = json!({
                    "status": "just connected",
                    "joined": &event.timestamp,
                });
                let _: Result<(), _> = redis::cmd("HSET")
                    .arg("claude:sessions")
                    .arg(&name)
                    .arg(meta.to_string())
                    .query_async(&mut con)
                    .await;

                let payload = json!({
                    "from": &name,
                    "body": &event.body,
                    "type": "join",
                });
                let _: Result<(), _> = redis::cmd("XADD")
                    .arg("claude:stream")
                    .arg("MAXLEN")
                    .arg("~")
                    .arg("10000")
                    .arg("*")
                    .arg("payload")
                    .arg(payload.to_string())
                    .arg("channel")
                    .arg("claude:lobby")
                    .query_async(&mut con)
                    .await;

                // Publish for live dashboard
                let _: Result<(), _> = redis::cmd("PUBLISH")
                    .arg("claude:lobby")
                    .arg(payload.to_string())
                    .query_async(&mut con)
                    .await;
            }
        }

        let _ = self.event_tx.send(event);

        // Notify all other sessions
        self.broadcast_notification(&name, &format!("{} joined", name), "join")
            .await;

        eprintln!("[server] Session registered: {}", name);
    }

    async fn deregister(&self, name: &str) {
        let event = Event {
            timestamp: chrono::Utc::now().to_rfc3339(),
            from: name.to_string(),
            body: format!("{} left", name),
            event_type: "leave".to_string(),
        };

        {
            let mut sessions = self.sessions.write().await;
            sessions.remove(name);
        }

        // Persist to Redis
        if let Some(ref client) = self.redis {
            if let Ok(mut con) = client.get_multiplexed_async_connection().await {
                let _: Result<(), _> = redis::cmd("HDEL")
                    .arg("claude:sessions")
                    .arg(name)
                    .query_async(&mut con)
                    .await;

                let payload = json!({
                    "from": name,
                    "body": format!("{} left", name),
                    "type": "leave",
                });
                let _: Result<(), _> = redis::cmd("XADD")
                    .arg("claude:stream")
                    .arg("MAXLEN")
                    .arg("~")
                    .arg("10000")
                    .arg("*")
                    .arg("payload")
                    .arg(payload.to_string())
                    .arg("channel")
                    .arg("claude:lobby")
                    .query_async(&mut con)
                    .await;

                let _: Result<(), _> = redis::cmd("PUBLISH")
                    .arg("claude:lobby")
                    .arg(payload.to_string())
                    .query_async(&mut con)
                    .await;
            }
        }

        let _ = self.event_tx.send(event);

        eprintln!("[server] Session deregistered: {}", name);
    }

    async fn update_status(&self, name: &str, status: &str) {
        {
            let mut sessions = self.sessions.write().await;
            if let Some(handle) = sessions.get_mut(name) {
                handle.status = status.to_string();
            }
        }

        // Persist to Redis
        if let Some(ref client) = self.redis {
            if let Ok(mut con) = client.get_multiplexed_async_connection().await {
                let meta = json!({ "status": status });
                let _: Result<(), _> = redis::cmd("HSET")
                    .arg("claude:sessions")
                    .arg(name)
                    .arg(meta.to_string())
                    .query_async(&mut con)
                    .await;

                let payload = json!({
                    "from": name,
                    "body": format!("{} is now: {}", name, status),
                    "type": "status",
                });
                let _: Result<(), _> = redis::cmd("XADD")
                    .arg("claude:stream")
                    .arg("MAXLEN")
                    .arg("~")
                    .arg("10000")
                    .arg("*")
                    .arg("payload")
                    .arg(payload.to_string())
                    .arg("channel")
                    .arg("claude:status")
                    .query_async(&mut con)
                    .await;

                let _: Result<(), _> = redis::cmd("PUBLISH")
                    .arg("claude:status")
                    .arg(payload.to_string())
                    .query_async(&mut con)
                    .await;
            }
        }

        let event = Event {
            timestamp: chrono::Utc::now().to_rfc3339(),
            from: name.to_string(),
            body: format!("{} is now: {}", name, status),
            event_type: "status".to_string(),
        };
        let _ = self.event_tx.send(event);

        eprintln!("[server] Status updated: {} → {}", name, status);
    }

    async fn list_sessions(&self) -> Vec<(String, String)> {
        let sessions = self.sessions.read().await;
        sessions
            .values()
            .map(|h| (h.name.clone(), h.status.clone()))
            .collect()
    }

    async fn publish(&self, from: &str, message: &str) {
        let event = Event {
            timestamp: chrono::Utc::now().to_rfc3339(),
            from: from.to_string(),
            body: message.to_string(),
            event_type: "message".to_string(),
        };

        // Persist to Redis
        if let Some(ref client) = self.redis {
            if let Ok(mut con) = client.get_multiplexed_async_connection().await {
                let payload = json!({
                    "from": from,
                    "body": message,
                    "type": "message",
                });
                let _: Result<(), _> = redis::cmd("XADD")
                    .arg("claude:stream")
                    .arg("MAXLEN")
                    .arg("~")
                    .arg("10000")
                    .arg("*")
                    .arg("payload")
                    .arg(payload.to_string())
                    .arg("channel")
                    .arg("claude:questions")
                    .query_async(&mut con)
                    .await;

                let _: Result<(), _> = redis::cmd("PUBLISH")
                    .arg("claude:questions")
                    .arg(payload.to_string())
                    .query_async(&mut con)
                    .await;
            }
        }

        let _ = self.event_tx.send(event);

        // Send notification to all OTHER sessions
        self.broadcast_notification(from, message, "message").await;
    }

    async fn broadcast_notification(&self, from: &str, body: &str, event_type: &str) {
        let notification = ProtocolMessage::Notification {
            method: "notifications/claude/channel".to_string(),
            params: json!({
                "content": body,
                "meta": {
                    "from": from,
                    "event_type": event_type,
                },
            }),
        };

        let line = match serde_json::to_string(&notification) {
            Ok(l) => l,
            Err(_) => return,
        };

        let sessions = self.sessions.read().await;
        for (name, handle) in sessions.iter() {
            if name != from {
                let _ = handle.tx.send(line.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Request Handler — processes MCP requests from shims
// ---------------------------------------------------------------------------

async fn handle_mcp_request(
    sm: &SessionManager,
    session_name: &str,
    id: Value,
    method: &str,
    params: Value,
) -> ProtocolMessage {
    match method {
        "initialize" => ProtocolMessage::McpResponse {
            id,
            result: json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": "claude-channel-server",
                    "version": "0.3.0",
                },
                "capabilities": {
                    "experimental": { "claude/channel": {} },
                    "tools": {},
                },
                "instructions": format!(
                    "You are connected to the Claude Channel server as '{}'. \
                     Messages from other sessions arrive as channel notifications. \
                     Use the publish tool to broadcast messages. \
                     Use list_sessions to see who's online. \
                     Use update_status to announce what you're working on. \
                     Only respond to messages that are relevant to your current task.",
                    session_name
                ),
            }),
        },

        "notifications/initialized" | "initialized" => {
            // No response needed for notifications
            return ProtocolMessage::McpResponse {
                id: Value::Null,
                result: Value::Null,
            };
        }

        "tools/list" => ProtocolMessage::McpResponse {
            id,
            result: json!({
                "tools": [
                    {
                        "name": "publish",
                        "description": "Broadcast a message to all connected sessions",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "message": {
                                    "type": "string",
                                    "description": "The message to broadcast"
                                }
                            },
                            "required": ["message"]
                        }
                    },
                    {
                        "name": "list_sessions",
                        "description": "List all active sessions and what they're working on",
                        "inputSchema": {
                            "type": "object",
                            "properties": {},
                        }
                    },
                    {
                        "name": "update_status",
                        "description": "Update your status to let others know what you're working on",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "status": {
                                    "type": "string",
                                    "description": "What you are currently working on"
                                }
                            },
                            "required": ["status"]
                        }
                    }
                ]
            }),
        },

        "tools/call" => {
            let tool_name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));

            match tool_name {
                "publish" => {
                    let message = args
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if message.is_empty() {
                        return ProtocolMessage::McpError {
                            id,
                            code: -32602,
                            message: "message is required".to_string(),
                        };
                    }

                    sm.publish(session_name, message).await;

                    ProtocolMessage::McpResponse {
                        id,
                        result: json!({
                            "content": [{ "type": "text", "text": "message published" }]
                        }),
                    }
                }

                "list_sessions" => {
                    let sessions = sm.list_sessions().await;
                    let text = if sessions.is_empty() {
                        "No active sessions".to_string()
                    } else {
                        sessions
                            .iter()
                            .map(|(name, status)| format!("{} — {}", name, status))
                            .collect::<Vec<_>>()
                            .join("\n")
                    };

                    ProtocolMessage::McpResponse {
                        id,
                        result: json!({
                            "content": [{ "type": "text", "text": text }]
                        }),
                    }
                }

                "update_status" => {
                    let status = args
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if status.is_empty() {
                        return ProtocolMessage::McpError {
                            id,
                            code: -32602,
                            message: "status is required".to_string(),
                        };
                    }

                    sm.update_status(session_name, status).await;

                    ProtocolMessage::McpResponse {
                        id,
                        result: json!({
                            "content": [{ "type": "text", "text": "status updated" }]
                        }),
                    }
                }

                other => ProtocolMessage::McpError {
                    id,
                    code: -32601,
                    message: format!("unknown tool: {}", other),
                },
            }
        }

        "ping" => ProtocolMessage::McpResponse {
            id,
            result: json!({}),
        },

        other => ProtocolMessage::McpError {
            id,
            code: -32601,
            message: format!("method not found: {}", other),
        },
    }
}

// ---------------------------------------------------------------------------
// Dashboard — HTTP server with SSE
// ---------------------------------------------------------------------------

async fn run_dashboard(sm: Arc<SessionManager>, port: u16) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    eprintln!("[server] Dashboard listening on http://0.0.0.0:{}", port);

    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(v) => v,
            Err(_) => continue,
        };

        let sm = sm.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let n = match tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await {
                Ok(n) => n,
                Err(_) => return,
            };
            let raw = String::from_utf8_lossy(&buf[..n]).to_string();
            let first_line = raw.lines().next().unwrap_or("");
            let path = first_line.split_whitespace().nth(1).unwrap_or("/");

            if path.starts_with("/events") {
                // SSE stream
                let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\nAccess-Control-Allow-Origin: *\r\n\r\n";
                if stream.write_all(headers.as_bytes()).await.is_err() {
                    return;
                }

                // Replay history from Redis
                if let Some(ref client) = sm.redis {
                    if let Ok(mut con) = client.get_multiplexed_async_connection().await {
                        let result: Result<Vec<redis::Value>, _> = redis::cmd("XRANGE")
                            .arg("claude:stream")
                            .arg("-")
                            .arg("+")
                            .query_async(&mut con)
                            .await;

                        if let Ok(entries) = result {
                            for entry in entries {
                                if let Some(event) = parse_stream_entry(&entry) {
                                    let line = format!("data: {}\n\n", serde_json::to_string(&event).unwrap_or_default());
                                    if stream.write_all(line.as_bytes()).await.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }

                // Live events
                let mut rx = sm.event_tx.subscribe();
                loop {
                    match rx.recv().await {
                        Ok(event) => {
                            let data = json!({
                                "timestamp": event.timestamp,
                                "from": event.from,
                                "body": event.body,
                                "event_type": event.event_type,
                                "channel": match event.event_type.as_str() {
                                    "join" | "leave" => "claude:lobby",
                                    "status" => "claude:status",
                                    _ => "claude:questions",
                                },
                            });
                            let line = format!("data: {}\n\n", data);
                            if stream.write_all(line.as_bytes()).await.is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            } else if path.starts_with("/api/sessions") {
                let sessions = sm.list_sessions().await;
                let data: Vec<Value> = sessions
                    .iter()
                    .map(|(name, status)| json!({ "name": name, "status": status }))
                    .collect();
                let body = serde_json::to_string(&data).unwrap_or_default();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            } else {
                // Serve dashboard HTML
                let html = include_str!("dashboard.html");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                    html.len(),
                    html
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
    }
}

fn parse_stream_entry(entry: &redis::Value) -> Option<serde_json::Value> {
    // Redis XRANGE returns array of [id, [field, value, field, value, ...]]
    if let redis::Value::Array(ref arr) = entry {
        if arr.len() < 2 {
            return None;
        }

        let id = match &arr[0] {
            redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_string(),
            _ => return None,
        };

        // Parse timestamp from stream ID (milliseconds before the dash)
        let ts_ms: i64 = id.split('-').next()?.parse().ok()?;
        let timestamp = chrono::DateTime::from_timestamp_millis(ts_ms)?
            .to_rfc3339();

        // Parse fields
        let fields = match &arr[1] {
            redis::Value::Array(ref f) => f,
            _ => return None,
        };

        let mut payload_str = None;
        let mut channel = None;
        let mut i = 0;
        while i + 1 < fields.len() {
            let key = match &fields[i] {
                redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_string(),
                _ => { i += 2; continue; }
            };
            let val = match &fields[i + 1] {
                redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_string(),
                _ => { i += 2; continue; }
            };
            match key.as_str() {
                "payload" => payload_str = Some(val),
                "channel" => channel = Some(val),
                _ => {}
            }
            i += 2;
        }

        let payload: serde_json::Value = payload_str
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(json!({}));

        Some(json!({
            "timestamp": timestamp,
            "from": payload.get("from").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "body": payload.get("body").and_then(|v| v.as_str()).unwrap_or(""),
            "event_type": payload.get("type").and_then(|v| v.as_str()).unwrap_or("message"),
            "channel": channel.unwrap_or_else(|| "claude:questions".to_string()),
        }))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// TCP server — accepts shim connections
// ---------------------------------------------------------------------------

async fn run_tcp_server(sm: Arc<SessionManager>, port: u16) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    eprintln!("[server] TCP listener on 0.0.0.0:{}", port);

    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[server] Accept error: {}", e);
                continue;
            }
        };

        eprintln!("[server] New connection from {}", addr);
        let sm = sm.clone();

        tokio::spawn(async move {
            let (tcp_read, tcp_write) = tokio::io::split(stream);
            let mut reader = BufReader::new(tcp_read);

            // Channel for writing responses back to this shim
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

            // Writer task: drain rx → TCP
            let mut writer = tcp_write;
            let write_handle = tokio::spawn(async move {
                while let Some(line) = rx.recv().await {
                    if writer.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                    if writer.write_all(b"\n").await.is_err() {
                        break;
                    }
                    let _ = writer.flush().await;
                }
            });

            let mut session_name: Option<String> = None;
            let mut line = String::new();

            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // connection closed
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }

                        let msg: ProtocolMessage = match serde_json::from_str(trimmed) {
                            Ok(m) => m,
                            Err(e) => {
                                eprintln!("[server] Parse error from {}: {}", addr, e);
                                continue;
                            }
                        };

                        match msg {
                            ProtocolMessage::Hello { session_name: name } => {
                                session_name = Some(name.clone());
                                sm.register(name, tx.clone()).await;
                            }

                            ProtocolMessage::McpRequest { id, method, params } => {
                                let name = session_name.as_deref().unwrap_or("unknown");

                                // Don't send responses for notifications
                                if method.starts_with("notifications/") {
                                    continue;
                                }

                                let response =
                                    handle_mcp_request(&sm, name, id, &method, params).await;

                                // Skip null responses (notifications)
                                if matches!(&response, ProtocolMessage::McpResponse { id, .. } if *id == Value::Null) {
                                    continue;
                                }

                                if let Ok(s) = serde_json::to_string(&response) {
                                    let _ = tx.send(s);
                                }
                            }

                            ProtocolMessage::Goodbye => {
                                eprintln!("[server] Goodbye from {:?}", session_name);
                                break;
                            }

                            other => {
                                eprintln!("[server] Unexpected message: {:?}", other);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[server] Read error from {}: {}", addr, e);
                        break;
                    }
                }
            }

            // Cleanup
            if let Some(name) = session_name {
                sm.deregister(&name).await;
            }
            write_handle.abort();
            eprintln!("[server] Connection closed: {}", addr);
        });
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let tcp_port: u16 = std::env::var("TCP_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(9000);
    let dashboard_port: u16 = std::env::var("DASHBOARD_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8900);

    eprintln!("[server] Starting claude-channel-server");
    eprintln!("[server]   Redis: {}", redis_url);
    eprintln!("[server]   TCP port: {}", tcp_port);
    eprintln!("[server]   Dashboard port: {}", dashboard_port);

    let sm = SessionManager::new(Some(&redis_url))?;

    // Clear stale sessions from Redis on startup
    if let Some(ref client) = sm.redis {
        if let Ok(mut con) = client.get_multiplexed_async_connection().await {
            let _: Result<(), _> = redis::cmd("DEL")
                .arg("claude:sessions")
                .query_async(&mut con)
                .await;
        }
    }

    let sm_tcp = sm.clone();
    let sm_dash = sm.clone();

    let tcp_handle = tokio::spawn(async move {
        if let Err(e) = run_tcp_server(sm_tcp, tcp_port).await {
            eprintln!("[server] TCP server error: {}", e);
        }
    });

    let dash_handle = tokio::spawn(async move {
        if let Err(e) = run_dashboard(sm_dash, dashboard_port).await {
            eprintln!("[server] Dashboard error: {} (will retry in 2s)", e);
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            // Don't crash — TCP server continues even if dashboard fails
        }
    });

    let _ = tokio::join!(tcp_handle, dash_handle);

    Ok(())
}
