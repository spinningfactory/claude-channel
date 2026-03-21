#!/bin/bash
set -euo pipefail

REDIS_PORT="${REDIS_PORT:-16379}"
R="redis-cli -p $REDIS_PORT"

# Sequence counter for sub-millisecond ordering
SEQ=0

ts() {
  # Convert a datetime string to milliseconds
  echo $(( $(date -d "$1" +%s) * 1000 ))
}

add_room() {
  local room_id="$1" name="$2" created_at="$3" created_by="$4"
  local date_part="${created_at%% *}"
  $R HSET claude:rooms "$room_id" "{\"name\":\"$name\",\"created_at\":\"$created_at\",\"created_by\":\"$created_by\"}" > /dev/null
  $R SADD "claude:rooms:by_date:$date_part" "$room_id" > /dev/null
  $R HSET "claude:room:${room_id}:meta" name "$name" created_at "$created_at" created_by "$created_by" > /dev/null
}

add_event() {
  local room_id="$1" timestamp="$2" channel="$3" payload="$4"
  SEQ=$((SEQ + 1))
  local stream_id="${timestamp}-${SEQ}"
  $R XADD "claude:room:${room_id}:stream" "$stream_id" \
    channel "$channel" \
    payload "$payload" \
    room "$room_id" > /dev/null
  $R XADD "claude:stream" "$stream_id" \
    channel "$channel" \
    payload "$payload" \
    room "$room_id" > /dev/null
}

join_session() {
  local room_id="$1" session="$2" timestamp="$3" goal="${4:-}"
  local payload="{\"type\":\"join\",\"session\":\"${session}\",\"goal\":\"${goal}\"}"
  add_event "$room_id" "$timestamp" "claude:room:${room_id}:lobby" "$payload"
}

leave_session() {
  local room_id="$1" session="$2" timestamp="$3"
  local payload="{\"type\":\"leave\",\"session\":\"${session}\"}"
  add_event "$room_id" "$timestamp" "claude:room:${room_id}:lobby" "$payload"
}

broadcast() {
  local room_id="$1" from="$2" body="$3" timestamp="$4"
  # Escape double quotes in body for JSON
  local escaped_body="${body//\\/\\\\}"
  escaped_body="${escaped_body//\"/\\\"}"
  local payload="{\"from\":\"${from}\",\"body\":\"${escaped_body}\"}"
  add_event "$room_id" "$timestamp" "claude:room:${room_id}:questions" "$payload"
}

dm() {
  local room_id="$1" from="$2" to="$3" body="$4" timestamp="$5"
  local escaped_body="${body//\\/\\\\}"
  escaped_body="${escaped_body//\"/\\\"}"
  local payload="{\"from\":\"${from}\",\"to\":\"${to}\",\"body\":\"${escaped_body}\"}"
  add_event "$room_id" "$timestamp" "claude:room:${room_id}:session:${to}" "$payload"
}

register_active_session() {
  local room_id="$1" session="$2" joined_at="$3"
  $R HSET "claude:room:${room_id}:sessions" "$session" "$joined_at" > /dev/null
}

# ---------------------------------------------------------------------------
echo "Clearing existing data..."
KEYS=$($R KEYS 'claude:*')
if [ -n "$KEYS" ]; then
  echo "$KEYS" | xargs $R DEL > /dev/null
fi

# ===========================
echo "Generating Day 1 (2026-03-19)..."
# ===========================

# --- morning-standup (9:00-9:30) ---
ROOM="morning-standup"
add_room "$ROOM" "Morning Standup" "2026-03-19 09:00:00" "alice"

T=$(ts "2026-03-19 09:00:00")
join_session "$ROOM" alice  "$T" "morning sync"
T=$((T + 5000))
join_session "$ROOM" bob    "$T" "morning sync"
T=$((T + 3000))
join_session "$ROOM" charlie "$T" "morning sync"
T=$((T + 2000))
join_session "$ROOM" diana  "$T" "morning sync"
T=$((T + 4000))
join_session "$ROOM" eve    "$T" "morning sync"

