#!/bin/bash
# Launches 3 Claude Code sessions in a tmux window for testing coordination.
# Run this from any terminal — it creates a new tmux session with 3 panes.

set -euo pipefail

SESSION="claude-multi"

# Kill existing tmux session if any
tmux kill-session -t "$SESSION" 2>/dev/null || true

tmux new-session -d -s "$SESSION" -n "sessions"

# Pane 0: session-alpha
tmux send-keys -t "$SESSION:0.0" \
  "claude --dangerously-load-development-channels server:session-alpha --mcp-config /tmp/claude-sessions/session-alpha/mcp.json" Enter

# Split horizontally for pane 1: session-beta
tmux split-window -h -t "$SESSION:0"
tmux send-keys -t "$SESSION:0.1" \
  "claude --dangerously-load-development-channels server:session-beta --mcp-config /tmp/claude-sessions/session-beta/mcp.json" Enter

# Split pane 1 vertically for pane 2: session-gamma
tmux split-window -v -t "$SESSION:0.1"
tmux send-keys -t "$SESSION:0.2" \
  "claude --dangerously-load-development-channels server:session-gamma --mcp-config /tmp/claude-sessions/session-gamma/mcp.json" Enter

echo "tmux session '$SESSION' created with 3 panes."
echo ""
echo "Attach with:  tmux attach -t $SESSION"
echo "Kill with:    tmux kill-session -t $SESSION"
