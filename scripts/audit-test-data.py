#!/usr/bin/env python3
"""Audit test data in Redis for ordering and consistency issues."""

import json
import os
import subprocess
import sys

REDIS_PORT = os.environ.get("REDIS_PORT", "16379")


def redis_cmd(*args):
    """Run a redis-cli command and return stdout."""
    result = subprocess.run(
        ["redis-cli", "-p", REDIS_PORT] + list(args),
        capture_output=True, text=True
    )
    return result.stdout.strip()


def parse_stream_entries(raw):
    """Parse XRANGE output into list of (stream_id, {field: value}) dicts."""
    lines = raw.strip().split("\n") if raw.strip() else []
    entries = []
    i = 0
    while i < len(lines):
        line = lines[i].strip()
        # Stream IDs look like "1234567890-1"
        if line and "-" in line and line.replace("-", "").isdigit():
            stream_id = line
            fields = {}
            i += 1
            while i < len(lines):
                key_line = lines[i].strip()
                if key_line and "-" in key_line and key_line.replace("-", "").isdigit():
                    break  # next entry
                if not key_line:
                    i += 1
                    continue
                key = key_line
                i += 1
                if i < len(lines):
                    val = lines[i].strip()
                    fields[key] = val
                    i += 1
                else:
                    break
            entries.append((stream_id, fields))
        else:
            i += 1
    return entries


def parse_stream_id(sid):
    """Return (timestamp_ms, sequence) from a stream ID like '12345-6'."""
    parts = sid.split("-")
    return int(parts[0]), int(parts[1])


def get_all_room_ids():
    """Get all room IDs from claude:rooms hash."""
    raw = redis_cmd("HKEYS", "claude:rooms")
    return [r for r in raw.split("\n") if r.strip()]


