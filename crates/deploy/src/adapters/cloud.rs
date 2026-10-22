//! Shared REST plumbing of the cloud adapters (Vercel, Netlify, Fly,
//! Kubernetes): base URL resolution (`ORKESTR_<ADAPTER>_API`, alias
//! `ORKESTR_DEPLOY_<ADAPTER>_BASE`), bearer token from the vault, redacted
//! errors and previews, status polling.

use std::time::Duration;

use orkestr_protocol::methods::deploy::RequestPreview;
use reqwest::Method;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::common::{number, setting};
use crate::{DeployError, DeployRequest, LogSink};

/// Base URL: explicit override (tests), env vars, then the provider default.
#[must_use]
pub fn base_url(adapter_upper: &str, explicit: Option<&str>, default: Option<&str>) -> Option<String> {
    explicit
        .map(str::to_owned)
        .or_else(|| std::env::var(format!("ORKESTR_{adapter_upper}_API")).ok())
        .or_else(|| std::env::var(format!("ORKESTR_DEPLOY_{adapter_upper}_BASE")).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| default.map(str::to_owned))
        .map(|s| s.trim_end_matches('/').to_owned())
}

/// Request body.
#[derive(Debug)]
pub enum Body {
    /// No body.
    None,
    /// JSON (`content_type` overrides `application/json`).
    Json(Value, Option<&'static str>),
    /// Raw bytes with extra headers.
    Bytes(Vec<u8>, Vec<(&'static str, String)>),
}

/// An authenticated API client for one deployment.
pub struct Api {
    base: String,
    token: zeroize::Zeroizing<String>,
    client: reqwest::Client,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api").field("base", &self.base).finish_non_exhaustive()
    }
}

impl Api {
    /// Client for `base` with the bearer token `secret` from the request credentials.
    pub fn new(base: String, req: &DeployRequest, secret: &str, insecure: bool) -> Result<Self, DeployError> {
        let token = req
            .credentials
            .0
            .get(secret)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| DeployError::Unavailable(format!("needs credential {secret} (vault secret ref)")))?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .danger_accept_invalid_certs(insecure)
            .user_agent(concat!("orkestr-deploy/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| DeployError::Internal(e.to_string()))?;
        Ok(Self { base, token: zeroize::Zeroizing::new(token.clone()), client })
    }

    /// Base URL.
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    fn scrub(&self, s: &str) -> String {
        let s = if self.token.len() >= 4 { s.replace(self.token.as_str(), "[REDACTED]") } else { s.to_owned() };
        orkestr_security::redact::redact(&s)
    }

    /// Sends a request; non-2xx → `Failed` with a redacted message.
    pub async fn send(&self, method: Method, path: &str, body: Body) -> Result<Value, DeployError> {
        let url = format!("{}{path}", self.base);
        let mut rb = self.client.request(method.clone(), &url).bearer_auth(self.token.as_str());
        rb = match body {
            Body::None => rb,
            Body::Json(v, ct) => rb
                .header(reqwest::header::CONTENT_TYPE, ct.unwrap_or("application/json"))
                .body(serde_json::to_vec(&v).map_err(|e| DeployError::Internal(e.to_string()))?),
            Body::Bytes(b, headers) => {
                let mut rb = rb.header(reqwest::header::CONTENT_TYPE, "application/octet-stream");
                for (k, v) in headers {
                    rb = rb.header(k, v);
                }
                rb.body(b)
            }
        };
        let resp = rb.send().await.map_err(|e| DeployError::Failed(self.scrub(&format!("{method} {url}: {e}"))))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let short: String = text.chars().take(400).collect();
            return Err(DeployError::Failed(
                self.scrub(&format!("{method} {path} → HTTP {}: {short}", status.as_u16())),
            ));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}

/// A redacted request preview (the token is never part of it).
#[must_use]
pub fn preview(method: &str, url: String, body: Option<String>) -> RequestPreview {
    let auth = "Authorization: Bearer [REDACTED]".to_owned();
    RequestPreview {
        method: method.into(),
        target: url,
        body: Some(match body {
            Some(b) => format!("{auth}\n{}", orkestr_security::redact::redact(&b)),
            None => auth,
        }),
    }
}

/// Polling knobs: `pollIntervalMs` (2000) and `timeoutSecs` (900).
pub fn poll_settings(req: &DeployRequest) -> Result<(Duration, Duration), DeployError> {
    Ok((
        Duration::from_millis(number(req, "pollIntervalMs", 2000u64)?.max(10)),
        Duration::from_secs(number(req, "timeoutSecs", 900u64)?),
    ))
}

/// Outcome of one poll.
pub enum Poll<T> {
    /// Done.
    Ready(T),
    /// Still going (status text).
    Pending(String),
}

/// Polls `f` until ready, error, timeout or cancel; logs status changes.
pub async fn poll<T, F, Fut>(
    req: &DeployRequest,
    log: &LogSink,
    cancel: &CancellationToken,
    mut f: F,
) -> Result<T, DeployError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Poll<T>, DeployError>>,
{
    let (interval, timeout) = poll_settings(req)?;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last = String::new();
    loop {
        if cancel.is_cancelled() {
            return Err(DeployError::Cancelled);
        }
        match f().await? {
            Poll::Ready(t) => return Ok(t),
            Poll::Pending(s) => {
                if s != last {
                    log.line(&format!("status: {s}"));
                    last = s;
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(DeployError::Failed(format!("timed out waiting (last status {last})")));
        }
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            () = cancel.cancelled() => return Err(DeployError::Cancelled),
        }
    }
}

/// `image` setting with `{version}` substituted.
pub fn image(req: &DeployRequest) -> Result<String, DeployError> {
    let tpl =
        setting(req, "image").ok_or_else(|| DeployError::Invalid("adapter setting `image` is required".into()))?;
    Ok(tpl.replace("{version}", &req.version))
}

/// Percent-encodes one path segment set (keeps `/`).
#[must_use]
pub fn encode_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Missing-credential warning for plans.
pub fn credential_warning(req: &DeployRequest, secret: &str) -> Option<String> {
    (!req.credentials.0.contains_key(secret))
        .then(|| format!("needs credential {secret}: add a vault secret named {secret} to this environment"))
}

/// String field of a JSON object.
#[must_use]
pub fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}
