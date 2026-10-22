//! Local model store (Phase 6, ADR-0039). **Owner: WP-D.**
//!
//! A model is a [`ModelManifest`] (`{name, tag, format, layers[{digest, size,
//! mediaType}]}`) whose layers are blobs in the [`crate::blobs::BlobStore`];
//! the manifest's own digest (SHA-256 of its canonical JSON) identifies it.
//! Installed models are recorded as [`StoredModel`] JSON files under
//! `ORKESTR_HOME/models/<manifest hex>.json` (written atomically, only after
//! every layer is verified).
//!
//! **Scheduler feed (for WP-A heartbeat / hello):** call [`local_models`]
//! (→ `NodeInfo.localModels.models`, merged by [`merge_local_models`]) and
//! [`installed_digests`] (→ `NodeSnapshot.modelDigests`) with the worker's
//! `ORKESTR_HOME` (`Services.paths.data_dir`). Both are cheap, synchronous
//! directory reads.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use orkestr_domain::Digest;
pub use orkestr_protocol::methods::models::{ModelLayer, ModelManifest, StoredModel};
use orkestr_protocol::methods::nodes::{LocalModel, LocalModelsInfo};

use crate::blobs::{BlobError, BlobStore};

/// Media type of a GGUF weights layer.
pub const MEDIA_TYPE_GGUF: &str = "application/vnd.orkestr.model.gguf";
/// Media type of a mock weights layer (tests / acceptance).
pub const MEDIA_TYPE_MOCK: &str = "application/vnd.orkestr.model.mock";
/// Format of real GGUF models (registered with Ollama when present).
pub const FORMAT_GGUF: &str = "gguf";
/// Format of generated mock models (never registered with Ollama).
pub const FORMAT_MOCK: &str = "mock";
/// Largest generated mock model (16 GiB).
pub const MAX_MOCK_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Timeout of `ollama create`.
pub const OLLAMA_CREATE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Digest of a manifest (SHA-256 of its canonical JSON).
#[must_use]
pub fn manifest_digest(manifest: &ModelManifest) -> Digest {
    use sha2::{Digest as _, Sha256};
    let bytes = serde_json::to_vec(manifest).unwrap_or_default();
    Digest::from_sha256(&Sha256::digest(bytes).into())
}

/// `name:tag` of a manifest.
#[must_use]
pub fn model_ref(m: &ModelManifest) -> String {
    format!("{}:{}", m.name, m.tag)
}

/// Model store errors.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// Bad input (name, tag, path, size).
    #[error("invalid model: {0}")]
    Invalid(String),
    /// Manifest does not hash to the expected digest.
    #[error("manifest digest mismatch: expected {expected}, got {got}")]
    DigestMismatch {
        /// Expected.
        expected: Digest,
        /// Computed.
        got: Digest,
    },
    /// A layer blob is not present (verified) in the store.
    #[error("layer {0} missing")]
    MissingLayer(Digest),
    /// A layer's size differs from the manifest.
    #[error("layer {digest} has {actual} bytes, manifest says {expected}")]
    LayerSize {
        /// Layer.
        digest: Digest,
        /// Manifest size.
        expected: u64,
        /// Stored size.
        actual: u64,
    },
    /// Unknown model.
    #[error("model {0} not found")]
    NotFound(String),
    /// Blob store.
    #[error(transparent)]
    Blob(#[from] BlobError),
    /// I/O.
    #[error("model store io: {0}")]
    Io(#[from] std::io::Error),
}

/// Validates a model name / tag segment (`[A-Za-z0-9._-]`, 1..=128, no leading dot).
///
/// # Errors
/// [`ModelError::Invalid`].
pub fn validate_segment(what: &str, s: &str) -> Result<(), ModelError> {
    let ok = !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('.')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(ModelError::Invalid(format!("{what} `{s}` must match [A-Za-z0-9._-]{{1,128}}")))
    }
}