def main():
    failures = []
    passes = []

    def check(name, passed, detail=""):
        if passed:
            passes.append(name)
            print(f"  PASS: {name}")
        else:
            failures.append(name)
            print(f"  FAIL: {name}")
            if detail:
                for line in detail.split("\n"):
                    print(f"        {line}")

    # --- Check 1: Global stream is in strictly ascending timestamp order ---
    print("\n=== Global Stream (claude:stream) ===")
    global_raw = redis_cmd("XRANGE", "claude:stream", "-", "+")
    global_entries = parse_stream_entries(global_raw)
    print(f"  Total entries: {len(global_entries)}")

    ordering_errors = []
    prev_ts = 0
    for i, (sid, fields) in enumerate(global_entries):
        ts, seq = parse_stream_id(sid)
        if ts < prev_ts:
            ordering_errors.append(
                f"Entry {i}: {sid} (ts={ts}) < prev ts={prev_ts}"
            )
        prev_ts = ts

    check(
        "Global stream timestamps in ascending order",
        len(ordering_errors) == 0,
        "\n".join(ordering_errors[:10])
    )

    # Check sequence numbers are monotonically increasing
    seq_errors = []
    prev_seq = 0
    for i, (sid, fields) in enumerate(global_entries):
        ts, seq = parse_stream_id(sid)
        if seq <= prev_seq and i > 0:
            # Only an error if timestamp is the same as previous
            prev_ts2, _ = parse_stream_id(global_entries[i-1][0])
            if ts == prev_ts2:
                seq_errors.append(
                    f"Entry {i}: seq {seq} <= prev seq {prev_seq} at same ts={ts}"
                )
        prev_seq = seq

    check(
        "Global stream sequence numbers are monotonically increasing",
        len(seq_errors) == 0,
        "\n".join(seq_errors[:10])
    )

    # Build a set of all global stream entries for cross-referencing
    global_event_set = set()
    for sid, fields in global_entries:
        key = (fields.get("channel", ""), fields.get("payload", ""), fields.get("room", ""))
        global_event_set.add(key)

    # --- Check 2: Per-room stream checks ---
    room_ids = get_all_room_ids()
    print(f"\n=== Room Streams ({len(room_ids)} rooms) ===")

    for room_id in sorted(room_ids):
        stream_key = f"claude:room:{room_id}:stream"
        raw = redis_cmd("XRANGE", stream_key, "-", "+")
        entries = parse_stream_entries(raw)
        print(f"\n  --- {room_id} ({len(entries)} events) ---")

        # Check room stream is in ascending timestamp order
        room_order_errors = []
        prev_ts = 0
        for i, (sid, fields) in enumerate(entries):
            ts, seq = parse_stream_id(sid)
            if ts < prev_ts:
                room_order_errors.append(
                    f"Entry {i}: {sid} (ts={ts}) < prev ts={prev_ts}"
                )
            prev_ts = ts

        check(
            f"[{room_id}] timestamps in ascending order",
            len(room_order_errors) == 0,
            "\n".join(room_order_errors[:5])
        )

        # Parse events and check join/message/leave ordering
        # Track per-session state: currently_joined tracks active sessions,
        # ever_joined tracks all sessions that have ever joined.
        # A session can leave and rejoin (e.g., charlie in bugfix-prod).
        currently_joined = set()  # sessions currently in the room
        ever_joined = set()       # sessions that have ever joined
        join_before_msg_errors = []
        join_before_leave_errors = []
        msg_before_leave_errors = []

        # Track message timestamps per session per "stint" (join->leave interval)
        # When a session re-joins, we reset their message tracking
        session_msg_ts_since_join = {}  # session -> list of msg timestamps since last join

        for i, (sid, fields) in enumerate(entries):
            ts, seq = parse_stream_id(sid)
            payload_str = fields.get("payload", "{}")
            channel = fields.get("channel", "")
            try:
                payload = json.loads(payload_str)
            except json.JSONDecodeError:
                payload = {}

            event_type = payload.get("type", "")
            session = payload.get("session", "")
            sender = payload.get("from", "")

            if event_type == "join":
                currently_joined.add(session)
                ever_joined.add(session)
                session_msg_ts_since_join[session] = []
            elif event_type == "leave":
                if session not in ever_joined:
                    join_before_leave_errors.append(
                        f"Entry {i}: '{session}' left but never joined (sid={sid})"
                    )
                elif session not in currently_joined:
                    join_before_leave_errors.append(
                        f"Entry {i}: '{session}' left but not currently joined (sid={sid})"
                    )
                else:
                    # Check that no messages from this session have ts > leave ts
                    # (within this stint)
                    for msg_ts in session_msg_ts_since_join.get(session, []):
                        if msg_ts > ts:
                            msg_before_leave_errors.append(
                                f"'{session}' has message at {msg_ts} after leave at {ts}"
                            )
                currently_joined.discard(session)
            elif sender:
                # It's a message (broadcast or DM)
                if sender not in currently_joined:
                    join_before_msg_errors.append(
                        f"Entry {i}: '{sender}' sent message but not currently joined (sid={sid})"
                    )
                else:
                    session_msg_ts_since_join.setdefault(sender, []).append(ts)

        check(
            f"[{room_id}] joins come before messages",
            len(join_before_msg_errors) == 0,
            "\n".join(join_before_msg_errors[:5])
        )

        check(
            f"[{room_id}] joins come before leaves",
            len(join_before_leave_errors) == 0,
            "\n".join(join_before_leave_errors[:5])
        )

        check(
            f"[{room_id}] messages come before leaves",
            len(msg_before_leave_errors) == 0,
            "\n".join(msg_before_leave_errors[:5])
        )

        # Check that every event in this room stream also exists in global stream
        missing_from_global = []
        for sid, fields in entries:
            key = (fields.get("channel", ""), fields.get("payload", ""), fields.get("room", room_id))
            # Also try without room field since room might be stored differently
            key_with_room = (fields.get("channel", ""), fields.get("payload", ""), room_id)
            if key not in global_event_set and key_with_room not in global_event_set:
                missing_from_global.append(
                    f"{sid}: channel={fields.get('channel', '')} not found in global stream"
                )

        check(
            f"[{room_id}] all events exist in global stream",
            len(missing_from_global) == 0,
            "\n".join(missing_from_global[:5])
        )

    # --- Summary ---
    print(f"\n{'='*50}")
    print(f"Results: {len(passes)} passed, {len(failures)} failed")
    if failures:
        print("\nFailed checks:")
        for f in failures:
            print(f"  - {f}")
        sys.exit(1)
    else:
        print("\nAll checks passed!")
        sys.exit(0)


if __name__ == "__main__":
    main()
