//! App updates (ADR-0048): Settings → Updates and the title-bar badge.
//!
//! * Checks the configured remote endpoint (off until an update server
//!   exists) or a local release folder / `latest.json` / installer — on
//!   startup, periodically and on demand ([`orkestr_updater::Checker`]).
//! * Downloads with progress and verifies the signature against the updater
//!   public key of `tauri.conf.json` (`plugins.updater.pubkey`).
//! * Installs **only on the user's request** — now, or when the app quits:
//!   data backup → embedded core shut down → `tauri-plugin-updater`
//!   (`restore_update` + `install`, which re-verifies and runs the NSIS
//!   installer in `/UPDATE` mode; it replaces the app in place and never
//!   touches the data dirs).
//! * Before the core starts, [`startup_check`] protects data written by a
//!   newer Orkestr (downgrade) and backs the data up before a new version
//!   migrates it.
//!
//! This is app management for the installed binary (like window
//! management), not business data, hence Tauri commands rather than RPC (§67).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use orkestr_platform::Paths;
use orkestr_updater::backup::backups_dir;
use orkestr_updater::{
    create_backup, list_backups, prepare_start, BackupReason, BackupRequest, CheckOutcome, Checker, ErrorInfo, Phase,
    Progress, StartRequest, StartupReport, Target, UpdateError, UpdateSettings, UpdateSource, UpdateStatus,
};
use semver::Version;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt as _;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Event with the full [`UpdateStatus`] (main window).
pub const UPDATES_STATUS_EVENT: &str = "orkestr://updates/status";
/// Delay before the startup check (let the app settle first).
const STARTUP_CHECK_DELAY: Duration = Duration::from_secs(20);
/// How often the scheduler re-evaluates whether a check is due.
const SCHEDULER_TICK: Duration = Duration::from_secs(5 * 60);
/// Minimum spacing of progress events.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);

/// This build's version (workspace version — the single version source).
#[must_use]
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|_| Version::new(0, 0, 0))
}

/// Bundle type of the running install. Updates replace an NSIS install with
/// the NSIS installer (`windows-x86_64-nsis` / `windows-x86_64` manifest keys).
fn target() -> Target {
    Target::current(Some("nsis"))
}

struct Inner {
    status: UpdateStatus,
    /// Verified installer ready to install.
    downloaded: Option<PathBuf>,
    /// Unsigned local installer the user explicitly accepted.
    unsigned_accepted: bool,
}

/// Shell-side update state (managed by Tauri).
pub struct Updates {
    paths: Paths,
    pubkey: String,
    inner: Mutex<Inner>,
    cancel: std::sync::Mutex<Option<CancellationToken>>,
    started: Instant,
}

impl Updates {
    /// Loads the settings; `pubkey` comes from `plugins.updater.pubkey`.
    #[must_use]
    pub fn new(paths: Paths, pubkey: String) -> Self {
        let settings = UpdateSettings::load(&paths.config_dir);
        let backups = list_backups(&paths.data_dir).unwrap_or_default();
        let status = UpdateStatus {
            current_version: current_version().to_string(),
            phase: Phase::Idle,
            settings,
            available: None,
            progress: None,
            last_checked_at: None,
            latest_seen: None,
            last_error: None,
            install_on_exit: false,
            startup: None,
            backups_dir: backups_dir(&paths.data_dir).display().to_string(),
            backups,
        };
        Self {
            paths,
            pubkey,
            inner: Mutex::new(Inner { status, downloaded: None, unsigned_accepted: false }),
            cancel: std::sync::Mutex::new(None),
            started: Instant::now(),
        }
    }

    /// Current status.
    pub async fn status(&self) -> UpdateStatus {
        self.inner.lock().await.status.clone()
    }

    async fn update(&self, app: &AppHandle, f: impl FnOnce(&mut Inner)) -> UpdateStatus {
        let status = {
            let mut inner = self.inner.lock().await;
            f(&mut inner);
            inner.status.clone()
        };
        emit(app, &status);
        status
    }

