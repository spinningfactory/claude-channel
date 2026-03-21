mod config;
mod mcp;
mod sources;

use sources::EventSource;
use std::io::{self, BufRead};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_path = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("CHANNEL_CONFIG").ok())
        .unwrap_or_else(|| "config.yaml".to_string());

    let config = config::Config::load(&config_path)?;
    eprintln!("[channel] Loaded config from: {}", config_path);
    eprintln!("[channel] Server name: {}", config.server_name);
    eprintln!("[channel] Sources: {}", config.sources.len());

    let server_name = config.server_name.clone();
    let instructions = config.instructions.clone().unwrap_or_else(|| {
        format!(
            "Events from external sources arrive as <channel source=\"{}\" ...>.",
            server_name
        )
    });

    let (tx, mut rx) = mpsc::channel::<sources::Event>(256);

    // Spawn each configured source
    for source_config in config.sources {
        match source_config {
            config::SourceConfig::Webhook { port, bind } => {
                let source = sources::webhook::WebhookSource { port, bind };
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = source.run(tx).await {
                        eprintln!("[webhook] Fatal: {}", e);
                    }
                });
            }

            #[cfg(feature = "sqs")]
            config::SourceConfig::Sqs {
                queue_url,
                region,
                wait_seconds,
                max_messages,
            } => {
                let source = sources::sqs::SqsSource {
                    queue_url,
                    region,
                    wait_seconds,
                    max_messages,
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = source.run(tx).await {
                        eprintln!("[sqs] Fatal: {}", e);
                    }
                });
            }

            #[cfg(feature = "redis")]
            config::SourceConfig::Redis {
                url,
                mode,
                channels,
                keys,
            } => {
                let source = sources::redis::RedisSource {
                    url,
                    mode,
                    channels,
                    keys,
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = source.run(tx).await {
                        eprintln!("[redis] Fatal: {}", e);
                    }
                });
            }

            #[cfg(feature = "postgres")]
            config::SourceConfig::Postgres {
                connection,
                channels,
            } => {
                let source = sources::postgres::PostgresSource {
                    connection,
                    channels,
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = source.run(tx).await {
                        eprintln!("[postgres] Fatal: {}", e);
                    }
                });
            }
        }
    }

    // Drop original sender so rx closes when all sources exit
    drop(tx);

    // Notification emitter: reads events from all sources, writes MCP notifications
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            mcp::send_channel_notification(&event.content, event.meta);
        }
        eprintln!("[channel] All sources closed");
    });

    // MCP stdin loop (on a blocking thread so tokio tasks can still run)
    let handle = tokio::task::spawn_blocking(move || {
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

            match serde_json::from_str::<mcp::JsonRpcRequest>(&line) {
                Ok(req) => mcp::handle_request(req, &server_name, &instructions),
                Err(e) => {
                    eprintln!("[channel] Bad JSON-RPC: {} — {}", e, line);
                }
            }
        }

        eprintln!("[channel] stdin closed, shutting down");
    });

    let _ = handle.await;
    Ok(())
}
