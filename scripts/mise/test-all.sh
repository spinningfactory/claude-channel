#!/usr/bin/env bash
set -euo pipefail

BINARY="./target/release/claude-channel"
PORT=8799
STDOUT_LOG="/tmp/channel-test-stdout.log"
STDERR_LOG="/tmp/channel-test-stderr.log"

cat > /tmp/test-all-config.yaml <<YAML
server_name: "test"
sources:
  - type: webhook
    port: $PORT
    bind: "127.0.0.1"
  - type: sqs
    queue_url: "http://localhost:4566/000000000000/test-queue"
    region: "us-east-1"
    wait_seconds: 5
  - type: redis
    url: "redis://localhost:16379"
    mode: pubsub
    channels: ["test-events"]
  - type: postgres
    connection: "host=localhost port=15432 user=channel password=channel dbname=channel"
    channels: ["test_notify"]
  - type: nats
    url: "nats://localhost:14222"
    subjects: ["test-events"]
YAML

fuser -k $PORT/tcp 2>/dev/null || true

FIFO=$(mktemp -u)
mkfifo "$FIFO"
"$BINARY" /tmp/test-all-config.yaml < "$FIFO" > "$STDOUT_LOG" 2>"$STDERR_LOG" &
PID=$!
exec 3>"$FIFO"

cleanup() { exec 3>&- 2>/dev/null || true; kill $PID 2>/dev/null || true; rm -f "$FIFO" /tmp/test-all-config.yaml; }
trap cleanup EXIT
sleep 1

# MCP handshake
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' >&3
echo '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' >&3

# Wait for all sources to connect
for i in $(seq 1 15); do
    READY=0
    grep -q "Listening on http" "$STDERR_LOG" 2>/dev/null && READY=$((READY + 1))
    grep -q "Subscribed to channel" "$STDERR_LOG" 2>/dev/null && READY=$((READY + 1))
    grep -q "Listening on channel" "$STDERR_LOG" 2>/dev/null && READY=$((READY + 1))
    grep -q "Polling queue" "$STDERR_LOG" 2>/dev/null && READY=$((READY + 1))
    grep -q "nats.*Subscribed to" "$STDERR_LOG" 2>/dev/null && READY=$((READY + 1))
    [ "$READY" -ge 5 ] && break
    sleep 1
done

> "$STDOUT_LOG"

# Send events (SQS first — longest latency)
uvx --from awscli aws sqs send-message --endpoint-url http://localhost:4566 \
    --queue-url http://localhost:4566/000000000000/test-queue \
    --message-body 'sqs: test event' --region us-east-1 > /dev/null
curl -s -X POST http://127.0.0.1:$PORT/test -d 'webhook: test event' > /dev/null
redis-cli -p 16379 PUBLISH test-events 'redis: test event' > /dev/null
PGPASSWORD=channel psql -h localhost -p 15432 -U channel -d channel \
    -c "NOTIFY test_notify, 'postgres: test event'" > /dev/null
# NATS protocol: CONNECT then PUB (text-based wire protocol)
printf 'CONNECT {}\r\nPUB test-events 16\r\nnats: test event\r\n' | nc -q 1 localhost 14222 > /dev/null 2>&1

# Wait for all events
for i in $(seq 1 10); do
    COUNT=$(grep -c 'notifications/claude/channel' "$STDOUT_LOG" 2>/dev/null || echo 0)
    [ "$COUNT" -ge 5 ] && break
    sleep 1
done

PASS=0 FAIL=0
for source in webhook redis postgres sqs nats; do
    if grep -q "source.*$source" "$STDOUT_LOG"; then
        echo "  ✓ $source"
        PASS=$((PASS + 1))
    else
        echo "  ✗ $source"
        FAIL=$((FAIL + 1))
    fi
done

echo ""
if [ "$FAIL" -eq 0 ]; then
    echo "ALL $PASS TESTS PASSED"
else
    echo "$PASS passed, $FAIL failed"
    cat "$STDERR_LOG"
    exit 1
fi