T=$(ts "2026-03-19 09:01:00")
broadcast "$ROOM" alice "Good morning everyone! Let's get started. Alice here — yesterday I finished the auth middleware refactor, PR #42 is ready for review." "$T"
T=$((T + 30000))
broadcast "$ROOM" bob "Morning! I spent most of yesterday on the database migration scripts. Got the NULL handling sorted out for existing rows." "$T"
T=$((T + 25000))
broadcast "$ROOM" charlie "Hey all. I was debugging that Redis connection pool issue in staging. Still haven't found the root cause." "$T"
T=$((T + 20000))
broadcast "$ROOM" diana "I wrapped up the API docs for the new endpoints. Also started looking at the rate limiter config." "$T"
T=$((T + 35000))
broadcast "$ROOM" eve "Morning! I paired with charlie on the Redis thing for a bit, then moved to writing integration tests for the webhook handler." "$T"

T=$(ts "2026-03-19 09:05:00")
broadcast "$ROOM" alice "Charlie, is the connection pool issue the one that maxes out at 10 connections?" "$T"
T=$((T + 15000))
broadcast "$ROOM" charlie "Yeah exactly. It works fine locally but under load in staging it just saturates. I suspect the connection string is missing the pool_size param." "$T"
T=$((T + 20000))
broadcast "$ROOM" bob "I ran into something similar last month. Check if the idle timeout is set — without it connections never get recycled." "$T"
T=$((T + 18000))
broadcast "$ROOM" charlie "Oh good call, I'll check that. What was the default idle timeout you used?" "$T"
T=$((T + 12000))
broadcast "$ROOM" bob "We went with 300 seconds. Seemed to strike a good balance." "$T"

T=$(ts "2026-03-19 09:10:00")
broadcast "$ROOM" diana "Alice, I had a question about PR #42 — does the new middleware handle the case where the JWT is expired but the refresh token is still valid?" "$T"
T=$((T + 22000))
broadcast "$ROOM" alice "Yes! There's a fallback path that checks the refresh token. It's in the token_validator module, around line 85." "$T"
T=$((T + 15000))
broadcast "$ROOM" eve "Nice. I'll need that for the webhook auth too. Can I reuse that module?" "$T"
T=$((T + 10000))
broadcast "$ROOM" alice "Absolutely, it's designed to be shared. Just import TokenValidator and call validate_or_refresh()." "$T"

T=$(ts "2026-03-19 09:15:00")
broadcast "$ROOM" diana "Today I'm planning to finish the rate limiter and start on the monitoring dashboard." "$T"
T=$((T + 18000))
broadcast "$ROOM" eve "I should have the webhook tests done by lunch, then I'll pick up the error reporting task." "$T"
T=$((T + 25000))
broadcast "$ROOM" bob "I'll continue with migrations. There are three more tables that need the schema update." "$T"
T=$((T + 20000))
broadcast "$ROOM" charlie "I'll keep digging into the Redis issue. If I can't figure it out by noon I might need to pair with someone." "$T"
T=$((T + 15000))
broadcast "$ROOM" alice "Happy to help after lunch if you need it, charlie. Alright, anything else before we wrap up?" "$T"
T=$((T + 30000))
broadcast "$ROOM" bob "Nothing from me. Let's ship it." "$T"

T=$(ts "2026-03-19 09:25:00")
leave_session "$ROOM" alice   "$T"
T=$((T + 8000))
leave_session "$ROOM" bob     "$T"
T=$((T + 5000))
leave_session "$ROOM" charlie "$T"
T=$((T + 12000))
leave_session "$ROOM" diana   "$T"
T=$((T + 6000))
leave_session "$ROOM" eve     "$T"

# --- auth-refactor (10:00-15:00) ---
ROOM="auth-refactor"
add_room "$ROOM" "Auth Refactor" "2026-03-19 10:00:00" "alice"

T=$(ts "2026-03-19 10:00:00")
join_session "$ROOM" alice "$T" "refactor auth module"
T=$((T + 10000))
join_session "$ROOM" bob   "$T" "help with auth"

T=$(ts "2026-03-19 10:02:00")
broadcast "$ROOM" alice "Alright bob, I've been thinking about the auth architecture. Currently we have three separate middleware layers and they don't share state." "$T"
T=$((T + 45000))
broadcast "$ROOM" bob "Yeah I noticed. The session middleware re-validates the JWT even though the auth middleware already did it." "$T"
T=$((T + 30000))
broadcast "$ROOM" alice "Exactly. My proposal is to unify them into a single AuthContext that gets populated once and passed through." "$T"
T=$((T + 60000))
dm "$ROOM" alice bob "Between us — I think the original author didn't understand the middleware chain. The code has some... interesting choices." "$T"
T=$((T + 20000))
dm "$ROOM" bob alice "Ha, yeah. I saw the comment that says 'TODO: figure out why this works'. Classic." "$T"

