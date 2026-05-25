# Windows Support

Windows support has two modes. Pick the mode that matches where your
agent CLI actually runs.

## Rule Of Thumb

Run `install-mcp` and `install-hooks` from the same environment that
launches Claude Code, Codex, Cursor, Gemini CLI, or another agent.

- If the agent runs inside WSL2, install ai-memory inside WSL2.
- If the agent runs as a native Windows process, install ai-memory from
  PowerShell on Windows.
- Do not mix the Windows wrapper with WSL2-launched agents unless you
  deliberately override every config and hook path.

The difference matters because hook configs contain executable paths.
WSL2 agents need Linux paths and POSIX `.sh` hooks. Native Windows
agents need Windows paths and PowerShell `.ps1` hooks.

## Scenario A: Everything Inside WSL2

This is the most Linux-like Windows setup. Use it when your agent CLI is
installed and launched inside a WSL2 distro.

```bash
# Inside WSL2.
mkdir -p ~/.local/bin
curl -fsSL https://raw.githubusercontent.com/akitaonrails/ai-memory/main/bin/ai-memory \
    -o ~/.local/bin/ai-memory
chmod +x ~/.local/bin/ai-memory
export PATH="$HOME/.local/bin:$PATH"

docker run -d --name ai-memory \
    --restart unless-stopped \
    -p 127.0.0.1:49374:49374 \
    -v ai-memory-data:/data \
    akitaonrails/ai-memory:latest

ai-memory install-mcp --client claude-code --apply
ai-memory install-hooks --agent claude-code --apply
```

In this mode, ai-memory behaves like Linux:

- Config files are written under your WSL2 home directory.
- Hook scripts are staged under `~/.local/share/ai-memory/hooks/`.
- Hook commands point at `.sh` scripts.
- The agent should also be launched from WSL2 so it can execute those
  WSL paths.

If Docker Desktop provides the Docker engine to WSL2, enable WSL
integration for the distro first. If you run a native Docker engine
inside WSL2, no Windows wrapper is involved.

## Scenario B: Native Windows With Docker Desktop

Use this when the agent CLI runs as a native Windows process and you want
the ai-memory server to run from the Docker image.

```powershell
# Install the Windows Docker wrapper.
$UserBin = "$HOME\bin"
New-Item -ItemType Directory -Force $UserBin | Out-Null
foreach ($File in @("ai-memory.ps1", "ai-memory.cmd")) {
    Invoke-WebRequest `
        -Uri "https://raw.githubusercontent.com/akitaonrails/ai-memory/main/bin/$File" `
        -OutFile "$UserBin\$File"
}
Get-ChildItem "$UserBin\ai-memory.*" | Unblock-File

# Put the wrapper directory on your user PATH for future terminals.
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($UserPath -split ';') -notcontains $UserBin) {
    $NewUserPath = (($UserPath, $UserBin) | Where-Object { $_ }) -join ";"
    [Environment]::SetEnvironmentVariable("Path", $NewUserPath, "User")
    $env:Path = "$env:Path;$UserBin"
}

# Start the server with Docker Desktop.
docker run -d --name ai-memory `
    --restart unless-stopped `
    -p 127.0.0.1:49374:49374 `
    -v ai-memory-data:/data `
    akitaonrails/ai-memory:latest

# Verify the wrapper can reach the server.
ai-memory status

# Wire MCP and lifecycle hooks for a native Windows agent.
ai-memory install-mcp --client claude-code --apply
ai-memory install-hooks --agent claude-code --apply
```

In this mode, the PowerShell wrapper runs the Linux container but tells
the CLI to render Windows hook commands:

- Config files are written through the mounted Windows home directory.
- Hook scripts are staged under `$HOME\.local\share\ai-memory\hooks\`.
- Hook commands explicitly call `powershell.exe`.
- Hook commands point at `.ps1` scripts.

Use the matching `--client` / `--agent` values for other clients, for
example `codex`, `cursor`, or `gemini-cli`.

## Scenario C: Native Windows Source Build

Use this when developing ai-memory itself on Windows or when you do not
want the Docker wrapper for CLI commands.

```powershell
git clone https://github.com/akitaonrails/ai-memory .\ai-memory
Set-Location .\ai-memory
cargo build --workspace
cargo test --workspace

target\debug\ai-memory.exe init
target\debug\ai-memory.exe serve --transport http --bind 127.0.0.1:49374
```

The Tailwind build step supports the pinned
`tailwindcss-windows-x64.exe` binary and falls back to PowerShell
`Invoke-WebRequest` when `curl`/`wget` are unavailable. You should not
need `TAILWIND_SKIP=1` for normal Windows builds.

From another PowerShell window in the repo:

```powershell
target\debug\ai-memory.exe install-mcp --client claude-code --apply
target\debug\ai-memory.exe install-hooks --agent claude-code --apply
```

Native Windows builds automatically render `.ps1` lifecycle hooks. The
hook bundle ships matching `.sh` and `.ps1` event scripts, and tests
enforce one-to-one event/agent parity between them.

## Current Harness Caveats

Windows hook support is new and needs real-world testing against native
Windows agent builds.

- Claude Code may be used natively on Windows or from inside WSL2. Test
  which process launches the hooks before assuming path format.
- Codex, OpenCode, Cursor, Gemini CLI, and OpenClaw may each choose different
  Windows config locations or shell execution behavior. ai-memory uses
  the current best-known defaults, but they need validation on real
  installations.
- MCP over HTTP should be less path-sensitive than hooks, but
  `install-mcp --apply` still writes to a client-specific config file;
  confirm the agent actually loads it.
- OpenClaw, OpenCode, and OMP/Pi use generated TypeScript integrations
  rather than the shell hook bundle, so their Windows behavior depends
  on the host runtime loading those files correctly.

## Suggested Test Checklist

For WSL2:

1. Run all install commands inside WSL2.
2. Confirm generated hook commands reference `.sh` files under WSL paths.
3. Launch the agent from WSL2.
4. Call `memory_status` from the agent.
5. Send a prompt, then run `ai-memory status` or `ai-memory recent`.

For native Windows:

1. Run all install commands from PowerShell or `cmd.exe` using
   `ai-memory` / `ai-memory.ps1`.
2. Confirm generated hook commands reference `.ps1` files under your
   Windows home directory.
3. Launch the native Windows agent.
4. Call `memory_status` from the agent.
5. Send a prompt, then run `ai-memory status` or `ai-memory recent`.

Report which mode you tested, which agent and version you used, and
whether the hook command executed or failed with a path/shell error.