    /// Records what [`startup_check`] found.
    pub async fn set_startup(&self, report: StartupReport) {
        let backups = list_backups(&self.paths.data_dir).unwrap_or_default();
        let mut inner = self.inner.lock().await;
        inner.status.startup = Some(report);
        inner.status.backups = backups;
    }

    fn source(settings: &UpdateSettings, local_override: Option<&str>) -> Result<UpdateSource, UpdateError> {
        if let Some(p) = local_override.map(str::trim).filter(|p| !p.is_empty()) {
            return Ok(UpdateSource::Local { path: PathBuf::from(p) });
        }
        if settings.remote_enabled {
            return Ok(UpdateSource::Remote { endpoint: settings.endpoint.clone() });
        }
        match &settings.local_path {
            Some(p) => Ok(UpdateSource::Local { path: PathBuf::from(p) }),
            None => Err(UpdateError::NotConfigured),
        }
    }

    fn checker(&self, settings: &UpdateSettings) -> Result<Checker, UpdateError> {
        Checker::new(self.pubkey.clone(), target(), current_version(), settings.channel)
    }

    /// Checks for an update. `local` = a file/folder the user just picked.
    pub async fn check(self: &Arc<Self>, app: &AppHandle, local: Option<String>, automatic: bool) -> UpdateStatus {
        let settings = {
            let mut inner = self.inner.lock().await;
            if matches!(inner.status.phase, Phase::Checking | Phase::Downloading | Phase::Installing) {
                return inner.status.clone();
            }
            if inner.status.install_on_exit && automatic {
                return inner.status.clone();
            }
            inner.status.phase = Phase::Checking;
            inner.status.last_error = None;
            inner.status.settings.clone()
        };
        emit(app, &self.status().await);
        let outcome = match (Self::source(&settings, local.as_deref()), self.checker(&settings)) {
            (Ok(source), Ok(checker)) => checker.check(&source).await,
            (Err(e), _) | (_, Err(e)) => Err(e),
        };
        if let Err(e) = &outcome {
            tracing::info!(error = %e, automatic, "update check failed");
        }
        let status = self
            .update(app, |inner| {
                let s = &mut inner.status;
                s.last_checked_at = Some(Utc::now());
                match outcome {
                    Ok(CheckOutcome::UpToDate { latest }) => {
                        s.phase = Phase::UpToDate;
                        s.latest_seen = latest;
                        s.available = None;
                        s.install_on_exit = false;
                        inner.downloaded = None;
                    }
                    Ok(CheckOutcome::Available(info)) => {
                        let same = s
                            .available
                            .as_ref()
                            .is_some_and(|a| a.version == info.version && a.location == info.location);
                        s.latest_seen = Some(info.version.clone());
                        s.available = Some(*info);
                        if same && inner.downloaded.is_some() {
                            s.phase = Phase::Ready;
                        } else {
                            s.phase = Phase::Available;
                            s.install_on_exit = false;
                            inner.downloaded = None;
                            inner.unsigned_accepted = false;
                        }
                    }
                    Err(e) => {
                        // Keep an earlier offer; a failed periodic check only records the error.
                        s.phase = if s.available.is_some() {
                            if inner.downloaded.is_some() {
                                Phase::Ready
                            } else {
                                Phase::Available
                            }
                        } else {
                            Phase::Error
                        };
                        s.last_error = Some(e.to_info());
                    }
                }
            })
            .await;
        if automatic && status.phase == Phase::Available && status.settings.auto_download {
            let signed = status.available.as_ref().is_some_and(|a| a.signature.is_some());
            if signed {
                self.download(app, false).await;
            }
        }
        self.status().await
    }

