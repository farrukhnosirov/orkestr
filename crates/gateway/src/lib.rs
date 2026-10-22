//! # orkestr-gateway
//!
//! Self-hosted gateway / control plane (Phase 4, ADR-0030): accounts
//! (passkey + TOTP), device registry mirror, core enrollment, the E2E tunnel
//! relay, the sync store (`/v1/sync`, from `orkestr-sync`) and Web Push
//! delivery ([`push`]).
//!
//! Routes:
//! * `GET /healthz`
//! * `/v1/auth/*`, `/v1/cores*`, `/v1/audit` — see [`auth`]
//! * `GET /v1/tunnel` — core tunnel (WS; CIK challenge), see [`relay`]
//! * `GET /v1/connect?coreId=…` — client WS relayed opaquely to the core
//! * `POST /v1/pairings/{id}/claim`, `GET /v1/pairings/{id}` — pairing relay
//! * `/v1/sync/*` — `orkestr_sync::server_router` (bearer: session or core token)
//! * everything else — static files from `--web-dist` (SPA fallback to `index.html`)
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod auth;
pub mod limits;
pub mod push;
pub mod push_native;
pub mod relay;
pub mod store;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use orkestr_security::webauthn::RelyingParty;
use parking_lot::Mutex;
use rand_core06::RngCore;

/// Default bind address (loopback only).
pub const DEFAULT_BIND: &str = "127.0.0.1:8787";

/// Gateway version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayConfig {
    /// Data directory (SQLite database, sync store).
    pub data_dir: PathBuf,
    /// Listen address.
    pub bind: SocketAddr,
    /// Built web client to serve (`apps/web/dist`).
    pub web_dist: Option<PathBuf>,
    /// TLS is terminated by a reverse proxy (allows non-loopback binds).
    pub behind_tls_proxy: bool,
    /// WebAuthn RP id (default `localhost`).
    pub rp_id: String,
    /// Allowed WebAuthn origins (default `http://localhost:<port>`).
    pub origins: Vec<String>,
    /// FCM service-account JSON (enables `fcm` native push).
    pub fcm_service_account: Option<PathBuf>,
    /// FCM API base.
    pub fcm_endpoint: String,
    /// APNs token key + ids (enables `apns` native push).
    pub apns: Option<push_native::ApnsConfig>,
}

/// Command-line error.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    /// Bad usage.
    #[error("{0}")]
    Usage(String),
    /// Non-loopback bind without a TLS proxy.
    #[error(
        "refusing to bind non-loopback address {0} without --behind-tls-proxy (the gateway does not terminate TLS)"
    )]
    NonLoopback(SocketAddr),
    /// `--help` / `-h`: print [`HELP`] and exit successfully.
    #[error("{HELP}")]
    Help,
}

/// Usage text.
pub const USAGE: &str = "usage: orkestr-gateway --data-dir <DIR> [--bind <ADDR:PORT>] [--web-dist <DIR>] [--behind-tls-proxy] [--rp-id <ID>] [--origin <URL>]... [--fcm-service-account <FILE>] [--fcm-endpoint <URL>] [--apns-key <FILE> --apns-key-id <ID> --apns-team-id <ID> --apns-topic <BUNDLE>] [--apns-endpoint <URL>] (see --help)";

/// `--help` text.
pub const HELP: &str =
    "orkestr-gateway - self-hosted Orkestr gateway (accounts, device registry, E2E relay, sync, push)

