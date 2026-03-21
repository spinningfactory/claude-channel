use super::Event;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio_postgres::NoTls;

pub struct PostgresSource {
    pub connection: String,
    pub channels: Vec<String>,
}

#[async_trait::async_trait]
impl super::EventSource for PostgresSource {
    async fn run(self, tx: mpsc::Sender<Event>) -> anyhow::Result<()> {
        let (client, mut connection) = tokio_postgres::connect(&self.connection, NoTls).await?;

        // Spawn the connection handler — it drives I/O and delivers notifications
        let (notify_tx, mut notify_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            use futures::StreamExt;
            let mut stream = futures::stream::poll_fn(|cx| connection.poll_message(cx));
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(tokio_postgres::AsyncMessage::Notification(n)) => {
                        let _ = notify_tx.send(n);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("[postgres] Connection error: {}", e);
                        break;
                    }
                }
            }
        });

        for channel in &self.channels {
            // Channel names are identifiers, quote them to be safe
            client
                .execute(&format!("LISTEN \"{}\"", channel), &[])
                .await?;
            eprintln!("[postgres] Listening on channel: {}", channel);
        }

        while let Some(notification) = notify_rx.recv().await {
            let mut meta = HashMap::new();
            meta.insert("source".to_string(), "postgres".to_string());
            meta.insert("channel".to_string(), notification.channel().to_string());

            eprintln!(
                "[postgres] Notification on {}: {}",
                notification.channel(),
                notification.payload()
            );
            let _ = tx
                .send(Event {
                    content: notification.payload().to_string(),
                    meta,
                })
                .await;
        }

        anyhow::bail!("postgres connection closed")
    }
}