    /// Downloads (or, for a local file, verifies) the offered update in the background.
    pub async fn download(self: &Arc<Self>, app: &AppHandle, allow_unsigned: bool) -> UpdateStatus {
        let (info, settings) = {
            let mut inner = self.inner.lock().await;
            let Some(info) = inner.status.available.clone() else { return inner.status.clone() };
            if matches!(inner.status.phase, Phase::Downloading | Phase::Installing) {
                return inner.status.clone();
            }
            inner.status.phase = Phase::Downloading;
            inner.status.progress = Some(Progress { downloaded: 0, total: info.size });
            inner.status.last_error = None;
            (info, inner.status.settings.clone())
        };
        emit(app, &self.status().await);
        let cancel = CancellationToken::new();
        if let Ok(mut slot) = self.cancel.lock() {
            *slot = Some(cancel.clone());
        }
        let this = Arc::clone(self);
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let dest = this.paths.cache_dir.join("updates");
            let result = match this.checker(&settings) {
                Ok(checker) => {
                    let mut last = Instant::now() - PROGRESS_EVERY;
                    let progress_app = app2.clone();
                    let progress_this = Arc::clone(&this);
                    checker
                        .fetch(&info, &dest, allow_unsigned, &cancel, move |done, total| {
                            let finished = total.is_some_and(|t| t == done);
                            if last.elapsed() < PROGRESS_EVERY && !finished {
                                return;
                            }
                            last = Instant::now();
                            let (a, t) = (progress_app.clone(), Arc::clone(&progress_this));
                            tauri::async_runtime::spawn(async move {
                                t.update(&a, |inner| {
                                    if inner.status.phase == Phase::Downloading {
                                        inner.status.progress = Some(Progress { downloaded: done, total });
                                    }
                                })
                                .await;
                            });
                        })
                        .await
                }
                Err(e) => Err(e),
            };
            if let Ok(mut slot) = this.cancel.lock() {
                *slot = None;
            }
            this.update(&app2, |inner| match result {
                Ok(path) => {
                    tracing::info!(path = %path.display(), version = %info.version, "update ready to install");
                    inner.downloaded = Some(path);
                    inner.unsigned_accepted = allow_unsigned && info.signature.is_none();
                    inner.status.phase = Phase::Ready;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "update download failed");
                    inner.status.phase = Phase::Available;
                    inner.status.progress = None;
                    if !matches!(e, UpdateError::Cancelled) {
                        inner.status.last_error = Some(e.to_info());
                    }
                }
            })
            .await;
        });
        self.status().await
    }

    /// Cancels a running download.
    pub fn cancel(&self) {
        if let Ok(slot) = self.cancel.lock() {
            if let Some(c) = slot.as_ref() {
                c.cancel();
            }
        }
    }

    /// Saves new preferences.
    pub async fn set_settings(&self, app: &AppHandle, settings: UpdateSettings) -> Result<UpdateStatus, String> {
        let settings = settings.normalized().map_err(|e| error_json(&e))?;
        settings.save(&self.paths.config_dir).map_err(|e| error_json(&e))?;
        Ok(self.update(app, |inner| inner.status.settings = settings).await)
    }

    /// Takes a data backup now.
    pub async fn backup_now(
        &self,
        app: &AppHandle,
        reason: BackupReason,
        to: Option<String>,
    ) -> Result<UpdateStatus, String> {
        let retention = self.inner.lock().await.status.settings.backup_retention as usize;
        let paths = self.paths.clone();
        let from = current_version().to_string();
        let result = tokio::task::spawn_blocking(move || {
            create_backup(&BackupRequest {
                data_dir: &paths.data_dir,
                config_dir: Some(&paths.config_dir),
                reason,
                from_version: &from,
                to_version: to.as_deref(),
                retention,
            })
        })
        .await
        .map_err(|e| e.to_string())?;
        let backups = list_backups(&self.paths.data_dir).unwrap_or_default();
        match result {
            Ok(_) => Ok(self.update(app, |inner| inner.status.backups = backups).await),
            Err(e) => Err(error_json(&e)),
        }
    }

    /// Prepares the install: the verified file and, for signed updates, the
    /// plugin's `Update` (which verifies the bytes once more).
    async fn prepared(&self, app: &AppHandle) -> Result<Prepared, UpdateError> {
        let inner = self.inner.lock().await;
        let info = inner.status.available.clone().ok_or(UpdateError::NotConfigured)?;
        let path = inner.downloaded.clone().ok_or(UpdateError::NotConfigured)?;
        if info.signature.is_none() {
            if !inner.unsigned_accepted {
                return Err(UpdateError::Unsigned);
            }
            return Ok(Prepared { version: info.version, path, plugin: None });
        }
        drop(inner);
        // The endpoint is never contacted: `restore_update` builds the update from our verified manifest.
        let dummy =
            tauri::Url::parse("https://localhost/orkestr-updates").map_err(|e| UpdateError::Io(e.to_string()))?;
        let updater = app
            .updater_builder()
            .endpoints(vec![dummy])
            .and_then(tauri_plugin_updater::UpdaterBuilder::build)
            .map_err(|e| UpdateError::Io(format!("updater: {e}")))?;
        let update = updater
            .restore_update(info.manifest.clone())
            .map_err(|e| UpdateError::BadManifest(format!("updater: {e}")))?
            .ok_or_else(|| UpdateError::BadManifest(format!("{} is not newer than this build", info.version)))?;
        let bytes = tokio::fs::read(&path).await?;
        update.verify(&bytes).map_err(|e| UpdateError::BadSignature(e.to_string()))?;
        Ok(Prepared { version: info.version, path, plugin: Some((update, bytes)) })
    }

    /// Installs now: backup, stop the core, hand over to the installer (exits the app).
    pub async fn install_now(
        &self,
        app: &AppHandle,
        core: Arc<Mutex<Option<orkestr_core::CoreHandle>>>,
    ) -> Result<UpdateStatus, String> {
        let prepared = match self.prepared(app).await {
            Ok(p) => p,
            Err(e) => return Err(error_json(&e)),
        };
        self.update(app, |inner| inner.status.phase = Phase::Installing).await;
        if let Err(e) = self.backup_now(app, BackupReason::PreUpdate, Some(prepared.version.clone())).await {
            self.update(app, |inner| {
                inner.status.phase = Phase::Ready;
                inner.status.last_error = serde_json::from_str::<ErrorInfo>(&e).ok();
            })
            .await;
            return Err(e);
        }
        // Close the database cleanly before the installer replaces the binary.
        if let Some(handle) = core.lock().await.take() {
            handle.shutdown().await;
        }
        tracing::warn!(version = %prepared.version, "launching the update installer");
        match launch(app, prepared, true) {
            // Signed updates never get here (the plugin exits the process); an
            // unsigned installer was started and the app is exiting.
            Ok(()) => Ok(self.status().await),
            Err(e) => {
                // The core is already down: restart the app rather than leave it broken.
                tracing::error!(error = %e, "the update installer could not be started; restarting");
                app.restart()
            }
        }
    }

    /// Schedules the install for when the app quits.
    pub async fn install_on_exit(&self, app: &AppHandle, on: bool) -> Result<UpdateStatus, String> {
        if on {
            // Validate now so a broken update is reported while the user is here.
            if let Err(e) = self.prepared(app).await {
                return Err(error_json(&e));
            }
        }
        Ok(self.update(app, |inner| inner.status.install_on_exit = on && inner.downloaded.is_some()).await)
    }

    /// Called on `RunEvent::Exit` after the core has shut down.
    pub fn run_pending_install(&self, app: &AppHandle) {
        let pending = tauri::async_runtime::block_on(async { self.inner.lock().await.status.install_on_exit });
        if !pending {
            return;
        }
        let prepared = tauri::async_runtime::block_on(async {
            let version = self.inner.lock().await.status.available.as_ref().map(|a| a.version.clone());
            if let Err(e) = self.backup_now(app, BackupReason::PreUpdate, version).await {
                tracing::error!(error = %e, "pre-update backup failed; not installing");
                return None;
            }
            self.prepared(app).await.inspect_err(|e| tracing::error!(error = %e, "pending update invalid")).ok()
        });
        if let Some(p) = prepared {
            tracing::warn!(version = %p.version, "installing the pending update on exit");
            if let Err(e) = launch(app, p, false) {
                tracing::error!(error = %e, "could not start the pending update installer");
            }
        }
    }
}

