#!/usr/bin/env bash
# Manual, opt-in acceptance test for managed cross-harness workstreams.
# This is intentionally not called by CI: the real-harness phase uses the
# operator's installed CLIs, credentials, model defaults, and native stores.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
BIN=${AI_MEMORY_ACCEPTANCE_BIN:-"$ROOT/target/debug/ai-memory"}
KEEP=${AI_MEMORY_ACCEPTANCE_KEEP:-0}
DETERMINISTIC_ONLY=${AI_MEMORY_ACCEPTANCE_DETERMINISTIC_ONLY:-0}
HARNESS_WORDS=${AI_MEMORY_ACCEPTANCE_HARNESSES:-"claude codex opencode pi omp"}
TMP=$(mktemp -d "${TMPDIR:-/tmp}/ai-memory-workstream-acceptance.XXXXXX")
DATA="$TMP/data"
REPO="$TMP/repo"
CONFIG="$TMP/config"
LOGS="$TMP/logs"
SERVER_PID=""

cleanup() {
  local code=$?
  if [ -n "$SERVER_PID" ]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  if [ "$KEEP" = 1 ] || [ "$code" -ne 0 ]; then
    printf 'acceptance artifacts retained at %s\n' "$TMP" >&2
  else
    rm -rf "$TMP"
  fi
}
trap cleanup EXIT INT TERM

for command in cargo curl diff git jq sqlite3; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'missing required command: %s\n' "$command" >&2
    exit 1
  }
done

if [ ! -x "$BIN" ] || [ "${AI_MEMORY_ACCEPTANCE_REBUILD:-1}" = 1 ]; then
  (cd "$ROOT" && TAILWIND_SKIP=1 cargo build -p ai-memory-cli)
fi

mkdir -p "$DATA" "$REPO" "$CONFIG" "$LOGS"
git -C "$REPO" init -q
git -C "$REPO" config user.name "ai-memory acceptance"
git -C "$REPO" config user.email "acceptance@localhost"
printf '# Managed workstream acceptance\n' >"$REPO/README.md"
git -C "$REPO" add README.md
git -C "$REPO" commit -qm "acceptance fixture"

TOKEN="managed-acceptance-$(date +%s)-$$"
PORT=${AI_MEMORY_ACCEPTANCE_PORT:-$((52000 + ($$ % 10000)))}
for _ in $(seq 1 50); do
  if ! curl -sS --max-time 0.1 "http://127.0.0.1:$PORT/" >/dev/null 2>&1; then
    break
  fi
  PORT=$((PORT + 1))
done
URL="http://127.0.0.1:$PORT"
export AI_MEMORY_SERVER_URL="$URL"
export AI_MEMORY_AUTH_TOKEN="$TOKEN"
export AI_MEMORY_NO_VERSION_CHECK=1

"$BIN" --data-dir "$DATA" serve \
  --transport http \
  --bind "127.0.0.1:$PORT" \
  --no-watcher >"$LOGS/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 100); do
  status=$(curl -sS --max-time 0.2 -o /dev/null -w '%{http_code}' \
    -H "Authorization: Bearer $TOKEN" \
    "$URL/workstream/not-a-uuid/events" 2>/dev/null || true)
  [ "$status" = 400 ] && break
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    printf 'ai-memory server exited during startup\n' >&2
    tail -80 "$LOGS/server.log" >&2
    exit 1
  fi
  sleep 0.1
done
[ "${status:-}" = 400 ] || {
  printf 'ai-memory server did not become ready at %s\n' "$URL" >&2
  tail -80 "$LOGS/server.log" >&2
  exit 1
}

FAKE="$TMP/fake-harness.sh"
cat >"$FAKE" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${AI_MEMORY_ACCEPTANCE_FAKE_MODE:-argv}" in
  argv)
    printf '%s\n' "$@" >"$AI_MEMORY_ACCEPTANCE_ARGV_LOG"
    ;;
  exit)
    exit "${AI_MEMORY_ACCEPTANCE_EXIT_CODE:-23}"
    ;;
  lease)
    : >"$AI_MEMORY_ACCEPTANCE_STARTED"
    sleep "${AI_MEMORY_ACCEPTANCE_SLEEP:-3}"
    ;;
esac
EOF
chmod +x "$FAKE"

printf 'running deterministic wrapper edge checks\n'
(
  cd "$REPO"
  AI_MEMORY_ACCEPTANCE_FAKE_MODE=argv \
  AI_MEMORY_ACCEPTANCE_ARGV_LOG="$TMP/argv.log" \
    "$BIN" --data-dir "$DATA" run --new edge-argv --executable "$FAKE" \
      codex --yolo -m gpt-5 "prompt words" >"$LOGS/edge-argv.log" 2>&1
)
diff -u <(printf '%s\n' --yolo -m gpt-5 "prompt words") "$TMP/argv.log"

