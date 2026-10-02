# ADR-0014: Terminals on portable-pty (ConPTY on Windows), streamed over RPC

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 1 (local), 4 (remote terminal through the gateway)
- **Spec:** §15 (multiple sessions, user- and agent-owned terminals, tabs, stream, history, approval interception; shells: PowerShell, CMD, Git Bash, WSL, bash, zsh, fish, custom), §46 (Shell), §62.2, §62.11, §64 ("exposed raw terminal port" forbidden)
- **Review:** RR-P6, RR-P7
- **Spikes:** SP-03, SP-17 · **Risks:** R-02, R-11

## Context

The terminal is both a user tool and an agent tool. User terminals must feel native: full TTY, colors, TUIs such as `vim` and `htop`, resize. Agent commands must go through the Dangerous Action Guard (ADR-0010). Streams go to Desktop now and to Web and Mobile later, through the RPC only (§64: never a raw terminal port).

## Decision drivers

1. Real PTY semantics on Windows (ConPTY), macOS and Linux.
2. A single streaming path over the WebSocket protocol (ADR-0005).
3. Agents cannot bypass the guard by typing keystrokes.
4. Late subscribers and reconnects get the recent scrollback.
5. Shell detection without hardcoded paths (§46).

## Considered options

| Option | Verdict |
|---|---|
| **`portable-pty` (wezterm) in `orkestr-terminal`, xterm.js in the UI** | **Chosen**. ConPTY, forkpty and resize are battle-tested in wezterm |
| Plain pipes (`tokio::process`) for everything | Rejected for user terminals (no TTY, so TUIs break). **Used** for agent one-shot commands (below) |
| `tauri-plugin-pty` / node-pty | Rejected. The terminal would be tied to the desktop host and bypass the core and RPC |
| winpty | Obsolete. ConPTY needs Windows 10 1809 or later, which matches §2's Windows 10/11 target |

## Decision

### Two execution paths

| Path | Used by | Mechanism | Guard |
|---|---|---|---|
| **Interactive PTY session** | Users, and agents that explicitly need a TTY | `portable-pty` session, raw bytes in and out | User: none for the local user (the user is trusted); remote device sessions need `Terminal` scope (RR-P7). Agent: **write is line-gated** (below) |
| **Command execution** | Agent tool `run_command` (default for agents) | `tokio::process` with the shell's `-c`/`-Command`/`/C`, cwd confined to the worktree, timeout, output captured and streamed | Full `PermissionEngine::check(Terminal, Resource::Command{…})` before spawn |

**Agent-owned PTYs** accept input only through `terminal.execLine {terminalId, line}`. Each line is classified and gated before it is written followed by `\r`. `terminal.write` (raw bytes) is **rejected** with `forbidden` for agent-owned sessions. Users can take over an agent terminal ("Take control"), which turns it into a user-owned session and pauses the agent's access.

### API (`orkestr-terminal`)

```rust
pub struct TerminalSpec { pub project_id: ProjectId, pub owner: TerminalOwner /* User{device_id} | Agent{agent_id} */,
                          pub shell: ShellSpec /* Detected(ShellKind) | Custom{program, args} */, pub cwd: PathBuf,
                          pub env: Vec<EnvBinding> /* literal or SecretReferenceId */, pub cols: u16, pub rows: u16 }

#[async_trait::async_trait]
pub trait TerminalManager: Send + Sync + 'static {
    async fn create(&self, spec: TerminalSpec) -> Result<TerminalInfo, TerminalError>;
    async fn write(&self, id: &TerminalId, by: &Actor, data: &[u8]) -> Result<(), TerminalError>;   // user-owned only
    async fn exec_line(&self, id: &TerminalId, by: &Actor, line: &str) -> Result<(), TerminalError>; // gated
    async fn resize(&self, id: &TerminalId, cols: u16, rows: u16) -> Result<(), TerminalError>;
    async fn kill(&self, id: &TerminalId, by: &Actor) -> Result<(), TerminalError>;
    async fn snapshot(&self, id: &TerminalId) -> Result<TerminalSnapshot, TerminalError>; // last N bytes of scrollback, as UTF-8
    async fn list(&self, project: Option<&ProjectId>) -> Result<Vec<TerminalInfo>, TerminalError>;
}
```

- **Shell detection** (`PlatformAdapter::shell().detect() -> Vec<ShellInfo>`):
  - Windows: `pwsh`, `powershell`, `cmd` (`%ComSpec%`), Git Bash (from the git install location via `git --exec-path`, then `../../bin/bash.exe`), WSL distros (`wsl -l -q`).
  - Unix: `$SHELL`, `/etc/shells`.
  - No hardcoded user paths.
- **Streaming:**
  - A reader thread per PTY feeds a ring buffer (scrollback default 2 MiB per session).
  - An incremental UTF-8 decoder carries incomplete sequences across chunk boundaries.
  - Output is coalesced into ≤ 16 ms batches and capped at 64 KiB per event, then published as `TerminalOutput{terminalId, data}` on topic `terminal:<id>`. These are ephemeral events (ADR-0018).
  - Process exit emits `TerminalExited{terminalId, code, reason}`.
