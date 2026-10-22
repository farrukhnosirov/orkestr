//! Self-hosted marketplace registry at `/registry/*` (Phase 7, ADR-0040).
//! **Owner: WP2.**
//!
//! Serves a static signed registry from `<data_dir>/registry`:
//!
//! * `GET /registry/index.json`, `GET /registry/index.sig` — the index and
//!   its detached ed25519 signature (`{keyId, publicKey, signature}` over the
//!   exact bytes of `index.json`), signed with the gateway's registry key
//!   (`registry.key`, created on first start; clients pin it on first use).
//! * `GET /registry/packages/<sha256>` — package bytes.
//! * `POST /registry/publish` (bearer: gateway session or core token) —
//!   `{entry, publicKey, signature, packageBase64}`; the publisher signature
//!   over the raw sha256 digest is verified before the package is stored.
//! * `POST /registry/revoke` (bearer) — `{id, version?}` revokes entries,
//!   `{keyId}` revokes a publisher key.
//!
//! The gateway does not decide trust: clients pin publisher keys per package
//! and refuse key changes (ADR-0040 §3).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path as UrlPath, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Resolves a bearer token to an account (session or core token).
pub type AuthFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Largest accepted package.
pub const MAX_PACKAGE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
struct RegistryState {
    root: PathBuf,
    key: Arc<SigningKey>,
    auth: AuthFn,
    lock: Arc<tokio::sync::Mutex<()>>,
}

/// Registry router, nested at `/registry` by [`crate::router`]. Publishing
/// and revocation need a bearer token accepted by `auth`.
///
/// # Errors
/// The registry directory or key cannot be created.
pub fn router(root: &Path, auth: AuthFn) -> anyhow::Result<Router> {
    std::fs::create_dir_all(root.join("packages"))?;
    let key = Arc::new(load_or_create_key(&root.join("registry.key"))?);
    if !root.join("index.json").exists() {
        let doc = json!({ "format": 1, "name": "orkestr-gateway", "keys": [], "entries": [], "revokedKeys": [] });
        write_index(root, &doc, &key)?;
    }
    let st = RegistryState { root: root.to_path_buf(), key, auth, lock: Arc::new(tokio::sync::Mutex::new(())) };
    Ok(Router::new()
        .route("/index.json", get(index_json))
        .route("/index.sig", get(index_sig))
        .route("/packages/{sha}", get(package))
        .route("/publish", post(publish))
        .route("/revoke", post(revoke))
        .layer(DefaultBodyLimit::max(MAX_PACKAGE_BYTES * 4 / 3 + 64 * 1024))
        .with_state(st))
}

fn load_or_create_key(path: &Path) -> anyhow::Result<SigningKey> {
    if let Ok(text) = std::fs::read_to_string(path) {
        let raw: [u8; 32] = hex::decode(text.trim())?
            .try_into()
            .map_err(|_| anyhow::anyhow!("{}: registry key must be 32 bytes", path.display()))?;
        return Ok(SigningKey::from_bytes(&raw));
    }
    let key = SigningKey::generate(&mut rand_core06::OsRng);
    std::fs::write(path, hex::encode(key.to_bytes()))?;
    Ok(key)
}

fn key_id(public_key: &[u8]) -> String {
    hex::encode(Sha256::digest(public_key))[..16].to_owned()
}

fn write_index(root: &Path, doc: &Value, key: &SigningKey) -> anyhow::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(doc)?;
    bytes.push(b'\n');
    let public = key.verifying_key().to_bytes();
    let sig = json!({
        "keyId": key_id(&public),
        "publicKey": B64.encode(public),
        "signature": B64.encode(key.sign(&bytes).to_bytes()),
    });
    for (name, data) in [("index.json", bytes), ("index.sig", serde_json::to_vec_pretty(&sig)?)] {
        let tmp = root.join(format!("{name}.tmp"));
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, root.join(name))?;
    }
    Ok(())
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn file(path: PathBuf, content_type: &'static str) -> Response {
    match std::fs::read(path) {
        Ok(b) => ([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "no-cache")], b).into_response(),
        Err(_) => err(StatusCode::NOT_FOUND, "not found"),
    }
}

async fn index_json(State(st): State<RegistryState>) -> Response {
    file(st.root.join("index.json"), "application/json")
}

async fn index_sig(State(st): State<RegistryState>) -> Response {
    file(st.root.join("index.sig"), "application/json")
}

async fn package(State(st): State<RegistryState>, UrlPath(sha): UrlPath<String>) -> Response {
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return err(StatusCode::BAD_REQUEST, "bad package digest");
    }
    file(st.root.join("packages").join(sha.to_ascii_lowercase()), "application/octet-stream")
}

fn authorized(st: &RegistryState, headers: &HeaderMap) -> Option<String> {
    let token = headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ")?.trim();
    (st.auth)(token)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishBody {
    entry: Value,
    public_key: String,
    signature: String,
    package_base64: String,
}

fn read_index(root: &Path) -> anyhow::Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(root.join("index.json"))?)?)
}

