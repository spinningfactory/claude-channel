mod config;
#[cfg(feature = "redis")]
mod coordination;
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
    let (tx, mut rx) = mpsc::channel::<sources::Event>(256);

    // Set up coordination if configured
    #[cfg(feature = "redis")]
    let coordinator = if let Some(ref coord_config) = config.coordination {
        eprintln!("[channel] Coordination enabled: {}", coord_config.goal);
        let coordinator = coordination::Coordinator::new(
            server_name.clone(),
            coord_config.goal.clone(),
            coord_config.url.clone(),
            coord_config.room_id().to_string(),
        )?;
        coordinator.register().await?;

        // Spawn coordination subscriber
        let coord_tx = tx.clone();
        let coord_sub = coordination::Coordinator::new(
            server_name.clone(),
            coord_config.goal.clone(),
            coord_config.url.clone(),
            coord_config.room_id().to_string(),
        )?;
        tokio::spawn(async move {
            if let Err(e) = coord_sub.subscribe(coord_tx).await {
                eprintln!("[coord] Fatal: {}", e);
            }
        });

        Some(coordinator)
    } else {
        None
    };

    #[cfg(feature = "redis")]
    let publisher = coordinator.as_ref().and_then(|c| c.publisher().ok());

    let instructions = config.instructions.clone().unwrap_or_else(|| {
        #[cfg(feature = "redis")]
        if config.coordination.is_some() {
            return format!(
                "You are session \"{}\". Messages from other Claude sessions arrive as channel notifications. \
                 Use the publish tool to send messages and list_sessions to see who's online. \
                 ONLY respond to messages directly relevant to your work.",
                server_name
            );
        }
        format!(
            "Events from external sources arrive as <channel source=\"{}\" ...>.",
            server_name
        )
    });

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

            #[cfg(feature = "nats")]
            config::SourceConfig::Nats { url, subjects } => {
                let source = sources::nats::NatsSource { url, subjects };
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = source.run(tx).await {
                        eprintln!("[nats] Fatal: {}", e);
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

    // Build MCP context
    let ctx = mcp::McpContext {
        server_name: server_name.clone(),
        instructions,
        #[cfg(feature = "redis")]
        publisher,
    };

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
                Ok(req) => ctx.handle_request(req),
                Err(e) => {
                    eprintln!("[channel] Bad JSON-RPC: {} — {}", e, line);
                }
            }
        }

        eprintln!("[channel] stdin closed, shutting down");
    });

    let _ = handle.await;

    // Deregister on shutdown
    #[cfg(feature = "redis")]
    if let Some(ref coord) = coordinator {
        let _ = coord.deregister().await;
    }

    Ok(())
}
