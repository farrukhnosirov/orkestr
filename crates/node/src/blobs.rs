//! Content-addressed blob store + chunked transfer (Phase 6, ADR-0039).
//! **Owner: WP-D.**
//!
//! * [`FsBlobStore`] — verified blobs at `ORKESTR_HOME/blobs/sha256/<hex>`,
//!   partial uploads in `ORKESTR_HOME/blobs/staging/<hex>.part`. Chunks are
//!   appended only at the exact current offset; when `received == total` the
//!   staging file is SHA-256 verified and atomically renamed into place. A
//!   mismatch discards the partial upload — nothing becomes visible.
//! * [`MemBlobStore`] — the same contract in memory (tests; WP-C).
//! * [`push_blob`] / [`pull_blob`] — resumable transfers over a [`NodeLink`]
//!   (`node.blobHas` → `node.blobPut` / `node.blobGet`, 512 KiB base64 chunks).

use std::collections::HashMap;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use orkestr_domain::Digest;
use orkestr_protocol::methods::models::{
    BlobGet, BlobGetParams, BlobHas, BlobHasParams, BlobPut, BlobPutParams, BLOB_CHUNK_SIZE,
};
use parking_lot::Mutex;
use sha2::{Digest as _, Sha256};

use crate::link::{call_typed, NodeLink};

/// Default maximum size of one blob (64 GiB).
pub const DEFAULT_MAX_BLOB_BYTES: u64 = 64 * 1024 * 1024 * 1024;

/// Blob errors.
#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    /// Malformed digest.
    #[error("invalid digest {0}")]
    InvalidDigest(String),
    /// Unknown blob.
    #[error("blob {0} not found")]
    NotFound(Digest),
    /// Chunk offset does not continue the partial upload.
    #[error("unexpected offset {got}, expected {expected}")]
    BadOffset {
        /// Offset sent.
        got: u64,
        /// Bytes received so far.
        expected: u64,
    },
    /// Finalized content does not hash to the digest.
    #[error("digest mismatch for {0}")]
    DigestMismatch(Digest),
    /// Chunk too large.
    #[error("chunk too large: {0} bytes")]
    ChunkTooLarge(usize),
    /// Declared blob size exceeds the store limit, or chunks overrun `total`.
    #[error("blob too large: {0} bytes")]
    TooLarge(u64),
    /// I/O.
    #[error("blob io: {0}")]
    Io(#[from] std::io::Error),
    /// Transport to the peer node.
    #[error("blob transfer: {0}")]
    Transfer(String),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

/// Content-addressed store (SHA-256).
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Whether a verified blob is present.
    async fn has(&self, d: &Digest) -> bool;
    /// Writes one chunk; returns bytes received so far. Finalizes (verifies the
    /// hash, atomically moves into place) when `received == total`.
    async fn put_chunk(&self, d: &Digest, offset: u64, total: u64, bytes: &[u8]) -> Result<u64, BlobError>;
    /// Reads up to `len` bytes at `offset` of a verified blob.
    async fn get_chunk(&self, d: &Digest, offset: u64, len: u32) -> Result<Vec<u8>, BlobError>;
    /// Imports a local file, returning its digest.
    async fn put_file(&self, path: &Path) -> Result<Digest, BlobError>;
    /// Path of a verified blob (only after finalize).
    fn path(&self, d: &Digest) -> Option<PathBuf>;
    /// Size of a verified blob.
    async fn size(&self, d: &Digest) -> Option<u64> {
        let p = self.path(d)?;
        tokio::fs::metadata(p).await.ok().map(|m| m.len())
    }
    /// Resume offset: bytes of a partial upload received so far (the full size
    /// when the blob is already verified, 0 when unknown).
    async fn received(&self, d: &Digest) -> u64 {
        self.size(d).await.unwrap_or(0)
    }
}

fn check_digest(d: &Digest) -> Result<(), BlobError> {
    if d.is_valid() {
        Ok(())
    } else {
        Err(BlobError::InvalidDigest(d.0.clone()))
    }
}

fn check_chunk(bytes: &[u8]) -> Result<(), BlobError> {
    if bytes.len() > BLOB_CHUNK_SIZE as usize {
        return Err(BlobError::ChunkTooLarge(bytes.len()));
    }
    Ok(())
}

/// SHA-256 of a reader, streamed.
fn hash_reader(mut r: impl std::io::Read) -> std::io::Result<Digest> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(Digest::from_sha256(&h.finalize().into()))
}