usage: orkestr-gateway --data-dir <DIR> [options]

  --data-dir <DIR>               SQLite database, sync store, VAPID key (required)
  --bind <ADDR:PORT>             listen address (default 127.0.0.1:8787; non-loopback needs --behind-tls-proxy)
  --web-dist <DIR>               serve the built web client from DIR
  --behind-tls-proxy             TLS is terminated by a reverse proxy (Secure cookies, X-Forwarded-For)
  --rp-id <ID>                   WebAuthn relying-party id (default localhost)
  --origin <URL>                 allowed WebAuthn origin (repeatable; default http://localhost:<port>)

native push (ADR-0037; payloads stay end-to-end encrypted, the visible alert is generic):
  --fcm-service-account <FILE>   Firebase service-account JSON; enables FCM HTTP v1 delivery
  --fcm-endpoint <URL>           FCM API base (default https://fcm.googleapis.com; for mocks)
  --apns-key <FILE>              APNs token-signing key (.p8, PKCS#8 PEM); enables APNs delivery
  --apns-key-id <ID>             key id (JWT kid)                       [required with --apns-key]
  --apns-team-id <ID>            Apple team id (JWT iss)                [required with --apns-key]
  --apns-topic <BUNDLE>          app bundle id (apns-topic)             [required with --apns-key]
  --apns-endpoint <URL>          APNs API base (default https://api.push.apple.com;
                                 https://api.sandbox.push.apple.com for development builds;
                                 http:// = HTTP/2 prior knowledge, for mocks)
  -h, --help                     print this help";

impl GatewayConfig {
    /// Parses arguments (without the program name).
    ///
    /// # Errors
    /// Usage errors; non-loopback bind without `--behind-tls-proxy`.
    pub fn from_args<I: IntoIterator<Item = String>>(args: I) -> Result<Self, ConfigError> {
        let mut data_dir = None;
        let mut bind = DEFAULT_BIND.to_owned();
        let mut web_dist = None;
        let mut behind_tls_proxy = false;
        let mut rp_id = "localhost".to_owned();
        let mut origins = Vec::new();
        let mut fcm_service_account = None;
        let mut fcm_endpoint = push_native::DEFAULT_FCM_ENDPOINT.to_owned();
        let (mut apns_key, mut apns_key_id, mut apns_team_id, mut apns_topic) = (None, None, None, None);
        let mut apns_endpoint = push_native::DEFAULT_APNS_ENDPOINT.to_owned();
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            let mut value =
                |name: &str| it.next().ok_or_else(|| ConfigError::Usage(format!("{name} needs a value\n{USAGE}")));
            match arg.as_str() {
                "--data-dir" => data_dir = Some(PathBuf::from(value("--data-dir")?)),
                "--bind" => bind = value("--bind")?,
                "--web-dist" => web_dist = Some(PathBuf::from(value("--web-dist")?)),
                "--behind-tls-proxy" => behind_tls_proxy = true,
                "--rp-id" => rp_id = value("--rp-id")?,
                "--origin" => origins.push(value("--origin")?.trim_end_matches('/').to_owned()),
                "--fcm-service-account" => fcm_service_account = Some(PathBuf::from(value("--fcm-service-account")?)),
                "--fcm-endpoint" => fcm_endpoint = value("--fcm-endpoint")?,
                "--apns-key" => apns_key = Some(PathBuf::from(value("--apns-key")?)),
                "--apns-key-id" => apns_key_id = Some(value("--apns-key-id")?),
                "--apns-team-id" => apns_team_id = Some(value("--apns-team-id")?),
                "--apns-topic" => apns_topic = Some(value("--apns-topic")?),
                "--apns-endpoint" => apns_endpoint = value("--apns-endpoint")?,
                "-h" | "--help" => return Err(ConfigError::Help),
                other => return Err(ConfigError::Usage(format!("unknown argument `{other}`\n{USAGE}"))),
            }
        }
        let data_dir = data_dir.ok_or_else(|| ConfigError::Usage(format!("--data-dir is required\n{USAGE}")))?;
        let bind: SocketAddr =
            bind.parse().map_err(|e| ConfigError::Usage(format!("invalid --bind `{bind}`: {e}\n{USAGE}")))?;
        if !bind.ip().is_loopback() && !behind_tls_proxy {
            return Err(ConfigError::NonLoopback(bind));
        }
        let apns = match (apns_key, apns_key_id, apns_team_id, apns_topic) {
            (None, None, None, None) => None,
            (Some(key_path), Some(key_id), Some(team_id), Some(topic)) => {
                Some(push_native::ApnsConfig { key_path, key_id, team_id, topic, endpoint: apns_endpoint })
            }
            _ => {
                return Err(ConfigError::Usage(format!(
                    "--apns-key, --apns-key-id, --apns-team-id and --apns-topic must be given together\n{USAGE}"
                )))
            }
        };
        Ok(Self { data_dir, bind, web_dist, behind_tls_proxy, rp_id, origins, fcm_service_account, fcm_endpoint, apns })
    }
}

/// Random bytes.
#[must_use]
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    rand_core06::OsRng.fill_bytes(&mut b);
    b
}

/// Random 32-byte bearer token (base64url).
#[must_use]
pub fn random_token() -> String {
    orkestr_security::webauthn::b64(&random_bytes(32))
}

/// JSON API error `{error}`.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
    retry_after: Option<Duration>,
}

impl ApiError {
    /// New error.
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self { status, message: message.into(), retry_after: None }
    }
    /// 400.
    pub fn bad(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
    /// 400 from an error value.
    pub fn bad_e(e: impl std::fmt::Display) -> Self {
        Self::bad(e.to_string())
    }
    /// 500.
    pub fn internal(e: impl std::fmt::Display) -> Self {
        tracing::error!(error = %e, "gateway internal error");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
    }
    /// 429 with `retryAfterMs`.
    #[must_use]
    pub fn too_many(after: Duration) -> Self {
        Self { status: StatusCode::TOO_MANY_REQUESTS, message: "rate limited".into(), retry_after: Some(after) }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = serde_json::json!({ "error": self.message });
        if let Some(d) = self.retry_after {
            body["retryAfterMs"] = serde_json::json!(u64::try_from(d.as_millis()).unwrap_or(u64::MAX).max(1));
        }
        (self.status, Json(body)).into_response()
    }
}

/// Client IP (`X-Forwarded-For` only behind a TLS proxy).
#[derive(Debug, Clone)]
pub struct ClientIp(pub String);

impl FromRequestParts<GatewayState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, st: &GatewayState) -> Result<Self, Self::Rejection> {
        if st.behind_tls_proxy {
            if let Some(ip) =
                parts.headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|v| v.split(',').next())
            {
                return Ok(Self(ip.trim().to_owned()));
            }
        }
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map_or_else(|| "unknown".to_owned(), |c| c.0.ip().to_string());
        Ok(Self(ip))
    }
}

