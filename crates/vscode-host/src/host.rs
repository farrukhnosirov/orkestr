//! The VS Code host service: install / start / stop / supervise.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use orkestr_domain::{Arch, OsKind};
use orkestr_events::EventBus;
use orkestr_platform::ProcessControl;
use orkestr_protocol::event::{EventKind, NewEvent, VsCodeProgressPayload, VsCodeStatusPayload};
use orkestr_protocol::methods::vscode::{
    VsCodeConfig, VsCodeConfigPatch, VsCodeInstallPhase, VsCodeInstallation, VsCodeProgress, VsCodeRelease,
    VsCodeRunning, VsCodeStartResult, VsCodeState, VsCodeStatus,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::error::{HostError, HostResult};
use crate::install::{self, Layout};
use crate::lifecycle::{next_state, Backoff, Trigger};
use crate::release::{self, ReleaseAsset};
use crate::server::{self, LaunchSpec};
use crate::target::{select_target, ServerTarget};

/// Lines of server output kept for `vscode.logs`.
const LOG_LINES: usize = 500;
/// Consecutive failed health checks before the server counts as hung.
const HEALTH_FAILURES: u32 = 3;

/// Construction options.
pub struct HostOptions {
    /// `<data_dir>/vscode-server`.
    pub root: PathBuf,
    /// Host OS (from `orkestr-platform`).
    pub os: OsKind,
    /// Host architecture (from `orkestr-platform`).
    pub arch: Arch,
    /// Process control (tree kill).
    pub process: Arc<dyn ProcessControl>,
    /// Event bus for status / progress events.
    pub events: Option<EventBus>,
    /// GitHub releases API base (tests point it elsewhere).
    pub releases_api: String,
    /// Crash-restart back-off.
    pub backoff: Backoff,
    /// Health check period.
    pub health_interval: Duration,
    /// How long a start may take until the port is announced.
    pub ready_timeout: Duration,
    /// Idle auto-stop check period.
    pub idle_check: Duration,
}

impl std::fmt::Debug for HostOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostOptions").field("root", &self.root).finish_non_exhaustive()
    }
}

impl HostOptions {
    /// Defaults for a data root and host.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, os: OsKind, arch: Arch, process: Arc<dyn ProcessControl>) -> Self {
        Self {
            root: root.into(),
            os,
            arch,
            process,
            events: None,
            releases_api: release::RELEASES_API.to_owned(),
            backoff: Backoff::default(),
            health_interval: Duration::from_secs(15),
            ready_timeout: Duration::from_secs(90),
            idle_check: Duration::from_secs(15),
        }
    }

    /// Publishes status/progress events on `bus`.
    #[must_use]
    pub fn with_events(mut self, bus: EventBus) -> Self {
        self.events = Some(bus);
        self
    }
}

/// Persisted runtime state.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RuntimeState {
    last_port: Option<u16>,
}

struct Proc {
    generation: u64,
    port: Option<u16>,
    pid: Option<u32>,
    started_at: orkestr_domain::Timestamp,
    token: String,
}

struct St {
    state: VsCodeState,
    installed: Option<VsCodeInstallation>,
    progress: Option<VsCodeProgress>,
    error: Option<String>,
    config: VsCodeConfig,
    proc: Option<Proc>,
    restarts: u32,
    generation: u64,
    runtime: RuntimeState,
    backoff: Backoff,
}

struct Inner {
    opts: HostOptions,
    layout: Layout,
    target: Result<ServerTarget, crate::target::UnsupportedTarget>,
    http: reqwest::Client,
    local_http: reqwest::Client,
    st: Mutex<St>,
    state_tx: watch::Sender<VsCodeState>,
    logs: Mutex<VecDeque<String>>,
    ops: tokio::sync::Mutex<()>,
    install_cancel: Mutex<Option<CancellationToken>>,
    supervisor_done: Mutex<Option<watch::Receiver<bool>>>,
    /// Stop token of the current user start (covers the restart back-off too).
    supervisor_stop: Mutex<Option<CancellationToken>>,
    shutdown: CancellationToken,
    last_seen: Mutex<Instant>,
}

/// The embedded VS Code server host (cheap to clone).
#[derive(Clone)]
pub struct VsCodeHost {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for VsCodeHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VsCodeHost").field("root", &self.inner.layout.root).finish_non_exhaustive()
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "ignoring unreadable VS Code host file");
            None
        }
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> HostResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_vec_pretty(value).map_err(|e| HostError::Invalid(e.to_string()))?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind((server::BIND_HOST, port)).is_ok()
}