/// SHA-256 of bytes.
#[must_use]
pub fn digest_of(bytes: &[u8]) -> Digest {
    Digest::from_sha256(&Sha256::digest(bytes).into())
}

/// Filesystem store under `ORKESTR_HOME/blobs`.
#[derive(Debug)]
pub struct FsBlobStore {
    root: PathBuf,
    staging: PathBuf,
    max_blob_bytes: u64,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl FsBlobStore {
    /// Opens (creating directories) the store of `home` (`ORKESTR_HOME`):
    /// verified blobs at `home/blobs/sha256/<hex>`, partial uploads in
    /// `home/blobs/staging`.
    ///
    /// # Errors
    /// Directory creation failures.
    pub fn open(home: &Path) -> Result<Self, BlobError> {
        let base = home.join("blobs");
        let root = base.join("sha256");
        let staging = base.join("staging");
        std::fs::create_dir_all(&root)?;
        std::fs::create_dir_all(&staging)?;
        Ok(Self { root, staging, max_blob_bytes: DEFAULT_MAX_BLOB_BYTES, locks: Mutex::new(HashMap::new()) })
    }

    /// Sets the maximum accepted blob size.
    #[must_use]
    pub fn with_max_blob_bytes(mut self, max: u64) -> Self {
        self.max_blob_bytes = max;
        self
    }

    /// Directory of verified blobs.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Staging directory (partial uploads; scratch space for imports).
    #[must_use]
    pub fn staging_dir(&self) -> &Path {
        &self.staging
    }

    fn final_path(&self, d: &Digest) -> PathBuf {
        self.root.join(d.hex())
    }

    fn part_path(&self, d: &Digest) -> PathBuf {
        self.staging.join(format!("{}.part", d.hex()))
    }

    fn lock_for(&self, d: &Digest) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(self.locks.lock().entry(d.hex().to_owned()).or_default())
    }

    /// Discards a partial upload.
    ///
    /// # Errors
    /// I/O other than "not found".
    pub async fn discard_partial(&self, d: &Digest) -> Result<(), BlobError> {
        check_digest(d)?;
        let lock = self.lock_for(d);
        let _g = lock.lock().await;
        match tokio::fs::remove_file(self.part_path(d)).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

/// Verifies `part` against `d` and renames it to `dst`; removes it on mismatch.
fn finalize(part: &Path, dst: &Path, d: &Digest) -> Result<(), BlobError> {
    let got = hash_reader(std::fs::File::open(part)?)?;
    if &got != d {
        let _ = std::fs::remove_file(part);
        return Err(BlobError::DigestMismatch(d.clone()));
    }
    if dst.is_file() {
        // Concurrent identical upload already landed: content is the same.
        let _ = std::fs::remove_file(part);
        return Ok(());
    }
    std::fs::rename(part, dst)?;
    Ok(())
}

#[async_trait]
impl BlobStore for FsBlobStore {
    async fn has(&self, d: &Digest) -> bool {
        d.is_valid() && self.final_path(d).is_file()
    }

    async fn put_chunk(&self, d: &Digest, offset: u64, total: u64, bytes: &[u8]) -> Result<u64, BlobError> {
        check_digest(d)?;
        check_chunk(bytes)?;
        if total > self.max_blob_bytes {
            return Err(BlobError::TooLarge(total));
        }
        let lock = self.lock_for(d);
        let _g = lock.lock().await;
        let dst = self.final_path(d);
        if let Ok(m) = tokio::fs::metadata(&dst).await {
            return Ok(m.len());
        }
        let part = self.part_path(d);
        let bytes = bytes.to_vec();
        let d = d.clone();
        tokio::task::spawn_blocking(move || -> Result<u64, BlobError> {
            let mut f = std::fs::OpenOptions::new().create(true).read(true).write(true).truncate(false).open(&part)?;
            let cur = f.metadata()?.len();
            if offset != cur {
                return Err(BlobError::BadOffset { got: offset, expected: cur });
            }
            let len = bytes.len() as u64;
            let new = cur + len;
            if new > total {
                drop(f);
                let _ = std::fs::remove_file(&part);
                return Err(BlobError::TooLarge(new));
            }
            f.seek(SeekFrom::Start(cur))?;
            f.write_all(&bytes)?;
            f.sync_data()?;
            drop(f);
            if new == total {
                finalize(&part, &dst, &d)?;
            }
            Ok(new)
        })
        .await
        .map_err(|e| BlobError::Io(std::io::Error::other(e)))?
    }

    async fn get_chunk(&self, d: &Digest, offset: u64, len: u32) -> Result<Vec<u8>, BlobError> {
        check_digest(d)?;
        let len = len.min(BLOB_CHUNK_SIZE);
        let p = self.final_path(d);
        let dc = d.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<u8>, BlobError> {
            let mut f = match std::fs::File::open(&p) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(BlobError::NotFound(dc)),
                Err(e) => return Err(e.into()),
            };
            let size = f.metadata()?.len();
            if offset > size {
                return Err(BlobError::BadOffset { got: offset, expected: size });
            }
            f.seek(SeekFrom::Start(offset))?;
            let mut out = Vec::with_capacity(len as usize);
            f.take(u64::from(len)).read_to_end(&mut out)?;
            Ok(out)
        })
        .await
        .map_err(|e| BlobError::Io(std::io::Error::other(e)))?
    }