set +e
(
  cd "$REPO"
  AI_MEMORY_ACCEPTANCE_FAKE_MODE=exit AI_MEMORY_ACCEPTANCE_EXIT_CODE=23 \
    "$BIN" --data-dir "$DATA" run --new edge-exit --executable "$FAKE" \
      codex >"$LOGS/edge-exit.log" 2>&1
)
exit_code=$?
set -e
[ "$exit_code" -eq 23 ] || {
  printf 'managed child exit code was %s, expected 23\n' "$exit_code" >&2
  exit 1
}

(
  cd "$REPO"
  AI_MEMORY_ACCEPTANCE_FAKE_MODE=lease \
  AI_MEMORY_ACCEPTANCE_STARTED="$TMP/lease-started" \
    "$BIN" --data-dir "$DATA" run --new edge-lease --executable "$FAKE" \
      codex >"$LOGS/edge-lease-owner.log" 2>&1
) &
lease_pid=$!
for _ in $(seq 1 100); do
  [ -f "$TMP/lease-started" ] && break
  sleep 0.05
done
[ -f "$TMP/lease-started" ] || {
  printf 'lease owner did not start\n' >&2
  exit 1
}
set +e
(
  cd "$REPO"
  AI_MEMORY_ACCEPTANCE_FAKE_MODE=argv \
  AI_MEMORY_ACCEPTANCE_ARGV_LOG="$TMP/lease-contender-argv.log" \
    "$BIN" --data-dir "$DATA" run --workstream edge-lease --executable "$FAKE" \
      codex >"$LOGS/edge-lease-contender.log" 2>&1
)
lease_code=$?
set -e
[ "$lease_code" -ne 0 ] || {
  printf 'a concurrent managed writer unexpectedly acquired the lease\n' >&2
  exit 1
}
wait "$lease_pid"

if [ "$DETERMINISTIC_ONLY" = 1 ]; then
  printf 'deterministic managed-workstream acceptance passed\n'
  exit 0
fi

read -r -a requested_harnesses <<<"$HARNESS_WORDS"
harnesses=()
for harness in "${requested_harnesses[@]}"; do
  if command -v "$harness" >/dev/null 2>&1; then
    harnesses+=("$harness")
  else
    printf 'skipping unavailable harness: %s\n' "$harness" >&2
  fi
done
[ "${#harnesses[@]}" -ge 2 ] || {
  printf 'real acceptance needs at least two installed harnesses\n' >&2
  exit 1
}

CLAUDE_CONFIG_HOME="$CONFIG/claude"
CLAUDE_SETTINGS="$CLAUDE_CONFIG_HOME/settings.json"
CODEX_ACCEPTANCE_HOME="$CONFIG/codex-home"
CODEX_HOOKS="$CODEX_ACCEPTANCE_HOME/.codex/hooks.json"
OPENCODE_CONFIG_HOME="$CONFIG/opencode-xdg"
OPENCODE_PLUGIN="$OPENCODE_CONFIG_HOME/opencode/plugins/ai-memory.ts"
OPENCODE_DATA_HOME="$CONFIG/opencode-xdg-data"
PI_EXTENSION="$CONFIG/pi/ai-memory.ts"
OMP_EXTENSION="$CONFIG/omp/ai-memory.ts"
OMP_AGENT_DIR="$CONFIG/omp/agent"
mkdir -p "$(dirname "$CLAUDE_SETTINGS")" "$(dirname "$CODEX_HOOKS")" \
  "$(dirname "$OPENCODE_PLUGIN")" "$(dirname "$PI_EXTENSION")" \
  "$(dirname "$OMP_EXTENSION")" "$OMP_AGENT_DIR" "$OPENCODE_DATA_HOME/opencode"

# Redirect native transcript stores into the fixture while reusing only the
# minimum authentication material required for real model calls.
if [ -f "$HOME/.claude/.credentials.json" ]; then
  cp "$HOME/.claude/.credentials.json" "$CLAUDE_CONFIG_HOME/.credentials.json"
fi
if [ -f "$HOME/.local/share/opencode/auth.json" ]; then
  cp "$HOME/.local/share/opencode/auth.json" "$OPENCODE_DATA_HOME/opencode/auth.json"
fi

# Codex only discovers hooks below its home. Use a temporary home so the
# acceptance config cannot modify or depend on the operator's trusted hooks.
if [ -f "$HOME/.codex/auth.json" ]; then
  cp "$HOME/.codex/auth.json" "$CODEX_ACCEPTANCE_HOME/.codex/auth.json"
fi

