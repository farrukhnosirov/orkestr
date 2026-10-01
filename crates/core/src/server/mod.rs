//! HTTP + WebSocket server (contract §4).
//!
//! * `GET /health` — no auth → `{"status":"ok","version":"…"}`.
//! * `GET /rpc` — WebSocket. Auth via `Authorization: Bearer <token>` header
//!   (a wrong header token is rejected with 401 before the upgrade) or via the
//!   first frame `{"type":"auth","token":"…"}` within 10 s.
//!
//! Defense in depth: the server binds loopback only, and requests whose
//! `Host` header is not a loopback name are rejected (DNS-rebinding guard).

mod connection;

use std::sync::Arc;

use axum::extract::{State, WebSocketUpgrade};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use tokio_util::sync::CancellationToken;

use crate::auth::{bearer_token, token_matches};
use crate::rpc::RpcRouter;
use crate::services::Services;

/// Max size of one inbound WebSocket message.
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// Shared server state.
#[derive(Clone)]
pub struct AppState {
    pub(crate) services: Arc<Services>,
    pub(crate) token: Arc<str>,
    pub(crate) router: Arc<RpcRouter>,
    pub(crate) shutdown: CancellationToken,
}

impl AppState {
    /// Creates the state.
    #[must_use]
    pub fn new(services: Arc<Services>, token: String, router: Arc<RpcRouter>, shutdown: CancellationToken) -> Self {
        Self { services, token: token.into(), router, shutdown }
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").field("token", &"<redacted>").finish_non_exhaustive()
    }
}

/// Builds the axum application.
pub fn app(state: AppState) -> Router {
    Router::new().route("/health", get(health)).route("/rpc", get(rpc)).with_state(state)
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok", "version": state.services.version }))
}

/// Whether the `Host` header names a loopback host.
fn host_is_loopback(headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        host.split(':').next().unwrap_or_default()
    };
    matches!(name.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1")
}

async fn rpc(State(state): State<AppState>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    if !host_is_loopback(&headers) {
        return (StatusCode::FORBIDDEN, "forbidden host").into_response();
    }
    let header_authed = match headers.get(header::AUTHORIZATION) {
        None => false,
        Some(value) => {
            let ok = value.to_str().ok().and_then(bearer_token).is_some_and(|t| token_matches(&state.token, t));
            if !ok {
                tracing::warn!("rejected WebSocket upgrade with invalid bearer token");
                return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
            }
            true
        }
    };
    ws.max_message_size(MAX_MESSAGE_BYTES).on_upgrade(move |socket| connection::run(socket, state, header_authed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn host_guard() {
        let mut h = HeaderMap::new();
        assert!(!host_is_loopback(&h));
        for (host, ok) in [
            ("127.0.0.1:5000", true),
            ("localhost:1", true),
            ("[::1]:80", true),
            ("evil.example:5000", false),
            ("127.0.0.1.evil.example", false),
        ] {
            h.insert(header::HOST, HeaderValue::from_static(host));
            assert_eq!(host_is_loopback(&h), ok, "{host}");
        }
    }
}