T=$(ts "2026-03-19 10:15:00")
broadcast "$ROOM" bob "Should we use a trait for the auth provider so we can swap implementations for testing?" "$T"
T=$((T + 35000))
broadcast "$ROOM" alice "Yes, definitely. I'm thinking AuthProvider trait with verify() and refresh() methods." "$T"
T=$((T + 50000))
broadcast "$ROOM" bob "Makes sense. What about the RBAC checks? Those are currently scattered across every handler." "$T"
T=$((T + 40000))
broadcast "$ROOM" alice "Good point. Let's add a permissions() method to the trait that returns a PermissionSet. Then handlers just check context.can(Action::Write)." "$T"

T=$(ts "2026-03-19 10:30:00")
broadcast "$ROOM" bob "I started sketching out the trait definition. Take a look:" "$T"
T=$((T + 20000))
broadcast "$ROOM" bob "trait AuthProvider { fn verify(&self, token: &str) -> Result<Claims>; fn refresh(&self, token: &str) -> Result<TokenPair>; fn permissions(&self, claims: &Claims) -> PermissionSet; }" "$T"
T=$((T + 45000))
broadcast "$ROOM" alice "Love it. Clean and minimal. Let's also add an async variant since the DB lookup for permissions will be async." "$T"

T=$(ts "2026-03-19 11:00:00")
broadcast "$ROOM" alice "I'm going to start implementing the JwtAuthProvider. Bob, can you work on the mock provider for tests?" "$T"
T=$((T + 25000))
broadcast "$ROOM" bob "On it. I'll also set up the test fixtures we'll need." "$T"

T=$(ts "2026-03-19 11:30:00")
join_session "$ROOM" charlie "$T" "reviewing auth changes"
T=$((T + 15000))
broadcast "$ROOM" charlie "Hey, just joined to catch up. What's the current approach?" "$T"
T=$((T + 40000))
broadcast "$ROOM" alice "We're unifying the three auth middlewares into a single AuthContext with a trait-based provider. Check the shared doc I linked in the standup." "$T"
T=$((T + 30000))
broadcast "$ROOM" charlie "Got it. I like the trait approach. One thing though — the OAuth flow has some quirks. The state parameter handling is different for mobile vs web." "$T"
T=$((T + 55000))
broadcast "$ROOM" alice "Good catch. We should probably have platform-specific provider implementations then." "$T"

T=$(ts "2026-03-19 12:00:00")
dm "$ROOM" alice bob "I found a bug in the current token rotation logic. It doesn't invalidate the old refresh token." "$T"
T=$((T + 30000))
dm "$ROOM" bob alice "That's a security issue. Should we flag it separately?" "$T"
T=$((T + 20000))
dm "$ROOM" alice bob "Yeah, I'll file a security advisory. Let's fix it as part of this refactor." "$T"

T=$(ts "2026-03-19 12:15:00")
broadcast "$ROOM" bob "Found an interesting edge case: if the user's role changes while they have an active session, the cached permissions are stale." "$T"
T=$((T + 35000))
broadcast "$ROOM" charlie "We had a ticket about that last sprint. The current workaround is a 5-minute cache TTL but it's not great." "$T"
T=$((T + 40000))
broadcast "$ROOM" alice "Let's use an event-driven approach. When roles change, publish an event and invalidate the cache." "$T"

T=$(ts "2026-03-19 13:00:00")
leave_session "$ROOM" bob "$(ts '2026-03-19 13:00:00')"

T=$(ts "2026-03-19 13:15:00")
broadcast "$ROOM" charlie "Bob left? I had a question about the mock provider setup." "$T"
T=$((T + 25000))
broadcast "$ROOM" alice "He had another meeting. I can probably help — what's the question?" "$T"
T=$((T + 50000))
broadcast "$ROOM" charlie "How should the mock handle expired tokens? Return an error or auto-refresh?" "$T"
T=$((T + 30000))
broadcast "$ROOM" alice "Return an error. The middleware should handle the refresh logic, not the provider. That way we test the retry path too." "$T"