/// Installed-model index at `ORKESTR_HOME/models`.
#[derive(Debug, Clone)]
pub struct ModelStore {
    dir: PathBuf,
}

impl ModelStore {
    /// Opens (creating) the index of `home`.
    ///
    /// # Errors
    /// Directory creation.
    pub fn open(home: &Path) -> Result<Self, ModelError> {
        let dir = home.join("models");
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Index directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Installed models, sorted by `modelRef`.
    #[must_use]
    pub fn list(&self) -> Vec<StoredModel> {
        read_index(&self.dir)
    }

    /// Model by `name:tag`.
    #[must_use]
    pub fn get(&self, model_ref: &str) -> Option<StoredModel> {
        self.list().into_iter().find(|m| m.model_ref == model_ref)
    }

    /// Records an installed model (replacing other versions of the same ref).
    ///
    /// # Errors
    /// I/O.
    pub fn record(&self, m: &StoredModel) -> Result<(), ModelError> {
        let path = self.dir.join(format!("{}.json", m.digest.hex()));
        let tmp = self.dir.join(format!("{}.json.tmp", m.digest.hex()));
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&serde_json::to_vec_pretty(m).map_err(std::io::Error::other)?)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &path)?;
        for old in self.list() {
            if old.model_ref == m.model_ref && old.digest != m.digest {
                let _ = std::fs::remove_file(self.dir.join(format!("{}.json", old.digest.hex())));
            }
        }
        Ok(())
    }
}

fn read_index(dir: &Path) -> Vec<StoredModel> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<StoredModel> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<StoredModel>(&b).ok())
        .collect();
    out.sort_by(|a, b| a.model_ref.cmp(&b.model_ref));
    out
}

/// Installed models of `home` (`ORKESTR_HOME`). Cheap, synchronous.
#[must_use]
pub fn list_installed(home: &Path) -> Vec<StoredModel> {
    read_index(&home.join("models"))
}

/// Installed models as [`LocalModel`] rows (`name` = `name:tag`) — the
/// `NodeInfo.localModels.models` entries the scheduler routes on.
#[must_use]
pub fn local_models(home: &Path) -> Vec<LocalModel> {
    list_installed(home)
        .into_iter()
        .map(|m| LocalModel {
            name: m.model_ref,
            size_bytes: Some(m.size_bytes),
            family: None,
            parameter_size: None,
            quantization: None,
        })
        .collect()
}

/// Manifest digests of installed models (`NodeSnapshot.modelDigests`).
#[must_use]
pub fn installed_digests(home: &Path) -> Vec<Digest> {
    list_installed(home).into_iter().map(|m| m.digest).collect()
}

/// Adds the store's models to an Ollama discovery result (deduplicated by name).
pub fn merge_local_models(info: &mut LocalModelsInfo, home: &Path) {
    for m in local_models(home) {
        if !info.models.iter().any(|x| x.name == m.name) {
            info.models.push(m);
        }
    }
}

/// Checks every layer is present and sized as declared; returns the total size.
///
/// # Errors
/// [`ModelError::MissingLayer`] / [`ModelError::LayerSize`].
pub async fn verify_layers(blobs: &dyn BlobStore, manifest: &ModelManifest) -> Result<u64, ModelError> {
    let mut total = 0u64;
    for l in &manifest.layers {
        if !blobs.has(&l.digest).await {
            return Err(ModelError::MissingLayer(l.digest.clone()));
        }
        let actual = blobs.size(&l.digest).await.ok_or_else(|| ModelError::MissingLayer(l.digest.clone()))?;
        if actual != l.size {
            return Err(ModelError::LayerSize { digest: l.digest.clone(), expected: l.size, actual });
        }
        total += l.size;
    }
    Ok(total)
}