    async fn put_file(&self, path: &Path) -> Result<Digest, BlobError> {
        let src = path.to_path_buf();
        let staging = self.staging.clone();
        let root = self.root.clone();
        let max = self.max_blob_bytes;
        tokio::task::spawn_blocking(move || -> Result<Digest, BlobError> {
            let size = std::fs::metadata(&src)?.len();
            if size > max {
                return Err(BlobError::TooLarge(size));
            }
            // Copy first, then hash the copy: the digest always describes the
            // bytes that land in the store even if the source changes.
            let tmp = staging.join(format!("import-{}.tmp", orkestr_domain::CorrelationId::new()));
            std::fs::copy(&src, &tmp)?;
            let d = match hash_reader(std::fs::File::open(&tmp)?) {
                Ok(d) => d,
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e.into());
                }
            };
            let dst = root.join(d.hex());
            finalize(&tmp, &dst, &d)?;
            Ok(d)
        })
        .await
        .map_err(|e| BlobError::Io(std::io::Error::other(e)))?
    }

    fn path(&self, d: &Digest) -> Option<PathBuf> {
        if !d.is_valid() {
            return None;
        }
        let p = self.final_path(d);
        p.is_file().then_some(p)
    }

    async fn received(&self, d: &Digest) -> u64 {
        if !d.is_valid() {
            return 0;
        }
        if let Ok(m) = tokio::fs::metadata(self.final_path(d)).await {
            return m.len();
        }
        tokio::fs::metadata(self.part_path(d)).await.map(|m| m.len()).unwrap_or(0)
    }
}

/// In-memory [`BlobStore`] with the same verification contract (tests).
#[derive(Debug, Default)]
pub struct MemBlobStore {
    done: Mutex<HashMap<Digest, Arc<Vec<u8>>>>,
    partial: Mutex<HashMap<Digest, Vec<u8>>>,
}

impl MemBlobStore {
    /// Empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts bytes directly, returning their digest.
    pub fn insert(&self, bytes: Vec<u8>) -> Digest {
        let d = digest_of(&bytes);
        self.done.lock().insert(d.clone(), Arc::new(bytes));
        d
    }

    /// Bytes of a verified blob.
    #[must_use]
    pub fn get(&self, d: &Digest) -> Option<Arc<Vec<u8>>> {
        self.done.lock().get(d).cloned()
    }
}

#[async_trait]
impl BlobStore for MemBlobStore {
    async fn has(&self, d: &Digest) -> bool {
        self.done.lock().contains_key(d)
    }

    async fn put_chunk(&self, d: &Digest, offset: u64, total: u64, bytes: &[u8]) -> Result<u64, BlobError> {
        check_digest(d)?;
        check_chunk(bytes)?;
        if let Some(b) = self.done.lock().get(d) {
            return Ok(b.len() as u64);
        }
        let mut partial = self.partial.lock();
        let buf = partial.entry(d.clone()).or_default();
        let cur = buf.len() as u64;
        if offset != cur {
            return Err(BlobError::BadOffset { got: offset, expected: cur });
        }
        let new = cur + bytes.len() as u64;
        if new > total {
            partial.remove(d);
            return Err(BlobError::TooLarge(new));
        }
        buf.extend_from_slice(bytes);
        if new == total {
            let buf = partial.remove(d).unwrap_or_default();
            if &digest_of(&buf) != d {
                return Err(BlobError::DigestMismatch(d.clone()));
            }
            self.done.lock().insert(d.clone(), Arc::new(buf));
        }
        Ok(new)
    }