T=$(ts "2026-03-19 14:00:00")
broadcast "$ROOM" alice "Making good progress. The basic flow works end to end now. JWT verify -> extract claims -> load permissions -> attach to request context." "$T"
T=$((T + 60000))
broadcast "$ROOM" charlie "Nice! I'll start updating the handlers to use the new context instead of re-parsing the token." "$T"
T=$((T + 45000))
broadcast "$ROOM" alice "Perfect. There are about 15 handlers that need updating. Most are straightforward — just replace the manual token parsing." "$T"

T=$(ts "2026-03-19 14:30:00")
broadcast "$ROOM" charlie "Found it — three handlers were doing their own JWT validation with a different secret key. No wonder we had inconsistent behavior." "$T"
T=$((T + 25000))
broadcast "$ROOM" alice "Wow. That explains the intermittent 401s users reported. Everything should go through the same provider now." "$T"

T=$(ts "2026-03-19 15:00:00")
leave_session "$ROOM" charlie "$(ts '2026-03-19 15:00:00')"
T=$((T + 15000))
leave_session "$ROOM" alice "$(ts '2026-03-19 15:00:15')"

# --- quick-question (14:00-14:10) ---
ROOM="quick-question"
add_room "$ROOM" "Quick Question" "2026-03-19 14:00:00" "diana"

T=$(ts "2026-03-19 14:00:00")
join_session "$ROOM" diana "$T" "need a config value"
T=$((T + 8000))
join_session "$ROOM" eve "$T" "helping diana"

T=$(ts "2026-03-19 14:01:00")
broadcast "$ROOM" diana "Hey eve, quick question — do you know what the REDIS_MAX_RETRIES env var should be set to in production?" "$T"
T=$((T + 25000))
broadcast "$ROOM" eve "Yeah, we use 3 in prod with a 500ms backoff between retries. It's in the ops runbook under the Redis section." "$T"
T=$((T + 30000))
broadcast "$ROOM" diana "Perfect, thanks! And the REDIS_RETRY_BACKOFF_MS is just a flat delay, not exponential?" "$T"
T=$((T + 20000))
broadcast "$ROOM" eve "Correct, flat delay. We talked about exponential backoff but decided it wasn't worth the complexity for our traffic patterns." "$T"
T=$((T + 15000))
broadcast "$ROOM" diana "Makes sense. Thanks, that's all I needed!" "$T"

T=$(ts "2026-03-19 14:08:00")
leave_session "$ROOM" diana "$T"
T=$((T + 5000))
leave_session "$ROOM" eve "$T"

# ===========================
echo "Generating Day 2 (2026-03-20)..."
# ===========================

# --- sprint-planning (9:00-10:00) ---
ROOM="sprint-planning"
add_room "$ROOM" "Sprint Planning" "2026-03-20 09:00:00" "alice"

T=$(ts "2026-03-20 09:00:00")
join_session "$ROOM" alice   "$T" "sprint planning"
T=$((T + 3000))
join_session "$ROOM" bob     "$T" "sprint planning"
T=$((T + 5000))
join_session "$ROOM" charlie "$T" "sprint planning"
T=$((T + 2000))
join_session "$ROOM" diana   "$T" "sprint planning"
T=$((T + 8000))
join_session "$ROOM" eve     "$T" "sprint planning"
T=$((T + 4000))
join_session "$ROOM" frank   "$T" "sprint planning"

T=$(ts "2026-03-20 09:02:00")
broadcast "$ROOM" alice "Welcome everyone. Sprint 14 planning. We have 6 people and 10 days. Let's review the backlog." "$T"
T=$((T + 40000))
broadcast "$ROOM" alice "Top priority is finishing the auth refactor. Charlie and I made good progress yesterday. Estimating 3 more days." "$T"
T=$((T + 35000))
broadcast "$ROOM" bob "I can pick up the remaining migration scripts. I'd estimate 2 days for the schema changes plus 1 day for testing." "$T"
T=$((T + 30000))
broadcast "$ROOM" diana "The monitoring dashboard is about 60% done. I think 2 days to finish and 1 day for QA." "$T"
T=$((T + 25000))
broadcast "$ROOM" eve "Webhook integration tests are almost done. Half a day to finish, then I can pick up something new." "$T"

T=$(ts "2026-03-20 09:10:00")
broadcast "$ROOM" frank "I'm new to the team so still ramping up. Happy to take smaller tasks or pair with someone." "$T"
T=$((T + 30000))
broadcast "$ROOM" alice "Welcome frank! How about pairing with bob on the migrations? Great way to learn the data model." "$T"
T=$((T + 20000))
broadcast "$ROOM" frank "Sounds great, I'd like that." "$T"
T=$((T + 25000))
broadcast "$ROOM" bob "Works for me. Frank, let's sync after this meeting and I'll walk you through the schema." "$T"

