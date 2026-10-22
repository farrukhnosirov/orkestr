//! The voice model manager (ADR-0046): downloads catalog packages into
//! `<ORKESTR_HOME>/voice/models/<id>` one at a time, verifies every file
//! against its pinned SHA-256 *before* unpacking, unpacks into a staging
//! directory, writes a manifest of the installed files' SHA-256 (for
//! `verify`) and renames the staging directory into place. Archives are
//! deleted right after unpacking; a failed or cancelled download leaves
//! nothing behind.

use std::collections::{HashMap, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use orkestr_protocol::methods::voice::{VoiceModelsResult, VoicePackageInfo, VoicePackageState, VoiceVerifyResult};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::catalog::{self, Package, Unpack};
use crate::VoiceError;

/// Test hook type: rewrites a catalog URL.
pub type UrlOverride = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// File written into every installed package directory.
pub const MANIFEST: &str = ".orkestr-package.json";

#[derive(Debug, Clone, Default)]
struct Job {
    state: Option<VoicePackageState>,
    progress: u64,
    error: Option<String>,
}

#[derive(Default)]
struct State {
    jobs: HashMap<String, Job>,
    queue: VecDeque<String>,
    running: Option<(String, CancellationToken)>,
    worker: bool,
    sizes: HashMap<String, u64>,
}

/// The manager. Cheap to clone.
#[derive(Clone)]
pub struct ModelManager {
    root: PathBuf,
    http: reqwest::Client,
    state: Arc<Mutex<State>>,
    /// Test hook: rewrites download URLs (mock server).
    url_override: Option<UrlOverride>,
}

impl std::fmt::Debug for ModelManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelManager").field("root", &self.root).finish_non_exhaustive()
    }
}

fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

fn sha256_file(p: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex::encode(h.finalize()))
}

fn walk_files(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_files(base, &p, out);
        } else if let Ok(rel) = p.strip_prefix(base) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            if rel != MANIFEST {
                out.push(rel);
            }
        }
    }
}

impl ModelManager {
    /// A manager rooted at `models_root` (`<ORKESTR_HOME>/voice/models`).
    #[must_use]
    pub fn new(models_root: PathBuf, http: reqwest::Client) -> Self {
        Self { root: models_root, http, state: Arc::default(), url_override: None }
    }

    /// Test hook: rewrite catalog URLs.
    #[must_use]
    pub fn with_url_override(mut self, f: UrlOverride) -> Self {
        self.url_override = Some(f);
        self
    }

    /// The models directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory of an installed package (`None` when not installed).
    #[must_use]
    pub fn installed_dir(&self, id: &str) -> Option<PathBuf> {
        let d = self.root.join(id);
        d.join(MANIFEST).is_file().then_some(d)
    }

    /// Whether `id` is installed.
    #[must_use]
    pub fn is_installed(&self, id: &str) -> bool {
        self.installed_dir(id).is_some()
    }

    fn tmp_dir(&self) -> PathBuf {
        self.root.join(".tmp")
    }

    /// Catalog with state.
    #[must_use]
    pub fn list(&self) -> VoiceModelsResult {
        let mut st = self.state.lock();
        let mut packages = Vec::new();
        let mut total = 0;
        for p in catalog::CATALOG {
            let installed = self.is_installed(p.id);
            let size = if installed {
                *st.sizes.entry(p.id.to_owned()).or_insert_with(|| dir_size(&self.root.join(p.id)))
            } else {
                0
            };
            total += size;
            let job = st.jobs.get(p.id).cloned().unwrap_or_default();
            let state = job.state.unwrap_or(if installed {
                VoicePackageState::Installed
            } else {
                VoicePackageState::NotInstalled
            });
            packages.push(VoicePackageInfo {
                id: p.id.to_owned(),
                title: p.title.to_owned(),
                kind: p.kind,
                stt: p.stt,
                tts: p.tts,
                languages: p.languages.iter().map(|s| (*s).to_owned()).collect(),
                download_bytes: p.download_bytes(),
                installed_bytes: size,
                state,
                progress_bytes: job.progress,
                error: job.error,
                recommended: p.recommended,
                depends_on: p.depends.iter().map(|s| (*s).to_owned()).collect(),
                sha256: p.files.iter().map(|f| f.sha256.to_owned()).collect(),
                supported: p.supported(),
                license: p.license.to_owned(),
                source: p.source(),
            });
        }
        total += dir_size(&self.tmp_dir());
        VoiceModelsResult { packages, disk_usage_bytes: total, root: self.root.display().to_string() }
    }

