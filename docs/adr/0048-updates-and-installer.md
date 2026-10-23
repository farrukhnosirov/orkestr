# ADR-0048 — App updates and the Windows installer

Status: accepted — implemented (wave 8, task 9; the installer run of task 10 builds on it)

## Context

The owner asked for automatic or manual updates with a configurable update
server (the server comes later), and — for now — manual updates by running a
newer installer over the old one: no conflicts, no duplicate entries in
*Apps & features*, data preserved. Updates concern the installed desktop
binary, not business data shared with Web/Mobile.

## Decision

### Split of responsibilities

* **`orkestr-updater`** (new crate, pure Rust, fully tested) owns everything
  that can be verified without a window: update settings, release manifests,
  signature verification, remote/local checks, downloads with progress,
  data backups, and the first-start checks of a new version. It also ships
  the release tool `orkestr-release` (sign / verify / manifest).
* **Desktop shell** (`apps/desktop/src-tauri/src/updates.rs`) drives it:
  status machine, scheduler, `updates_*` Tauri commands (main window only),
  the `orkestr://updates/status` event, and the install hand-off.
* **`tauri-plugin-updater`** performs the install: `Updater::restore_update`
  with our verified manifest + `Update::install(bytes)`, which verifies the
  signature **again** and launches the NSIS installer with `/UPDATE` (passive
  mode, restart after install). Its own commands are not granted to any
  window (tested); the network side of the plugin is not used.
* Updates are app management like window management, so they use Tauri
  commands, not the RPC protocol (§67 exception, same as `shell_*`).

### Settings

`<config_dir>/updates.json` (shell-owned): `remoteEnabled` (default **off**),
`endpoint` (default `https://updates.orkestr.dev/{{target}}/{{arch}}/{{current_version}}`,
variables `{{target}} {{arch}} {{current_version}} {{channel}} {{bundle_type}}`;
without `{{channel}}`, beta adds `?channel=beta`), `channel` (`stable` | `beta`;
stable never offers pre-releases), `checkOnStartup` (20 s after start),
`checkIntervalHours` (0 = never, ≤ 168), `autoDownload` (signed updates only),
`localPath`, `backupRetention` (1–50, default 5). Endpoints must be `https://`
(`http://` only for loopback).

### Sources

* **Remote:** `latest.json` in the plugin's format (static `platforms` map or
  the dynamic single-target form; `204` = no update). Keys looked up in the
  plugin's order: `windows-x86_64-nsis`, `windows-x86_64`.
* **Local channel (manual updates without a server):** a release folder
  (e.g. a network share — its `latest.json`, else the newest `Orkestr_*`
  installer), a `latest.json` (relative URLs resolve against its folder), or an
  installer file. A local installer with a `.sig` next to it is verified at
  check time; without one it is offered as **unsigned** and needs an explicit
  consent checkbox — equivalent to the user double-clicking it; it is then
  started interactively (not through the plugin). Automatic checks use the
  remote endpoint when enabled, else the saved local path.

### Signatures and keys

Minisign (Ed25519), base64-wrapped — exactly what the plugin verifies. The
trusted comment carries `version:X.Y.Z` and the signer checks it against the
manifest version (a tampered manifest cannot pair a new version number with
an old installer; `restore_update` requires it). The Tauri CLI 2.12 signer
does not record the version, so releases are signed by `orkestr-release`
(minisign crate, reads the `tauri signer generate` key format).

* Private key: `%USERPROFILE%\.orkestr-tools\keys\updater.key`, password in
  `updater.key.password` next to it — **never in the repository**. Public key
  (`updater.key.pub`) is committed in `tauri.conf.json` →
  `plugins.updater.pubkey` (tested to be a public key). Losing the private key
  means existing installs can no longer verify updates: back it up offline.
* `bundle.createUpdaterArtifacts` is `false`: the bundler does not need the key;
  the release script signs.

### Install flow (never automatic)

Download → verify → **Ready**. The user chooses *Install now and restart*
(pre-update backup → embedded core shut down cleanly → plugin install → app
exits; NSIS updates in place and restarts it) or *Install when Orkestr closes*
(same on `RunEvent::Exit`, without restart). A failure after the core stopped
restarts the app. The scheduled install is in memory: if the app is killed,
the user re-confirms after the next check.

### Data safety

* User data is never inside the install folder: `%APPDATA%\Orkestr\Orkestr\{data,config}`
  (or `ORKESTR_HOME`), cache `%LOCALAPPDATA%\Orkestr\Orkestr\cache`.