T=$(ts "2026-03-20 09:20:00")
broadcast "$ROOM" charlie "What about the Redis connection pool fix? I think I found the issue yesterday — missing pool_size config." "$T"
T=$((T + 20000))
broadcast "$ROOM" alice "Good point. Can you wrap that up today? Shouldn't take long if you found the root cause." "$T"
T=$((T + 15000))
broadcast "$ROOM" charlie "Yeah, I'll have a PR up by noon. Just need to verify in staging." "$T"

T=$(ts "2026-03-20 09:30:00")
broadcast "$ROOM" diana "Should we add the API versioning task to this sprint? Product has been asking about it." "$T"
T=$((T + 30000))
broadcast "$ROOM" alice "Let's scope it first. Eve, once you finish webhooks, can you write up a technical spec for API versioning?" "$T"
T=$((T + 20000))
broadcast "$ROOM" eve "Sure. I can have a draft by Wednesday." "$T"

T=$(ts "2026-03-20 09:40:00")
broadcast "$ROOM" alice "Let me summarize the assignments:" "$T"
T=$((T + 15000))
broadcast "$ROOM" alice "Alice + Charlie: auth refactor (3 days). Bob + Frank: migrations (3 days). Diana: monitoring dashboard (3 days). Eve: webhooks then API versioning spec. Charlie: Redis fix today." "$T"
T=$((T + 25000))
broadcast "$ROOM" bob "Looks good. Capacity seems right for a 10-day sprint." "$T"
T=$((T + 18000))
broadcast "$ROOM" diana "Agreed. Let's revisit mid-sprint if anything changes." "$T"
T=$((T + 20000))
broadcast "$ROOM" eve "Sounds solid. Let's do it." "$T"
T=$((T + 12000))
broadcast "$ROOM" frank "Looking forward to getting started!" "$T"

T=$(ts "2026-03-20 10:00:00")
leave_session "$ROOM" alice   "$T"
T=$((T + 3000))
leave_session "$ROOM" bob     "$T"
T=$((T + 5000))
leave_session "$ROOM" charlie "$T"
T=$((T + 2000))
leave_session "$ROOM" diana   "$T"
T=$((T + 4000))
leave_session "$ROOM" eve     "$T"
T=$((T + 6000))
leave_session "$ROOM" frank   "$T"

# --- bugfix-prod (11:00-17:00) ---
ROOM="bugfix-prod"
add_room "$ROOM" "Bugfix Prod" "2026-03-20 11:00:00" "bob"

T=$(ts "2026-03-20 11:00:00")
join_session "$ROOM" bob     "$T" "investigating prod issue"
T=$((T + 5000))
join_session "$ROOM" charlie "$T" "helping with prod"

T=$(ts "2026-03-20 11:02:00")
broadcast "$ROOM" bob "We've got a P1 in production. Users are getting 500 errors on the /api/users endpoint. Started about 30 minutes ago." "$T"
T=$((T + 30000))
broadcast "$ROOM" charlie "Pulling up the logs now. What region?" "$T"
T=$((T + 15000))
broadcast "$ROOM" bob "US-East primarily but starting to see it in EU-West too." "$T"
T=$((T + 45000))
broadcast "$ROOM" charlie "Found something — there's a spike in database connection errors. 'too many connections' errors in the logs." "$T"
T=$((T + 25000))
broadcast "$ROOM" bob "That's the Redis pool issue I was worried about. Can you check the connection count?" "$T"

T=$(ts "2026-03-20 11:15:00")
dm "$ROOM" bob charlie "Should we page alice? She knows the connection handling code best." "$T"
T=$((T + 20000))
dm "$ROOM" charlie bob "Let's try to narrow it down first. If we can't fix it in an hour, we escalate." "$T"

T=$(ts "2026-03-20 11:20:00")
broadcast "$ROOM" charlie "Connection count is at 847 out of 1000 max. Normally we're around 200." "$T"
T=$((T + 30000))
broadcast "$ROOM" bob "Something is leaking connections. Let me check the recent deployments." "$T"
T=$((T + 40000))
broadcast "$ROOM" bob "Last deploy was at 10:30 this morning. It included the new batch processing job." "$T"
T=$((T + 20000))
broadcast "$ROOM" charlie "That's our culprit. The batch job probably isn't releasing connections back to the pool." "$T"

