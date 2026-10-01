//! Content-addressed object store (§41 content-addressed chunks, §25 snapshots).
//!
//! Objects are identified by the blake3 hash of their **uncompressed** bytes
//! and stored at `<root>/<h[0..2]>/<h[2..4]>/<h>`. Each file starts with a
//! 5-byte header: magic `OKO1` + codec byte (`0` raw, `1` zstd). Writes go to a
//! temp file and are atomically renamed; reads verify the hash.

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

    fn put_blocking(&self, bytes: &[u8]) -> StorageResult<ObjectId> {
        let id = ObjectId::of(bytes);
        let path = self.path_of(&id);
        if path.exists() {
            return Ok(id);
        }
        let encoded = self.encode(bytes)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.root.join("tmp").join(format!("{}.{}", id.as_str(), uuid_like_suffix()));
        std::fs::write(&tmp, &encoded)?;
        match std::fs::rename(&tmp, &path) {
            Ok(()) => Ok(id),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                // A concurrent writer may have won the race; that is fine.
                if path.exists() {
                    Ok(id)
                } else {
                    Err(e.into())
                }
            }
        }
    }
}

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

    #[test]
    fn object_id_parsing() {
        let id = ObjectId::of(b"x");
        assert_eq!(ObjectId::parse(id.as_str()).unwrap(), id);
        assert!(ObjectId::parse("../../etc/passwd").is_err());
        assert!(ObjectId::parse(&"A".repeat(64)).is_err());
    }
}