    /// Queues `id` and its missing dependencies; returns immediately.
    pub fn download(&self, id: &str) -> Result<(), VoiceError> {
        let p = catalog::package(id).ok_or_else(|| VoiceError::BadRequest(format!("unknown voice package {id}")))?;
        if !p.supported() {
            return Err(VoiceError::Unavailable(format!("{id} is not available for this OS/CPU")));
        }
        let mut order = Vec::new();
        for d in p.depends {
            if !self.is_installed(d) {
                order.push(*d);
            }
        }
        order.push(p.id);
        let start_worker = {
            let mut st = self.state.lock();
            for i in order {
                let busy = st.queue.iter().any(|q| q == i) || st.running.as_ref().is_some_and(|(r, _)| r == i);
                if busy || (i != id && self.is_installed(i)) {
                    continue;
                }
                st.queue.push_back(i.to_owned());
                st.jobs.insert(i.to_owned(), Job { state: Some(VoicePackageState::Queued), ..Job::default() });
            }
            let start = !st.worker;
            st.worker = true;
            start
        };
        if start_worker {
            let me = self.clone();
            tokio::spawn(async move { me.worker().await });
        }
        Ok(())
    }

    /// Downloads `id` (and dependencies) and waits until done.
    pub async fn install_blocking(&self, id: &str) -> Result<(), VoiceError> {
        self.download(id)?;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let st = self.state.lock();
            let idle = st.running.is_none() && st.queue.is_empty();
            if idle {
                if let Some(e) = st.jobs.get(id).and_then(|j| j.error.clone()) {
                    return Err(VoiceError::Failed(e));
                }
                return if self.is_installed(id) {
                    Ok(())
                } else {
                    Err(VoiceError::Failed(format!("{id} was not installed")))
                };
            }
        }
    }

    /// Cancels a queued or running download.
    pub fn cancel(&self, id: &str) {
        let mut st = self.state.lock();
        st.queue.retain(|q| q != id);
        if let Some((r, token)) = &st.running {
            if r == id {
                token.cancel();
            }
        }
        if st.running.as_ref().is_none_or(|(r, _)| r != id) {
            st.jobs.remove(id);
        }
    }

    /// Removes an installed package.
    pub fn remove(&self, id: &str) -> Result<(), VoiceError> {
        catalog::package(id).ok_or_else(|| VoiceError::BadRequest(format!("unknown voice package {id}")))?;
        {
            let st = self.state.lock();
            if st.running.as_ref().is_some_and(|(r, _)| r == id) {
                return Err(VoiceError::BadRequest(format!("{id} is being downloaded; cancel it first")));
            }
        }
        let dir = self.root.join(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| VoiceError::Failed(format!("remove {id}: {e}")))?;
        }
        let mut st = self.state.lock();
        st.sizes.remove(id);
        st.jobs.remove(id);
        Ok(())
    }

    /// Re-hashes the installed files against the install manifest.
    pub async fn verify(&self, id: &str) -> Result<VoiceVerifyResult, VoiceError> {
        let dir = self.installed_dir(id).ok_or_else(|| VoiceError::NotInstalled(id.to_owned()))?;
        tokio::task::spawn_blocking(move || {
            let raw = std::fs::read_to_string(dir.join(MANIFEST)).map_err(|e| VoiceError::Failed(e.to_string()))?;
            let manifest: serde_json::Value =
                serde_json::from_str(&raw).map_err(|e| VoiceError::Failed(format!("manifest: {e}")))?;
            let files = manifest.get("files").and_then(|v| v.as_object()).cloned().unwrap_or_default();
            let mut mismatches = Vec::new();
            for (rel, want) in &files {
                let ok = sha256_file(&dir.join(rel)).is_ok_and(|h| Some(h.as_str()) == want.as_str());
                if !ok {
                    mismatches.push(rel.clone());
                }
            }
            Ok(VoiceVerifyResult {
                ok: mismatches.is_empty() && !files.is_empty(),
                files: u32::try_from(files.len()).unwrap_or(u32::MAX),
                mismatches,
            })
        })
        .await
        .map_err(|e| VoiceError::Failed(e.to_string()))?
    }

    async fn worker(self) {
        loop {
            let next = {
                let mut st = self.state.lock();
                match st.queue.pop_front() {
                    Some(id) => {
                        let token = CancellationToken::new();
                        st.running = Some((id.clone(), token.clone()));
                        st.jobs
                            .insert(id.clone(), Job { state: Some(VoicePackageState::Downloading), ..Job::default() });
                        Some((id, token))
                    }
                    None => {
                        st.worker = false;
                        st.running = None;
                        None
                    }
                }
            };
            let Some((id, token)) = next else { return };
            let result = match catalog::package(&id) {
                Some(p) => self.install(p, &token).await,
                None => Err(VoiceError::BadRequest(format!("unknown package {id}"))),
            };
            let mut st = self.state.lock();
            st.running = None;
            st.sizes.remove(&id);
            match result {
                Ok(()) => {
                    tracing::info!(package = %id, "voice package installed");
                    st.jobs.remove(&id);
                }
                Err(_) if token.is_cancelled() => {
                    st.jobs.remove(&id);
                }
                Err(e) => {
                    tracing::warn!(package = %id, error = %e, "voice package download failed");
                    st.jobs.insert(
                        id,
                        Job { state: Some(VoicePackageState::Failed), progress: 0, error: Some(e.to_string()) },
                    );
                }
            }
        }
    }

    fn set_progress(&self, id: &str, state: VoicePackageState, progress: u64) {
        let mut st = self.state.lock();
        let job = st.jobs.entry(id.to_owned()).or_default();
        job.state = Some(state);
        job.progress = progress;
    }

    async fn install(&self, p: &'static Package, token: &CancellationToken) -> Result<(), VoiceError> {
        let tmp = self.tmp_dir();
        std::fs::create_dir_all(&tmp).map_err(|e| VoiceError::Failed(format!("create {}: {e}", tmp.display())))?;
        let staging = tmp.join(format!("{}.staging", p.id));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(|e| VoiceError::Failed(e.to_string()))?;
        let result = self.install_into(p, token, &staging).await;
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
            return result;
        }
        let dest = self.root.join(p.id);
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&staging, &dest).map_err(|e| VoiceError::Failed(format!("install {}: {e}", p.id)))?;
        Ok(())
    }

    async fn install_into(
        &self,
        p: &'static Package,
        token: &CancellationToken,
        staging: &Path,
    ) -> Result<(), VoiceError> {
        let mut done_before = 0u64;
        for (n, file) in p.files.iter().enumerate() {
            let part = self.tmp_dir().join(format!("{}-{n}.part", p.id));
            let r = self.fetch(p.id, file, &part, token, done_before).await;
            if let Err(e) = r {
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
            done_before += file.size;
            self.set_progress(p.id, VoicePackageState::Installing, done_before);
            let staging2 = staging.to_path_buf();
            let part2 = part.clone();
            let unpack = file.unpack;
            let r = tokio::task::spawn_blocking(move || unpack_file(&part2, unpack, &staging2))
                .await
                .map_err(|e| VoiceError::Failed(e.to_string()))?;
            let _ = std::fs::remove_file(&part);
            r?;
        }
        // Manifest of installed files (for `verify`).
        let staging2 = staging.to_path_buf();
        let id = p.id;
        tokio::task::spawn_blocking(move || -> Result<(), VoiceError> {
            let mut rels = Vec::new();
            walk_files(&staging2, &staging2, &mut rels);
            rels.sort();
            let mut files = serde_json::Map::new();
            for rel in rels {
                let h = sha256_file(&staging2.join(&rel)).map_err(|e| VoiceError::Failed(e.to_string()))?;
                files.insert(rel, serde_json::Value::String(h));
            }
            let doc = serde_json::json!({ "id": id, "files": files });
            std::fs::write(staging2.join(MANIFEST), serde_json::to_vec_pretty(&doc).unwrap_or_default())
                .map_err(|e| VoiceError::Failed(e.to_string()))
        })
        .await
        .map_err(|e| VoiceError::Failed(e.to_string()))?
    }

    async fn fetch(
        &self,
        id: &str,
        file: &catalog::PackageFile,
        part: &Path,
        token: &CancellationToken,
        done_before: u64,
    ) -> Result<(), VoiceError> {
        let url = self.url_override.as_ref().map_or_else(|| file.url.to_owned(), |f| f(file.url));
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| VoiceError::Network(format!("download {id}: {e}")))?;
        let mut out = std::fs::File::create(part).map_err(|e| VoiceError::Failed(e.to_string()))?;
        let mut hasher = Sha256::new();
        let mut got = 0u64;
        let mut stream = resp.bytes_stream();
        let mut last = std::time::Instant::now();
        loop {
            let chunk = tokio::select! {
                () = token.cancelled() => return Err(VoiceError::Failed("cancelled".into())),
                c = stream.next() => c,
            };
            let Some(chunk) = chunk else { break };
            let chunk = chunk.map_err(|e| VoiceError::Network(format!("download {id}: {e}")))?;
            got += chunk.len() as u64;
            if got > file.size {
                return Err(VoiceError::Failed(format!("{id}: download is larger than the pinned size")));
            }
            hasher.update(&chunk);
            out.write_all(&chunk).map_err(|e| VoiceError::Failed(e.to_string()))?;
            if last.elapsed().as_millis() > 200 {
                last = std::time::Instant::now();
                self.set_progress(id, VoicePackageState::Downloading, done_before + got);
            }
        }
        out.flush().map_err(|e| VoiceError::Failed(e.to_string()))?;
        drop(out);
        let hash = hex::encode(hasher.finalize());
        if got != file.size || hash != file.sha256 {
            return Err(VoiceError::Failed(format!(
                "{id}: checksum mismatch (expected {} bytes sha256 {}, got {got} bytes sha256 {hash})",
                file.size, file.sha256
            )));
        }
        Ok(())
    }
}