# OMP's installed release drops explicit extension paths when
# --no-extensions is set. Isolate discovery with a temporary agent directory
# and copy only settings plus consistent credential/model database backups.
for database in agent.db models.db; do
  if [ -f "$HOME/.omp/agent/$database" ]; then
    sqlite3 "$HOME/.omp/agent/$database" ".backup '$OMP_AGENT_DIR/$database'"
  fi
done
for config_name in auth.json config.yml models-store.json settings.json; do
  if [ -f "$HOME/.omp/agent/$config_name" ]; then
    cp "$HOME/.omp/agent/$config_name" "$OMP_AGENT_DIR/$config_name"
  fi
done

# Preserve OpenCode's provider/model preferences while loading only the
# acceptance plugin from the isolated XDG config root.
for config_name in opencode.json opencode.jsonc tui.json; do
  if [ -f "$HOME/.config/opencode/$config_name" ]; then
    cp "$HOME/.config/opencode/$config_name" \
      "$OPENCODE_CONFIG_HOME/opencode/$config_name"
  fi
done

install_hook() {
  local agent=$1
  local target=$2
  local -a command=(
    "$BIN" --data-dir "$DATA" install-hooks --apply
    --agent "$agent" --server-url "$URL" --auth-token "$TOKEN"
    --config-file "$target"
  )
  case "$agent" in
    claude-code | codex)
      command+=(--hooks-dir "$ROOT/hooks")
      ;;
  esac
  XDG_DATA_HOME="$TMP/xdg-data" "${command[@]}" \
    >"$LOGS/install-$agent.log" 2>&1
}

install_hook claude-code "$CLAUDE_SETTINGS"
install_hook codex "$CODEX_HOOKS"
install_hook opencode "$OPENCODE_PLUGIN"
install_hook pi "$PI_EXTENSION"
install_hook omp "$OMP_EXTENSION"

uuid_from_hex() {
  local hex=$1
  printf '%s-%s-%s-%s-%s\n' \
    "${hex:0:8}" "${hex:8:4}" "${hex:12:4}" "${hex:16:4}" "${hex:20:12}"
}

workstream_id() {
  local name=$1
  local hex
  hex=$(sqlite3 "$DATA/db/memory.sqlite" \
    "SELECT lower(hex(id)) FROM workstreams WHERE name = '$name' ORDER BY selected_at DESC LIMIT 1;")
  [ "${#hex}" -eq 32 ] || return 1
  uuid_from_hex "$hex"
}

agent_wire_name() {
  case "$1" in
    claude) printf 'claude-code\n' ;;
    opencode) printf 'open-code\n' ;;
    *) printf '%s\n' "$1" ;;
  esac
}

uppercase() {
  printf '%s' "$1" | tr '[:lower:]' '[:upper:]'
}

