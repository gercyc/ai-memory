# Kimi Code injects UserPromptSubmit hook stdout as a user message
# (origin hook_result) before the turn, so the pending handoff is
# fetched here. Empty stdout injects nothing.
. "$PSScriptRoot\..\lib\ai-memory-hook.ps1"
Invoke-AiMemoryHook -Event "user-prompt" -Agent "kimi-code" -FetchHandoff
exit 0
