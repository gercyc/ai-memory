# Managed cross-harness workstreams

`ai-memory run` is an opt-in launcher that lets one logical coding session move
between Claude Code, Codex, OpenCode, Pi, and OMP. Direct agent launches keep
their existing ai-memory behavior. There is no global mode toggle and no
`switch` command: using `run` selects the current workstream and transparently
creates or resumes the correct native session for the requested harness.

```bash
cd /path/to/project

ai-memory run claude
# quit Claude Code, then continue the same logical workstream in Codex
ai-memory run codex --yolo
# return to Claude Code later; ai-memory supplies Claude's native --resume
ai-memory run claude --model opus
```

Everything after the harness name is native argv. No `--` separator is needed,
and ai-memory does not maintain a second copy of each harness's option schema.
Wrapper options must come first:

```text
ai-memory run [--workspace NAME] [--project NAME]
              [--workstream NAME | --new NAME] [--executable PATH]
              <claude|codex|opencode|pi|omp> [native arguments...]
```

The default is the most recently selected workstream for the current repository
and worktree, creating one named `default` on first use. `--new NAME` starts an
independent line of work; `--workstream NAME` returns to one. These are optional
branching controls, not harness-switch controls.

## What happens on each run

1. The host client resolves the normal workspace/project scope and a stable
   repository plus worktree fingerprint. It opens a 90-second renewable lease.
   One writer may own a workstream at a time, so two terminals cannot silently
   race its native-session pointers or delivery cursors.
2. The harness adapter passes every native argument through in order and adds a
   native create/resume selector only when the user did not supply one.
3. `AI_MEMORY_RUN_ID` marks lifecycle hooks as managed. SessionStart links the
   actual native session and injects only the portable events that session has
   not seen. Direct launches do not set this variable and continue to use the
   existing single-use handoff path.
4. When the child exits, ai-memory reads the native transcript store without
   modifying it. Visible user/assistant messages, completed tool calls/results,
   compaction summaries, and a non-mutating Git checkpoint enter an append-only
   workstream ledger. Hidden reasoning and unsupported/private records are
   excluded and recorded as extraction-loss annotations.
5. Imports use deterministic event ids, incremental source cursors, immutable
   sanitized JSONL segments, and bounded batches. A retry cannot duplicate
   history. The native process's exit code is preserved.

The next harness receives a bounded recent delta because no agent context window
can safely absorb an unbounded transcript. The complete visible ledger remains
searchable from inside a managed agent process:

```bash
ai-memory workstream-search "scope resolver decision"
ai-memory workstream-search --limit 50 --json "failed migration"
```

`AI_MEMORY_WORKSTREAM_ID` supplies the id automatically inside the child. From
another shell, pass `--workstream-id <uuid>` explicitly. Search results preserve
the source harness, role, event sequence, and content. Historical tool activity
is labelled completed evidence and must never be replayed as a pending call.

## Native adapter behavior

| Harness | Fresh native session | Returning native session | Read-only source |
|---|---|---|---|
| Claude Code | generated `--session-id` | `--resume <id>` | `~/.claude/projects/**/*.jsonl` |
| Codex | native default creation | `resume <id>` | `~/.codex/sessions/**/rollout-*.jsonl` |
| OpenCode | native default creation | `--session <id>` | `~/.local/share/opencode/opencode.db` opened read-only |
| Pi | generated `--session-id` | `--session <id>` | `~/.pi/agent/sessions/**/*.jsonl` |
| OMP | native default creation | `--resume=<id>` | `~/.omp/agent/sessions/**/*.jsonl` |

An explicit native selector such as Claude's `--resume`, OpenCode's `--session`,
or Codex's `resume` wins. ai-memory links the selected native session and resets
an unrelated adapter cursor rather than assuming it belongs to the old session.
Pi and OMP `--session-dir` values are passed through unchanged and used as the
read-only import root. Native store environment overrides are also honored:
`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `XDG_DATA_HOME`,
`PI_CODING_AGENT_SESSION_DIR`, and `PI_CODING_AGENT_DIR`. The Pi-family adapter
also recognizes a complete `.jsonl.<nonce>.tmp` atomic-write file when a native
process exits before renaming it; incomplete final JSONL records are never
imported. Help, version, and known utility subcommands pass through without
session flags.

## Installation and recovery

Managed runs need current ai-memory lifecycle hooks so SessionStart can receive
the portable delta. Refresh them after upgrading:

```bash
ai-memory install-hooks --agent claude-code --apply
ai-memory install-hooks --agent codex --apply
ai-memory install-hooks --agent opencode --apply
ai-memory install-hooks --agent pi --apply
ai-memory install-hooks --agent omp --apply
```

The Linux/macOS Docker shell wrapper cannot execute a host agent from inside its
helper container. For `run` only, it downloads the matching native release into
`~/.cache/ai-memory/native-runner`, verifies the published SHA-256 checksum, and
executes that host client. Set `AI_MEMORY_NATIVE_BIN=/path/to/ai-memory` to use a
specific native build. Native package, release, and source installs need no
shim. On native Windows, use the published `ai-memory.exe` or a source build.

If a client is killed before final import, its lease expires. A later managed
run starts from the last committed adapter cursor, so already linked native
sessions can import the missing tail without duplicating earlier events. A
server or authentication failure before process launch is fatal; ai-memory does
not silently start an unmanaged agent.

## Privacy and storage boundaries

Managed mode does not write to Claude, Codex, OpenCode, Pi, or OMP private
stores. Adapters read only documented/observed local session formats. Provider
credentials, encrypted content, system/developer prompt records, and hidden
reasoning are not copied. The server sanitizer runs before both the SQLite FTS
ledger and immutable files under
`<data_dir>/raw/workstreams/<workstream-id>/segments/` are written.

The ledger is an operational continuity substrate, not a replacement for the
markdown wiki. Durable decisions, rules, procedures, and project facts still
belong in wiki pages through consolidation or explicit durable writes.

## Manual acceptance

The opt-in acceptance runner exercises wrapper edge cases and then orchestrates
the locally installed Claude, Codex, OpenCode, Pi, and OMP CLIs through one real
workstream:

```bash
scripts/managed-workstream-acceptance.sh
```

It is deliberately separate from CI because it uses local harness credentials
and model calls. Hook configs, native session stores, the ai-memory server, and
the Git fixture are isolated under a temporary directory. Claude, Codex, and
OpenCode receive only copied authentication material; OMP receives a temporary
agent directory with read-consistent credential/model database backups and
copied settings. Native session creation, read-only extraction, cross-harness
injection, and returning resume paths are all exercised. Set
`AI_MEMORY_ACCEPTANCE_HARNESSES="claude codex"` to select
a subset, `AI_MEMORY_ACCEPTANCE_DETERMINISTIC_ONLY=1` to skip model calls, or
`AI_MEMORY_ACCEPTANCE_KEEP=1` to retain all temporary logs and data.
