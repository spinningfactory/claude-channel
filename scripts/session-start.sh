#!/bin/bash
# Launches a Claude Code session with multi-session coordination.
#
# The channel binary handles registration, deregistration, pub/sub subscriptions,
# and exposes publish/list_sessions MCP tools — no hook scripts needed.
#
# Usage:
#   ./scripts/session-start.sh <session-name> <goal> [project-dir]
#
# Examples:
#   ./scripts/session-start.sh auth-refactor "refactoring auth middleware" ~/myproject
#   ./scripts/session-start.sh frontend "building the new dashboard" .

set -euo pipefail

if [ $# -lt 2 ]; then
    echo "Usage: $0 <session-name> <goal> [project-dir]"
    echo ""
    echo "  session-name  Short identifier (e.g. auth-refactor, frontend, api-tests)"
    echo "  goal          What this session is working on"
    echo "  project-dir   Working directory (default: current dir)"
    exit 1
fi

SESSION_NAME="$1"
SESSION_GOAL="$2"
PROJECT_DIR="${3:-.}"
REDIS_PORT="${REDIS_PORT:-16379}"
CHANNEL_BINARY="${CHANNEL_BINARY:-$(dirname "$0")/../target/release/claude-channel}"

# Resolve paths
PROJECT_DIR=$(cd "$PROJECT_DIR" && pwd)
CHANNEL_BINARY=$(realpath "$CHANNEL_BINARY" 2>/dev/null || echo "$CHANNEL_BINARY")

# Pick a unique webhook port based on session name hash
PORT=$((8800 + $(echo -n "$SESSION_NAME" | cksum | cut -d' ' -f1) % 100))

# Generate session config
SESSION_DIR="/tmp/claude-sessions/$SESSION_NAME"
mkdir -p "$SESSION_DIR"

cat > "$SESSION_DIR/channel-config.yaml" <<YAML
server_name: "$SESSION_NAME"

coordination:
  url: "redis://localhost:$REDIS_PORT"
  goal: "$SESSION_GOAL"

sources:
  - type: webhook
    port: $PORT
    bind: "127.0.0.1"
YAML

# Generate per-session CLAUDE.md
cat > "$SESSION_DIR/CLAUDE.md" <<INSTRUCTIONS
# Multi-Session Coordination Protocol

You are session "$SESSION_NAME". Your goal: $SESSION_GOAL

You are part of a multi-session Claude Code setup. Other sessions are working
on different tasks in parallel.

## Tools available

- **publish** — send a message to other sessions via Redis
- **list_sessions** — see who's online and what they're working on

## Channels

- \`claude:questions\` — ask for help (all sessions see it)
- \`claude:session:<name>\` — DM a specific session
- \`claude:lobby\` — broadcast announcements

## Rules of engagement

1. **Don't butt in.** When you see a question, ONLY respond if you have direct,
   concrete knowledge. "I might know" is NOT enough.

2. **Ignore lobby messages.** Join/leave notifications are informational.
   Don't respond unless explicitly asked.

3. **Be brief.** Short factual answers. Don't elaborate unless asked.

4. **Am I the best session?** If another session is closer to the topic, stay quiet.

5. **Stay focused.** Your primary job is YOUR task. Helping others is secondary.
INSTRUCTIONS

# Generate .mcp.json for this session
cat > "$SESSION_DIR/mcp.json" <<MCP
{
  "mcpServers": {
    "$SESSION_NAME": {
      "command": "$CHANNEL_BINARY",
      "args": ["$SESSION_DIR/channel-config.yaml"]
    }
  }
}
MCP

echo "=== Session: $SESSION_NAME ==="
echo "Goal: $SESSION_GOAL"
echo "Webhook port: $PORT"
echo "Config dir: $SESSION_DIR"
echo "Project dir: $PROJECT_DIR"
echo ""
echo "Starting Claude Code..."
echo ""

# Copy CLAUDE.md into the project dir (back it up if exists)
if [ -f "$PROJECT_DIR/CLAUDE.md" ]; then
    cp "$PROJECT_DIR/CLAUDE.md" "$SESSION_DIR/CLAUDE.md.project-backup"
    echo "" >> "$PROJECT_DIR/CLAUDE.md"
    cat "$SESSION_DIR/CLAUDE.md" >> "$PROJECT_DIR/CLAUDE.md"
    RESTORE_CLAUDE_MD=1
else
    cp "$SESSION_DIR/CLAUDE.md" "$PROJECT_DIR/CLAUDE.md"
    RESTORE_CLAUDE_MD=0
fi

cleanup() {
    if [ "${RESTORE_CLAUDE_MD:-0}" = "1" ]; then
        cp "$SESSION_DIR/CLAUDE.md.project-backup" "$PROJECT_DIR/CLAUDE.md"
    else
        rm -f "$PROJECT_DIR/CLAUDE.md"
    fi
}
trap cleanup EXIT

# Launch Claude Code
cd "$PROJECT_DIR"
claude \
    --dangerously-load-development-channels "server:$SESSION_NAME" \
    --mcp-config "$SESSION_DIR/mcp.json"
