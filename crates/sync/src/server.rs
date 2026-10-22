//! Sync server (ref / chunk store) mounted by the gateway at `/v1/sync`.
//!
//! The server is a dumb, untrusted store (§1.2): it only ever sees sealed
//! chunks, sealed manifests, signed refs with opaque stream ids and the sealed
//! key ring. Objects live on the filesystem (`<dir>/<account>/{chunks,manifests}`),
//! heads in SQLite (`<dir>/refs.db`) with compare-and-swap via ETags.
//!
//! API (all requests need `Authorization: Bearer <token>`):
//! * `POST /chunks/has` `{"ids":[..]}` → `{"have":[bool..]}`
//! * `PUT|GET /chunks/{id}` raw sealed bytes
//! * `PUT|GET /manifests/{id}` JSON `SignedManifest`
//! * `GET /heads/{stream}` → `{"etag","value"}` | 404
//! * `PUT /heads/{stream}` `{"expected": etag|null, "value"}` → `{"etag"}` | 412 `{"etag","value"}`

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Bearer-token → account id resolver supplied by the gateway.
pub type AuthFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Shared state of the sync server.
#[derive(Clone)]
pub struct SyncServerState {
    dir: PathBuf,
    auth: AuthFn,
    refs: Arc<Mutex<Connection>>,
}

impl std::fmt::Debug for SyncServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncServerState").field("dir", &self.dir).finish_non_exhaustive()
    }
}

impl SyncServerState {
    /// Opens (creating if needed) the store under `dir`.
    ///
    /// # Errors
    /// Fails when `dir` cannot be created or the ref db cannot be opened.
    pub fn open(dir: &Path, auth: AuthFn) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let c = Connection::open(dir.join("refs.db"))?;
        c.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS heads (account TEXT NOT NULL, stream TEXT NOT NULL, etag TEXT NOT NULL,
               value TEXT NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY (account, stream));",
        )?;
        Ok(Self { dir: dir.to_path_buf(), auth, refs: Arc::new(Mutex::new(c)) })
    }

    /// Store directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn account(&self, headers: &HeaderMap) -> Result<String, StatusCode> {
        let token = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let acct = (self.auth)(token).ok_or(StatusCode::UNAUTHORIZED)?;
        Ok(blake3::hash(acct.as_bytes()).to_hex()[..32].to_owned())
    }

    fn obj_path(&self, acct: &str, kind: &str, id: &str) -> PathBuf {
        self.dir.join(acct).join(kind).join(&id[..2]).join(id)
    }
}

fn valid_hex_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_stream(s: &str) -> bool {
    !s.is_empty() && s.len() <= 80 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp{}", crate::crypto::b64(&crate::crypto::random::<6>())));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::warn!(error = %e, "sync server error");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

#[derive(Deserialize)]
struct HasReq {
    ids: Vec<String>,
}

#[derive(Serialize)]
struct HasResp {
    have: Vec<bool>,
}

async fn chunks_has(State(s): State<SyncServerState>, headers: HeaderMap, Json(req): Json<HasReq>) -> Response {
    let acct = match s.account(&headers) {
        Ok(a) => a,
        Err(r) => return r.into_response(),
    };
    let have = req.ids.iter().map(|id| valid_hex_id(id) && s.obj_path(&acct, "chunks", id).exists()).collect();
    Json(HasResp { have }).into_response()
}

async fn put_obj(s: SyncServerState, headers: HeaderMap, kind: &str, id: String, body: Bytes) -> Response {
    let acct = match s.account(&headers) {
        Ok(a) => a,
        Err(r) => return r.into_response(),
    };
    if !valid_hex_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = s.obj_path(&acct, kind, &id);
    if path.exists() {
        return StatusCode::OK.into_response();
    }
    match tokio::task::spawn_blocking(move || write_atomic(&path, &body)).await {
        Ok(Ok(())) => StatusCode::CREATED.into_response(),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e),
    }
}