T=$(ts "2026-03-20 11:40:00")
broadcast "$ROOM" bob "Confirmed. The batch processor opens a new connection per item instead of using the pool. It processed 600 items this morning." "$T"
T=$((T + 35000))
broadcast "$ROOM" charlie "Classic. Let's disable the batch job as an immediate fix and then patch the code." "$T"
T=$((T + 20000))
broadcast "$ROOM" bob "Disabling now. Connection count should start dropping as idle connections timeout." "$T"

T=$(ts "2026-03-20 12:00:00")
join_session "$ROOM" alice "$T" "helping with prod issue"
T=$((T + 10000))
broadcast "$ROOM" alice "Hey, just saw the alert. What's the status?" "$T"
T=$((T + 25000))
broadcast "$ROOM" bob "We found it — the new batch job was leaking connections. Already disabled it. Connections are dropping back to normal." "$T"
T=$((T + 30000))
broadcast "$ROOM" alice "Good catch. I knew that batch code looked sketchy in review. Should have pushed harder on the connection handling." "$T"

T=$(ts "2026-03-20 12:20:00")
broadcast "$ROOM" charlie "Error rate is back to baseline. Crisis averted for now." "$T"
T=$((T + 40000))
broadcast "$ROOM" alice "Let's write a proper fix. The batch job should use a connection pool with a bounded size." "$T"
T=$((T + 25000))
broadcast "$ROOM" bob "I'll start on the fix. Should be straightforward — wrap the batch processing in a pool-aware executor." "$T"

T=$(ts "2026-03-20 12:40:00")
dm "$ROOM" alice bob "Hey, should we write a post-mortem for this? It affected users for about an hour." "$T"
T=$((T + 15000))
dm "$ROOM" bob alice "Definitely. I'll draft one after we ship the fix. Good learning opportunity for the team." "$T"

T=$(ts "2026-03-20 13:00:00")
leave_session "$ROOM" charlie "$(ts '2026-03-20 13:00:00')"
T=$(ts "2026-03-20 13:05:00")
broadcast "$ROOM" bob "Charlie stepped out. Alice, can you review the batch fix while I code it?" "$T"
T=$((T + 20000))
broadcast "$ROOM" alice "Sure, push to a branch and I'll keep an eye on it." "$T"

T=$(ts "2026-03-20 13:30:00")
broadcast "$ROOM" bob "PR #47 is up. Rewrote the batch processor to use a bounded connection pool of 20. Also added a cleanup hook." "$T"
T=$((T + 45000))
broadcast "$ROOM" alice "Looking at it now. The pool initialization looks good. One question — why 20 and not configurable?" "$T"
T=$((T + 30000))
broadcast "$ROOM" bob "Good point. Let me make it configurable via BATCH_POOL_SIZE env var with 20 as the default." "$T"

T=$(ts "2026-03-20 14:00:00")
leave_session "$ROOM" alice "$(ts '2026-03-20 14:00:00')"
join_session "$ROOM" frank "$T" "helping with prod fix"
T=$((T + 15000))
broadcast "$ROOM" frank "Hey bob, alice asked me to help with testing the batch fix." "$T"
T=$((T + 20000))
broadcast "$ROOM" bob "Great timing. Can you run the batch job in staging with the new code and monitor the connection count?" "$T"

T=$(ts "2026-03-20 14:30:00")
broadcast "$ROOM" frank "Running now with 500 items. Connection count is stable at 220 — the pool is working correctly." "$T"
T=$((T + 25000))
broadcast "$ROOM" bob "Excellent! That's exactly what we want to see. Let's also test with 2000 items to make sure it handles queuing." "$T"
T=$((T + 40000))
broadcast "$ROOM" frank "2000 items done. Peak was 240 connections, then dropped right back. No leaks." "$T"

T=$(ts "2026-03-20 15:00:00")
join_session "$ROOM" charlie "$(ts '2026-03-20 15:00:00')" "back to help"
T=$(ts "2026-03-20 15:05:00")
broadcast "$ROOM" charlie "Back. How's the fix coming?" "$T"
T=$((T + 20000))
broadcast "$ROOM" bob "Fix is ready and tested. PR #47 — frank verified it in staging." "$T"
T=$((T + 30000))
broadcast "$ROOM" charlie "Reviewing now. Looks clean. LGTM, just one nit — add a log line when the pool reaches 80% capacity as a warning." "$T"

