//! HTTP + WebSocket server (contract §4).
//!
//! * `GET /health` — no auth → `{"status":"ok","version":"…"}`.
//! * `GET /rpc` — WebSocket. Auth via `Authorization: Bearer <token>` header
//!   (a wrong header token is rejected with 401 before the upgrade) or via the
//!   first frame `{"type":"auth","token":"…"}` within 10 s.
//!
//! Defense in depth (Phase 1 hardening, threat model T-RPC):
//! * the server binds loopback only;
//! * `Host` must be a loopback name **with the bound port** (DNS rebinding);
//! * `Origin`, when present, must be one of [`ALLOWED_ORIGINS`] (the Tauri
//!   webview and the Vite dev server); native clients send no `Origin`;
//! * failed authentication is rate limited per peer IP with exponential
//!   backoff ([`AuthLimiter`]) — blocked peers get HTTP 429 before upgrade;
//! * request ids must be unique among a connection's in-flight requests.

pub(crate) mod connection;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::connect_info::ConnectInfo;
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::auth::{bearer_token, token_matches};
use crate::rpc::RpcRouter;
use crate::services::Services;

/// Max size of one inbound WebSocket message.
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// Browser origins allowed to open the RPC WebSocket. Clients without an
/// `Origin` header (native/CLI/tests) are allowed; everything else is 403.
pub const ALLOWED_ORIGINS: &[&str] = &[
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
    "http://127.0.0.1:1420",
    "http://localhost:1420",
];

/// Failures allowed before backoff starts.
pub const FREE_AUTH_FAILURES: u32 = 5;
/// Longest backoff.
pub const MAX_AUTH_BACKOFF: Duration = Duration::from_secs(60);
/// Failures older than this are forgotten.
const FAILURE_WINDOW: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy)]
struct FailureEntry {
    failures: u32,
    last_failure: Instant,
    blocked_until: Option<Instant>,
}

/// Per-peer failed-authentication limiter with exponential backoff.
#[derive(Debug, Default)]
pub struct AuthLimiter {
    entries: Mutex<HashMap<IpAddr, FailureEntry>>,
}

impl AuthLimiter {
    /// `Err(retry_after)` while the peer is blocked.
    pub fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        let now = Instant::now();
        let entries = self.entries.lock();
        match entries.get(&ip).and_then(|e| e.blocked_until) {
            Some(until) if until > now => Err(until - now),
            _ => Ok(()),
        }
    }

    /// Records a failed authentication; returns the new block duration, if any.
    pub fn record_failure(&self, ip: IpAddr) -> Option<Duration> {
        let now = Instant::now();
        let mut entries = self.entries.lock();
        if entries.len() > 4096 {
            entries.retain(|_, e| now.duration_since(e.last_failure) < FAILURE_WINDOW);
        }
        let e = entries.entry(ip).or_insert(FailureEntry { failures: 0, last_failure: now, blocked_until: None });
        if now.duration_since(e.last_failure) > FAILURE_WINDOW {
            e.failures = 0;
        }
        e.failures = e.failures.saturating_add(1);
        e.last_failure = now;
        if e.failures > FREE_AUTH_FAILURES {
            let exp = (e.failures - FREE_AUTH_FAILURES - 1).min(6);
            let backoff = Duration::from_secs(1u64 << exp).min(MAX_AUTH_BACKOFF);
            e.blocked_until = Some(now + backoff);
            tracing::warn!(%ip, failures = e.failures, backoff_s = backoff.as_secs(), "rpc auth failures: peer blocked");
            return Some(backoff);
        }
        None
    }

    /// Clears the peer's failure history after a successful authentication.
    pub fn record_success(&self, ip: IpAddr) {
        self.entries.lock().remove(&ip);
    }
}

/// Shared server state.
#[derive(Clone)]
pub struct AppState {
    pub(crate) services: Arc<Services>,
    pub(crate) token: Arc<str>,
    pub(crate) router: Arc<RpcRouter>,
    pub(crate) shutdown: CancellationToken,
    pub(crate) port: Option<u16>,
    pub(crate) limiter: Arc<AuthLimiter>,
}

impl AppState {
    /// Creates the state.
    #[must_use]
    pub fn new(services: Arc<Services>, token: String, router: Arc<RpcRouter>, shutdown: CancellationToken) -> Self {
        Self { services, token: token.into(), router, shutdown, port: None, limiter: Arc::new(AuthLimiter::default()) }
    }

    /// Requires the `Host` header to carry this port.
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").field("token", &"<redacted>").field("port", &self.port).finish_non_exhaustive()
    }
}

/// Builds the axum application. Serve it with
/// `into_make_service_with_connect_info::<SocketAddr>()` so the auth limiter
/// knows the peer.
pub fn app(state: AppState) -> Router {
    Router::new().route("/health", get(health)).route("/rpc", get(rpc)).with_state(state)
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok", "version": state.services.version }))
}

