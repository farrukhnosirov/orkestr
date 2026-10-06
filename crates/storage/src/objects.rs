//! Content-addressed object store (§41 content-addressed chunks, §25 snapshots).
//!
//! Objects are identified by the blake3 hash of their **uncompressed** bytes
//! and stored at `<root>/<h[0..2]>/<h[2..4]>/<h>`. Each file starts with a
//! 5-byte header: magic `OKO1` + codec byte (`0` raw, `1` zstd). Writes go to a
//! temp file that is fsynced before it is atomically renamed (and the parent
//! directory is fsynced on Unix), so a crash never leaves a renamed object
//! with missing content; reads verify the hash. Storing content whose object
//! already exists verifies the existing object and rewrites it when it is
//! corrupt (e.g. damaged by a crash before this fix), so later snapshots never
//! keep pointing to a broken object.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::{StorageError, StorageResult};

const MAGIC: &[u8; 4] = b"OKO1";
const CODEC_RAW: u8 = 0;
const CODEC_ZSTD: u8 = 1;

/// blake3 hex digest identifying an object.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(String);

impl ObjectId {
    /// Computes the id of some bytes.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(blake3::hash(bytes).to_hex().to_string())
    }

    /// Parses a 64-char lowercase hex id.
    pub fn parse(s: &str) -> StorageResult<Self> {
        if s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            Ok(Self(s.to_owned()))
        } else {
            Err(StorageError::Invalid(format!("invalid object id `{}`", s.chars().take(80).collect::<String>())))
        }
    }

    /// Hex string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Compression policy for new objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// Store raw bytes.
    None,
    /// zstd at the given level; only kept when it actually saves space.
    Zstd {
        /// zstd level (1..=19).
        level: i32,
    },
}

/// Object store interface (§45). Implementations: local FS (here), S3/MinIO later.
#[async_trait]
pub trait ObjectStore: Send + Sync {
    /// Stores bytes and returns their id. Idempotent.
    async fn put(&self, bytes: Vec<u8>) -> StorageResult<ObjectId>;
    /// Like [`Self::put`], also reporting whether the object was (re)written.
    /// An existing object is verified and rewritten when it is corrupt, so
    /// callers must use this (not `exists` + `put`) to dedupe content.
    async fn put_checked(&self, bytes: Vec<u8>) -> StorageResult<(ObjectId, bool)> {
        let id = ObjectId::of(&bytes);
        let existed = self.get(&id).await.ok().flatten().is_some();
        let id = self.put(bytes).await?;
        Ok((id, !existed))
    }
    /// Loads an object, verifying its hash.
    async fn get(&self, id: &ObjectId) -> StorageResult<Option<Vec<u8>>>;
    /// Whether an object exists.
    async fn exists(&self, id: &ObjectId) -> StorageResult<bool>;
    /// Deletes an object. Returns whether it existed.
    async fn delete(&self, id: &ObjectId) -> StorageResult<bool>;
}

/// Filesystem implementation.
#[derive(Debug, Clone)]
pub struct FsObjectStore {
    root: PathBuf,
    compression: Compression,
}

impl FsObjectStore {
    /// Creates (if needed) the store directory.
    pub fn open(root: impl Into<PathBuf>, compression: Compression) -> StorageResult<Self> {
        let root = root.into();
        std::fs::create_dir_all(root.join("tmp"))?;
        Ok(Self { root, compression })
    }

