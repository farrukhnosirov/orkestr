//! Self-hosted marketplace registry at `/registry/*` (Phase 7, ADR-0040).
//! **Owner: WP2.**
//!
//! Serves a static signed registry (`index.json`, `index.sig`, packages)
//! from `<data_dir>/registry` and accepts authenticated publishes. Step-0
//! skeleton: every route answers `501 Not Implemented` with the explicit
//! `unimplementedFeature` marker.

use std::path::Path;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, Router};

/// Registry router, nested at `/registry` by [`crate::router`].
pub fn router(_root: &Path) -> Router {
    Router::new().fallback(not_implemented)
}

async fn not_implemented() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "`registry` is not implemented yet (unimplemented-feature)",
            "unimplementedFeature": "registry",
        })),
    )
}