struct Prepared {
    version: String,
    path: PathBuf,
    plugin: Option<(tauri_plugin_updater::Update, Vec<u8>)>,
}

/// Hands over to the installer. Signed updates go through the plugin
/// (NSIS `/UPDATE` + passive mode; exits the process). An unsigned local
/// installer the user accepted is started interactively, then the app exits.
fn launch(app: &AppHandle, prepared: Prepared, restart: bool) -> Result<(), String> {
    match prepared.plugin {
        Some((update, bytes)) => {
            let update = update.restart_after_install(restart);
            update.install(bytes).map_err(|e| e.to_string())
        }
        None => {
            orkestr_platform::process::background_command(&prepared.path).spawn().map_err(|e| e.to_string())?;
            app.exit(0);
            Ok(())
        }
    }
}

fn emit(app: &AppHandle, status: &UpdateStatus) {
    if let Err(e) = app.emit_to(crate::MAIN_WINDOW, UPDATES_STATUS_EVENT, status) {
        tracing::debug!(error = %e, "could not send the update status");
    }
}

/// Command errors are `{code, message}` JSON so the UI can localize them.
fn error_json(e: &UpdateError) -> String {
    serde_json::to_string(&e.to_info()).unwrap_or_else(|_| e.to_string())
}

/// Runs before the embedded core opens the data (see [`orkestr_updater::startup`]).
/// `Err` only for data written by a newer Orkestr (`database_newer: …`).
pub fn startup_check(paths: &Paths, retention: usize) -> Result<Option<StartupReport>, String> {
    let all = orkestr_core::migrations::all();
    let known: Vec<&str> = all.iter().map(|m| m.id).collect();
    let version = current_version();
    match prepare_start(&StartRequest {
        data_dir: &paths.data_dir,
        config_dir: Some(&paths.config_dir),
        version: &version,
        known_migrations: &known,
        retention,
    }) {
        Ok(report) => Ok(Some(report)),
        Err(e @ UpdateError::DatabaseNewer(_)) => Err(format!(
            "{}: {e}. Install that version again, or restore a backup from {}",
            e.code(),
            backups_dir(&paths.data_dir).display()
        )),
        Err(e) => {
            tracing::warn!(error = %e, "startup update checks failed; continuing");
            Ok(None)
        }
    }
}

