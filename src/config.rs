use serde::Deserialize;

fn default_server_name() -> String {
    "claude-channel".to_string()
}

fn default_bind() -> String {
    "127.0.0.1".to_string()
}

#[cfg(feature = "sqs")]
fn default_wait_seconds() -> i32 {
    20
}

#[cfg(feature = "sqs")]
fn default_max_messages() -> i32 {
    10
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_server_name")]
    pub server_name: String,
    pub instructions: Option<String>,
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
    #[cfg(feature = "redis")]
    pub coordination: Option<CoordinationConfig>,
}

#[cfg(feature = "redis")]
#[derive(Debug, Deserialize)]
pub struct CoordinationConfig {
    pub url: String,
    pub goal: String,
    pub room: Option<String>,
}

#[cfg(feature = "redis")]
impl CoordinationConfig {
    pub fn room_id(&self) -> &str {
        self.room.as_deref().unwrap_or("default")
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SourceConfig {
    Webhook {
        port: u16,
        #[serde(default = "default_bind")]
        bind: String,
    },

    #[cfg(feature = "sqs")]
    Sqs {
        queue_url: String,
        region: Option<String>,
        #[serde(default = "default_wait_seconds")]
        wait_seconds: i32,
        #[serde(default = "default_max_messages")]
        max_messages: i32,
    },

    #[cfg(feature = "redis")]
    Redis {
        url: String,
        mode: RedisMode,
        #[serde(default)]
        channels: Vec<String>,
        #[serde(default)]
        keys: Vec<String>,
    },

    #[cfg(feature = "postgres")]
    Postgres {
        connection: String,
        channels: Vec<String>,
    },

    #[cfg(feature = "nats")]
    Nats {
        url: String,
        subjects: Vec<String>,
    },
}

#[cfg(feature = "redis")]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RedisMode {
    Pubsub,
    Brpop,
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = serde_yaml::from_str(&content)?;
        Ok(config)
    }
}