/// Moves the contents of a single top-level directory up one level.
fn strip_single_root(dir: &Path) -> std::io::Result<()> {
    let entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
    if entries.len() != 1 || !entries[0].path().is_dir() {
        return Ok(());
    }
    let inner = entries[0].path();
    let tmp = dir.join(".strip-tmp");
    std::fs::rename(&inner, &tmp)?;
    for e in std::fs::read_dir(&tmp)?.flatten() {
        std::fs::rename(e.path(), dir.join(e.file_name()))?;
    }
    std::fs::remove_dir(&tmp)
}

fn unpack_file(part: &Path, unpack: Unpack, staging: &Path) -> Result<(), VoiceError> {
    let fail = |e: &dyn std::fmt::Display| VoiceError::Failed(format!("unpack: {e}"));
    match unpack {
        Unpack::Raw(name) => {
            std::fs::copy(part, staging.join(name)).map_err(|e| fail(&e))?;
        }
        Unpack::Zip => {
            let out = staging.join(".unzip");
            std::fs::create_dir_all(&out).map_err(|e| fail(&e))?;
            let f = std::fs::File::open(part).map_err(|e| fail(&e))?;
            let mut zip = zip::ZipArchive::new(f).map_err(|e| fail(&e))?;
            for i in 0..zip.len() {
                let mut entry = zip.by_index(i).map_err(|e| fail(&e))?;
                // Zip-slip safe: `enclosed_name` rejects absolute and `..` paths.
                let Some(rel) = entry.enclosed_name() else { continue };
                let target = out.join(rel);
                if entry.is_dir() {
                    std::fs::create_dir_all(&target).map_err(|e| fail(&e))?;
                    continue;
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| fail(&e))?;
                }
                let mut w = std::fs::File::create(&target).map_err(|e| fail(&e))?;
                std::io::copy(&mut entry, &mut w).map_err(|e| fail(&e))?;
            }
            strip_single_root(&out).map_err(|e| fail(&e))?;
            for e in std::fs::read_dir(&out).map_err(|e| fail(&e))?.flatten() {
                std::fs::rename(e.path(), staging.join(e.file_name())).map_err(|e| fail(&e))?;
            }
            std::fs::remove_dir_all(&out).map_err(|e| fail(&e))?;
        }
        Unpack::TarBz2 => {
            let out = staging.join(".untar");
            std::fs::create_dir_all(&out).map_err(|e| fail(&e))?;
            // bsdtar (Windows 10+ System32\tar.exe) / GNU tar both refuse
            // absolute and `..` member paths by default.
            let tar = if cfg!(windows) {
                let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
                Path::new(&root).join("System32").join("tar.exe")
            } else {
                PathBuf::from("tar")
            };
            let status = orkestr_platform::process::background_command(&tar)
                .arg("-xjf")
                .arg(part)
                .arg("-C")
                .arg(&out)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map_err(|e| fail(&e))?;
            if !status.success() {
                return Err(VoiceError::Failed(format!("unpack: tar exited with {status}")));
            }
            strip_single_root(&out).map_err(|e| fail(&e))?;
            for e in std::fs::read_dir(&out).map_err(|e| fail(&e))?.flatten() {
                std::fs::rename(e.path(), staging.join(e.file_name())).map_err(|e| fail(&e))?;
            }
            std::fs::remove_dir_all(&out).map_err(|e| fail(&e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_and_unzip() {
        let dir = tempfile::tempdir().unwrap();
        let zpath = dir.path().join("a.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&zpath).unwrap());
            let o: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            z.start_file("top/bin/x.txt", o).unwrap();
            z.write_all(b"hello").unwrap();
            z.start_file("../evil.txt", o).unwrap();
            z.write_all(b"no").unwrap();
            z.finish().unwrap();
        }
        let staging = dir.path().join("s");
        std::fs::create_dir_all(&staging).unwrap();
        unpack_file(&zpath, Unpack::Zip, &staging).unwrap();
        assert_eq!(std::fs::read_to_string(staging.join("bin/x.txt")).unwrap(), "hello");
        assert!(!dir.path().join("evil.txt").exists());
    }
}