T=$(ts "2026-03-20 15:30:00")
broadcast "$ROOM" bob "Good idea. Added a warning log at 80% and a critical log at 95%. Pushing update." "$T"
T=$((T + 35000))
dm "$ROOM" charlie frank "Nice work on the staging tests. Thorough." "$T"
T=$((T + 15000))
dm "$ROOM" frank charlie "Thanks! Bob walked me through the setup. Learning a lot." "$T"

T=$(ts "2026-03-20 16:00:00")
broadcast "$ROOM" bob "Deploying to prod now. Fingers crossed." "$T"
T=$((T + 60000))
broadcast "$ROOM" bob "Deployed. Re-enabling the batch job..." "$T"
T=$((T + 45000))
broadcast "$ROOM" charlie "Watching the metrics. Connection count is steady at 225. Looking good!" "$T"
T=$((T + 30000))
broadcast "$ROOM" frank "No errors in the logs either. Clean run." "$T"

T=$(ts "2026-03-20 16:30:00")
broadcast "$ROOM" bob "We're in the clear. Batch job processed the backlog of 600 items without any issues." "$T"
T=$((T + 25000))
broadcast "$ROOM" charlie "Ship it! Great teamwork everyone." "$T"
T=$((T + 20000))
broadcast "$ROOM" bob "Writing up the post-mortem now. Root cause: batch processor opened individual connections instead of using the pool. Fix: bounded pool with configurable size." "$T"
T=$((T + 35000))
broadcast "$ROOM" frank "Should I update the runbook with the new BATCH_POOL_SIZE config?" "$T"
T=$((T + 15000))
broadcast "$ROOM" bob "Yes please! That would be super helpful." "$T"

T=$(ts "2026-03-20 17:00:00")
leave_session "$ROOM" bob     "$T"
T=$((T + 5000))
leave_session "$ROOM" charlie "$T"
T=$((T + 3000))
leave_session "$ROOM" frank   "$T"

# --- code-review (15:00-16:00) ---
ROOM="code-review"
add_room "$ROOM" "Code Review" "2026-03-20 15:00:00" "alice"

T=$(ts "2026-03-20 15:00:00")
join_session "$ROOM" alice "$T" "reviewing PR #45"
T=$((T + 5000))
join_session "$ROOM" diana "$T" "reviewing PR #45"
T=$((T + 8000))
join_session "$ROOM" eve   "$T" "reviewing PR #45"

T=$(ts "2026-03-20 15:02:00")
broadcast "$ROOM" alice "Let's review diana's PR #45 — the monitoring dashboard. Diana, want to walk us through?" "$T"
T=$((T + 35000))
broadcast "$ROOM" diana "Sure! The main changes are: a new Dashboard component that polls /api/metrics every 30 seconds, a chart library integration for visualizing response times, and alert threshold configuration." "$T"
T=$((T + 45000))
broadcast "$ROOM" eve "The component structure looks clean. I like how you separated the data fetching from the rendering." "$T"
T=$((T + 30000))
broadcast "$ROOM" alice "Agreed. One concern though — polling every 30 seconds could be expensive. Have you considered WebSockets or SSE?" "$T"
T=$((T + 25000))
broadcast "$ROOM" diana "I thought about it. For v1 I wanted to keep it simple. We can add WebSocket support as a follow-up. The API endpoint is already optimized with caching." "$T"

T=$(ts "2026-03-20 15:15:00")
broadcast "$ROOM" eve "Line 142 — you're using unwrap() on the config parse. That'll panic if someone puts invalid JSON in the config." "$T"
T=$((T + 20000))
broadcast "$ROOM" diana "Good catch. I'll switch to unwrap_or_default() with a warning log." "$T"
T=$((T + 30000))
broadcast "$ROOM" alice "Also on line 87 — the error handling in the fetch loop swallows errors silently. We should at least log them." "$T"
T=$((T + 25000))
broadcast "$ROOM" diana "You're right. I'll add error logging and a retry counter that backs off if we get consecutive failures." "$T"

