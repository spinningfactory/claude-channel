use super::Event;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct SqsSource {
    pub queue_url: String,
    pub region: Option<String>,
    pub wait_seconds: i32,
    pub max_messages: i32,
}

#[async_trait::async_trait]
impl super::EventSource for SqsSource {
    async fn run(self, tx: mpsc::Sender<Event>) -> anyhow::Result<()> {
        let mut config_loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
        if let Some(ref region) = self.region {
            config_loader = config_loader.region(aws_config::Region::new(region.clone()));
        }
        let aws_config = config_loader.load().await;
        let client = aws_sdk_sqs::Client::new(&aws_config);

        eprintln!("[sqs] Polling queue: {}", self.queue_url);

        loop {
            let result = client
                .receive_message()
                .queue_url(&self.queue_url)
                .wait_time_seconds(self.wait_seconds)
                .max_number_of_messages(self.max_messages)
                .send()
                .await;

            match result {
                Ok(output) => {
                    for msg in output.messages() {
                        let body = msg.body().unwrap_or("");
                        let message_id = msg.message_id().unwrap_or("unknown");

                        let mut meta = HashMap::new();
                        meta.insert("source".to_string(), "sqs".to_string());
                        meta.insert("message_id".to_string(), message_id.to_string());
                        meta.insert("queue_url".to_string(), self.queue_url.clone());

                        eprintln!("[sqs] Message {}: {}", message_id, body);
                        let _ = tx.send(Event {
                            content: body.to_string(),
                            meta,
                        }).await;

                        // Delete the message after forwarding
                        if let Some(receipt) = msg.receipt_handle() {
                            let _ = client
                                .delete_message()
                                .queue_url(&self.queue_url)
                                .receipt_handle(receipt)
                                .send()
                                .await;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[sqs] Error receiving messages: {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }
}