    async fn get_chunk(&self, d: &Digest, offset: u64, len: u32) -> Result<Vec<u8>, BlobError> {
        let b = self.get(d).ok_or_else(|| BlobError::NotFound(d.clone()))?;
        let size = b.len() as u64;
        if offset > size {
            return Err(BlobError::BadOffset { got: offset, expected: size });
        }
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let end = start.saturating_add(len.min(BLOB_CHUNK_SIZE) as usize).min(b.len());
        Ok(b[start..end].to_vec())
    }

    async fn put_file(&self, path: &Path) -> Result<Digest, BlobError> {
        let bytes = tokio::fs::read(path).await?;
        Ok(self.insert(bytes))
    }

    fn path(&self, _d: &Digest) -> Option<PathBuf> {
        None
    }

    async fn size(&self, d: &Digest) -> Option<u64> {
        self.get(d).map(|b| b.len() as u64)
    }

    async fn received(&self, d: &Digest) -> u64 {
        if let Some(b) = self.get(d) {
            return b.len() as u64;
        }
        self.partial.lock().get(d).map_or(0, |b| b.len() as u64)
    }
}

fn transfer(e: impl std::fmt::Display) -> BlobError {
    BlobError::Transfer(e.to_string())
}

/// Pushes blob `d` from `src` to the worker behind `link` (`node.blobHas` →
/// resume offset → `node.blobPut` 512 KiB chunks).
///
/// # Errors
/// Missing source blob, transport errors, digest mismatch on the worker.
pub async fn push_blob(src: &dyn BlobStore, link: &dyn NodeLink, d: &Digest) -> Result<(), BlobError> {
    push_blob_counted(src, link, d).await.map(|_| ())
}

/// [`push_blob`] returning the number of bytes actually sent (0 when the
/// worker already had the blob; less than the size when resuming).
///
/// # Errors
/// See [`push_blob`].
pub async fn push_blob_counted(src: &dyn BlobStore, link: &dyn NodeLink, d: &Digest) -> Result<u64, BlobError> {
    check_digest(d)?;
    let total = src.size(d).await.ok_or_else(|| BlobError::NotFound(d.clone()))?;
    let has = call_typed::<BlobHas>(link, &BlobHasParams { digest: d.clone() }).await.map_err(transfer)?;
    if has.present {
        return Ok(0);
    }
    let mut offset = has.received;
    if offset > total {
        return Err(BlobError::BadOffset { got: offset, expected: total });
    }
    let mut sent = 0u64;
    let b64 = base64::engine::general_purpose::STANDARD;
    loop {
        let chunk = src.get_chunk(d, offset, BLOB_CHUNK_SIZE).await?;
        if chunk.is_empty() && offset < total {
            return Err(BlobError::Transfer(format!("source blob {d} truncated at {offset}")));
        }
        let res =
            call_typed::<BlobPut>(link, &BlobPutParams { digest: d.clone(), offset, total, data: b64.encode(&chunk) })
                .await
                .map_err(transfer)?;
        sent += chunk.len() as u64;
        if res.complete {
            return Ok(sent);
        }
        if res.received <= offset && !chunk.is_empty() {
            return Err(BlobError::Transfer(format!("worker did not advance past {offset}")));
        }
        offset = res.received;
        if offset >= total {
            return Err(BlobError::Transfer(format!("worker did not finalize {d}")));
        }
    }
}

