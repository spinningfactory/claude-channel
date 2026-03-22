#!/usr/bin/env bash
set -euo pipefail

BINARY=$(realpath ./target/release/claude-channel)
redis-cli -p 16379 DEL claude:sessions > /dev/null 2>&1 || true

for session in session-alpha session-beta session-gamma; do
    mkdir -p /tmp/claude-sessions/$session
done

cat > /tmp/claude-sessions/session-alpha/config.yaml <<YAML
server_name: "session-alpha"
coordination:
  url: "redis://localhost:16379"
  goal: "working on the Rust channel binary"
sources:
  - type: webhook
    port: 8801
    bind: "127.0.0.1"
YAML

cat > /tmp/claude-sessions/session-beta/config.yaml <<YAML
server_name: "session-beta"
coordination:
  url: "redis://localhost:16379"
  goal: "writing integration tests for the API"
sources:
  - type: webhook
    port: 8802
    bind: "127.0.0.1"
YAML

cat > /tmp/claude-sessions/session-gamma/config.yaml <<YAML
server_name: "session-gamma"
coordination:
  url: "redis://localhost:16379"
  goal: "setting up CI/CD pipeline"
sources:
  - type: webhook
    port: 8803
    bind: "127.0.0.1"
YAML

for s in session-alpha session-beta session-gamma; do
    cat > /tmp/claude-sessions/$s/mcp.json <<JSON
{"mcpServers":{"$s":{"command":"$BINARY","args":["/tmp/claude-sessions/$s/config.yaml"]}}}
JSON
done

tmux kill-session -t claude-multi 2>/dev/null || true
tmux new-session -d -s claude-multi

tmux send-keys -t claude-multi \
  "claude --dangerously-load-development-channels server:session-alpha --mcp-config /tmp/claude-sessions/session-alpha/mcp.json" Enter

tmux split-window -h -t claude-multi
tmux send-keys -t claude-multi \
  "claude --dangerously-load-development-channels server:session-beta --mcp-config /tmp/claude-sessions/session-beta/mcp.json" Enter

tmux split-window -v -t claude-multi
tmux send-keys -t claude-multi \
  "claude --dangerously-load-development-channels server:session-gamma --mcp-config /tmp/claude-sessions/session-gamma/mcp.json" Enter

echo ""
echo "3 sessions launched in tmux."
echo ""
echo "  Attach:  tmux attach -t claude-multi"
echo "  Kill:    tmux kill-session -t claude-multi"
echo "  List:    mise run sessions"