/// Startup + periodic checks (never installs by itself).
pub fn spawn_scheduler(app: AppHandle, updates: Arc<Updates>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_CHECK_DELAY).await;
        loop {
            let status = updates.status().await;
            let s = &status.settings;
            let configured = s.remote_enabled || s.local_path.is_some();
            let interval = Duration::from_secs(u64::from(s.check_interval_hours) * 3600);
            let due = match status.last_checked_at {
                None => s.check_on_startup || (!interval.is_zero() && updates.started.elapsed() >= interval),
                Some(at) => !interval.is_zero() && (Utc::now() - at).to_std().is_ok_and(|elapsed| elapsed >= interval),
            };
            if configured && due {
                updates.check(&app, None, true).await;
            }
            tokio::time::sleep(SCHEDULER_TICK).await;
        }
    });
}

/// Arguments of `updates_install`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallWhen {
    /// Now (exits the app).
    Now,
    /// When the app quits.
    OnExit,
    /// Cancel a scheduled install-on-exit.
    Cancel,
}

type State<'a> = tauri::State<'a, Arc<Updates>>;

/// Current update status.
#[tauri::command]
pub async fn updates_status(state: State<'_>) -> Result<UpdateStatus, String> {
    Ok(state.status().await)
}

/// Saves the update preferences.
#[tauri::command]
pub async fn updates_set_settings(
    settings: UpdateSettings,
    app: AppHandle,
    state: State<'_>,
) -> Result<UpdateStatus, String> {
    state.set_settings(&app, settings).await
}

/// Checks now (configured source, or the picked local file/folder).
#[tauri::command]
pub async fn updates_check(
    local_path: Option<String>,
    app: AppHandle,
    state: State<'_>,
) -> Result<UpdateStatus, String> {
    Ok(state.inner().check(&app, local_path, false).await)
}

/// Starts downloading / verifying the offered update.
#[tauri::command]
pub async fn updates_download(allow_unsigned: bool, app: AppHandle, state: State<'_>) -> Result<UpdateStatus, String> {
    Ok(state.inner().download(&app, allow_unsigned).await)
}

/// Cancels the download.
#[tauri::command]
pub async fn updates_cancel(state: State<'_>) -> Result<UpdateStatus, String> {
    state.cancel();
    Ok(state.status().await)
}