/// Installs a model whose layers are already in `blobs`: verifies the manifest
/// digest and every layer, registers GGUF models with Ollama when `ollama` (the
/// binary, see [`crate::ollama::find_binary`]) is given, then records it.
/// Ollama failures are logged and leave `ollamaInstalled: false`.
///
/// # Errors
/// Digest mismatch, missing / mis-sized layers, invalid names, I/O.
pub async fn install(
    store: &ModelStore,
    blobs: &dyn BlobStore,
    manifest: &ModelManifest,
    expected: &Digest,
    ollama: Option<&Path>,
) -> Result<StoredModel, ModelError> {
    validate_segment("name", &manifest.name)?;
    validate_segment("tag", &manifest.tag)?;
    let got = manifest_digest(manifest);
    if &got != expected {
        return Err(ModelError::DigestMismatch { expected: expected.clone(), got });
    }
    if manifest.layers.is_empty() {
        return Err(ModelError::Invalid("manifest has no layers".into()));
    }
    let size_bytes = verify_layers(blobs, manifest).await?;
    let mref = model_ref(manifest);
    let mut ollama_installed = false;
    if let (Some(bin), true) = (ollama, manifest.format == FORMAT_GGUF) {
        let weights =
            manifest.layers.iter().find(|l| l.media_type == MEDIA_TYPE_GGUF).and_then(|l| blobs.path(&l.digest));
        if let Some(weights) = weights {
            let (bin, r, scratch) = (bin.to_path_buf(), mref.clone(), store.dir.join("tmp"));
            let res = tokio::task::spawn_blocking(move || {
                crate::ollama::import_gguf(&bin, &r, &weights, &scratch, OLLAMA_CREATE_TIMEOUT)
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            match res {
                Ok(()) => ollama_installed = true,
                Err(e) => {
                    tracing::warn!(model = %mref, error = %e, "ollama create failed; model kept in the local store")
                }
            }
        }
    }
    let stored = StoredModel {
        model_ref: mref,
        digest: got,
        manifest: manifest.clone(),
        size_bytes,
        ollama_installed,
        installed_at: orkestr_domain::now(),
    };
    store.record(&stored)?;
    Ok(stored)
}

/// Writes a deterministic mock model of `size` bytes (GGUF magic + a PRNG
/// stream seeded by `seed`) to `path`. Blocking.
///
/// # Errors
/// I/O.
pub fn write_mock_model(path: &Path, seed: &str, size: u64) -> std::io::Result<()> {
    use sha2::{Digest as _, Sha256};
    let h = Sha256::digest(seed.as_bytes());
    let mut state = u64::from_le_bytes(h[..8].try_into().unwrap_or([7; 8]));
    let mut f = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
    let mut header = Vec::with_capacity(8);
    header.extend_from_slice(b"GGUF");
    header.extend_from_slice(&3u32.to_le_bytes());
    let mut written = 0u64;
    let head = usize::try_from(size.min(header.len() as u64)).unwrap_or(0);
    f.write_all(&header[..head])?;
    written += head as u64;
    let mut buf = vec![0u8; 64 * 1024];
    while written < size {
        for c in buf.chunks_mut(8) {
            // splitmix64
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            c.copy_from_slice(&z.to_le_bytes()[..c.len()]);
        }
        let n = usize::try_from((size - written).min(buf.len() as u64)).unwrap_or(buf.len());
        f.write_all(&buf[..n])?;
        written += n as u64;
    }
    f.flush()?;
    f.get_ref().sync_all()
}

/// Imports a file already copied into the blob store as a one-layer model and
/// installs it locally.
///
/// # Errors
/// See [`install`].
pub async fn import_blob(
    store: &ModelStore,
    blobs: &dyn BlobStore,
    name: &str,
    tag: &str,
    format: &str,
    layer: Digest,
    ollama: Option<&Path>,
) -> Result<StoredModel, ModelError> {
    validate_segment("name", name)?;
    validate_segment("tag", tag)?;
    let size = blobs.size(&layer).await.ok_or_else(|| ModelError::MissingLayer(layer.clone()))?;
    let media_type = if format == FORMAT_GGUF { MEDIA_TYPE_GGUF } else { MEDIA_TYPE_MOCK };
    let manifest = ModelManifest {
        name: name.to_owned(),
        tag: tag.to_owned(),
        format: format.to_owned(),
        layers: vec![ModelLayer { digest: layer, size, media_type: media_type.to_owned() }],
    };
    let d = manifest_digest(&manifest);
    install(store, blobs, &manifest, &d, ollama).await
}

/// Generates a deterministic mock model (`format: mock`) of `size` bytes in
/// `scratch`, imports it into `blobs` and installs it.
///
/// # Errors
/// Size limits, invalid names, I/O.
pub async fn import_mock(
    store: &ModelStore,
    blobs: &dyn BlobStore,
    scratch: &Path,
    name: &str,
    tag: &str,
    size: u64,
) -> Result<StoredModel, ModelError> {
    validate_segment("name", name)?;
    validate_segment("tag", tag)?;
    if size == 0 || size > MAX_MOCK_BYTES {
        return Err(ModelError::Invalid(format!("mock size must be 1..={MAX_MOCK_BYTES} bytes")));
    }
    std::fs::create_dir_all(scratch)?;
    let tmp = scratch.join(format!("mock-{}.gguf", orkestr_domain::CorrelationId::new()));
    let (p, seed) = (tmp.clone(), format!("{name}:{tag}"));
    tokio::task::spawn_blocking(move || write_mock_model(&p, &seed, size))
        .await
        .map_err(|e| ModelError::Io(std::io::Error::other(e)))??;
    let d = blobs.put_file(&tmp).await;
    let _ = std::fs::remove_file(&tmp);
    import_blob(store, blobs, name, tag, FORMAT_MOCK, d?, None).await
}

/// Imports a GGUF file located under `import_root` (`ORKESTR_HOME/import`).
///
/// # Errors
/// Paths outside `import_root`, non-GGUF files, invalid names, I/O.
pub async fn import_gguf_file(
    store: &ModelStore,
    blobs: &dyn BlobStore,
    import_root: &Path,
    path: &Path,
    name: &str,
    tag: &str,
    ollama: Option<&Path>,
) -> Result<StoredModel, ModelError> {
    validate_segment("name", name)?;
    validate_segment("tag", tag)?;
    let root = std::fs::canonicalize(import_root)
        .map_err(|_| ModelError::Invalid(format!("import directory {} does not exist", import_root.display())))?;
    let candidate = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };
    let file = std::fs::canonicalize(&candidate)
        .map_err(|_| ModelError::Invalid(format!("{} not found", candidate.display())))?;
    if !file.starts_with(&root) || !file.is_file() {
        return Err(ModelError::Invalid(format!("path must be a file inside {}", root.display())));
    }
    let mut magic = [0u8; 4];
    {
        use std::io::Read as _;
        std::fs::File::open(&file)?
            .read_exact(&mut magic)
            .map_err(|_| ModelError::Invalid("not a GGUF file".into()))?;
    }
    if &magic != b"GGUF" {
        return Err(ModelError::Invalid("not a GGUF file (bad magic)".into()));
    }
    let d = blobs.put_file(&file).await?;
    import_blob(store, blobs, name, tag, FORMAT_GGUF, d, ollama).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blobs::{FsBlobStore, MemBlobStore};

    #[tokio::test]
    async fn mock_import_install_list() {
        let home = tempfile::tempdir().unwrap();
        let blobs = FsBlobStore::open(home.path()).unwrap();
        let store = ModelStore::open(home.path()).unwrap();
        let m = import_mock(&store, &blobs, &home.path().join("scratch"), "tiny", "1m", 1 << 20).await.unwrap();
        assert_eq!(m.model_ref, "tiny:1m");
        assert_eq!(m.size_bytes, 1 << 20);
        assert!(!m.ollama_installed);
        assert_eq!(m.digest, manifest_digest(&m.manifest));
        // Deterministic: re-import gives the same layer digest.
        let again = import_mock(&store, &blobs, &home.path().join("scratch"), "tiny", "1m", 1 << 20).await.unwrap();
        assert_eq!(again.digest, m.digest);
        assert_eq!(list_installed(home.path()).len(), 1);
        assert_eq!(local_models(home.path())[0].name, "tiny:1m");
        assert_eq!(installed_digests(home.path()), vec![m.digest.clone()]);
        let mut info = LocalModelsInfo { ollama_endpoint: String::new(), ollama_reachable: false, models: vec![] };
        merge_local_models(&mut info, home.path());
        merge_local_models(&mut info, home.path());
        assert_eq!(info.models.len(), 1);
        let bytes = std::fs::read(blobs.path(&m.manifest.layers[0].digest).unwrap()).unwrap();
        assert_eq!(&bytes[..4], b"GGUF");
        assert!(import_mock(&store, &blobs, home.path(), "bad name", "x", 1).await.is_err());
        assert!(import_mock(&store, &blobs, home.path(), "x", "y", 0).await.is_err());
    }

    #[tokio::test]
    async fn install_verifies_digest_and_layers() {
        let home = tempfile::tempdir().unwrap();
        let store = ModelStore::open(home.path()).unwrap();
        let blobs = MemBlobStore::new();
        let d = blobs.insert(b"GGUFweights".to_vec());
        let manifest = ModelManifest {
            name: "m".into(),
            tag: "t".into(),
            format: FORMAT_MOCK.into(),
            layers: vec![ModelLayer { digest: d.clone(), size: 11, media_type: MEDIA_TYPE_MOCK.into() }],
        };
        let wrong = crate::blobs::digest_of(b"x");
        assert!(matches!(
            install(&store, &blobs, &manifest, &wrong, None).await,
            Err(ModelError::DigestMismatch { .. })
        ));
        let mut missing = manifest.clone();
        missing.layers[0].digest = wrong.clone();
        let md = manifest_digest(&missing);
        assert!(matches!(install(&store, &blobs, &missing, &md, None).await, Err(ModelError::MissingLayer(_))));
        let mut sized = manifest.clone();
        sized.layers[0].size = 12;
        let sd = manifest_digest(&sized);
        assert!(matches!(install(&store, &blobs, &sized, &sd, None).await, Err(ModelError::LayerSize { .. })));
        assert!(store.list().is_empty(), "nothing recorded on failure");
        let ok = install(&store, &blobs, &manifest, &manifest_digest(&manifest), None).await.unwrap();
        assert_eq!(store.get("m:t").unwrap(), ok);
    }

    #[tokio::test]
    async fn gguf_import_confined_to_import_dir() {
        let home = tempfile::tempdir().unwrap();
        let blobs = FsBlobStore::open(home.path()).unwrap();
        let store = ModelStore::open(home.path()).unwrap();
        let import = home.path().join("import");
        std::fs::create_dir_all(&import).unwrap();
        std::fs::write(import.join("a.gguf"), b"GGUF\x03\0\0\0rest").unwrap();
        std::fs::write(import.join("bad.gguf"), b"nope").unwrap();
        std::fs::write(home.path().join("outside.gguf"), b"GGUF").unwrap();
        // Ollama adapter skipped (no binary) → stays in the local store.
        let m = import_gguf_file(&store, &blobs, &import, Path::new("a.gguf"), "g", "1", None).await.unwrap();
        assert_eq!(m.manifest.format, FORMAT_GGUF);
        assert!(!m.ollama_installed);
        let err = |r: Result<StoredModel, ModelError>| matches!(r, Err(ModelError::Invalid(_)));
        assert!(err(import_gguf_file(&store, &blobs, &import, Path::new("bad.gguf"), "g", "2", None).await));
        assert!(err(import_gguf_file(&store, &blobs, &import, Path::new("../outside.gguf"), "g", "3", None).await));
        assert!(
            err(import_gguf_file(&store, &blobs, &import, &home.path().join("outside.gguf"), "g", "4", None).await)
        );
    }
}
