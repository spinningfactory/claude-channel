use super::Event;
use futures::StreamExt;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct NatsSource {
    pub url: String,
    pub subjects: Vec<String>,
}

#[async_trait::async_trait]
impl super::EventSource for NatsSource {
    async fn run(self, tx: mpsc::Sender<Event>) -> anyhow::Result<()> {
        let client = async_nats::connect(&self.url).await?;
        eprintln!("[nats] Connected to {}", self.url);

        // Subscribe to all configured subjects
        let mut subscribers = Vec::new();
        for subject in &self.subjects {
            let sub = client.subscribe(subject.clone()).await?;
            eprintln!("[nats] Subscribed to: {}", subject);
            subscribers.push((subject.clone(), sub));
        }

        // Merge all subscriptions into a single stream
        let mut merged = futures::stream::select_all(
            subscribers.into_iter().map(|(subject, sub)| {
                sub.map(move |msg| (subject.clone(), msg))
            }),
        );

        while let Some((subject, msg)) = merged.next().await {
            let payload = String::from_utf8_lossy(&msg.payload).to_string();

            let mut meta = HashMap::new();
            meta.insert("source".to_string(), "nats".to_string());
            meta.insert("subject".to_string(), msg.subject.to_string());

            if let Some(reply) = &msg.reply {
                meta.insert("reply_to".to_string(), reply.to_string());
            }

            eprintln!("[nats] Message on {}: {}", subject, payload);
            let _ = tx.send(Event {
                content: payload,
                meta,
            }).await;
        }

        anyhow::bail!("nats subscription stream ended")
    }
}