/// Whether the `Host` header names a loopback host (and, when `port` is
/// given, exactly that port).
fn host_is_allowed(headers: &HeaderMap, port: Option<u16>) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let (name, host_port) = if let Some(rest) = host.strip_prefix('[') {
        let mut it = rest.splitn(2, ']');
        let name = it.next().unwrap_or_default();
        let after = it.next().unwrap_or_default();
        (name, after.strip_prefix(':'))
    } else {
        match host.rsplit_once(':') {
            Some((n, p)) => (n, Some(p)),
            None => (host, None),
        }
    };
    if !matches!(name.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1") {
        return false;
    }
    match port {
        None => true,
        Some(expected) => host_port.and_then(|p| p.parse::<u16>().ok()) == Some(expected),
    }
}

/// Whether the `Origin` header (if any) is allow-listed.
fn origin_is_allowed(headers: &HeaderMap) -> bool {
    match headers.get(header::ORIGIN) {
        None => true,
        Some(v) => v
            .to_str()
            .ok()
            .map(|o| o.trim_end_matches('/').to_ascii_lowercase())
            .is_some_and(|o| ALLOWED_ORIGINS.contains(&o.as_str())),
    }
}

async fn rpc(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !host_is_allowed(&headers, state.port) {
        tracing::warn!("rejected WebSocket upgrade: forbidden Host header");
        return (StatusCode::FORBIDDEN, "forbidden host").into_response();
    }
    if !origin_is_allowed(&headers) {
        tracing::warn!(origin = ?headers.get(header::ORIGIN), "rejected WebSocket upgrade: forbidden Origin");
        return (StatusCode::FORBIDDEN, "forbidden origin").into_response();
    }
    let ip = peer.map(|Extension(ConnectInfo(addr))| addr.ip());
    if let Some(ip) = ip {
        if let Err(retry) = state.limiter.check(ip) {
            let secs = retry.as_secs().max(1).to_string();
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, secs)],
                "too many failed authentication attempts",
            )
                .into_response();
        }
    }
    let header_authed = match headers.get(header::AUTHORIZATION) {
        None => false,
        Some(value) => {
            let ok = value.to_str().ok().and_then(bearer_token).is_some_and(|t| token_matches(&state.token, t));
            if !ok {
                tracing::warn!("rejected WebSocket upgrade with invalid bearer token");
                if let Some(ip) = ip {
                    state.limiter.record_failure(ip);
                }
                return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
            }
            if let Some(ip) = ip {
                state.limiter.record_success(ip);
            }
            true
        }
    };
    ws.max_message_size(MAX_MESSAGE_BYTES).on_upgrade(move |socket| connection::run(socket, state, header_authed, ip))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn host_guard() {
        let mut h = HeaderMap::new();
        assert!(!host_is_allowed(&h, None));
        for (host, ok) in [
            ("127.0.0.1:5000", true),
            ("localhost:1", true),
            ("[::1]:80", true),
            ("evil.example:5000", false),
            ("127.0.0.1.evil.example", false),
        ] {
            h.insert(header::HOST, HeaderValue::from_static(host));
            assert_eq!(host_is_allowed(&h, None), ok, "{host}");
        }
    }

    #[test]
    fn host_guard_checks_port() {
        let mut h = HeaderMap::new();
        for (host, ok) in [
            ("127.0.0.1:5000", true),
            ("localhost:5000", true),
            ("[::1]:5000", true),
            ("127.0.0.1:5001", false),
            ("127.0.0.1", false),
            ("localhost:abc", false),
            ("evil.example:5000", false),
        ] {
            h.insert(header::HOST, HeaderValue::from_static(host));
            assert_eq!(host_is_allowed(&h, Some(5000)), ok, "{host}");
        }
    }

    #[test]
    fn origin_allow_list() {
        let mut h = HeaderMap::new();
        assert!(origin_is_allowed(&h), "native clients send no Origin");
        for (origin, ok) in [
            ("tauri://localhost", true),
            ("http://tauri.localhost", true),
            ("https://tauri.localhost", true),
            ("http://127.0.0.1:1420", true),
            ("http://localhost:1420", true),
            ("HTTP://LOCALHOST:1420/", true),
            ("http://localhost:3000", false),
            ("https://evil.example", false),
            ("null", false),
            ("http://localhost:1420.evil.example", false),
        ] {
            h.insert(header::ORIGIN, HeaderValue::from_static(origin));
            assert_eq!(origin_is_allowed(&h), ok, "{origin}");
        }
    }

    #[test]
    fn limiter_backs_off_exponentially_and_resets() {
        let l = AuthLimiter::default();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        for _ in 0..FREE_AUTH_FAILURES {
            assert!(l.record_failure(ip).is_none());
            assert!(l.check(ip).is_ok());
        }
        assert_eq!(l.record_failure(ip), Some(Duration::from_secs(1)));
        assert!(l.check(ip).is_err());
        assert_eq!(l.record_failure(ip), Some(Duration::from_secs(2)));
        assert_eq!(l.record_failure(ip), Some(Duration::from_secs(4)));
        for _ in 0..10 {
            l.record_failure(ip);
        }
        assert_eq!(l.record_failure(ip), Some(MAX_AUTH_BACKOFF));
        l.record_success(ip);
        assert!(l.check(ip).is_ok());
        let other: IpAddr = "::1".parse().unwrap();
        assert!(l.check(other).is_ok());
    }
}
