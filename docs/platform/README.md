# Platform documentation

`/docs/platform` is the platform folder required by MASTER_SPEC §59. The
platform abstraction itself (§65 deliverable #17) is
[`../architecture/platform-abstraction.md`](../architecture/platform-abstraction.md).
It lives in `architecture/` because the other C4 and abstraction deliverables
are there. Use this folder for per-OS operational material.

| Topic | Where |
|---|---|
| `PlatformAdapter` design, per-OS behaviour, capability matrix, what is built | [`../architecture/platform-abstraction.md`](../architecture/platform-abstraction.md) (§0 = as built) |
| Data directories and `ORKESTR_HOME` | same document, §0 "Paths as built" |
| Floating/transparent windows | [`../adr/0016-floating-robots-transparent-windows.md`](../adr/0016-floating-robots-transparent-windows.md), spike SP-04 |
| Terminals and shells | [`../adr/0014-portable-pty-terminals.md`](../adr/0014-portable-pty-terminals.md), spike SP-03 |
| Keyring and vault | [`../adr/0011-secrets-keyring-and-encrypted-vault.md`](../adr/0011-secrets-keyring-and-encrypted-vault.md), spike SP-18 |
| Process containment after a crash | [`../adr/0022-crash-recovery-and-interruption.md`](../adr/0022-crash-recovery-and-interruption.md), spike SP-13 |
| Build targets per OS | [`../architecture/monorepo.md`](../architecture/monorepo.md) §4.2 |

## Status (2026-10-02)

- The foundation was built and run on **Windows 11 only**. macOS and Linux code
  paths compile through `#[cfg]` branches in `crates/platform`, but no one has
  run them yet.
- Manual checklists for GUI and permission-dependent capabilities (macOS TCC
  prompts for the microphone, accessibility and notifications; Wayland
  limitations) belong in this folder. Testing strategy:
  `platform-abstraction.md` §4. None has been written yet, because none of
  those capabilities is implemented.

## Windows: npm/pnpm/yarn shims and the PowerShell execution policy

Node package managers install three shims per program on Windows: `npm`
(sh), `npm.cmd` and `npm.ps1`. Windows PowerShell resolves a bare `npm` to
`npm.ps1`, and the default execution policy (`Restricted`) refuses it
("running scripts is disabled on this system"). Orkestr never passes
`-ExecutionPolicy` and never changes the user's policy. Instead:

- **Quality checks** (`orkestr-quality` `TerminalExecutor`) run with
  `ShellSpec::Command`, the platform's non-interactive command shell from
  `orkestr_platform::shell::command_shell_preference()`. On Windows that is
  `cmd.exe /d /s /c "…"`, which resolves `npm` to `npm.cmd` through
  `PATHEXT`. macOS and Linux keep the default shell.
- **Non-PTY PowerShell runs** (agent `run_command`, `terminal.run`) prepend
  `orkestr_platform::shell::powershell_shim_prelude()`. For each name in
  `SCRIPT_SHIMS` with a `.cmd` on `PATH`, it aliases the name to that
  `.cmd`. The prelude is empty on other OSes.
- The command gate and `DangerousActionGuard` still classify the caller's own
  command, in the dialect of the shell that runs it. The guard also unwraps
  `cmd /d /s /c "…"` forms.
