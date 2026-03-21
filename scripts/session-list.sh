#!/bin/bash
# List all active Claude Code sessions registered in Redis.
#
# Usage: ./scripts/session-list.sh

REDIS_PORT="${REDIS_PORT:-16379}"

SESSIONS=$(redis-cli -p "$REDIS_PORT" HGETALL claude:sessions 2>/dev/null)

if [ -z "$SESSIONS" ]; then
    echo "No active sessions"
    exit 0
fi

echo "Active sessions:"
echo ""

# HGETALL returns key\nvalue\nkey\nvalue...
echo "$SESSIONS" | while IFS= read -r name; do
    IFS= read -r data || break
    goal=$(echo "$data" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("goal","(unknown)"))' 2>/dev/null || echo "$data")
    printf "  %-20s %s\n" "$name" "$goal"
done