- **Redaction:** agent-owned output goes through the `Redactor` (ADR-0011). User-owned output is redacted by default (setting `terminal.redactUserSessions`).
- **History:** commands run through `exec_line` and `run_command` are stored in `terminal_commands(id, terminal_id, task_id, line_redacted, decision, exit_code, started_at, finished_at)`. On session end, full scrollback can optionally be persisted to the object store (`terminal.persistScrollback`, default off for user sessions and on for agent sessions, size-capped at 5 MiB).
- **Windows specifics:**
  - The ConPTY child is attached to a Job Object (ADR-0022) so the whole tree dies with the session.
  - `TERM=xterm-256color`.
  - Code page: `chcp 65001` is not forced. ConPTY is UTF-16 internally and portable-pty emits UTF-8.

## Consequences

- **Positive:** real TTY for users; agent commands cannot bypass the guard; one stream path for Desktop, Web and Mobile; reconnect-safe through `snapshot`.
- **Negative:** an agent that needs an interactive TUI (e.g. `npm init` prompts) has to work line by line. The agent prompts tell agents to use non-interactive flags. This is acceptable.
- **Negative:** ConPTY quirks (resize reflow, the initial clear-screen sequence) are covered by SP-03.
- **Phase 4:** remote terminals go through the same RPC behind the gateway, with per-device `Terminal` scope and an idle timeout. No raw port is ever exposed (§36, §64).

## Implementation notes (Phase 0 foundation, 2026-10-02)

Not implemented yet. `orkestr-terminal` has the skeleton `TerminalManager { open(TerminalSpec) -> TerminalId, write(id, &[u8]), resize(id, cols, rows), close(id) }`; `portable-pty` is not a dependency yet. RPC names follow the code reservation: `terminal.open` / `terminal.close` (not `terminal.create` / `terminal.kill`, `../api/protocol.md` §8.27). The v1 `TerminalOutput` payload is `{terminalId, data}` (no `offset` yet). Shell detection exists in `orkestr-platform` (`ShellKind { PowerShell7, WindowsPowerShell, Cmd, GitBash, Wsl, Bash, Zsh, Fish, Sh }`) and `system.info.shells` lists the detected display names. SP-03 has not run.

## Implementation notes (Phase 1, terminal area)

Implemented in `crates/terminal` (`TerminalService`) and `crates/core/src/rpc/terminal.rs`:

- RPC: `terminal.shells`, `terminal.open`, `terminal.write`, `terminal.execLine`, `terminal.resize`, `terminal.rename`, `terminal.close`, `terminal.list`, `terminal.history`, `terminal.run`. Names follow the code reservation (`open`/`close`, not `create`/`kill`); `history` is the ADR's `snapshot`.
- `TerminalOutput` gained an optional `offset` (absolute byte offset of the chunk) so clients de-duplicate live events against `terminal.history {data, startOffset, endOffset}` on reconnect. `TerminalOutput` is routed **only** to `terminal:<id>` (and `*`), never to `project:`/`global`.
- Output pump: reader thread → bounded channel → pump task (incremental UTF-8 decoder, 16 ms / 32 KiB coalescing, 2 MiB scrollback). ConPTY's start-up cursor-position request (`ESC[6n`) is answered by the core and stripped from the stream, so headless/agent sessions don't hang and history replays don't make xterm.js answer it again.
- Agent safety: agent-owned PTYs are read-only for users (`forbidden`); their agent's input is assembled into lines and every line is checked by the `CommandGate` trait before it is written (escape sequences are rejected, Ctrl+C/Ctrl+D pass). `terminal.run` checks the gate **before** spawning. Until the security area wires a real gate, the core uses `UserOnlyGate` (user commands allowed, every agent command → `approval_required`).
- Non-PTY `run`: `tokio::process` with the shell dialect's argv (`cmd /d /s /c "…"`, PowerShell `-NoProfile -NonInteractive -Command` with UTF-8 output encoding, `sh -c`, `wsl -e sh -c`), sanitized env (`ORKESTR_*` and secret-looking variables removed; explicit `env` injected), cwd confined to the project root, capped output, timeout/cancellation, streaming into a per-agent read-only "command view" session. Known limitation: `cmd.exe` built-ins (`echo`) write the OEM code page to pipes; PowerShell and most tools emit UTF-8.
- Tree kill without Job Objects (ADR-0022 still pending in `orkestr-platform`): snapshot the tree via `sysinfo`, kill **top-down** level by level (a dead parent cannot run its next command), re-scan for late-spawned orphans, then the platform `kill_tree` as backstop.
- History: `terminal_commands` table (migration `terminal_0001_commands`) records every gated command with decision/exit code; the command text is passed through the core redactor. Scrollback is not persisted to the object store yet (`terminal.persistScrollback` not implemented).