/// Shared gateway state.
#[derive(Clone)]
pub struct GatewayState(Arc<GatewayInner>);

/// Contents of [`GatewayState`].
pub struct GatewayInner {
    /// Persistence.
    pub store: Arc<store::Store>,
    /// WebAuthn relying party.
    pub rp: RelyingParty,
    /// Set `Secure` on cookies / trust `X-Forwarded-For`.
    pub behind_tls_proxy: bool,
    /// `Secure` cookie attribute.
    pub secure_cookie: bool,
    /// Pending WebAuthn ceremonies.
    pub ceremonies: Mutex<HashMap<String, auth::Ceremony>>,
    /// Rate-limit windows.
    pub windows: limits::Windows,
    /// Login lockout.
    pub lockout: limits::Lockout,
    /// Tunnels.
    pub relay: relay::Relay,
    /// Native push senders (FCM / APNs).
    pub native: push_native::NativeSenders,
}

impl std::ops::Deref for GatewayState {
    type Target = GatewayInner;
    fn deref(&self) -> &GatewayInner {
        &self.0
    }
}

impl std::fmt::Debug for GatewayState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayState").field("rp", &self.rp).finish_non_exhaustive()
    }
}

impl GatewayState {
    /// Opens the store under `cfg.data_dir`; `port` resolves the default origin.
    ///
    /// # Errors
    /// Database failure.
    pub fn open(cfg: &GatewayConfig, port: u16, relay: relay::Relay) -> anyhow::Result<Self> {
        let store = Arc::new(store::Store::open(&cfg.data_dir)?);
        let native = native_senders(cfg)?;
        let origins =
            if cfg.origins.is_empty() { vec![format!("http://localhost:{port}")] } else { cfg.origins.clone() };
        Ok(Self(Arc::new(GatewayInner {
            store,
            rp: RelyingParty { rp_id: cfg.rp_id.clone(), origins },
            behind_tls_proxy: cfg.behind_tls_proxy,
            secure_cookie: cfg.behind_tls_proxy,
            ceremonies: Mutex::new(HashMap::new()),
            windows: limits::Windows::default(),
            lockout: limits::Lockout::default(),
            relay,
            native,
        })))
    }
}