run_harness() {
  local harness=$1
  local current=$2
  local previous=$3
  local first_run=$4
  local log="$LOGS/real-$harness-$current.log"
  local prompt
  local expected_agent
  local -a wrapper_args native_args
  expected_agent=$(agent_wire_name "$harness")
  if [ -z "$previous" ]; then
    prompt="Do not use tools. Reply with exactly: $current"
  else
    prompt="Do not use tools. From the injected ai-memory managed-workstream context, identify the most recent assistant sentinel beginning with AMWS-. Reply on one line with that prior sentinel, then $current."
  fi
  if [ "$first_run" = 1 ]; then
    wrapper_args=(--new "$WORKSTREAM_NAME")
  else
    wrapper_args=(--workstream "$WORKSTREAM_NAME")
  fi
  case "$harness" in
    claude)
      native_args=(-p --settings "$CLAUDE_SETTINGS" --model "${AI_MEMORY_ACCEPTANCE_CLAUDE_MODEL:-haiku}" --permission-mode plan "$prompt")
      ;;
    codex)
      native_args=(exec -c 'sandbox_mode="read-only"' --dangerously-bypass-hook-trust --json "$prompt")
      if [ -n "${AI_MEMORY_ACCEPTANCE_CODEX_MODEL:-}" ]; then
        native_args=(exec -c 'sandbox_mode="read-only"' --dangerously-bypass-hook-trust --json --model "$AI_MEMORY_ACCEPTANCE_CODEX_MODEL" "$prompt")
      fi
      ;;
    opencode)
      native_args=(run --format json --auto "$prompt")
      [ -z "${AI_MEMORY_ACCEPTANCE_OPENCODE_MODEL:-}" ] || native_args=(run --format json --auto --model "$AI_MEMORY_ACCEPTANCE_OPENCODE_MODEL" "$prompt")
      ;;
    pi)
      native_args=(-p --no-tools --no-extensions --extension "$PI_EXTENSION" --session-dir "$CONFIG/pi/sessions" "$prompt")
      [ -z "${AI_MEMORY_ACCEPTANCE_PI_MODEL:-}" ] || native_args=(-p --no-tools --no-extensions --extension "$PI_EXTENSION" --session-dir "$CONFIG/pi/sessions" --model "$AI_MEMORY_ACCEPTANCE_PI_MODEL" "$prompt")
      ;;
    omp)
      native_args=(-p --no-tools --extension "$OMP_EXTENSION" --session-dir "$CONFIG/omp/sessions" "$prompt")
      [ -z "${AI_MEMORY_ACCEPTANCE_OMP_MODEL:-}" ] || native_args=(-p --no-tools --extension "$OMP_EXTENSION" --session-dir "$CONFIG/omp/sessions" --model "$AI_MEMORY_ACCEPTANCE_OMP_MODEL" "$prompt")
      ;;
    *)
      printf 'unsupported acceptance harness: %s\n' "$harness" >&2
      return 1
      ;;
  esac

  printf 'running real harness: %s\n' "$harness" >&2
  if [ "$harness" = claude ]; then
    (cd "$REPO" && CLAUDE_CONFIG_DIR="$CLAUDE_CONFIG_HOME" \
      "$BIN" --data-dir "$DATA" run "${wrapper_args[@]}" "$harness" "${native_args[@]}") \
      >"$log" 2>&1
  elif [ "$harness" = codex ]; then
    (cd "$REPO" && HOME="$CODEX_ACCEPTANCE_HOME" \
      CODEX_HOME="$CODEX_ACCEPTANCE_HOME/.codex" \
      "$BIN" --data-dir "$DATA" run "${wrapper_args[@]}" "$harness" "${native_args[@]}") \
      >"$log" 2>&1
  elif [ "$harness" = opencode ]; then
    (cd "$REPO" && XDG_CONFIG_HOME="$OPENCODE_CONFIG_HOME" \
      XDG_DATA_HOME="$OPENCODE_DATA_HOME" \
      "$BIN" --data-dir "$DATA" run "${wrapper_args[@]}" "$harness" "${native_args[@]}") \
      >"$log" 2>&1
  elif [ "$harness" = omp ]; then
    (cd "$REPO" && PI_CODING_AGENT_DIR="$OMP_AGENT_DIR" \
      "$BIN" --data-dir "$DATA" run "${wrapper_args[@]}" "$harness" "${native_args[@]}") \
      >"$log" 2>&1
  else
    (cd "$REPO" && "$BIN" --data-dir "$DATA" run \
      "${wrapper_args[@]}" "$harness" "${native_args[@]}") >"$log" 2>&1
  fi

  local id results event native_id
  id=$(workstream_id "$WORKSTREAM_NAME")
  results=$("$BIN" --data-dir "$DATA" workstream-search \
    --workstream-id "$id" --limit 100 --json "$current")
  event=$(jq -c --arg agent "$expected_agent" --arg current "$current" \
    '[.[] | select(.agent == $agent and .role == "assistant" and (.content | contains($current)))] | last // empty' \
    <<<"$results")
  [ -n "$event" ] || {
    printf '%s did not persist an assistant event containing %s\n' "$harness" "$current" >&2
    tail -120 "$log" >&2
    return 1
  }
  if [ -n "$previous" ] && ! jq -e --arg previous "$previous" \
    '.content | contains($previous)' <<<"$event" >/dev/null; then
    printf '%s did not demonstrate receipt of prior sentinel %s\n' "$harness" "$previous" >&2
    jq -r '.content' <<<"$event" >&2
    return 1
  fi
  native_id=$(jq -r '.native_session_id' <<<"$event")
  printf '%s\n' "$native_id"
}

WORKSTREAM_NAME="native-acceptance-$(date +%s)-$$"
RUN_TAG="$(date +%s)-$$"
previous=""
first_harness=${harnesses[0]}
first_native=""
index=0
for harness in "${harnesses[@]}"; do
  current="AMWS-$RUN_TAG-$(uppercase "$harness")"
  first_run=0
  [ "$index" -ne 0 ] || first_run=1
  native_id=$(run_harness "$harness" "$current" "$previous" "$first_run")
  if [ "$index" -eq 0 ]; then
    first_native=$native_id
  fi
  previous=$current
  index=$((index + 1))
done

return_sentinel="AMWS-$RUN_TAG-$(uppercase "$first_harness")-RETURN"
returned_native=$(run_harness "$first_harness" "$return_sentinel" "$previous" 0)
[ "$returned_native" = "$first_native" ] || {
  printf '%s resumed native session %s, expected %s\n' \
    "$first_harness" "$returned_native" "$first_native" >&2
  exit 1
}

printf 'real managed-workstream acceptance passed: %s\n' "${harnesses[*]}"
printf 'returned to %s native session %s\n' "$first_harness" "$first_native"
printf 'native harness session stores and resume paths were exercised\n'
