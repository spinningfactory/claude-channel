use super::Event;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub struct WebhookSource {
    pub port: u16,
    pub bind: String,
}

#[async_trait::async_trait]
impl super::EventSource for WebhookSource {
    async fn run(self, tx: mpsc::Sender<Event>) -> anyhow::Result<()> {
        let listener = TcpListener::bind((&*self.bind, self.port)).await?;
        eprintln!("[webhook] Listening on http://{}:{}", self.bind, self.port);

        loop {
            let (mut stream, addr) = match listener.accept().await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[webhook] Accept error: {}", e);
                    continue;
                }
            };

            let tx = tx.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let raw = String::from_utf8_lossy(&buf[..n]).to_string();

                let (_headers, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
                let first_line = raw.lines().next().unwrap_or("");
                let method = first_line.split_whitespace().next().unwrap_or("GET");
                let path = first_line.split_whitespace().nth(1).unwrap_or("/");

                if method == "POST" {
                    let req_id = REQUEST_ID.fetch_add(1, Ordering::Relaxed).to_string();
                    let mut meta = HashMap::new();
                    meta.insert("source".to_string(), "webhook".to_string());
                    meta.insert("path".to_string(), path.to_string());
                    meta.insert("request_id".to_string(), req_id.clone());
                    meta.insert("sender".to_string(), addr.to_string());

                    eprintln!("[webhook] Event (id={}): {}", req_id, body);
                    let _ = tx.send(Event {
                        content: body.to_string(),
                        meta,
                    }).await;

                    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok";
                    let _ = stream.write_all(response.as_bytes()).await;
                } else {
                    let help = format!(
                        "Claude Channel\n\nPOST a message to forward it into Claude's session:\n  curl -X POST http://{}:{} -d 'your message'\n",
                        "127.0.0.1", 8788
                    );
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
                        help.len(), help
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            });
        }
    }
}