/// Loads the configured native push senders (fails fast on bad key files).
///
/// # Errors
/// Unreadable / invalid service account or APNs key.
pub fn native_senders(cfg: &GatewayConfig) -> anyhow::Result<push_native::NativeSenders> {
    let fcm = cfg
        .fcm_service_account
        .as_deref()
        .map(|p| push_native::FcmSender::from_file(p, &cfg.fcm_endpoint).map(Arc::new))
        .transpose()?;
    let apns = cfg.apns.as_ref().map(|c| push_native::ApnsSender::from_config(c).map(Arc::new)).transpose()?;
    let senders = push_native::NativeSenders { fcm, apns };
    if !senders.transports().is_empty() {
        tracing::info!(transports = ?senders.transports(), "native push enabled");
    }
    Ok(senders)
}

/// VAPID `sub` claim (RFC 8292): contact for push services.
const VAPID_SUBJECT: &str = "mailto:admin@orkestr.localhost";

/// Builds the application.
///
/// # Errors
/// The sync store cannot be opened.
pub fn router(st: GatewayState, cfg: &GatewayConfig) -> anyhow::Result<Router> {
    let api = Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/auth/state", get(auth::state))
        .route("/v1/auth/register/begin", post(auth::register_begin))
        .route("/v1/auth/register/finish", post(auth::register_finish))
        .route("/v1/auth/login/begin", post(auth::login_begin))
        .route("/v1/auth/login/finish", post(auth::login_finish))
        .route("/v1/auth/totp/setup", post(auth::totp_setup))
        .route("/v1/auth/totp/confirm", post(auth::totp_confirm))
        .route("/v1/auth/totp/login", post(auth::totp_login))
        .route("/v1/auth/logout", post(auth::logout))
        .route("/v1/auth/panic", post(auth::panic))
        .route("/v1/cores", get(auth::cores))
        .route("/v1/cores/enrollment-token", post(auth::enrollment_token))
        .route("/v1/audit", get(auth::audit))
        .route("/v1/tunnel", get(relay::tunnel))
        .route("/v1/connect", get(relay::connect))
        .route("/v1/pairings/{pairing_id}/claim", post(relay::claim))
        .route("/v1/pairings/{pairing_id}", get(relay::pairing_status))
        .route("/v1/push/vapid", get(push::vapid_handler))
        .with_state(st.clone());
    let store = Arc::clone(&st.store);
    let sync_auth: orkestr_sync::server::AuthFn =
        Arc::new(move |token: &str| store.session(token).or_else(|| store.core_token_account(token)));
    let sync = orkestr_sync::server::SyncServerState::open(&cfg.data_dir.join("sync"), sync_auth)?;
    let mut app = api.nest("/v1/sync", orkestr_sync::server::server_router(sync));
    if let Some(dist) = &cfg.web_dist {
        let index = dist.join("index.html");
        app = app.fallback_service(
            tower_http::services::ServeDir::new(dist).not_found_service(tower_http::services::ServeFile::new(index)),
        );
    }
    Ok(app)
}

async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "version": VERSION }))
}

