 claude-channel

A configurable MCP channel server for [Claude Code](https://claude.ai/code). Aggregates events from multiple sources and pushes them into a running Claude Code session as channel notifications.

## Sources

| Source | Description | Feature flag |
|--------|-------------|-------------|
| Webhook | HTTP POST listener | `webhook` (default) |
| SQS | AWS SQS long-polling | `sqs` |
| Redis | Pub/sub or BRPOP | `redis` |
| PostgreSQL | LISTEN/NOTIFY | `postgres` |
| NATS | Subject subscription | `nats` |

## Quick start

```bash
cargo build --release
./target/release/claude-channel examples/webhook.yaml
```

Register in `.mcp.json`:

```json
{
  "mcpServers": {
    "my-channel": {
      "command": "/path/to/claude-channel",
      "args": ["/path/to/config.yaml"]
    }
  }
}
```

Start Claude Code:

```bash
claude --dangerously-load-development-channels server:my-channel
```

Send an event:

```bash
curl -X POST http://127.0.0.1:8788 -d 'deploy failed on staging'
```

## Configuration

The binary takes a YAML config file as its first argument (or via `CHANNEL_CONFIG` env var).

```yaml
server_name: "my-channel"
instructions: "Events arrive as <channel source=\"my-channel\" ...>."

sources:
  - type: webhook
    port: 8788
    bind: "127.0.0.1"          # optional, default 127.0.0.1

  - type: sqs
    queue_url: "https://sqs.us-east-1.amazonaws.com/123/my-queue"
    region: "us-east-1"        # optional, uses AWS env
    wait_seconds: 20           # optional, default 20
    max_messages: 10           # optional, default 10

  - type: redis
    url: "redis://127.0.0.1:6379"
    mode: "pubsub"             # "pubsub" or "brpop"
    channels: ["events"]       # for pubsub
    keys: ["queue"]            # for brpop

  - type: postgres
    connection: "host=localhost user=app dbname=mydb"
    channels: ["events"]

  - type: nats
    url: "nats://127.0.0.1:4222"
    subjects: ["events.>", "alerts.*"]  # NATS wildcards supported
```

See `examples/` for ready-to-use configs.

## Building

```bash
# Webhook only (default)
cargo build --release

# All sources
cargo build --release --all-features

# Specific sources
cargo build --release --features "webhook,redis"
```

## Development

### Prerequisites

- Rust toolchain
- [mise](https://mise.jdx.dev) (task runner)
- [uv](https://docs.astral.sh/uv/) (for AWS CLI via `uvx`)
- Docker (for integration tests)

### Tasks

```bash
mise run build      # Build release binary with all features
mise run test       # Quick webhook-only test (no containers)
mise run test-all   # Full integration test (all sources with containers)
mise run up         # Start test infrastructure
mise run down       # Stop test infrastructure
mise run clean      # Stop infrastructure + cargo clean

# Multi-session coordination
mise run session -- <name> <goal> [project-dir]
mise run sessions   # List active sessions
```

### Multi-session coordination

Run multiple Claude Code sessions that can discover each other and communicate
via Redis pub/sub. Each session registers its goal, and only responds to messages
that are directly relevant to its work.

```bash
# Start infrastructure (Redis required)
mise run up

# Launch sessions (each in its own terminal)
mise run session -- auth-refactor "refactoring auth middleware" ~/myproject
mise run session -- frontend "building the new dashboard" ~/myproject
mise run session -- api-tests "writing integration tests for the API" ~/myproject

# Check who's online
mise run sessions
```

The channel binary handles everything automatically:
- **Registration** in Redis on startup, deregistration on exit
- **Subscribes** to `claude:lobby`, `claude:questions`, and `claude:session:<name>`
- **Exposes MCP tools**: `publish` (send messages) and `list_sessions` (see who's online)

Sessions are instructed to **only respond when they have direct, concrete knowledge**
relevant to the question. They won't butt in with unsolicited advice.

Add coordination to any config with:

```yaml
coordination:
  url: "redis://localhost:16379"
  goal: "what this session is working on"
```

### Project structure

```
src/
  main.rs             Config loading, source dispatch, mpsc wiring
  config.rs           YAML config structs
  mcp.rs              MCP JSON-RPC protocol (one-way, no tools)
  sources/
    mod.rs             Event struct + EventSource trait
    webhook.rs         HTTP POST listener
    sqs.rs             AWS SQS long-polling
    redis.rs           Redis pub/sub + BRPOP
    postgres.rs        PostgreSQL LISTEN/NOTIFY
    nats.rs            NATS subject subscription
examples/
  webhook.yaml         Minimal webhook-only config
  all-sources.yaml     Config with all 5 source types
scripts/
  session-start.sh     Launch a coordinated Claude Code session
  session-list.sh      List active sessions
.devcontainer/         Docker Compose services for testing
```

### Architecture

Each configured source runs as an independent tokio task. All sources send `Event` objects through a bounded `mpsc` channel to a single notification emitter that writes MCP JSON-RPC notifications to stdout. The MCP stdin handler runs on a blocking thread to avoid starving the async runtime.

```
[source 1] ──┐
[source 2] ──┤── mpsc(256) ──> [emitter] ──> stdout (MCP notifications)
[source 3] ──┤
[source N] ──┘

stdin (MCP JSON-RPC) ──> [handler] ──> stdout (responses)
```
