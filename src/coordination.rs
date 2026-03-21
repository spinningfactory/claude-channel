use crate::sources::Event;
use futures::StreamExt;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct Coordinator {
    pub session_name: String,
    pub goal: String,
    pub room: String,
    pub url: String,
    client: redis::Client,
}

impl Coordinator {
    pub fn new(session_name: String, goal: String, url: String, room: String) -> anyhow::Result<Self> {
        let client = redis::Client::open(url.as_str())?;
        Ok(Self {
            session_name,
            goal,
            room,
            url,
            client,
        })
    }

    /// Register this session in the Redis hash and announce to the lobby.
    pub async fn register(&self) -> anyhow::Result<()> {
        let mut con = self.client.get_multiplexed_async_connection().await?;

        let meta = serde_json::json!({
            "goal": self.goal,
            "room": self.room,
            "started": chrono::Utc::now().to_rfc3339(),
        });

        // Room-scoped session registration
        redis::cmd("HSET")
            .arg(format!("claude:room:{}:sessions", self.room))
            .arg(&self.session_name)
            .arg(meta.to_string())
            .query_async::<()>(&mut con)
            .await?;

        // Register room in global rooms hash
        let now = chrono::Utc::now();
        let room_meta = serde_json::json!({
            "name": self.room,
            "created_at": now.to_rfc3339(),
            "created_by": self.session_name,
        });
        // HSETNX - only set if room doesn't exist yet
        redis::cmd("HSETNX")
            .arg("claude:rooms")
            .arg(&self.room)
            .arg(room_meta.to_string())
            .query_async::<()>(&mut con)
            .await?;

        // Add room to date-based set
        let date_key = format!("claude:rooms:by_date:{}", now.format("%Y-%m-%d"));
        redis::cmd("SADD")
            .arg(&date_key)
            .arg(&self.room)
            .query_async::<()>(&mut con)
            .await?;

        // Create room meta hash if it doesn't exist
        let room_meta_key = format!("claude:room:{}:meta", self.room);
        redis::cmd("HSETNX")
            .arg(&room_meta_key)
            .arg("created_at")
            .arg(now.to_rfc3339())
            .query_async::<()>(&mut con)
            .await?;
        redis::cmd("HSETNX")
            .arg(&room_meta_key)
            .arg("created_by")
            .arg(&self.session_name)
            .query_async::<()>(&mut con)
            .await?;

        let announce = serde_json::json!({
            "type": "join",
            "session": self.session_name,
            "goal": self.goal,
            "room": self.room,
        });

        // Publish to room-scoped lobby
        redis::cmd("PUBLISH")
            .arg(format!("claude:room:{}:lobby", self.room))
            .arg(announce.to_string())
            .query_async::<()>(&mut con)
            .await?;

        // Persist to room-scoped stream
        redis::cmd("XADD")
            .arg(format!("claude:room:{}:stream", self.room)).arg("MAXLEN").arg("~").arg("10000").arg("*")
            .arg("channel").arg(format!("claude:room:{}:lobby", self.room))
            .arg("payload").arg(announce.to_string())
            .arg("room").arg(&self.room)
            .query_async::<String>(&mut con)
            .await?;

        // Persist to global stream
        redis::cmd("XADD")
            .arg("claude:stream").arg("MAXLEN").arg("~").arg("10000").arg("*")
            .arg("channel").arg(format!("claude:room:{}:lobby", self.room))
            .arg("payload").arg(announce.to_string())
            .arg("room").arg(&self.room)
            .query_async::<String>(&mut con)
            .await?;

        eprintln!("[coord] Registered as '{}' in room '{}': {}", self.session_name, self.room, self.goal);
        Ok(())
    }

    /// Deregister this session from Redis and announce departure.
    pub async fn deregister(&self) -> anyhow::Result<()> {
        let mut con = self.client.get_multiplexed_async_connection().await?;

        redis::cmd("HDEL")
            .arg(format!("claude:room:{}:sessions", self.room))
            .arg(&self.session_name)
            .query_async::<()>(&mut con)
            .await?;

        let announce = serde_json::json!({
            "type": "leave",
            "session": self.session_name,
            "room": self.room,
        });

        redis::cmd("PUBLISH")
            .arg(format!("claude:room:{}:lobby", self.room))
            .arg(announce.to_string())
            .query_async::<()>(&mut con)
            .await?;

        // Persist to room-scoped stream
        redis::cmd("XADD")
            .arg(format!("claude:room:{}:stream", self.room)).arg("MAXLEN").arg("~").arg("10000").arg("*")
            .arg("channel").arg(format!("claude:room:{}:lobby", self.room))
            .arg("payload").arg(announce.to_string())
            .arg("room").arg(&self.room)
            .query_async::<String>(&mut con)
            .await?;

        // Persist to global stream
        redis::cmd("XADD")
            .arg("claude:stream").arg("MAXLEN").arg("~").arg("10000").arg("*")
            .arg("channel").arg(format!("claude:room:{}:lobby", self.room))
            .arg("payload").arg(announce.to_string())
            .arg("room").arg(&self.room)
            .query_async::<String>(&mut con)
            .await?;

        eprintln!("[coord] Deregistered '{}' from room '{}'", self.session_name, self.room);
        Ok(())
    }

    /// Subscribe to coordination channels and forward events.
    pub async fn subscribe(&self, tx: mpsc::Sender<Event>) -> anyhow::Result<()> {
        let mut pubsub = self.client.get_async_pubsub().await?;

        let channels = vec![
            format!("claude:room:{}:lobby", self.room),
            format!("claude:room:{}:questions", self.room),
            format!("claude:room:{}:session:{}", self.room, self.session_name),
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
            meta.insert("room".to_string(), self.room.clone());

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
            room: self.room.clone(),
        })
    }
}

/// Sync handle for publishing messages, safe to call from blocking threads.
#[derive(Clone)]
pub struct Publisher {
    client: redis::Client,
    pub session_name: String,
    pub room: String,
}

impl Publisher {
    pub fn publish(&self, channel: &str, message: &str) -> anyhow::Result<()> {
        let mut con = self.client.get_connection()?;
        redis::cmd("PUBLISH")
            .arg(channel)
            .arg(message)
            .query::<()>(&mut con)?;

        // Persist to room-scoped stream
        redis::cmd("XADD")
            .arg(format!("claude:room:{}:stream", self.room)).arg("MAXLEN").arg("~").arg("10000").arg("*")
            .arg("channel").arg(channel)
            .arg("payload").arg(message)
            .arg("room").arg(&self.room)
            .query::<String>(&mut con)?;

        // Persist to global stream
        redis::cmd("XADD")
            .arg("claude:stream").arg("MAXLEN").arg("~").arg("10000").arg("*")
            .arg("channel").arg(channel)
            .arg("payload").arg(message)
            .arg("room").arg(&self.room)
            .query::<String>(&mut con)?;

        eprintln!("[coord] Published to {}: {}", channel, message);
        Ok(())
    }

    pub fn list_sessions(&self) -> anyhow::Result<Vec<(String, String)>> {
        let mut con = self.client.get_connection()?;
        let result: Vec<(String, String)> = redis::cmd("HGETALL")
            .arg(format!("claude:room:{}:sessions", self.room))
            .query(&mut con)?;
        Ok(result)
    }

    pub fn list_rooms(&self) -> anyhow::Result<Vec<(String, String)>> {
        let mut con = self.client.get_connection()?;
        let result: Vec<(String, String)> = redis::cmd("HGETALL")
            .arg("claude:rooms")
            .query(&mut con)?;
        Ok(result)
    }
}
