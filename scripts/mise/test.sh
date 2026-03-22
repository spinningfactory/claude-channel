#!/usr/bin/env bash
set -euo pipefail

BINARY="./target/release/claude-channel"
PORT=8799
STDOUT_LOG="/tmp/channel-test-stdout.log"
STDERR_LOG="/tmp/channel-test-stderr.log"

cat > /tmp/test-webhook.yaml <<YAML
server_name: "test"
sources:
  - type: webhook
    port: $PORT
    bind: "127.0.0.1"
YAML

fuser -k $PORT/tcp 2>/dev/null || true

FIFO=$(mktemp -u)
mkfifo "$FIFO"
"$BINARY" /tmp/test-webhook.yaml < "$FIFO" > "$STDOUT_LOG" 2>"$STDERR_LOG" &
PID=$!
exec 3>"$FIFO"

cleanup() { exec 3>&- 2>/dev/null || true; kill $PID 2>/dev/null || true; rm -f "$FIFO" /tmp/test-webhook.yaml; }
trap cleanup EXIT
sleep 0.5

echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' >&3
sleep 0.3

PASS=0 FAIL=0
check() { if "$@"; then echo "  ✓ $DESC"; PASS=$((PASS+1)); else echo "  ✗ $DESC"; FAIL=$((FAIL+1)); fi; }

DESC="claude/channel capability"; check grep -q '"claude/channel"' "$STDOUT_LOG"
DESC="no tools capability";       check sh -c '! grep -q "\"tools\"" "$0"' "$STDOUT_LOG"

echo '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' >&3
sleep 0.1

> "$STDOUT_LOG"
curl -s -X POST http://127.0.0.1:$PORT/test -d 'hello' > /dev/null
sleep 0.3

DESC="webhook notification"; check grep -q 'notifications/claude/channel' "$STDOUT_LOG"
DESC="source metadata";      check grep -q 'source.*webhook' "$STDOUT_LOG"

> "$STDOUT_LOG"
echo '{"jsonrpc":"2.0","id":2,"method":"ping","params":{}}' >&3
sleep 0.2
DESC="ping/pong"; check grep -q '"id":2' "$STDOUT_LOG"

echo ""
[ "$FAIL" -eq 0 ] && echo "ALL $PASS TESTS PASSED" || { echo "$PASS passed, $FAIL failed"; exit 1; }
