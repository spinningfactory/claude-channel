use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct Event {
    pub content: String,
    pub meta: HashMap<String, String>,
}

#[async_trait::async_trait]
pub trait EventSource: Send {
    async fn run(self, tx: mpsc::Sender<Event>) -> anyhow::Result<()>;
}

pub mod webhook;

#[cfg(feature = "sqs")]
pub mod sqs;

#[cfg(feature = "redis")]
pub mod redis;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "nats")]
pub mod nats;