T=$(ts "2026-03-20 15:30:00")
broadcast "$ROOM" eve "The chart rendering looks great. One suggestion — can we add a toggle to switch between line and bar charts?" "$T"
T=$((T + 35000))
broadcast "$ROOM" diana "Easy addition. I'll add a chart type selector to the toolbar." "$T"
T=$((T + 20000))
broadcast "$ROOM" alice "What about accessibility? Do the charts have aria labels for screen readers?" "$T"
T=$((T + 30000))
broadcast "$ROOM" diana "Not yet. I'll add aria-label attributes and a data table fallback for screen reader users." "$T"

T=$(ts "2026-03-20 15:45:00")
broadcast "$ROOM" alice "Overall LGTM with the changes we discussed. Fix the unwrap, add error logging, and accessibility, and it's good to merge." "$T"
T=$((T + 15000))
broadcast "$ROOM" eve "Same from me. Nice work diana, this is going to be really useful." "$T"
T=$((T + 20000))
broadcast "$ROOM" diana "Thanks both! I'll push the updates this afternoon." "$T"

T=$(ts "2026-03-20 16:00:00")
leave_session "$ROOM" alice "$T"
T=$((T + 5000))
leave_session "$ROOM" diana "$T"
T=$((T + 3000))
leave_session "$ROOM" eve   "$T"

# ===========================
echo "Generating Day 3 (2026-03-21 — today)..."
# ===========================

# --- morning-sync (9:00-ongoing) ---
ROOM="morning-sync"
add_room "$ROOM" "Morning Sync" "2026-03-21 09:00:00" "alice"

T=$(ts "2026-03-21 09:00:00")
join_session "$ROOM" alice   "$T" "daily sync"
T=$((T + 5000))
join_session "$ROOM" bob     "$T" "daily sync"
T=$((T + 3000))
join_session "$ROOM" charlie "$T" "daily sync"
T=$((T + 7000))
join_session "$ROOM" diana   "$T" "daily sync"

# Register active sessions
register_active_session "$ROOM" alice   "2026-03-21 09:00:00"
register_active_session "$ROOM" bob     "2026-03-21 09:00:05"
register_active_session "$ROOM" charlie "2026-03-21 09:00:08"
register_active_session "$ROOM" diana   "2026-03-21 09:00:15"

T=$(ts "2026-03-21 09:01:00")
broadcast "$ROOM" alice "Morning everyone. Quick sync — what's on the agenda today?" "$T"
T=$((T + 25000))
broadcast "$ROOM" bob "I'm going to finalize the post-mortem from yesterday's prod incident and then continue with the migration scripts." "$T"
T=$((T + 20000))
broadcast "$ROOM" charlie "Auth refactor — I need to update the remaining 8 handlers to use the new AuthContext. Should be done by end of day." "$T"
T=$((T + 30000))
broadcast "$ROOM" diana "I'm pushing the code review fixes from yesterday. Then back to the monitoring dashboard." "$T"

T=$(ts "2026-03-21 09:05:00")
broadcast "$ROOM" alice "Great. I'll be working on the auth refactor too — charlie and I will sync on the handler updates. Also need to file that security advisory about the token rotation bug." "$T"
T=$((T + 35000))
broadcast "$ROOM" bob "Oh, before I forget — frank said he'll be in around 10. He's finishing the runbook updates from yesterday." "$T"
T=$((T + 25000))
broadcast "$ROOM" charlie "Cool. Alice, want to split the handlers? I'll take the user-facing ones, you take the admin endpoints?" "$T"
T=$((T + 20000))
broadcast "$ROOM" alice "Works for me. Let's touch base after lunch to merge our changes." "$T"
T=$((T + 30000))
broadcast "$ROOM" diana "One more thing — the staging environment is still running the old batch processor. Should someone update it?" "$T"
T=$((T + 18000))
broadcast "$ROOM" bob "I'll handle that after the post-mortem. Good catch diana." "$T"

# No leave events for active sessions — they're still in the room.

echo ""
echo "Done. Stats:"
echo "  Rooms: $($R HLEN claude:rooms)"
echo "  Global stream entries: $($R XLEN claude:stream)"
echo "  Day 1 rooms: $($R SCARD claude:rooms:by_date:2026-03-19)"
echo "  Day 2 rooms: $($R SCARD claude:rooms:by_date:2026-03-20)"
echo "  Day 3 rooms: $($R SCARD claude:rooms:by_date:2026-03-21)"
echo "  Active sessions in morning-sync: $($R HLEN claude:room:morning-sync:sessions)"
