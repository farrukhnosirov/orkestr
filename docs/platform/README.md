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
