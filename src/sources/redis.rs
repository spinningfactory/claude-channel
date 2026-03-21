use super::Event;
use futures::StreamExt;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct RedisSource {
    pub url: String,
    pub mode: crate::config::RedisMode,
    pub channels: Vec<String>,
    pub keys: Vec<String>,
}

#[async_trait::async_trait]
impl super::EventSource for RedisSource {
    async fn run(self, tx: mpsc::Sender<Event>) -> anyhow::Result<()> {
        match self.mode {
            crate::config::RedisMode::Pubsub => self.run_pubsub(tx).await,
            crate::config::RedisMode::Brpop => self.run_brpop(tx).await,
        }
    }
}

impl RedisSource {
    async fn run_pubsub(self, tx: mpsc::Sender<Event>) -> anyhow::Result<()> {
        let client = redis::Client::open(self.url.as_str())?;
        let mut pubsub = client.get_async_pubsub().await?;

        for channel in &self.channels {
            pubsub.subscribe(channel).await?;
            eprintln!("[redis] Subscribed to channel: {}", channel);
        }

        loop {
            let msg: redis::Msg = pubsub.on_message().next().await
                .ok_or_else(|| anyhow::anyhow!("redis pubsub stream ended"))?;

            let payload: String = msg.get_payload()?;
            let channel_name: String = msg.get_channel_name().to_string();

            let mut meta = HashMap::new();
            meta.insert("source".to_string(), "redis".to_string());
            meta.insert("mode".to_string(), "pubsub".to_string());
            meta.insert("channel".to_string(), channel_name.clone());

            eprintln!("[redis] Message on {}: {}", channel_name, payload);
            let _ = tx.send(Event {
                content: payload,
                meta,
            }).await;
        }
    }

    async fn run_brpop(self, tx: mpsc::Sender<Event>) -> anyhow::Result<()> {
        let client = redis::Client::open(self.url.as_str())?;
        let mut con = client.get_multiplexed_async_connection().await?;

        eprintln!("[redis] BRPOP on keys: {:?}", self.keys);

        loop {
            let result: Option<(String, String)> = redis::cmd("BRPOP")
                .arg(&self.keys)
                .arg(0) // block forever
                .query_async(&mut con)
                .await?;

            if let Some((key, value)) = result {
                let mut meta = HashMap::new();
                meta.insert("source".to_string(), "redis".to_string());
                meta.insert("mode".to_string(), "brpop".to_string());
                meta.insert("key".to_string(), key.clone());

                eprintln!("[redis] BRPOP from {}: {}", key, value);
                let _ = tx.send(Event {
                    content: value,
                    meta,
                }).await;
            }
        }
    }
}
