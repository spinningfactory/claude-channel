use anyhow::{Context, Result};
use serde_json::Value;
use std::io::{self, BufRead, Write as _};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

mod protocol;
use protocol::ProtocolMessage;

fn send_stdout(msg: &Value) {
    let s = serde_json::to_string(msg).unwrap();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(s.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

fn send_tcp_sync(line: &str, tcp_tx: &tokio::sync::mpsc::UnboundedSender<String>) {
    let _ = tcp_tx.send(line.to_string());
}

#[tokio::main]
async fn main() -> Result<()> {
    let server_url = std::env::var("CHANNEL_SERVER")
        .or_else(|_| std::env::args().nth(1).ok_or(std::env::VarError::NotPresent))
        .unwrap_or_else(|_| "127.0.0.1:9000".to_string());

    let prefix = std::env::var("CHANNEL_SESSION")
        .or_else(|_| std::env::args().nth(2).ok_or(std::env::VarError::NotPresent))
        .unwrap_or_else(|_| "session".to_string());
    let session_name = format!("{}-{}", prefix, std::process::id());

    eprintln!("[shim] Connecting to {} as {}", server_url, session_name);

    let stream = TcpStream::connect(&server_url)
        .await
        .with_context(|| format!("Failed to connect to server at {}", server_url))?;

    let (tcp_read, mut tcp_write) = tokio::io::split(stream);
    let mut tcp_reader = BufReader::new(tcp_read);

    // Send Hello
    let hello = ProtocolMessage::Hello {
        session_name: session_name.clone(),
    };
    let hello_line = serde_json::to_string(&hello)?;
    tcp_write.write_all(hello_line.as_bytes()).await?;
    tcp_write.write_all(b"\n").await?;
    tcp_write.flush().await?;

    eprintln!("[shim] Connected and registered as {}", session_name);

    // Channel for stdin → TCP writes (so blocking stdin can send to async TCP)
    let (tcp_tx, mut tcp_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Task: forward tcp_rx → TCP socket
    let write_handle = tokio::spawn(async move {
        while let Some(line) = tcp_rx.recv().await {
            if tcp_write.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if tcp_write.write_all(b"\n").await.is_err() {
                break;
            }
            let _ = tcp_write.flush().await;
        }
    });

    // Task: read TCP → stdout
    let read_handle = tokio::spawn(async move {
        let mut line = String::new();
        loop {
            line.clear();
            match tcp_reader.read_line(&mut line).await {
                Ok(0) => {
                    eprintln!("[shim] Server disconnected");
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    match serde_json::from_str::<ProtocolMessage>(trimmed) {
                        Ok(ProtocolMessage::McpResponse { id, result }) => {
                            send_stdout(&serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": result,
                            }));
                        }
                        Ok(ProtocolMessage::McpError { id, code, message }) => {
                            send_stdout(&serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": code, "message": message },
                            }));
                        }
                        Ok(ProtocolMessage::Notification { method, params }) => {
                            send_stdout(&serde_json::json!({
                                "jsonrpc": "2.0",
                                "method": method,
                                "params": params,
                            }));
                        }
                        Ok(ProtocolMessage::ServerError { message }) => {
                            eprintln!("[shim] Server error: {}", message);
                            break;
                        }
                        Ok(other) => {
                            eprintln!("[shim] Unexpected message from server: {:?}", other);
                        }
                        Err(e) => {
                            eprintln!("[shim] Failed to parse server message: {}", e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[shim] TCP read error: {}", e);
                    break;
                }
            }
        }
    });

    // Blocking task: read stdin → TCP
    let stdin_handle = tokio::task::spawn_blocking(move || {
        let stdin = io::stdin();
        let reader = stdin.lock();

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };

            if line.trim().is_empty() {
                continue;
            }

            // Parse as JSON-RPC, wrap as McpRequest
            match serde_json::from_str::<serde_json::Value>(&line) {
                Ok(req) => {
                    let msg = ProtocolMessage::McpRequest {
                        id: req.get("id").cloned().unwrap_or(Value::Null),
                        method: req
                            .get("method")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        params: req.get("params").cloned().unwrap_or(Value::Null),
                    };
                    if let Ok(s) = serde_json::to_string(&msg) {
                        send_tcp_sync(&s, &tcp_tx);
                    }
                }
                Err(e) => {
                    eprintln!("[shim] Failed to parse stdin JSON: {}", e);
                }
            }
        }

        // stdin closed — send Goodbye
        eprintln!("[shim] stdin closed, sending Goodbye");
        if let Ok(s) = serde_json::to_string(&ProtocolMessage::Goodbye) {
            let _ = tcp_tx.send(s);
        }
    });

    // Wait for any task to finish (stdin close or TCP disconnect)
    tokio::select! {
        _ = stdin_handle => {}
        _ = read_handle => {}
    }

    write_handle.abort();
    eprintln!("[shim] Shutting down");
    Ok(())
}