    /// Root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_of(&self, id: &ObjectId) -> PathBuf {
        let h = id.as_str();
        self.root.join(&h[0..2]).join(&h[2..4]).join(h)
    }

    fn encode(&self, bytes: &[u8]) -> StorageResult<Vec<u8>> {
        let mut out = Vec::with_capacity(bytes.len() + 5);
        out.extend_from_slice(MAGIC);
        match self.compression {
            Compression::Zstd { level } if bytes.len() > 64 => {
                let compressed = zstd::bulk::compress(bytes, level)?;
                if compressed.len() < bytes.len() {
                    out.push(CODEC_ZSTD);
                    out.extend_from_slice(&compressed);
                    return Ok(out);
                }
                out.push(CODEC_RAW);
                out.extend_from_slice(bytes);
            }
            _ => {
                out.push(CODEC_RAW);
                out.extend_from_slice(bytes);
            }
        }
        Ok(out)
    }

    fn decode(id: &ObjectId, data: &[u8]) -> StorageResult<Vec<u8>> {
        if data.len() < 5 || &data[0..4] != MAGIC {
            return Err(StorageError::Corrupt(format!("object {id}: bad header")));
        }
        let body = &data[5..];
        let bytes = match data[4] {
            CODEC_RAW => body.to_vec(),
            CODEC_ZSTD => zstd::stream::decode_all(body)?,
            other => return Err(StorageError::Corrupt(format!("object {id}: unknown codec {other}"))),
        };
        if ObjectId::of(&bytes) != *id {
            return Err(StorageError::Corrupt(format!("object {id}: hash mismatch")));
        }
        Ok(bytes)
    }

    /// Whether the object at `path` holds exactly the content `id`.
    fn intact(id: &ObjectId, path: &Path) -> StorageResult<bool> {
        match std::fs::read(path) {
            Ok(data) => Ok(Self::decode(id, &data).is_ok()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Stores `bytes`; returns the id and whether the object was (re)written.
    fn put_blocking(&self, bytes: &[u8]) -> StorageResult<(ObjectId, bool)> {
        let id = ObjectId::of(bytes);
        let path = self.path_of(&id);
        if path.exists() {
            if Self::intact(&id, &path)? {
                return Ok((id, false));
            }
            tracing::warn!(object = %id, "stored object is corrupt; rewriting it");
        }
        let encoded = self.encode(bytes)?;
        let parent = path.parent().map(Path::to_path_buf);
        if let Some(parent) = &parent {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.root.join("tmp").join(format!("{}.{}", id.as_str(), uuid_like_suffix()));
        if let Err(e) = write_synced(&tmp, &encoded) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        match std::fs::rename(&tmp, &path) {
            Ok(()) => {
                if let Some(parent) = &parent {
                    sync_dir(parent);
                }
                Ok((id, true))
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                // A concurrent writer may have won the race; that is fine
                // when what it wrote is intact.
                if Self::intact(&id, &path).unwrap_or(false) {
                    Ok((id, false))
                } else {
                    Err(e.into())
                }
            }
        }
    }
}

/// Writes `data` to a new file and flushes it to stable storage.
fn write_synced(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(data)?;
    f.sync_all()
}

/// Makes a rename in `dir` durable (Unix; directories cannot be fsynced on
/// Windows, where NTFS journals the rename itself).
#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

fn uuid_like_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{nanos}-{n}.tmp", std::process::id())
}

#[async_trait]
impl ObjectStore for FsObjectStore {
    async fn put(&self, bytes: Vec<u8>) -> StorageResult<ObjectId> {
        let this = self.clone();
        Ok(self.put_checked(bytes).await?.0)
    }

    async fn put_checked(&self, bytes: Vec<u8>) -> StorageResult<(ObjectId, bool)> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.put_blocking(&bytes))
            .await
            .map_err(|e| StorageError::Join(e.to_string()))?
    }

    async fn get(&self, id: &ObjectId) -> StorageResult<Option<Vec<u8>>> {
        let path = self.path_of(id);
        let id = id.clone();
        tokio::task::spawn_blocking(move || match std::fs::read(&path) {
            Ok(data) => FsObjectStore::decode(&id, &data).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        })
        .await
        .map_err(|e| StorageError::Join(e.to_string()))?
    }

    async fn exists(&self, id: &ObjectId) -> StorageResult<bool> {
        Ok(tokio::fs::try_exists(self.path_of(id)).await?)
    }

    async fn delete(&self, id: &ObjectId) -> StorageResult<bool> {
        match tokio::fs::remove_file(self.path_of(id)).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_get_roundtrip_with_and_without_compression() {
        for compression in [Compression::None, Compression::Zstd { level: 3 }] {
            let dir = tempfile::tempdir().unwrap();
            let store = FsObjectStore::open(dir.path().join("objects"), compression).unwrap();
            let data = b"hello orkestr ".repeat(100);
            let id = store.put(data.clone()).await.unwrap();
            assert_eq!(id, ObjectId::of(&data));
            assert!(store.exists(&id).await.unwrap());
            assert_eq!(store.get(&id).await.unwrap().unwrap(), data);
            // idempotent
            assert_eq!(store.put(data.clone()).await.unwrap(), id);
            assert!(store.delete(&id).await.unwrap());
            assert!(!store.exists(&id).await.unwrap());
            assert!(store.get(&id).await.unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn zstd_actually_compresses_repetitive_data() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsObjectStore::open(dir.path(), Compression::Zstd { level: 3 }).unwrap();
        let data = vec![b'a'; 100_000];
        let id = store.put(data.clone()).await.unwrap();
        let on_disk = std::fs::metadata(store.path_of(&id)).unwrap().len();
        assert!(on_disk < 1_000, "compressed size {on_disk}");
    }

    #[tokio::test]
    async fn corruption_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsObjectStore::open(dir.path(), Compression::None).unwrap();
        let id = store.put(b"important".to_vec()).await.unwrap();
        let path = store.path_of(&id);
        let mut raw = std::fs::read(&path).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        std::fs::write(&path, raw).unwrap();
        assert!(matches!(store.get(&id).await, Err(StorageError::Corrupt(_))));
    }

    #[tokio::test]
    async fn corrupt_object_is_repaired_by_the_next_put() {
        for compression in [Compression::None, Compression::Zstd { level: 3 }] {
            let dir = tempfile::tempdir().unwrap();
            let store = FsObjectStore::open(dir.path(), compression).unwrap();
            let data = b"content that a crash damaged ".repeat(20);
            let (id, written) = store.put_checked(data.clone()).await.unwrap();
            assert!(written);
            let (_, again) = store.put_checked(data.clone()).await.unwrap();
            assert!(!again, "an intact object is not rewritten");
            // A power loss left the renamed object empty / truncated.
            for damaged in [Vec::new(), b"OKO1".to_vec(), vec![0u8; 64]] {
                std::fs::write(store.path_of(&id), &damaged).unwrap();
                assert!(store.get(&id).await.is_err());
                let (id2, rewritten) = store.put_checked(data.clone()).await.unwrap();
                assert_eq!(id2, id);
                assert!(rewritten, "corrupt object rewritten");
                assert_eq!(store.get(&id).await.unwrap().unwrap(), data);
            }
            // `put` repairs too.
            std::fs::write(store.path_of(&id), b"").unwrap();
            store.put(data.clone()).await.unwrap();
            assert_eq!(store.get(&id).await.unwrap().unwrap(), data);
            assert_eq!(std::fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0, "no temp files left");
        }
    }

    #[test]
    fn object_id_parsing() {
        let id = ObjectId::of(b"x");
        assert_eq!(ObjectId::parse(id.as_str()).unwrap(), id);
        assert!(ObjectId::parse("../../etc/passwd").is_err());
        assert!(ObjectId::parse(&"A".repeat(64)).is_err());
    }
}