async fn get_obj(s: SyncServerState, headers: HeaderMap, kind: &str, id: String) -> Response {
    let acct = match s.account(&headers) {
        Ok(a) => a,
        Err(r) => return r.into_response(),
    };
    if !valid_hex_id(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match tokio::fs::read(s.obj_path(&acct, kind, &id)).await {
        Ok(b) => b.into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal(e),
    }
}

async fn put_chunk(
    State(s): State<SyncServerState>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    put_obj(s, headers, "chunks", id, body).await
}

async fn get_chunk(State(s): State<SyncServerState>, UrlPath(id): UrlPath<String>, headers: HeaderMap) -> Response {
    get_obj(s, headers, "chunks", id).await
}

async fn put_manifest(
    State(s): State<SyncServerState>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    put_obj(s, headers, "manifests", id, body).await
}

async fn get_manifest(State(s): State<SyncServerState>, UrlPath(id): UrlPath<String>, headers: HeaderMap) -> Response {
    get_obj(s, headers, "manifests", id).await
}

/// Head value + etag on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeadDoc {
    /// ETag.
    pub etag: String,
    /// Opaque value (signed ref JSON or sealed ring).
    pub value: String,
}

/// CAS request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CasReq {
    /// Expected current etag (`None` = must not exist).
    pub expected: Option<String>,
    /// New value.
    pub value: String,
}

fn read_head(c: &Connection, acct: &str, stream: &str) -> rusqlite::Result<Option<HeadDoc>> {
    c.query_row("SELECT etag, value FROM heads WHERE account=?1 AND stream=?2", params![acct, stream], |r| {
        Ok(HeadDoc { etag: r.get(0)?, value: r.get(1)? })
    })
    .optional()
}

async fn get_head(State(s): State<SyncServerState>, UrlPath(stream): UrlPath<String>, headers: HeaderMap) -> Response {
    let acct = match s.account(&headers) {
        Ok(a) => a,
        Err(r) => return r.into_response(),
    };
    if !valid_stream(&stream) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let c = s.refs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match read_head(&c, &acct, &stream) {
        Ok(Some(h)) => Json(h).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal(e),
    }
}

async fn put_head(
    State(s): State<SyncServerState>,
    UrlPath(stream): UrlPath<String>,
    headers: HeaderMap,
    Json(req): Json<CasReq>,
) -> Response {
    let acct = match s.account(&headers) {
        Ok(a) => a,
        Err(r) => return r.into_response(),
    };
    if !valid_stream(&stream) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let c = s.refs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let current = match read_head(&c, &acct, &stream) {
        Ok(h) => h,
        Err(e) => return internal(e),
    };
    if current.as_ref().map(|h| &h.etag) != req.expected.as_ref() {
        return match current {
            Some(h) => (StatusCode::PRECONDITION_FAILED, Json(h)).into_response(),
            None => StatusCode::PRECONDITION_FAILED.into_response(),
        };
    }
    let etag = blake3::hash(format!("{}|{}", req.expected.unwrap_or_default(), req.value).as_bytes()).to_hex()[..24]
        .to_owned();
    let res = c.execute(
        "INSERT INTO heads(account, stream, etag, value, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(account, stream) DO UPDATE SET etag=excluded.etag, value=excluded.value, updated_at=excluded.updated_at",
        params![acct, stream, etag, req.value, orkestr_domain::now().to_rfc3339()],
    );
    match res {
        Ok(_) => Json(serde_json::json!({ "etag": etag })).into_response(),
        Err(e) => internal(e),
    }
}

/// The `/v1/sync` router (mount with `Router::nest("/v1/sync", …)`).
pub fn server_router(state: SyncServerState) -> Router {
    Router::new()
        .route("/chunks/has", post(chunks_has))
        .route("/chunks/{id}", get(get_chunk).put(put_chunk))
        .route("/manifests/{id}", get(get_manifest).put(put_manifest))
        .route("/heads/{stream}", get(get_head).put(put_head))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state)
}
