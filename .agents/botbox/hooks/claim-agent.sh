#!/bin/bash
# botbox SessionStart/PostToolUse hook: claim and refresh agent:// advisory lock
# Only active when BOTBUS_AGENT is set. All errors silently ignored.

[ -z "$BOTBUS_AGENT" ] && exit 0

CLAIM_URI="agent://$BOTBUS_AGENT"
CLAIM_TTL=600
REFRESH_THRESHOLD=120

# Read hook event from stdin JSON (PostToolUse passes JSON input)
INPUT=$(cat 2>/dev/null)
EVENT=$(echo "$INPUT" | jq -r '.event // empty' 2>/dev/null)

# SessionStart (or PreCompact, or no event info): stake the claim
if [ "$EVENT" != "PostToolUse" ]; then
  bus claims stake --agent "$BOTBUS_AGENT" "$CLAIM_URI" --ttl "$CLAIM_TTL" -q 2>/dev/null
  exit 0
fi

# PostToolUse: refresh only if claim is within REFRESH_THRESHOLD seconds of expiring
EXPIRES=$(bus claims list --mine --agent "$BOTBUS_AGENT" --format json 2>/dev/null \
  | jq -r ".claims[] | select(.patterns[] == \"$CLAIM_URI\") | .expires_in_secs" 2>/dev/null)

if [ -n "$EXPIRES" ] && [ "$EXPIRES" -lt "$REFRESH_THRESHOLD" ] 2>/dev/null; then
  bus claims refresh --agent "$BOTBUS_AGENT" "$CLAIM_URI" --ttl "$CLAIM_TTL" -q 2>/dev/null
fi

exit 0