impl VsCodeHost {
    /// Creates the host and loads the installation / config from disk. The
    /// server is not started.
    pub fn new(opts: HostOptions) -> HostResult<Self> {
        let layout = Layout::new(&opts.root);
        let target = select_target(opts.os, opts.arch);
        let http = reqwest::Client::builder()
            .user_agent(concat!("Orkestr/", env!("CARGO_PKG_VERSION"), " (+vscode-host)"))
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(120))
            .build()?;
        let local_http = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(5)).build()?;
        let config: VsCodeConfig = read_json(&layout.config_file()).unwrap_or_default();
        let runtime: RuntimeState = read_json(&layout.state_file()).unwrap_or_default();
        let installed: Option<VsCodeInstallation> =
            read_json::<VsCodeInstallation>(&layout.manifest_file()).filter(|i| {
                release::is_valid_version(&i.version)
                    && target.as_ref().is_ok_and(|t| {
                        install::validate_package(&layout.version_dir(&i.version), t.node_executable()).is_ok()
                    })
            });
        let state = if installed.is_some() { VsCodeState::Stopped } else { VsCodeState::NotInstalled };
        let (state_tx, _) = watch::channel(state);
        let backoff = opts.backoff.clone();
        Ok(Self {
            inner: Arc::new(Inner {
                layout,
                target,
                http,
                local_http,
                st: Mutex::new(St {
                    state,
                    installed,
                    progress: None,
                    error: None,
                    config,
                    proc: None,
                    restarts: 0,
                    generation: 0,
                    runtime,
                    backoff,
                }),
                state_tx,
                logs: Mutex::new(VecDeque::with_capacity(LOG_LINES)),
                ops: tokio::sync::Mutex::new(()),
                install_cancel: Mutex::new(None),
                supervisor_done: Mutex::new(None),
                supervisor_stop: Mutex::new(None),
                shutdown: CancellationToken::new(),
                last_seen: Mutex::new(Instant::now()),
                opts,
            }),
        })
    }

    /// Starts the idle auto-stop loop (needs a tokio runtime).
    pub fn spawn_background(&self) {
        let host = self.clone();
        let every = self.inner.opts.idle_check;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = host.inner.shutdown.cancelled() => return,
                    () = tokio::time::sleep(every) => {}
                }
                if host.idle_expired() {
                    host.log("Orkestr: no VS Code view was open for the idle period; stopping the server");
                    if let Err(e) = host.stop().await {
                        tracing::warn!(error = %e, "idle auto-stop of the VS Code server failed");
                    }
                }
            }
        });
    }

    fn idle_expired(&self) -> bool {
        let (auto, secs, running) = {
            let st = self.inner.st.lock();
            (st.config.auto_stop, st.config.idle_stop_secs, st.state == VsCodeState::Running)
        };
        auto && running && self.inner.last_seen.lock().elapsed() > Duration::from_secs(u64::from(secs))
    }

    /// Root directory.
    #[must_use]
    pub fn layout(&self) -> &Layout {
        &self.inner.layout
    }

    /// Current status (never contains the token).
    #[must_use]
    pub fn status(&self) -> VsCodeStatus {
        let st = self.inner.st.lock();
        self.snapshot(&st)
    }

    fn snapshot(&self, st: &St) -> VsCodeStatus {
        let (target, unsupported_reason) = match &self.inner.target {
            Ok(t) => (Some(t.id()), None),
            Err(e) => (None, Some(e.to_string())),
        };
        VsCodeStatus {
            state: st.state,
            target,
            unsupported_reason,
            installed: st.installed.clone(),
            running: st.proc.as_ref().and_then(|p| {
                p.port.map(|port| VsCodeRunning {
                    port,
                    pid: p.pid,
                    started_at: p.started_at,
                    restarts: st.restarts,
                })
            }),
            progress: st.progress.clone(),
            error: st.error.clone(),
            data_dir: self.inner.layout.root.to_string_lossy().into_owned(),
            config: st.config.clone(),
        }
    }

    /// Recent server output (redacted).
    #[must_use]
    pub fn logs(&self) -> Vec<String> {
        self.inner.logs.lock().iter().cloned().collect()
    }

    fn log(&self, line: &str) {
        let token = self.inner.st.lock().proc.as_ref().map(|p| p.token.clone());
        let line = server::redact_line(line, token.as_deref());
        tracing::info!(target: "orkestr::vscode", "{line}");
        let mut logs = self.inner.logs.lock();
        if logs.len() >= LOG_LINES {
            logs.pop_front();
        }
        logs.push_back(line);
    }

    /// Applies a trigger; returns the new snapshot (or the transition error).
    fn transition(&self, trigger: Trigger) -> Result<VsCodeStatus, HostError> {
        let mut st = self.inner.st.lock();
        let to = next_state(st.state, trigger).map_err(|e| HostError::Busy(e.to_string()))?;
        st.state = to;
        if to != VsCodeState::Installing {
            st.progress = None;
        }
        if matches!(to, VsCodeState::Starting | VsCodeState::Installing | VsCodeState::Running) {
            st.error = None;
        }
        let snap = self.snapshot(&st);
        drop(st);
        self.inner.state_tx.send_replace(to);
        Ok(snap)
    }

    async fn emit(&self, status: VsCodeStatus) {
        if let Some(bus) = &self.inner.opts.events {
            let ev = NewEvent::new(EventKind::VsCodeStatusChanged(VsCodeStatusPayload { status }));
            if let Err(e) = bus.publish(ev).await {
                tracing::warn!(error = %e, "failed to publish VsCodeStatusChanged");
            }
        }
    }

    async fn emit_current(&self) {
        let s = self.status();
        self.emit(s).await;
    }

    async fn emit_progress(&self, progress: VsCodeProgress) {
        self.inner.st.lock().progress = Some(progress.clone());
        if let Some(bus) = &self.inner.opts.events {
            let ev = NewEvent::new(EventKind::VsCodeProgress(VsCodeProgressPayload { progress }));
            if let Err(e) = bus.publish(ev).await {
                tracing::debug!(error = %e, "failed to publish VsCodeProgress");
            }
        }
    }

    fn target(&self) -> HostResult<ServerTarget> {
        self.inner.target.clone().map_err(|e| HostError::Unsupported(e.to_string()))
    }

    // ---------------------------------------------------------------- config

    /// Updates the configuration.
    pub async fn set_config(&self, patch: VsCodeConfigPatch) -> HostResult<VsCodeStatus> {
        if let Some(secs) = patch.idle_stop_secs {
            if !(30..=3600).contains(&secs) {
                return Err(HostError::Invalid("idleStopSecs must be between 30 and 3600".into()));
            }
        }
        let config = {
            let mut st = self.inner.st.lock();
            if let Some(v) = patch.auto_stop {
                st.config.auto_stop = v;
            }
            if let Some(v) = patch.idle_stop_secs {
                st.config.idle_stop_secs = v;
            }
            if let Some(v) = patch.auto_restart {
                st.config.auto_restart = v;
            }
            st.config.clone()
        };
        write_json(&self.inner.layout.config_file(), &config)?;
        self.emit_current().await;
        Ok(self.status())
    }

    /// A VS Code view is open: postpones the idle auto-stop.
    pub fn keep_alive(&self) {
        *self.inner.last_seen.lock() = Instant::now();
    }

    // --------------------------------------------------------------- release

    async fn fetch_release(&self, version: Option<&str>) -> HostResult<ReleaseAsset> {
        let target = self.target()?;
        let url = match version {
            Some(v) if release::is_valid_version(v) => format!("{}/tags/{v}", self.inner.opts.releases_api),
            Some(v) => return Err(HostError::Invalid(format!("invalid version `{v}`"))),
            None => format!("{}/latest", self.inner.opts.releases_api),
        };
        let resp = self.inner.http.get(&url).header("Accept", "application/vnd.github+json").send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(HostError::Network(format!("GitHub releases API answered HTTP {status}")));
        }
        let text = resp.text().await?;
        release::pick_asset(&text, &target)
    }

    /// Latest release for this host.
    pub async fn check_update(&self) -> HostResult<VsCodeRelease> {
        let asset = self.fetch_release(None).await?;
        let installed = self.inner.st.lock().installed.as_ref().map(|i| i.version.clone());
        let update_available = installed
            .as_deref()
            .is_some_and(|v| release::compare_versions(&asset.version, v) == std::cmp::Ordering::Greater);
        Ok(VsCodeRelease {
            version: asset.version,
            asset_name: asset.asset_name,
            size_bytes: asset.size_bytes,
            download_url: asset.download_url,
            sha256_url: asset.sha256_url,
            published_at: asset.published_at,
            release_page: asset.release_page,
            license: release::LICENSE.to_owned(),
            extension_registry: release::EXTENSION_REGISTRY.to_owned(),
            update_available,
        })
    }

    // --------------------------------------------------------------- install

    /// Starts a background install (download → sha256 → extract). `accept`
    /// must be true (the user saw the risk dialog).
    pub async fn install(&self, accept: bool, version: Option<String>) -> HostResult<VsCodeStatus> {
        if !accept {
            return Err(HostError::ConsentRequired);
        }
        self.target()?;
        let _ops = self.inner.ops.lock().await;
        let cancel = CancellationToken::new();
        let snap = self.transition(Trigger::InstallStarted)?;
        *self.inner.install_cancel.lock() = Some(cancel.clone());
        self.emit(snap.clone()).await;
        self.log("Orkestr: installing the VS Code server (VSCodium REH web)");
        let host = self.clone();
        tokio::spawn(async move {
            let result = host.do_install(version.as_deref(), &cancel).await;
            *host.inner.install_cancel.lock() = None;
            let _ops = host.inner.ops.lock().await;
            match result {
                Ok(installation) => {
                    host.log(&format!("Orkestr: VS Code server {} installed and verified", installation.version));
                    let old = {
                        let mut st = host.inner.st.lock();
                        st.installed.replace(installation.clone())
                    };
                    if let Some(old) = old.filter(|o| o.version != installation.version) {
                        let _ = std::fs::remove_dir_all(host.inner.layout.version_dir(&old.version));
                    }
                    match host.transition(Trigger::InstallFinished) {
                        Ok(s) => host.emit(s).await,
                        Err(e) => tracing::error!(error = %e, "install finished in an unexpected state"),
                    }
                }
                Err(e) => {
                    let failed = !matches!(e, HostError::Cancelled);
                    let msg = server::redact_line(&e.to_string(), None);
                    host.log(&format!("Orkestr: VS Code server install {}: {msg}", if failed { "failed" } else { "cancelled" }));
                    let installed = host.inner.st.lock().installed.is_some();
                    if failed {
                        host.inner.st.lock().error = Some(msg.clone());
                    }
                    match host.transition(Trigger::InstallAborted { installed, failed }) {
                        Ok(mut s) => {
                            if failed {
                                host.inner.st.lock().error = Some(msg.clone());
                                s.error = Some(msg);
                            }
                            host.emit(s).await;
                        }
                        Err(e) => tracing::error!(error = %e, "install aborted in an unexpected state"),
                    }
                }
            }
        });
        Ok(snap)
    }

    /// Cancels a running install.
    pub fn cancel_install(&self) -> VsCodeStatus {
        if let Some(c) = self.inner.install_cancel.lock().as_ref() {
            c.cancel();
        }
        self.status()
    }

    async fn do_install(&self, version: Option<&str>, cancel: &CancellationToken) -> HostResult<VsCodeInstallation> {
        let target = self.target()?;
        self.emit_progress(VsCodeProgress { phase: VsCodeInstallPhase::Resolving, done: 0, total: None, version: None })
            .await;
        let asset = self.fetch_release(version).await?;
        if let Some(v) = version {
            if v != asset.version {
                return Err(HostError::Release(format!("asked for {v}, the release is {}", asset.version)));
            }
        }
        let layout = &self.inner.layout;
        let dest = layout.version_dir(&asset.version);
        // Already installed and intact: nothing to download.
        let current = self.inner.st.lock().installed.clone();
        if let Some(cur) = current.filter(|c| c.version == asset.version) {
            if install::validate_package(&dest, target.node_executable()).is_ok() {
                return Ok(cur);
            }
        }
        let sha_text = self.inner.http.get(&asset.sha256_url).send().await?.error_for_status()?.text().await?;
        let expected = release::parse_sha256_file(&sha_text, &asset.asset_name)?;
        let part = layout.downloads_dir().join(format!("{}.part", asset.asset_name));
        let version_s = Some(asset.version.clone());
        // Throttled progress events (≈ every 1 % / 250 ms).
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, Option<u64>)>();
        let forward = {
            let host = self.clone();
            let version_s = version_s.clone();
            tokio::spawn(async move {
                let mut last = Instant::now() - Duration::from_secs(1);
                let mut last_pct = u64::MAX;
                while let Some((done, total)) = rx.recv().await {
                    let pct = total.map_or(0, |t| done.saturating_mul(100) / t.max(1));
                    let finished = total == Some(done);
                    if finished || pct != last_pct && last.elapsed() >= Duration::from_millis(250) {
                        last = Instant::now();
                        last_pct = pct;
                        host.emit_progress(VsCodeProgress {
                            phase: VsCodeInstallPhase::Downloading,
                            done,
                            total,
                            version: version_s.clone(),
                        })
                        .await;
                    }
                }
            })
        };
        self.log(&format!("Orkestr: downloading {} ({} bytes)", asset.download_url, asset.size_bytes));
        let downloaded =
            install::download(&self.inner.http, &asset.download_url, &part, Some(asset.size_bytes), cancel, |d, t| {
                let _ = tx.send((d, t));
            })
            .await;
        drop(tx);
        let _ = forward.await;
        let actual = match downloaded {
            Ok(h) => h,
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
        };
        self.emit_progress(VsCodeProgress {
            phase: VsCodeInstallPhase::Verifying,
            done: asset.size_bytes,
            total: Some(asset.size_bytes),
            version: version_s.clone(),
        })
        .await;
        if let Err(e) = release::verify_digest(&actual, &expected) {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
        self.log(&format!("Orkestr: sha256 verified ({actual})"));
        self.emit_progress(VsCodeProgress {
            phase: VsCodeInstallPhase::Extracting,
            done: 0,
            total: None,
            version: version_s.clone(),
        })
        .await;
        let (part2, dest2, cancel2) = (part.clone(), dest.clone(), cancel.clone());
        let report =
            tokio::task::spawn_blocking(move || install::extract_tar_gz(&part2, &dest2, &cancel2, |_| {}))
                .await
                .map_err(|e| HostError::Archive(format!("extraction task failed: {e}")))?;
        let _ = std::fs::remove_file(&part);
        let report = report?;
        if let Err(e) = install::validate_package(&dest, target.node_executable()) {
            let _ = std::fs::remove_dir_all(&dest);
            return Err(e);
        }
        self.log(&format!("Orkestr: unpacked {} files ({} links skipped)", report.files, report.skipped));
        let installation = VsCodeInstallation {
            version: asset.version,
            asset_name: asset.asset_name,
            sha256: actual,
            size_bytes: asset.size_bytes,
            installed_at: chrono::Utc::now(),
            dir: dest.to_string_lossy().into_owned(),
        };
        write_json(&layout.manifest_file(), &installation)?;
        Ok(installation)
    }

    /// Stops the server and removes the package (`purge` also removes VS Code
    /// settings and extensions).
    pub async fn uninstall(&self, purge: bool) -> HostResult<VsCodeStatus> {
        self.stop().await?;
        let _ops = self.inner.ops.lock().await;
        let snap = self.transition(Trigger::Uninstalled)?;
        let layout = &self.inner.layout;
        let old = self.inner.st.lock().installed.take();
        if let Some(old) = old {
            std::fs::remove_dir_all(layout.version_dir(&old.version)).or_else(ignore_missing)?;
        }
        // Leftovers of interrupted installs / older versions.
        if let Ok(entries) = std::fs::read_dir(&layout.root) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let base = name.strip_suffix(".partial").unwrap_or(&name);
                if release::is_valid_version(base) && e.path().is_dir() {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        std::fs::remove_file(layout.manifest_file()).or_else(ignore_missing)?;
        std::fs::remove_dir_all(layout.downloads_dir()).or_else(ignore_missing)?;
        if purge {
            for dir in [layout.user_data_dir(), layout.extensions_dir(), layout.server_data_dir()] {
                std::fs::remove_dir_all(dir).or_else(ignore_missing)?;
            }
        }
        self.log(if purge {
            "Orkestr: VS Code server, settings and extensions removed"
        } else {
            "Orkestr: VS Code server removed (settings and extensions kept)"
        });
        let snap = VsCodeStatus { installed: None, ..snap };
        self.emit(snap.clone()).await;
        Ok(snap)
    }

    // ------------------------------------------------------------ start/stop

    /// Starts the server (or reuses the running one) and returns the UI URL
    /// for `folder`.
    pub async fn start(&self, folder: Option<&Path>) -> HostResult<VsCodeStartResult> {
        let target = self.target()?;
        let folder_uri = match folder {
            Some(f) if f.is_dir() => Some(server::folder_uri_path(f)),
            Some(f) => return Err(HostError::Invalid(format!("the project folder does not exist: {}", f.display()))),
            None => None,
        };
        self.keep_alive();
        {
            let _ops = self.inner.ops.lock().await;
            let state = self.inner.st.lock().state;
            match state {
                VsCodeState::NotInstalled => return Err(HostError::NotInstalled),
                VsCodeState::Installing => return Err(HostError::Busy("the VS Code server is being installed".into())),
                VsCodeState::Stopping => return Err(HostError::Busy("the VS Code server is stopping; try again".into())),
                VsCodeState::Stopped | VsCodeState::Error => {
                    let installed = self.inner.st.lock().installed.clone().ok_or(HostError::NotInstalled)?;
                    let snap = self.transition(Trigger::StartRequested)?;
                    let (generation, stop) = {
                        let mut st = self.inner.st.lock();
                        st.generation += 1;
                        st.restarts = 0;
                        st.backoff.reset();
                        (st.generation, CancellationToken::new())
                    };
                    self.emit(snap).await;
                    let (done_tx, done_rx) = watch::channel(false);
                    *self.inner.supervisor_done.lock() = Some(done_rx);
                    *self.inner.supervisor_stop.lock() = Some(stop.clone());
                    let host = self.clone();
                    let dir = self.inner.layout.version_dir(&installed.version);
                    tokio::spawn(async move {
                        host.supervise(generation, stop, dir, target).await;
                        let _ = done_tx.send(true);
                    });
                }
                VsCodeState::Starting | VsCodeState::Running | VsCodeState::Restarting => {}
            }
        }
        // Wait until running (or failed).
        let mut rx = self.inner.state_tx.subscribe();
        let wait = self.inner.opts.ready_timeout + Duration::from_secs(10);
        let reached = tokio::time::timeout(wait, async {
            loop {
                let s = *rx.borrow_and_update();
                match s {
                    VsCodeState::Running => return Ok(()),
                    VsCodeState::Error | VsCodeState::Stopped | VsCodeState::NotInstalled => {
                        let err = self.inner.st.lock().error.clone();
                        return Err(HostError::Server(err.unwrap_or_else(|| "the VS Code server did not start".into())));
                    }
                    _ => {}
                }
                if rx.changed().await.is_err() {
                    return Err(HostError::Server("the VS Code host stopped".into()));
                }
            }
        })
        .await
        .map_err(|_| HostError::Server("timed out waiting for the VS Code server".into()))?;
        reached?;
        let (port, token) = {
            let st = self.inner.st.lock();
            let p = st.proc.as_ref().ok_or_else(|| HostError::Server("the VS Code server is not running".into()))?;
            (p.port.ok_or_else(|| HostError::Server("the VS Code server has no port yet".into()))?, p.token.clone())
        };
        Ok(VsCodeStartResult {
            url: server::ui_url(port, &token, folder_uri.as_deref()),
            origin: server::origin(port),
            folder: folder_uri,
            status: self.status(),
        })
    }

    /// Stops the server (no-op when not running).
    pub async fn stop(&self) -> HostResult<VsCodeStatus> {
        {
            let _ops = self.inner.ops.lock().await;
            let state = self.inner.st.lock().state;
            if !matches!(state, VsCodeState::Starting | VsCodeState::Running | VsCodeState::Restarting) {
                return Ok(self.status());
            }
            let snap = self.transition(Trigger::StopRequested)?;
            if let Some(stop) = self.inner.supervisor_stop.lock().as_ref() {
                stop.cancel();
            }
            self.emit(snap).await;
        }
        let done = self.inner.supervisor_done.lock().clone();
        if let Some(mut done) = done {
            let _ = tokio::time::timeout(Duration::from_secs(20), done.wait_for(|d| *d)).await;
        }
        Ok(self.status())
    }

    /// Core shutdown: stop the server and the background loops.
    pub async fn shutdown(&self) {
        self.inner.shutdown.cancel();
        if let Err(e) = self.stop().await {
            tracing::warn!(error = %e, "failed to stop the VS Code server on shutdown");
        }
        // Last resort: kill whatever process is still recorded.
        let pid = self.inner.st.lock().proc.as_ref().and_then(|p| p.pid);
        if let Some(pid) = pid {
            let _ = self.inner.opts.process.kill_tree(pid);
        }
    }

    /// Supervises one user start: runs the server, restarts it after crashes
    /// (back-off), stops on request.
    async fn supervise(&self, generation: u64, stop: CancellationToken, dir: PathBuf, target: ServerTarget) {
        let mut first = true;
        loop {
            let outcome = self.run_once(generation, &stop, &dir, &target).await;
            let unregister = self.inner.st.lock().proc.take();
            if let Some(p) = unregister {
                orkestr_security::redact::unregister_secret(&p.token);
            }
            let _ = std::fs::remove_file(self.inner.layout.token_file());
            if stop.is_cancelled() || self.inner.shutdown.is_cancelled() {
                let _ = self.transition(Trigger::Stopped);
                // Stop requested while still Starting/Running (shutdown path).
                let _ = self.force_state_stopped();
                self.log("Orkestr: VS Code server stopped");
                self.emit_current().await;
                return;
            }
            let (was_ready, reason) = match outcome {
                RunOutcome::Exited { was_ready, reason } => (was_ready, reason),
            };
            self.log(&format!("Orkestr: VS Code server exited: {reason}"));
            // A server that never came up is a configuration problem: no retry loop.
            let delay = {
                let mut st = self.inner.st.lock();
                if st.config.auto_restart && (was_ready || !first) {
                    st.backoff.next_delay(Instant::now())
                } else {
                    None
                }
            };
            first = false;
            self.inner.st.lock().error = Some(server::redact_line(&reason, None));
            match self.transition(Trigger::Crashed { restart: delay.is_some() }) {
                Ok(mut s) => {
                    s.error = Some(reason.clone());
                    self.emit(s).await;
                }
                Err(e) => tracing::warn!(error = %e, "unexpected VS Code state after exit"),
            }
            let Some(delay) = delay else {
                if was_ready {
                    self.log("Orkestr: restart budget exhausted; the VS Code server stays stopped");
                }
                return;
            };
            self.log(&format!("Orkestr: restarting the VS Code server in {} s", delay.as_secs()));
            tokio::select! {
                () = stop.cancelled() => {
                    let _ = self.transition(Trigger::Stopped);
                    self.emit_current().await;
                    return;
                }
                () = tokio::time::sleep(delay) => {}
            }
            match self.transition(Trigger::StartRequested) {
                Ok(s) => {
                    self.inner.st.lock().restarts += 1;
                    self.emit(s).await;
                }
                Err(_) => return,
            }
        }
    }

    fn force_state_stopped(&self) -> bool {
        let mut st = self.inner.st.lock();
        if st.state == VsCodeState::Stopped {
            return false;
        }
        st.state = if st.installed.is_some() { VsCodeState::Stopped } else { VsCodeState::NotInstalled };
        let s = st.state;
        drop(st);
        self.inner.state_tx.send_replace(s);
        true
    }

    async fn run_once(
        &self,
        generation: u64,
        stop: &CancellationToken,
        dir: &Path,
        target: &ServerTarget,
    ) -> RunOutcome {
        let fail = |reason: String| RunOutcome::Exited { was_ready: false, reason };
        let layout = &self.inner.layout;
        for d in [layout.user_data_dir(), layout.extensions_dir(), layout.server_data_dir()] {
            if let Err(e) = std::fs::create_dir_all(&d) {
                return fail(format!("cannot create {}: {e}", d.display()));
            }
        }
        let token = server::generate_token();
        orkestr_security::redact::register_secret(&token);
        if let Err(e) = orkestr_platform::fs::write_private_file(&layout.token_file(), token.as_bytes()) {
            return fail(format!("cannot write the connection token file: {e}"));
        }
        let last = self.inner.st.lock().runtime.last_port;
        let port = last.filter(|p| port_is_free(*p)).unwrap_or(0);
        let spec = LaunchSpec {
            install_dir: dir.to_path_buf(),
            node: dir.join(target.node_executable()),
            port,
            token_file: layout.token_file(),
            server_data_dir: layout.server_data_dir(),
            user_data_dir: layout.user_data_dir(),
            extensions_dir: layout.extensions_dir(),
        };
        let mut cmd = tokio::process::Command::new(&spec.node);
        cmd.args(server::build_args(&spec))
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Do not leak a parent VS Code / Electron / Node environment into the server.
        for (k, _) in std::env::vars_os() {
            let k_s = k.to_string_lossy().to_ascii_uppercase();
            if k_s.starts_with("VSCODE_") || k_s.starts_with("ELECTRON_") || k_s == "NODE_OPTIONS" {
                cmd.env_remove(&k);
            }
        }
        orkestr_platform::process::isolate_command(&mut cmd);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return fail(format!("cannot start {}: {e}", spec.node.display())),
        };
        let pid = child.id();
        {
            let mut st = self.inner.st.lock();
            st.proc = Some(Proc {
                generation,
                port: None,
                pid,
                started_at: chrono::Utc::now(),
                token: token.clone(),
            });
        }
        self.log(&format!("Orkestr: started VS Code server (pid {}, port {})", pid.unwrap_or(0), if port == 0 { "auto".to_owned() } else { port.to_string() }));
        let (line_tx, mut lines) = tokio::sync::mpsc::unbounded_channel::<String>();
        if let Some(out) = child.stdout.take() {
            let tx = line_tx.clone();
            tokio::spawn(async move {
                let mut r = BufReader::new(out).lines();
                while let Ok(Some(l)) = r.next_line().await {
                    let _ = tx.send(l);
                }
            });
        }
        if let Some(err) = child.stderr.take() {
            let tx = line_tx.clone();
            tokio::spawn(async move {
                let mut r = BufReader::new(err).lines();
                while let Ok(Some(l)) = r.next_line().await {
                    let _ = tx.send(l);
                }
            });
        }
        drop(line_tx);

        let kill = |child: &mut tokio::process::Child| {
            let process = Arc::clone(&self.inner.opts.process);
            let pid = child.id();
            async move {
                if let Some(pid) = pid {
                    let _ = tokio::task::spawn_blocking(move || process.kill_tree(pid)).await;
                }
            }
        };

        // Phase 1: wait for the port.
        let deadline = tokio::time::sleep(self.inner.opts.ready_timeout);
        tokio::pin!(deadline);
        let mut lines_open = true;
        // A fixed port that turns out to be taken must not wedge future starts.
        self.inner.st.lock().runtime.last_port = None;
        let port = loop {
            tokio::select! {
                () = stop.cancelled() => {
                    kill(&mut child).await;
                    let _ = child.kill().await;
                    return fail("stopped".into());
                }
                () = &mut deadline => {
                    kill(&mut child).await;
                    let _ = child.kill().await;
                    return fail("the server did not announce its port in time".into());
                }
                status = child.wait() => {
                    return fail(format!("the server exited during start-up ({})", status.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string())));
                }
                line = lines.recv(), if lines_open => {
                    match line {
                        Some(line) => {
                            self.log(&line);
                            if let Some(p) = server::parse_listening_port(&line) {
                                break p;
                            }
                        }
                        None => lines_open = false,
                    }
                }
            }
        };
        // Health check before announcing it.
        let mut healthy = false;
        for _ in 0..40 {
            if self.health(port).await {
                healthy = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if !healthy {
            kill(&mut child).await;
            let _ = child.kill().await;
            return fail(format!("the server on port {port} does not answer"));
        }
        {
            let mut st = self.inner.st.lock();
            if let Some(p) = st.proc.as_mut().filter(|p| p.generation == generation) {
                p.port = Some(port);
            }
            st.runtime.last_port = Some(port);
            let rt = st.runtime.clone();
            drop(st);
            if let Err(e) = write_json(&self.inner.layout.state_file(), &rt) {
                tracing::debug!(error = %e, "could not persist the VS Code port");
            }
        }
        if stop.is_cancelled() {
            kill(&mut child).await;
            let _ = child.kill().await;
            return fail("stopped".into());
        }
        match self.transition(Trigger::Ready) {
            Ok(s) => self.emit(s).await,
            Err(e) => tracing::warn!(error = %e, "VS Code server ready in an unexpected state"),
        }
        self.log(&format!("Orkestr: VS Code server ready on {}", server::origin(port)));

        // Phase 2: supervise.
        let mut failures = 0u32;
        let mut tick = tokio::time::interval(self.inner.opts.health_interval);
        tick.tick().await;
        loop {
            tokio::select! {
                () = stop.cancelled() => {
                    kill(&mut child).await;
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return RunOutcome::Exited { was_ready: true, reason: "stopped".into() };
                }
                status = child.wait() => {
                    // Drain what the process printed last.
                    while let Ok(line) = lines.try_recv() {
                        self.log(&line);
                    }
                    return RunOutcome::Exited {
                        was_ready: true,
                        reason: format!("the server process exited ({})", status.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string())),
                    };
                }
                line = lines.recv(), if lines_open => {
                    match line {
                        Some(line) => self.log(&line),
                        None => lines_open = false,
                    }
                }
                _ = tick.tick() => {
                    if self.health(port).await {
                        failures = 0;
                    } else {
                        failures += 1;
                        self.log(&format!("Orkestr: health check failed ({failures}/{HEALTH_FAILURES})"));
                        if failures >= HEALTH_FAILURES {
                            kill(&mut child).await;
                            let _ = child.kill().await;
                            let _ = child.wait().await;
                            return RunOutcome::Exited { was_ready: true, reason: "the server stopped responding".into() };
                        }
                    }
                }
            }
        }
    }

    /// `GET /version` (no token needed) answers 200.
    async fn health(&self, port: u16) -> bool {
        let url = format!("{}/version", server::origin(port));
        matches!(self.inner.local_http.get(url).send().await, Ok(r) if r.status().is_success())
    }
}

fn ignore_missing(e: std::io::Error) -> std::io::Result<()> {
    if e.kind() == std::io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(e)
    }
}

enum RunOutcome {
    Exited { was_ready: bool, reason: String },
}
