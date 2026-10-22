//! # orkestr-gateway
//!
//! Self-hosted gateway / control plane (Phase 4, ADR-0030): accounts
//! (passkey + TOTP), device registry mirror, core enrollment, the E2E tunnel
//! relay, the sync store (`/v1/sync`, from `orkestr-sync`) and Web Push
//! delivery ([`push`]).
//!
//! Step 0 skeleton: configuration parsing, the loopback-only bind rule and
//! `GET /healthz`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod push;

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::routing::get;
use axum::{Json, Router};

/// Default bind address.
pub const DEFAULT_BIND: &str = "127.0.0.1:8787";

/// Gateway version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Gateway configuration (from the command line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayConfig {
    /// Data directory (SQLite + object store).
    pub data_dir: PathBuf,
    /// Listen address.
    pub bind: SocketAddr,
    /// Static web client to serve (`apps/web/dist`).
    pub web_dist: Option<PathBuf>,
    /// TLS is terminated by a reverse proxy in front of the gateway; only
    /// then may it bind a non-loopback address.
    pub behind_tls_proxy: bool,
}

/// Configuration errors.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    /// Bad command line.
    #[error("{0}")]
    Usage(String),
    /// Non-loopback bind without `--behind-tls-proxy`.
    #[error(
        "refusing to bind non-loopback address {0} without --behind-tls-proxy (the gateway does not terminate TLS)"
    )]
    NonLoopback(SocketAddr),
}

/// Usage text.
pub const USAGE: &str =
    "usage: orkestr-gateway --data-dir <DIR> [--bind <ADDR:PORT>] [--web-dist <DIR>] [--behind-tls-proxy]";

impl GatewayConfig {
    /// Parses command-line arguments (without the program name).
    ///
    /// # Errors
    /// [`ConfigError::Usage`] on bad arguments, [`ConfigError::NonLoopback`]
    /// for a non-loopback bind without `--behind-tls-proxy`.
    pub fn from_args<I: IntoIterator<Item = String>>(args: I) -> Result<Self, ConfigError> {
        let mut data_dir = None;
        let mut bind = DEFAULT_BIND.to_owned();
        let mut web_dist = None;
        let mut behind_tls_proxy = false;
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            let mut value =
                |name: &str| it.next().ok_or_else(|| ConfigError::Usage(format!("{name} needs a value\n{USAGE}")));
            match arg.as_str() {
                "--data-dir" => data_dir = Some(PathBuf::from(value("--data-dir")?)),
                "--bind" => bind = value("--bind")?,
                "--web-dist" => web_dist = Some(PathBuf::from(value("--web-dist")?)),
                "--behind-tls-proxy" => behind_tls_proxy = true,
                other => return Err(ConfigError::Usage(format!("unknown argument `{other}`\n{USAGE}"))),
            }
        }
        let data_dir = data_dir.ok_or_else(|| ConfigError::Usage(format!("--data-dir is required\n{USAGE}")))?;
        let bind: SocketAddr =
            bind.parse().map_err(|e| ConfigError::Usage(format!("invalid --bind `{bind}`: {e}\n{USAGE}")))?;
        if !bind.ip().is_loopback() && !behind_tls_proxy {
            return Err(ConfigError::NonLoopback(bind));
        }
        Ok(Self { data_dir, bind, web_dist, behind_tls_proxy })
    }
}

/// The HTTP router.
pub fn router(_cfg: &GatewayConfig) -> Router {
    Router::new().route("/healthz", get(healthz))
}

async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "version": VERSION }))
}

/// Runs the gateway until the process is stopped.
///
/// # Errors
/// Fails when the data dir cannot be created or the address cannot be bound.
pub async fn run(cfg: GatewayConfig) -> anyhow::Result<()> {
    std::fs::create_dir_all(&cfg.data_dir)?;
    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    tracing::info!(addr = %listener.local_addr()?, "orkestr-gateway listening");
    axum::serve(listener, router(&cfg)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn defaults_to_loopback() {
        let cfg = GatewayConfig::from_args(args(&["--data-dir", "d"])).unwrap();
        assert_eq!(cfg.bind, DEFAULT_BIND.parse().unwrap());
        assert!(!cfg.behind_tls_proxy);
    }

    #[test]
    fn refuses_non_loopback_without_proxy_flag() {
        let err = GatewayConfig::from_args(args(&["--data-dir", "d", "--bind", "0.0.0.0:8787"])).unwrap_err();
        assert!(matches!(err, ConfigError::NonLoopback(_)));
        let ok = GatewayConfig::from_args(args(&["--data-dir", "d", "--bind", "0.0.0.0:8787", "--behind-tls-proxy"]));
        assert!(ok.is_ok());
    }

    #[test]
    fn requires_data_dir() {
        assert!(matches!(GatewayConfig::from_args(args(&[])), Err(ConfigError::Usage(_))));
    }

    #[tokio::test]
    async fn healthz_answers() {
        let cfg = GatewayConfig::from_args(args(&["--data-dir", "d", "--bind", "127.0.0.1:0"])).unwrap();
        let listener = tokio::net::TcpListener::bind(cfg.bind).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router(&cfg)).await });
        let body: serde_json::Value =
            reqwest::get(format!("http://{addr}/healthz")).await.unwrap().json().await.unwrap();
        assert_eq!(body["status"], "ok");
    }
}