fn verify_publish(b: &PublishBody) -> Result<(Vec<u8>, String, String, Vec<u8>), String> {
    let pk: [u8; 32] = B64
        .decode(b.public_key.trim())
        .ok()
        .and_then(|k| k.try_into().ok())
        .ok_or("publicKey must be a base64 32-byte ed25519 key")?;
    let vk = VerifyingKey::from_bytes(&pk).map_err(|e| format!("invalid public key: {e}"))?;
    let sig: [u8; 64] = B64
        .decode(b.signature.trim())
        .ok()
        .and_then(|s| s.try_into().ok())
        .ok_or("signature must be a base64 64-byte ed25519 signature")?;
    let bytes = B64.decode(b.package_base64.trim()).map_err(|e| format!("packageBase64: {e}"))?;
    if bytes.is_empty() || bytes.len() > MAX_PACKAGE_BYTES {
        return Err("package is empty or too large".into());
    }
    let digest = Sha256::digest(&bytes).to_vec();
    vk.verify_strict(&digest, &Signature::from_bytes(&sig)).map_err(|_| "publisher signature does not verify")?;
    Ok((bytes, hex::encode(&digest), key_id(&pk), pk.to_vec()))
}

fn valid_meta(e: &Value) -> Result<(), String> {
    for f in ["id", "name", "version", "publisher"] {
        let ok = e[f].as_str().is_some_and(|s| !s.trim().is_empty() && s.len() <= 200);
        if !ok {
            return Err(format!("entry.{f} is required"));
        }
    }
    if !matches!(e["kind"].as_str(), Some("Plugin" | "Widget" | "AgentTemplate")) {
        return Err("entry.kind must be Plugin, Widget or AgentTemplate".into());
    }
    Ok(())
}

async fn publish(State(st): State<RegistryState>, headers: HeaderMap, Json(body): Json<PublishBody>) -> Response {
    let Some(account) = authorized(&st, &headers) else {
        return err(StatusCode::UNAUTHORIZED, "a gateway session or core token is required");
    };
    if let Err(m) = valid_meta(&body.entry) {
        return err(StatusCode::BAD_REQUEST, m);
    }
    let (bytes, sha, kid, pk) = match verify_publish(&body) {
        Ok(v) => v,
        Err(m) => return err(StatusCode::BAD_REQUEST, m),
    };
    let _g = st.lock.lock().await;
    let result = (|| -> anyhow::Result<Result<Value, Response>> {
        let mut doc = read_index(&st.root)?;
        let (id, version) =
            (body.entry["id"].as_str().unwrap_or_default(), body.entry["version"].as_str().unwrap_or_default());
        if doc["revokedKeys"].as_array().is_some_and(|r| r.iter().any(|k| k == kid.as_str())) {
            return Ok(Err(err(StatusCode::FORBIDDEN, "publisher key is revoked")));
        }
        let entries = doc["entries"].as_array().cloned().unwrap_or_default();
        if entries.iter().any(|e| e["id"] == id && e["version"] == version) {
            return Ok(Err(err(StatusCode::CONFLICT, format!("{id}@{version} is already published"))));
        }
        std::fs::write(st.root.join("packages").join(&sha), &bytes)?;
        let keys = doc["keys"].as_array_mut().ok_or_else(|| anyhow::anyhow!("index has no keys"))?;
        if !keys.iter().any(|k| k["keyId"] == kid.as_str()) {
            keys.push(json!({ "keyId": kid, "publisher": body.entry["publisher"], "publicKey": B64.encode(&pk) }));
        }
        let mut entry = json!({
            "id": id,
            "kind": body.entry["kind"],
            "name": body.entry["name"],
            "version": version,
            "publisher": body.entry["publisher"],
            "description": body.entry["description"].as_str().unwrap_or_default(),
            "tags": body.entry.get("tags").cloned().unwrap_or_else(|| json!([])),
            "permissions": body.entry.get("permissions").cloned().unwrap_or_else(|| json!([])),
            "keyId": kid,
            "sha256": sha,
            "signature": body.signature.trim(),
            "url": format!("packages/{sha}"),
            "revoked": false,
        });
        if !entry["tags"].is_array() {
            entry["tags"] = json!([]);
        }
        doc["entries"].as_array_mut().ok_or_else(|| anyhow::anyhow!("index has no entries"))?.push(entry.clone());
        write_index(&st.root, &doc, &st.key)?;
        Ok(Ok(entry))
    })();
    match result {
        Ok(Ok(entry)) => {
            tracing::info!(%account, id = %entry["id"], version = %entry["version"], "registry: package published");
            Json(json!({ "entry": entry })).into_response()
        }
        Ok(Err(resp)) => resp,
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevokeBody {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    key_id: Option<String>,
}

async fn revoke(State(st): State<RegistryState>, headers: HeaderMap, Json(body): Json<RevokeBody>) -> Response {
    let Some(account) = authorized(&st, &headers) else {
        return err(StatusCode::UNAUTHORIZED, "a gateway session or core token is required");
    };
    let _g = st.lock.lock().await;
    let result = (|| -> anyhow::Result<usize> {
        let mut doc = read_index(&st.root)?;
        let mut changed = 0;
        if let Some(k) = &body.key_id {
            let list = doc["revokedKeys"].as_array_mut().ok_or_else(|| anyhow::anyhow!("index has no revokedKeys"))?;
            if !list.iter().any(|x| x == k.as_str()) {
                list.push(json!(k));
                changed += 1;
            }
        }
        if let Some(id) = &body.id {
            for e in doc["entries"].as_array_mut().into_iter().flatten() {
                let hit = e["id"] == id.as_str() && body.version.as_deref().is_none_or(|v| e["version"] == v);
                if hit && e["revoked"] != true {
                    e["revoked"] = json!(true);
                    changed += 1;
                }
            }
        }
        if changed > 0 {
            write_index(&st.root, &doc, &st.key)?;
        }
        Ok(changed)
    })();
    match result {
        Ok(0) => err(StatusCode::NOT_FOUND, "nothing to revoke"),
        Ok(n) => {
            tracing::info!(%account, ?body, "registry: revoked");
            Json(json!({ "revoked": n })).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}
