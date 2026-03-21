use crate::sources::Event;
use futures::StreamExt;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct Coordinator {
    pub session_name: String,
    pub goal: String,
    pub url: String,
    client: redis::Client,
}

impl Coordinator {
    pub fn new(session_name: String, goal: String, url: String) -> anyhow::Result<Self> {
        let client = redis::Client::open(url.as_str())?;
        Ok(Self {
            session_name,
            goal,
            url,
            client,
        })
    }

    /// Register this session in the Redis hash and announce to the lobby.
    pub async fn register(&self) -> anyhow::Result<()> {
        let mut con = self.client.get_multiplexed_async_connection().await?;

        let meta = serde_json::json!({
            "goal": self.goal,
            "started": chrono::Utc::now().to_rfc3339(),
        });

        redis::cmd("HSET")
            .arg("claude:sessions")
            .arg(&self.session_name)
            .arg(meta.to_string())
            .query_async::<()>(&mut con)
            .await?;

        let announce = serde_json::json!({
            "type": "join",
            "session": self.session_name,
            "goal": self.goal,
        });

        redis::cmd("PUBLISH")
            .arg("claude:lobby")
            .arg(announce.to_string())
            .query_async::<()>(&mut con)
            .await?;

        // Persist to stream
        redis::cmd("XADD")
            .arg("claude:stream").arg("MAXLEN").arg("~").arg("10000").arg("*")
            .arg("channel").arg("claude:lobby")
            .arg("payload").arg(announce.to_string())
            .query_async::<String>(&mut con)
            .await?;

        eprintln!("[coord] Registered as '{}': {}", self.session_name, self.goal);
        Ok(())
    }

    /// Deregister this session from Redis and announce departure.
    pub async fn deregister(&self) -> anyhow::Result<()> {
        let mut con = self.client.get_multiplexed_async_connection().await?;

        redis::cmd("HDEL")
            .arg("claude:sessions")
            .arg(&self.session_name)
            .query_async::<()>(&mut con)
            .await?;

        let announce = serde_json::json!({
            "type": "leave",
            "session": self.session_name,
        });

        redis::cmd("PUBLISH")
            .arg("claude:lobby")
            .arg(announce.to_string())
            .query_async::<()>(&mut con)
            .await?;

        // Persist to stream
        redis::cmd("XADD")
            .arg("claude:stream").arg("MAXLEN").arg("~").arg("10000").arg("*")
            .arg("channel").arg("claude:lobby")
            .arg("payload").arg(announce.to_string())
            .query_async::<String>(&mut con)
            .await?;

        eprintln!("[coord] Deregistered '{}'", self.session_name);
        Ok(())
    }

    /// Subscribe to coordination channels and forward events.
    pub async fn subscribe(&self, tx: mpsc::Sender<Event>) -> anyhow::Result<()> {
        let mut pubsub = self.client.get_async_pubsub().await?;

        let channels = vec![
            "claude:lobby".to_string(),
            "claude:questions".to_string(),
            format!("claude:session:{}", self.session_name),
        ];

        for channel in &channels {
            pubsub.subscribe(channel).await?;
            eprintln!("[coord] Subscribed to {}", channel);
        }

        loop {
            let msg: redis::Msg = pubsub
                .on_message()
                .next()
                .await
                .ok_or_else(|| anyhow::anyhow!("coordination pubsub stream ended"))?;

            let payload: String = msg.get_payload()?;
            let channel_name = msg.get_channel_name().to_string();

            let mut meta = HashMap::new();
            meta.insert("source".to_string(), "coordination".to_string());
            meta.insert("channel".to_string(), channel_name.clone());

            eprintln!("[coord] {} → {}", channel_name, payload);
            let _ = tx.send(Event {
                content: payload,
                meta,
            }).await;
        }
    }

    /// Create a shared handle for use by MCP tool handlers.
    pub fn publisher(&self) -> anyhow::Result<Publisher> {
        let client = redis::Client::open(self.url.as_str())?;
        Ok(Publisher {
            client,
            session_name: self.session_name.clone(),
        })
    }
}

/// Sync handle for publishing messages, safe to call from blocking threads.
#[derive(Clone)]
pub struct Publisher {
    client: redis::Client,
    pub session_name: String,
}

impl Publisher {
    pub fn publish(&self, channel: &str, message: &str) -> anyhow::Result<()> {
        let mut con = self.client.get_connection()?;
        redis::cmd("PUBLISH")
            .arg(channel)
            .arg(message)
            .query::<()>(&mut con)?;

        // Persist to stream
        redis::cmd("XADD")
            .arg("claude:stream").arg("MAXLEN").arg("~").arg("10000").arg("*")
            .arg("channel").arg(channel)
            .arg("payload").arg(message)
            .query::<String>(&mut con)?;

        eprintln!("[coord] Published to {}: {}", channel, message);
        Ok(())
    }

    pub fn list_sessions(&self) -> anyhow::Result<Vec<(String, String)>> {
        let mut con = self.client.get_connection()?;
        let result: Vec<(String, String)> = redis::cmd("HGETALL")
            .arg("claude:sessions")
            .query(&mut con)?;
        Ok(result)
    }
}
