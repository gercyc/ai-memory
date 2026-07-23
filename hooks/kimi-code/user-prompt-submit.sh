#!/bin/sh
# kimi-code user-prompt hook.
# 1. Forwards the event JSON to the ai-memory server (fire-and-forget).
# 2. Synchronously fetches any pending cross-agent handoff and prints
#    it to stdout — kimi-code injects UserPromptSubmit hook stdout as a
#    user message (origin hook_result) before the turn, so the agent
#    sees prior context with no human in the loop. SessionStart cannot
#    deliver it: kimi-code discards SessionStart hook stdout (v0.28.1,
#    packages/agent-core/src/session/index.ts). Empty stdout injects
#    nothing, so print only when a handoff exists.
_lib_dir="$(dirname "$0")"
[ -f "$_lib_dir/_lib.sh" ] || _lib_dir="$_lib_dir/.."
. "$_lib_dir/_lib.sh"

SERVER="${AI_MEMORY_HOOK_URL:-http://127.0.0.1:49374}"
PAYLOAD=$(cat)
CWD=$(ai_memory_extract_cwd "$PAYLOAD")
QS=$(ai_memory_marker_qs "$CWD")
SESSION_ID=$(ai_memory_extract_session_id "$PAYLOAD")
SESSION_QS=""
[ -n "$SESSION_ID" ] && SESSION_QS="&session_id=$(ai_memory_url_encode "$SESSION_ID")"

printf '%s' "$PAYLOAD" \
    | ai_memory_post_hook "$SERVER/hook?event=user-prompt&agent=kimi-code${QS}" >/dev/null 2>&1 || true

HANDOFF=$(ai_memory_get_handoff "$SERVER/handoff?agent=kimi-code${QS}${SESSION_QS}" 2>/dev/null || true)
[ -n "$HANDOFF" ] && printf '%s\n' "$HANDOFF"
exit 0
