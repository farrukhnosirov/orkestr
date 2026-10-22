//! Content-addressed blob store + chunked transfer (Phase 6, ADR-0039).
//! **Owner: WP-D.** `FsBlobStore` (at `ORKESTR_HOME/blobs/sha256/..`, atomic
//! finalize after hash verification) and [`push_blob`] land here.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use orkestr_domain::Digest;

use crate::link::NodeLink;

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
}

/// Pushes blob `d` from `src` to the worker behind `link` (`node.blobHas` →
/// resume offset → `node.blobPut` 512 KiB chunks).
///
/// # Errors
/// Missing source blob, transport errors, digest mismatch on the worker.
pub async fn push_blob(src: &dyn BlobStore, link: &dyn NodeLink, d: &Digest) -> Result<(), BlobError> {
    // Step 0 skeleton: WP-D implements the transfer.
    let _ = (src, link, d);
    Err(BlobError::Unimplemented("node.blobPut transfer"))
}