/// Installs now / on exit, or cancels a scheduled install.
#[tauri::command]
pub(crate) async fn updates_install(
    when: InstallWhen,
    app: AppHandle,
    state: State<'_>,
    core: tauri::State<'_, crate::CoreState>,
) -> Result<UpdateStatus, String> {
    match when {
        InstallWhen::Now => state.install_now(&app, Arc::clone(&core.handle)).await,
        InstallWhen::OnExit => state.install_on_exit(&app, true).await,
        InstallWhen::Cancel => state.install_on_exit(&app, false).await,
    }
}

/// Takes a manual data backup.
#[tauri::command]
pub async fn updates_backup_now(app: AppHandle, state: State<'_>) -> Result<UpdateStatus, String> {
    state.backup_now(&app, BackupReason::Manual, None).await
}

/// The updater public key from `tauri.conf.json` (`plugins.updater.pubkey`).
#[must_use]
pub fn configured_pubkey(app: &tauri::App) -> String {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(|k| k.as_str())
        .unwrap_or_default()
        .to_owned()
}

/// Resolves the app paths (`ORKESTR_HOME` aware).
pub fn paths() -> Option<Paths> {
    Paths::resolve().ok()
}

/// Makes the managed state available to `boot_core`.
pub fn managed(app: &AppHandle) -> Option<Arc<Updates>> {
    app.try_state::<Arc<Updates>>().map(|s| Arc::clone(s.inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_prefers_the_picked_file_then_remote_then_the_saved_local_path() {
        let mut s = UpdateSettings::default();
        assert!(matches!(Updates::source(&s, None), Err(UpdateError::NotConfigured)), "remote is off by default");
        s.local_path = Some("D:/releases".into());
        assert_eq!(Updates::source(&s, None).unwrap(), UpdateSource::Local { path: "D:/releases".into() });
        s.remote_enabled = true;
        assert!(matches!(Updates::source(&s, None).unwrap(), UpdateSource::Remote { .. }));
        assert_eq!(Updates::source(&s, Some(" C:/x.exe ")).unwrap(), UpdateSource::Local { path: "C:/x.exe".into() });
    }

    #[test]
    fn command_errors_carry_codes() {
        let e: ErrorInfo = serde_json::from_str(&error_json(&UpdateError::Unsigned)).unwrap();
        assert_eq!(e.code, "unsigned");
    }

    #[test]
    fn version_comes_from_the_workspace() {
        assert_eq!(current_version().to_string(), env!("CARGO_PKG_VERSION"));
    }

    /// Release configuration (ADR-0048): one version source, the committed
    /// public key, per-user NSIS replacing in place, fixed MSI upgrade code.
    #[test]
    fn bundle_and_updater_configuration() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert!(conf.get("version").is_none(), "the version comes from Cargo (workspace) only");
        assert_eq!(conf["identifier"], "dev.orkestr.desktop", "the identifier must never change");
        assert_eq!(conf["productName"], "Orkestr", "the NSIS uninstall key is derived from the product name");
        let pubkey = conf["plugins"]["updater"]["pubkey"].as_str().unwrap();
        assert!(orkestr_updater::signature::verify(b"x", "", pubkey, &current_version()).is_err());
        let text =
            String::from_utf8(base64::Engine::decode(&base64::engine::general_purpose::STANDARD, pubkey).unwrap())
                .unwrap();
        assert!(text.contains("minisign public key"), "a public key, never a private one");
        let win = &conf["bundle"]["windows"];
        assert_eq!(win["nsis"]["installMode"], "currentUser");
        assert_eq!(win["nsis"]["languages"], serde_json::json!(["English", "Russian"]));
        assert_eq!(
            win["wix"]["upgradeCode"], "c6529465-36f6-467f-aca3-22bbb51d90b1",
            "never change the MSI upgrade code"
        );
        assert_eq!(conf["bundle"]["createUpdaterArtifacts"], false, "update artifacts are signed by scripts/release");
        let hooks = include_str!("../windows/installer-hooks.nsh");
        assert!(hooks.contains("$DeleteAppDataCheckboxState = 1"), "data is removed only on explicit request");
    }
}
