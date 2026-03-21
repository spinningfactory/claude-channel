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
    #[serde(skip_serializing_if = "Option::is_none")]
    room: Option<String>,
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
                    room: None,
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

            let (base_path, query_params) = parse_path_and_query(path);

            match base_path {
                "/events" => {
                    let room_filter = query_params.get("room").cloned();

                    // SSE stream
                    let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nAccess-Control-Allow-Origin: *\r\nConnection: keep-alive\r\n\r\n";
                    if stream.write_all(headers.as_bytes()).await.is_err() { return; }

                    // Replay history from Redis stream
                    let redis_url = std::env::var("REDIS_URL")
                        .unwrap_or_else(|_| "redis://localhost:16379".to_string());
                    if let Ok(history) = replay_history(&redis_url, room_filter.as_deref()).await {
                        for event in history {
                            let data = serde_json::to_string(&event).unwrap_or_default();
                            let msg = format!("data: {}\n\n", data);
                            if stream.write_all(msg.as_bytes()).await.is_err() { return; }
                        }
                    }

                    // Then stream live events, filtering by room if specified
                    let mut rx = tx.subscribe();
                    while let Ok(event) = rx.recv().await {
                        if let Some(ref room) = room_filter {
                            // Filter by room: check event.room field or parse from channel name
                            let event_room = event.room.as_deref()
                                .or_else(|| extract_room_from_channel(&event.channel));
                            if event_room != Some(room.as_str()) {
                                continue;
                            }
                        }
                        let data = serde_json::to_string(&event).unwrap_or_default();
                        let msg = format!("data: {}\n\n", data);
                        if stream.write_all(msg.as_bytes()).await.is_err() { break; }
                    }
                }

                "/api/rooms" => {
                    let redis_url = std::env::var("REDIS_URL")
                        .unwrap_or_else(|_| "redis://localhost:16379".to_string());

                    let result = if let Some(date) = query_params.get("date") {
                        get_rooms_by_date(&redis_url, date)
                    } else {
                        get_all_rooms(&redis_url)
                    };

                    match result {
                        Ok(json_str) => {
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\n\r\n{}",
                                json_str.len(), json_str
                            );
                            let _ = stream.write_all(response.as_bytes()).await;
                        }
                        Err(e) => {
                            let err_json = serde_json::json!({"error": e.to_string()}).to_string();
                            let response = format!(
                                "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                err_json.len(), err_json
                            );
                            let _ = stream.write_all(response.as_bytes()).await;
                        }
                    }
                }

                "/api/dates" => {
                    let redis_url = std::env::var("REDIS_URL")
                        .unwrap_or_else(|_| "redis://localhost:16379".to_string());

                    match get_room_dates(&redis_url) {
                        Ok(json_str) => {
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\n\r\n{}",
                                json_str.len(), json_str
                            );
                            let _ = stream.write_all(response.as_bytes()).await;
                        }
                        Err(e) => {
                            let err_json = serde_json::json!({"error": e.to_string()}).to_string();
                            let response = format!(
                                "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                err_json.len(), err_json
                            );
                            let _ = stream.write_all(response.as_bytes()).await;
                        }
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

/// Parse a request path into base path and query parameters.
fn parse_path_and_query(path: &str) -> (&str, std::collections::HashMap<String, String>) {
    let mut params = std::collections::HashMap::new();
    if let Some((base, query)) = path.split_once('?') {
        for pair in query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                params.insert(k.to_string(), v.to_string());
            }
        }
        (base, params)
    } else {
        (path, params)
    }
}

/// Extract room ID from a channel name like "claude:room:morning-standup:questions" -> "morning-standup"
fn extract_room_from_channel(channel: &str) -> Option<&str> {
    if channel.starts_with("claude:room:") {
        let rest = &channel["claude:room:".len()..];
        // Room ID is everything up to the next colon
        rest.split(':').next()
    } else {
        None
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

        // Extract room from channel name
        let room = extract_room_from_channel(&channel).map(String::from);

        let event = Event {
            timestamp: chrono::Utc::now().to_rfc3339(),
            channel,
            from,
            body,
            event_type,
            room,
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

fn get_all_rooms(url: &str) -> anyhow::Result<String> {
    let client = redis::Client::open(url)?;
    let mut con = client.get_connection()?;
    let raw: Vec<(String, String)> = redis::cmd("HGETALL")
        .arg("claude:rooms")
        .query(&mut con)?;

    let rooms: Vec<serde_json::Value> = raw.into_iter()
        .map(|(id, data)| {
            let meta = serde_json::from_str::<serde_json::Value>(&data)
                .unwrap_or(serde_json::json!({}));
            serde_json::json!({
                "id": id,
                "meta": meta,
            })
        })
        .collect();

    Ok(serde_json::to_string(&rooms)?)
}

fn get_rooms_by_date(url: &str, date: &str) -> anyhow::Result<String> {
    let client = redis::Client::open(url)?;
    let mut con = client.get_connection()?;

    // Get room IDs for this date
    let room_ids: Vec<String> = redis::cmd("SMEMBERS")
        .arg(format!("claude:rooms:by_date:{}", date))
        .query(&mut con)?;

    if room_ids.is_empty() {
        return Ok("[]".to_string());
    }

    // Get metadata for each room from the rooms hash
    let mut rooms = Vec::new();
    for room_id in &room_ids {
        let data: Option<String> = redis::cmd("HGET")
            .arg("claude:rooms")
            .arg(room_id)
            .query(&mut con)?;

        let meta = data
            .and_then(|d| serde_json::from_str::<serde_json::Value>(&d).ok())
            .unwrap_or(serde_json::json!({}));

        rooms.push(serde_json::json!({
            "id": room_id,
            "meta": meta,
        }));
    }

    Ok(serde_json::to_string(&rooms)?)
}

fn get_room_dates(url: &str) -> anyhow::Result<String> {
    let client = redis::Client::open(url)?;
    let mut con = client.get_connection()?;

    let keys: Vec<String> = redis::cmd("KEYS")
        .arg("claude:rooms:by_date:*")
        .query(&mut con)?;

    let prefix = "claude:rooms:by_date:";
    let mut dates: Vec<String> = keys.into_iter()
        .filter_map(|k| k.strip_prefix(prefix).map(String::from))
        .collect();

    dates.sort();

    Ok(serde_json::to_string(&dates)?)
}

async fn replay_history(url: &str, room: Option<&str>) -> anyhow::Result<Vec<Event>> {
    let client = redis::Client::open(url)?;
    let mut con = client.get_multiplexed_async_connection().await?;

    // Use room-scoped stream if room specified, otherwise global stream
    let stream_key = match room {
        Some(r) => format!("claude:room:{}:stream", r),
        None => "claude:stream".to_string(),
    };

    // XRANGE returns all entries
    let result: Vec<redis::Value> = redis::cmd("XRANGE")
        .arg(&stream_key)
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
            let mut event_room = None;

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
                        "room" => event_room = Some(val),
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

            // If no room field in stream data, try to extract from channel name
            let room_val = event_room.or_else(|| extract_room_from_channel(&channel).map(String::from));

            events.push(Event {
                timestamp,
                channel,
                from,
                body,
                event_type,
                room: room_val,
            });
        }
    }

    eprintln!("[dashboard] Replayed {} events from {}", events.len(), stream_key);
    Ok(events)
}

use tokio::io::AsyncReadExt;