* **Backups** (`<data_dir>/backups/orkestr-backup-<ts>-<reason>-v<from>[-to-v<to>].zip`):
  the SQLite DBs of the data dir and its direct subfolders (`orkestr.db`,
  `sync/sync.db`, …) via the SQLite **online-backup API** (consistent while
  the core writes; WAL included), plus small `*.json` settings of data and
  config dirs. Excluded: `objects/` (append-only, content-addressed),
  downloaded runtimes (`vscode-server/`, `extensions/`), browser profiles
  (`wv2/`, `EBWebView/` — WebView2 keeps its DBs locked), logs, caches, and
  secrets (`*.key`, `*.token`, `*.pem`). A busy source is retried for at most
  30 s: `orkestr.db` failing aborts the backup (the install is not started),
  any other DB is listed as `skipped`. Taken before every
  install (`preUpdate`), on the first start of a new version before
  migrations (`preMigration` — this also covers updates installed manually by
  running a new installer), and on demand. Retention prunes the oldest.
  Never restored automatically.
* **First start of a version** (`startup::prepare_start`, before the core
  opens the DB): reads `<data_dir>/app-version.json` (version, previous
  version — the rollback info shown in Settings), compares the applied
  `_orkestr_migrations` with `orkestr_core::migrations::all()`. Unknown ids
  (data written by a newer Orkestr) → the shell refuses to start the core
  with `database_newer: …` (localized on the splash screen, pointing to the
  backups) and leaves the data and the marker untouched. The storage migrator
  refuses the same case as a second line of defence.
* **Rollback** = install the previous installer over the current one
  (`allowDowngrades: true`); data stays. If the newer version already
  migrated the data, the downgrade guard blocks the old one and the user
  restores the pre-update backup.

### Installer (Tauri bundler)

* **Single version source:** `[workspace.package] version` in `Cargo.toml`;
  `tauri.conf.json` has no `version` (Tauri then uses the crate version);
  the release script bumps Cargo.toml and the root/desktop `package.json`.
* **NSIS (primary), per-user** (`installMode: currentUser`): installs to
  `%LOCALAPPDATA%\Orkestr`, no UAC prompt, updates and autostart work without
  elevation, and the in-app updater can replace the binary silently.
  Per-machine installs would need admin rights for every update. The
  uninstall key `HKCU\…\Uninstall\Orkestr` is derived from `productName`, so a
  new version replaces the old entry in place (verified: one entry after
  0.1.0 → 0.1.1). Never rename `productName` / `identifier`.
* **MSI (secondary, enterprise / GPO):** per-machine (WiX), fixed
  `upgradeCode` `c6529465-36f6-467f-aca3-22bbb51d90b1` (major upgrade replaces
  in place; never change it). Do not mix MSI and NSIS on one machine — they
  are two products for Windows.
* Start menu shortcut (folder `Orkestr`); desktop shortcut optional (finish
  page checkbox; created automatically in passive mode, never touched by
  updates).
* Installer languages English + Russian with a language selector.
* WebView2: `downloadBootstrapper` (silent) — small installer, Windows 11
  ships WebView2; on machines without it the installer downloads it.
* Uninstall keeps user data by default. The `NSIS_HOOK_POSTUNINSTALL` hook
  (`windows/installer-hooks.nsh`) removes the disposable cache, and the data +
  settings only when the user ticks *Delete the application data* (and never
  in `/UPDATE` mode).
* **Code signing:** not configured — installers are unsigned (SmartScreen
  warns on first run). `build-installer.ps1 -CertThumbprint <sha1>` signs with
  a certificate from the Windows store (sha256 digest, DigiCert timestamp).
* **Sidecars:** none. The desktop embeds the core (`orkestrd` is optional
  for headless use and not bundled); VS Code REH (with Node), extensions, and
  local model/voice engines are downloaded at runtime into the data dir.

### Release process

`scripts/release/build-installer.ps1 [-Version X.Y.Z] [-Notes md] [-NotesRu md]
[-Channel stable|beta] [-BaseUrl https://…]` → `dist-release\<version>\`
with the NSIS installer, MSIs, `.sig` files, `latest.json` and
`SHA256SUMS.txt`. Upload the folder to the update server (or share it as a
release folder for the local channel).

## Consequences

* Update checks, downloads and verification are covered by unit and
  integration tests against a local HTTP server (newer / same / beta /
  bad signature / other key / tampered local file / unsigned / offline /
  HTTP 500 / captive-portal HTML / cancel).
* The update server only has to serve static `latest.json` + files.
* Unsigned (Authenticode) installers trigger SmartScreen until a code-signing
  certificate exists.
