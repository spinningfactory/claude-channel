use futures::StreamExt;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

#[derive(Clone, Debug, serde::Serialize)]
struct Event {
    timestamp: String,
    channel: String,
    from: String,
    body: String,
    event_type: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let redis_url = std::env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://localhost:16379".to_string());
    let port: u16 = std::env::var("DASHBOARD_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8900);

    let (tx, _) = broadcast::channel::<Event>(256);
    let tx = Arc::new(tx);

    // Subscribe to all claude:* channels via pattern subscribe
    let tx_redis = tx.clone();
    tokio::spawn(async move {
        loop {
            match subscribe_redis(&redis_url, &tx_redis).await {
                Ok(_) => break,
                Err(e) => {
                    eprintln!("[dashboard] Redis error: {}, reconnecting in 3s...", e);
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                }
            }
        }
    });

    // Also poll session registry periodically and emit session events
    let tx_sessions = tx.clone();
    let redis_url_sessions = std::env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://localhost:16379".to_string());
    tokio::spawn(async move {
        loop {
            if let Ok(sessions) = get_sessions(&redis_url_sessions) {
                let event = Event {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    channel: "_sessions".to_string(),
                    from: "_system".to_string(),
                    body: serde_json::to_string(&sessions).unwrap_or_default(),
                    event_type: "sessions".to_string(),
                };
                let _ = tx_sessions.send(event);
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    });

    // HTTP server
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    eprintln!("[dashboard] Listening on http://localhost:{}", port);

    loop {
        let (mut stream, _) = listener.accept().await?;
        let tx = tx.clone();

        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let raw = String::from_utf8_lossy(&buf[..n]);
            let path = raw.lines().next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/");

            match path {
                "/events" => {
                    // SSE stream
                    let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nAccess-Control-Allow-Origin: *\r\nConnection: keep-alive\r\n\r\n";
                    if stream.write_all(headers.as_bytes()).await.is_err() { return; }

                    // Replay history from Redis stream
                    let redis_url = std::env::var("REDIS_URL")
                        .unwrap_or_else(|_| "redis://localhost:16379".to_string());
                    if let Ok(history) = replay_history(&redis_url).await {
                        for event in history {
                            let data = serde_json::to_string(&event).unwrap_or_default();
                            let msg = format!("data: {}\n\n", data);
                            if stream.write_all(msg.as_bytes()).await.is_err() { return; }
                        }
                    }

                    // Then stream live events
                    let mut rx = tx.subscribe();
                    while let Ok(event) = rx.recv().await {
                        let data = serde_json::to_string(&event).unwrap_or_default();
                        let msg = format!("data: {}\n\n", data);
                        if stream.write_all(msg.as_bytes()).await.is_err() { break; }
                    }
                }
                _ => {
                    // Serve the dashboard HTML
                    let html = include_str!("dashboard.html");
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                        html.len(), html
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            }
        });
    }
}

async fn subscribe_redis(url: &str, tx: &broadcast::Sender<Event>) -> anyhow::Result<()> {
    let client = redis::Client::open(url)?;
    let mut pubsub = client.get_async_pubsub().await?;

    pubsub.psubscribe("claude:*").await?;
    eprintln!("[dashboard] Subscribed to claude:*");

    loop {
        let msg: redis::Msg = pubsub.on_message().next().await
            .ok_or_else(|| anyhow::anyhow!("pubsub stream ended"))?;

        let payload: String = msg.get_payload()?;
        let channel = msg.get_channel_name().to_string();

        // Try to parse the payload as JSON to extract from/body
        let (from, body, event_type) = if let Ok(v) = serde_json::from_str::<serde_json::Value>(&payload) {
            let from = v.get("from").or(v.get("session"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let body = v.get("body").or(v.get("goal")).or(v.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or(&payload)
                .to_string();
            let event_type = v.get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("message")
                .to_string();
            (from, body, event_type)
        } else {
            ("unknown".to_string(), payload.clone(), "message".to_string())
        };

        let event = Event {
            timestamp: chrono::Utc::now().to_rfc3339(),
            channel,
            from,
            body,
            event_type,
        };

        let _ = tx.send(event);
    }
}

fn get_sessions(url: &str) -> anyhow::Result<Vec<(String, serde_json::Value)>> {
    let client = redis::Client::open(url)?;
    let mut con = client.get_connection()?;
    let raw: Vec<(String, String)> = redis::cmd("HGETALL")
        .arg("claude:sessions")
        .query(&mut con)?;

    let sessions: Vec<(String, serde_json::Value)> = raw.into_iter()
        .map(|(name, data)| {
            let v = serde_json::from_str(&data).unwrap_or(serde_json::json!({"goal": data}));
            (name, v)
        })
        .collect();

    Ok(sessions)
}

async fn replay_history(url: &str) -> anyhow::Result<Vec<Event>> {
    let client = redis::Client::open(url)?;
    let mut con = client.get_multiplexed_async_connection().await?;

    // XRANGE claude:stream - + returns all entries
    let result: Vec<redis::Value> = redis::cmd("XRANGE")
        .arg("claude:stream")
        .arg("-")
        .arg("+")
        .query_async(&mut con)
        .await?;

    let mut events = Vec::new();

    for entry in result {
        if let redis::Value::Array(ref fields) = entry {
            if fields.len() < 2 { continue; }

            // Extract stream ID (contains timestamp)
            let stream_id = match &fields[0] {
                redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_string(),
                _ => continue,
            };

            // Extract field pairs
            let mut channel = String::new();
            let mut payload = String::new();

            if let redis::Value::Array(ref pairs) = fields[1] {
                let mut i = 0;
                while i + 1 < pairs.len() {
                    let key = match &pairs[i] {
                        redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_string(),
                        _ => { i += 2; continue; }
                    };
                    let val = match &pairs[i + 1] {
                        redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_string(),
                        _ => { i += 2; continue; }
                    };
                    match key.as_str() {
                        "channel" => channel = val,
                        "payload" => payload = val,
                        _ => {}
                    }
                    i += 2;
                }
            }

            if channel.is_empty() || payload.is_empty() { continue; }

            // Convert stream ID timestamp (milliseconds before the dash)
            let ts_millis: u64 = stream_id.split('-').next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let timestamp = chrono::DateTime::from_timestamp_millis(ts_millis as i64)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

            // Parse payload same as live events
            let (from, body, event_type) = if let Ok(v) = serde_json::from_str::<serde_json::Value>(&payload) {
                let from = v.get("from").or(v.get("session"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let body = v.get("body").or(v.get("goal")).or(v.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(&payload)
                    .to_string();
                let event_type = v.get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("message")
                    .to_string();
                (from, body, event_type)
            } else {
                ("unknown".to_string(), payload.clone(), "message".to_string())
            };

            events.push(Event {
                timestamp,
                channel,
                from,
                body,
                event_type,
            });
        }
    }

    eprintln!("[dashboard] Replayed {} events from stream", events.len());
    Ok(events)
}

use tokio::io::AsyncReadExt;