/// A running gateway (tests / embedding).
#[derive(Debug)]
pub struct Running {
    /// Bound address.
    pub addr: SocketAddr,
    /// State.
    pub state: GatewayState,
    /// Server task.
    pub task: tokio::task::JoinHandle<()>,
}

/// Binds and serves in the background.
///
/// # Errors
/// Bind or store failure.
pub async fn spawn(cfg: GatewayConfig, relay: relay::Relay) -> anyhow::Result<Running> {
    std::fs::create_dir_all(&cfg.data_dir)?;
    // Web Push sender (ADR-0036): VAPID key under the data dir, advertised to
    // cores in the tunnel `welcome` and to browsers at GET /v1/push/vapid.
    push::init(&cfg.data_dir, VAPID_SUBJECT).map_err(|e| anyhow::anyhow!("web push: {e}"))?;
    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    let addr = listener.local_addr()?;
    let state = GatewayState::open(&cfg, addr.port(), relay)?;
    let app = router(state.clone(), &cfg)?;
    tracing::info!(%addr, rp = %state.rp.rp_id, "orkestr-gateway listening");
    let task = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await {
            tracing::error!(error = %e, "gateway server failed");
        }
    });
    Ok(Running { addr, state, task })
}

/// Runs the gateway until the server stops.
///
/// # Errors
/// Bind or store failure.
pub async fn run(cfg: GatewayConfig) -> anyhow::Result<()> {
    let running = spawn(cfg, relay::Relay::default()).await?;
    running.task.await?;
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
    fn native_push_flags() {
        assert_eq!(GatewayConfig::from_args(args(&["--help"])), Err(ConfigError::Help));
        for flag in ["--fcm-service-account", "--apns-key", "--apns-endpoint", "--fcm-endpoint"] {
            assert!(HELP.contains(flag), "{flag} documented");
        }
        let cfg = GatewayConfig::from_args(args(&["--data-dir", "d"])).unwrap();
        assert_eq!(
            (cfg.fcm_service_account, cfg.apns, cfg.fcm_endpoint.as_str()),
            (None, None, push_native::DEFAULT_FCM_ENDPOINT)
        );
        let cfg = GatewayConfig::from_args(args(&[
            "--data-dir",
            "d",
            "--fcm-service-account",
            "sa.json",
            "--fcm-endpoint",
            "http://127.0.0.1:9",
            "--apns-key",
            "k.p8",
            "--apns-key-id",
            "K",
            "--apns-team-id",
            "T",
            "--apns-topic",
            "dev.orkestr.mobile",
            "--apns-endpoint",
            "http://127.0.0.1:10",
        ]))
        .unwrap();
        assert_eq!(cfg.fcm_service_account.as_deref(), Some(std::path::Path::new("sa.json")));
        assert_eq!(cfg.fcm_endpoint, "http://127.0.0.1:9");
        let apns = cfg.apns.unwrap();
        assert_eq!(
            (apns.key_id.as_str(), apns.team_id.as_str(), apns.endpoint.as_str()),
            ("K", "T", "http://127.0.0.1:10")
        );
        let partial = GatewayConfig::from_args(args(&["--data-dir", "d", "--apns-key", "k.p8"]));
        assert!(matches!(partial, Err(ConfigError::Usage(_))));
    }

    #[test]
    fn requires_data_dir() {
        assert!(matches!(GatewayConfig::from_args(args(&[])), Err(ConfigError::Usage(_))));
    }

    #[tokio::test]
    async fn healthz_answers() {
        let cfg = GatewayConfig::from_args(args(&["--data-dir", "d", "--bind", "127.0.0.1:0"])).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cfg = GatewayConfig { data_dir: dir.path().to_path_buf(), ..cfg };
        let addr = spawn(cfg, relay::Relay::default()).await.unwrap().addr;
        let body: serde_json::Value =
            reqwest::get(format!("http://{addr}/healthz")).await.unwrap().json().await.unwrap();
        assert_eq!(body["status"], "ok");
    }
}