/// Fetches blob `d` from the worker behind `link` into `dst` (`node.blobGet`),
/// resuming from `dst`'s partial upload. Verified by `dst` on finalize.
///
/// # Errors
/// Transport errors, digest mismatch (nothing is stored then).
pub async fn pull_blob(link: &dyn NodeLink, dst: &dyn BlobStore, d: &Digest) -> Result<u64, BlobError> {
    check_digest(d)?;
    if dst.has(d).await {
        return Ok(0);
    }
    let mut offset = dst.received(d).await;
    let mut got = 0u64;
    let b64 = base64::engine::general_purpose::STANDARD;
    loop {
        let res = call_typed::<BlobGet>(link, &BlobGetParams { digest: d.clone(), offset, len: BLOB_CHUNK_SIZE })
            .await
            .map_err(transfer)?;
        let data = b64.decode(res.data.as_bytes()).map_err(transfer)?;
        if data.is_empty() && offset < res.total {
            return Err(BlobError::Transfer(format!("peer returned no data at {offset}")));
        }
        let received = dst.put_chunk(d, offset, res.total, &data).await?;
        got += data.len() as u64;
        if received >= res.total {
            return Ok(got);
        }
        offset = received;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeError;
    use orkestr_domain::NodeId;
    use orkestr_protocol::methods::models::{BlobGetResult, BlobHasResult, BlobPutResult};
    use orkestr_protocol::methods::Method as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A link that serves `node.blob*` from a store directly, optionally failing
    /// after `fail_after` puts, or corrupting one chunk.
    struct StoreLink {
        store: Arc<dyn BlobStore>,
        puts: AtomicUsize,
        fail_after: Option<usize>,
        corrupt_put: Option<usize>,
    }

    impl StoreLink {
        fn new(store: Arc<dyn BlobStore>) -> Self {
            Self { store, puts: AtomicUsize::new(0), fail_after: None, corrupt_put: None }
        }
    }

    #[async_trait]
    impl NodeLink for StoreLink {
        fn node_id(&self) -> NodeId {
            NodeId::new()
        }
        async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, NodeError> {
            let b64 = base64::engine::general_purpose::STANDARD;
            let err = |e: BlobError| NodeError::Transport(e.to_string());
            match method {
                m if m == BlobHas::NAME => {
                    let p: BlobHasParams = serde_json::from_value(params).unwrap();
                    let r = BlobHasResult {
                        present: self.store.has(&p.digest).await,
                        received: self.store.received(&p.digest).await,
                    };
                    Ok(serde_json::to_value(r).unwrap())
                }
                m if m == BlobPut::NAME => {
                    let n = self.puts.fetch_add(1, Ordering::SeqCst);
                    if self.fail_after.is_some_and(|f| n >= f) {
                        return Err(NodeError::Transport("connection dropped".into()));
                    }
                    let p: BlobPutParams = serde_json::from_value(params).unwrap();
                    let mut data = b64.decode(p.data).unwrap();
                    if self.corrupt_put == Some(n) && !data.is_empty() {
                        data[0] ^= 0xff;
                    }
                    let received = self.store.put_chunk(&p.digest, p.offset, p.total, &data).await.map_err(err)?;
                    Ok(serde_json::to_value(BlobPutResult { received, complete: received == p.total }).unwrap())
                }
                m if m == BlobGet::NAME => {
                    let p: BlobGetParams = serde_json::from_value(params).unwrap();
                    let total = self.store.size(&p.digest).await.unwrap_or(0);
                    let data = self.store.get_chunk(&p.digest, p.offset, p.len).await.map_err(err)?;
                    Ok(serde_json::to_value(BlobGetResult { total, data: b64.encode(data) }).unwrap())
                }
                _ => Err(NodeError::Internal(format!("unexpected {method}"))),
            }
        }
        fn events(&self) -> tokio::sync::broadcast::Receiver<orkestr_protocol::Event> {
            tokio::sync::broadcast::channel(1).1
        }
    }

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| u8::try_from((i * 31 + i / 7) % 251).unwrap()).collect()
    }

    #[tokio::test]
    async fn fs_store_put_verify_get() {
        let dir = tempfile::tempdir().unwrap();
        let s = FsBlobStore::open(dir.path()).unwrap();
        let bytes = data(1_300_000);
        let d = digest_of(&bytes);
        let mut off = 0u64;
        for c in bytes.chunks(BLOB_CHUNK_SIZE as usize) {
            assert!(s.path(&d).is_none(), "not visible before finalize");
            off = s.put_chunk(&d, off, bytes.len() as u64, c).await.unwrap();
        }
        assert!(s.has(&d).await);
        assert_eq!(s.path(&d).unwrap(), dir.path().join("blobs").join("sha256").join(d.hex()));
        assert_eq!(s.get_chunk(&d, 10, 5).await.unwrap(), bytes[10..15].to_vec());
        assert_eq!(s.size(&d).await, Some(bytes.len() as u64));
        // Bad offset, oversize chunk, invalid digest.
        let d2 = digest_of(b"xyz");
        assert!(matches!(s.put_chunk(&d2, 1, 3, b"y").await, Err(BlobError::BadOffset { got: 1, expected: 0 })));
        let big = vec![0u8; BLOB_CHUNK_SIZE as usize + 1];
        assert!(matches!(s.put_chunk(&d2, 0, 3, &big).await, Err(BlobError::ChunkTooLarge(_))));
        assert!(matches!(
            s.put_chunk(&Digest("sha256:../x".into()), 0, 1, b"x").await,
            Err(BlobError::InvalidDigest(_))
        ));
        // Size limit.
        let small = FsBlobStore::open(dir.path()).unwrap().with_max_blob_bytes(2);
        assert!(matches!(small.put_chunk(&d2, 0, 3, b"x").await, Err(BlobError::TooLarge(3))));
        // put_file.
        let f = dir.path().join("f.bin");
        std::fs::write(&f, b"hello").unwrap();
        let d3 = s.put_file(&f).await.unwrap();
        assert_eq!(d3, digest_of(b"hello"));
        assert!(s.has(&d3).await);
        // Empty blob.
        let de = digest_of(b"");
        assert_eq!(s.put_chunk(&de, 0, 0, b"").await.unwrap(), 0);
        assert!(s.has(&de).await);
    }

    #[tokio::test]
    async fn corrupted_chunk_rejected_nothing_visible() {
        let dir = tempfile::tempdir().unwrap();
        let src = Arc::new(MemBlobStore::new());
        let bytes = data(1_200_000);
        let d = src.insert(bytes.clone());
        let dst: Arc<FsBlobStore> = Arc::new(FsBlobStore::open(dir.path()).unwrap());
        let mut link = StoreLink::new(dst.clone());
        link.corrupt_put = Some(1);
        let err = push_blob(src.as_ref(), &link, &d).await.unwrap_err();
        assert!(err.to_string().contains("digest mismatch"), "{err}");
        assert!(!dst.has(&d).await);
        assert!(dst.path(&d).is_none());
        assert_eq!(dst.received(&d).await, 0, "partial upload discarded");
        assert_eq!(std::fs::read_dir(dst.staging_dir()).unwrap().count(), 0);
        // A clean retry succeeds.
        let link = StoreLink::new(dst.clone());
        assert_eq!(push_blob_counted(src.as_ref(), &link, &d).await.unwrap(), bytes.len() as u64);
        assert!(dst.has(&d).await);
        assert_eq!(std::fs::read(dst.path(&d).unwrap()).unwrap(), bytes);

        // Same contract for the in-memory store.
        let mem = Arc::new(MemBlobStore::new());
        let mut link = StoreLink::new(mem.clone());
        link.corrupt_put = Some(0);
        assert!(push_blob(src.as_ref(), &link, &d).await.is_err());
        assert!(!mem.has(&d).await);
        assert_eq!(mem.received(&d).await, 0);
    }

    #[tokio::test]
    async fn resume_interrupted_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let src = Arc::new(MemBlobStore::new());
        let bytes = data(5 * BLOB_CHUNK_SIZE as usize + 123);
        let d = src.insert(bytes.clone());
        let dst: Arc<FsBlobStore> = Arc::new(FsBlobStore::open(dir.path()).unwrap());
        let mut link = StoreLink::new(dst.clone());
        link.fail_after = Some(2);
        assert!(matches!(push_blob(src.as_ref(), &link, &d).await, Err(BlobError::Transfer(_))));
        assert!(!dst.has(&d).await);
        assert_eq!(dst.received(&d).await, 2 * u64::from(BLOB_CHUNK_SIZE));
        // Resume: only the remainder is sent.
        let link = StoreLink::new(dst.clone());
        let sent = push_blob_counted(src.as_ref(), &link, &d).await.unwrap();
        assert_eq!(sent, bytes.len() as u64 - 2 * u64::from(BLOB_CHUNK_SIZE));
        assert_eq!(std::fs::read(dst.path(&d).unwrap()).unwrap(), bytes);
        // Already present → nothing sent.
        assert_eq!(push_blob_counted(src.as_ref(), &link, &d).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn pull_blob_resumes_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let remote = Arc::new(MemBlobStore::new());
        let bytes = data(3 * BLOB_CHUNK_SIZE as usize + 7);
        let d = remote.insert(bytes.clone());
        let link = StoreLink::new(remote.clone());
        let local = FsBlobStore::open(dir.path()).unwrap();
        // Simulate an interrupted earlier pull: first chunk already staged.
        local.put_chunk(&d, 0, bytes.len() as u64, &bytes[..BLOB_CHUNK_SIZE as usize]).await.unwrap();
        let got = pull_blob(&link, &local, &d).await.unwrap();
        assert_eq!(got, bytes.len() as u64 - u64::from(BLOB_CHUNK_SIZE));
        assert_eq!(std::fs::read(local.path(&d).unwrap()).unwrap(), bytes);
        // Missing remote blob.
        let missing = digest_of(b"nope");
        assert!(pull_blob(&link, &local, &missing).await.is_err());
        assert!(!local.has(&missing).await);
    }
}
