//! Shared HTTP plumbing: client, endpoint config, streaming parsers (SSE,
//! NDJSON), error classification (spec §4) and the stream driver that
//! guarantees exactly one terminal event.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};
use orkestr_domain::{ProviderId, SecretRefId};
use reqwest::header::HeaderMap;
use reqwest::{RequestBuilder, Response};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::credentials::{CredentialError, CredentialResolver};
use crate::types::{ChatEvent, CodingAgentEvent, ProviderError};

/// Connect timeout for every provider request (spec §5.4).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Default stream idle timeout (spec §5.4).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// Timeout for health/model-list probes.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest wait for a local server's response headers (Ollama answers only
/// after loading the model into memory, which can take minutes).
pub const LOCAL_LOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Builds the shared HTTP client (rustls, HTTP/2, no global request timeout —
/// streams are governed by the idle timeout).
#[must_use]
pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("orkestr/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "custom HTTP client failed to build; using defaults");
            reqwest::Client::new()
        })
}

/// Configuration shared by HTTP adapters.
#[derive(Clone)]
pub struct HttpConfig {
    /// Provider instance id.
    pub id: ProviderId,
    /// Display name.
    pub name: String,
    /// Base URL without trailing slash.
    pub base_url: String,
    /// Extra non-secret headers.
    pub headers: Vec<(String, String)>,
    /// API key reference.
    pub secret: Option<SecretRefId>,
    /// Resolver for `secret`.
    pub credentials: Arc<dyn CredentialResolver>,
    /// Shared client.
    pub client: reqwest::Client,
    /// Default model.
    pub default_model: Option<String>,
    /// Stream idle timeout.
    pub idle_timeout: Duration,
}

impl std::fmt::Debug for HttpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpConfig")
            .field("id", &self.id)
            .field("base_url", &self.base_url)
            .field("headers", &self.headers.iter().map(|(k, _)| k).collect::<Vec<_>>())
            .field("has_secret", &self.secret.is_some())
            .finish_non_exhaustive()
    }
}

impl HttpConfig {
    /// `base_url` + `path`.
    #[must_use]
    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), path.trim_start_matches('/'))
    }

    /// Resolves the API key (None when no secret is configured).
    pub async fn api_key(&self) -> Result<Option<String>, ProviderError> {
        let Some(reference) = self.secret else { return Ok(None) };
        match self.credentials.resolve(reference).await {
            Ok(k) => Ok(Some(k)),
            Err(CredentialError::NotFound) => Err(ProviderError::Auth("API key not found in the secret vault".into())),
            Err(e) => Err(ProviderError::Auth(e.to_string())),
        }
    }

    /// Like [`Self::api_key`] but missing keys are an `Auth` error.
    pub async fn require_api_key(&self) -> Result<String, ProviderError> {
        self.api_key().await?.ok_or_else(|| ProviderError::Auth("no API key configured".into()))
    }

    /// Applies the configured extra headers.
    pub fn with_headers(&self, mut rb: RequestBuilder) -> RequestBuilder {
        for (k, v) in &self.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        rb
    }
}

/// Sends a request; non-2xx responses are classified.
pub async fn send(rb: RequestBuilder) -> Result<Response, ProviderError> {
    let resp = rb.send().await.map_err(map_reqwest_error)?;
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let headers = resp.headers().clone();
    let body = resp.text().await.unwrap_or_default();
    Err(classify_http_error(status.as_u16(), &headers, &body))
}

/// [`send`] for streaming requests: the idle timeout only starts once the
/// headers have arrived, so a server that accepts the connection but never
/// answers would otherwise hang the attempt forever. Expiry is a `Network`
/// error, which is fallback-eligible.
pub async fn send_with_header_timeout(rb: RequestBuilder, wait: Duration) -> Result<Response, ProviderError> {
    match tokio::time::timeout(wait, send(rb)).await {
        Ok(r) => r,
        Err(_) => Err(ProviderError::Network(format!("no response from the provider within {}s", wait.as_secs()))),
    }
}

/// Maps transport errors.
#[must_use]
pub fn map_reqwest_error(e: reqwest::Error) -> ProviderError {
    // reqwest errors never include our headers; strip the URL query anyway.
    let e = e.without_url();
    if e.is_timeout() {
        ProviderError::Network(format!("timeout: {e}"))
    } else if e.is_connect() || e.is_request() {
        ProviderError::Network(e.to_string())
    } else if e.is_decode() || e.is_body() {
        ProviderError::Server(format!("malformed response: {e}"))
    } else {
        ProviderError::Network(e.to_string())
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}

/// Extracts a human message from a JSON (or plain) error body.
#[must_use]
pub fn error_message(body: &str) -> String {
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let msg = parsed.as_ref().and_then(|v| {
        let v = if let Some(arr) = v.as_array() { arr.first()? } else { v };
        v.pointer("/error/message")
            .and_then(|m| m.as_str())
            .or_else(|| v.get("error").and_then(|e| e.as_str()))
            .or_else(|| v.get("message").and_then(|m| m.as_str()))
            .map(str::to_owned)
    });
    let msg = msg.unwrap_or_else(|| body.trim().to_owned());
    truncate(if msg.is_empty() { "no details" } else { &msg }, 300)
}

fn parse_duration_secs(s: &str) -> Option<u64> {
    // Accepts "12", "1.5", "30s", "6m0s", "20ms", "1h2m".
    let s = s.trim();
    if let Ok(v) = s.parse::<f64>() {
        return (v >= 0.0).then(|| v.ceil() as u64);
    }
    let mut total_ms: f64 = 0.0;
    let mut num = String::new();
    let mut chars = s.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            continue;
        }
        let v: f64 = num.parse().ok()?;
        num.clear();
        let unit_ms = match c {
            'h' => 3_600_000.0,
            'm' if chars.peek() == Some(&'s') => {
                chars.next();
                1.0
            }
            'm' => 60_000.0,
            's' => 1000.0,
            _ => return None,
        };
        total_ms += v * unit_ms;
        any = true;
    }
    if !num.is_empty() {
        return None;
    }
    any.then(|| (total_ms / 1000.0).ceil() as u64)
}

/// Derives retry-after seconds from headers / body (spec §4 table row 1).
#[must_use]
pub fn retry_after_secs(headers: &HeaderMap, body: &str) -> Option<u64> {
    let h = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
    if let Some(ms) = h("retry-after-ms").and_then(|v| v.trim().parse::<f64>().ok()) {
        return Some((ms / 1000.0).ceil() as u64);
    }
    if let Some(v) = h("retry-after") {
        if let Some(s) = parse_duration_secs(&v) {
            return Some(s);
        }
        if let Ok(t) = chrono::DateTime::parse_from_rfc2822(v.trim()) {
            let secs = (t.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds();
            return Some(secs.max(0) as u64);
        }
    }
    for name in ["x-ratelimit-reset-requests", "x-ratelimit-reset-tokens"] {
        if let Some(s) = h(name).and_then(|v| parse_duration_secs(&v)) {
            return Some(s);
        }
    }
    for name in [
        "anthropic-ratelimit-requests-reset",
        "anthropic-ratelimit-tokens-reset",
        "anthropic-ratelimit-input-tokens-reset",
    ] {
        if let Some(t) = h(name).and_then(|v| chrono::DateTime::parse_from_rfc3339(v.trim()).ok()) {
            let secs = (t.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds();
            return Some(secs.max(0) as u64);
        }
    }
    // Gemini: error.details[].retryDelay = "30s".
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let v = if let Some(arr) = v.as_array() { arr.first()?.clone() } else { v };
    v.pointer("/error/details")?
        .as_array()?
        .iter()
        .find_map(|d| d.get("retryDelay").and_then(|r| r.as_str()).and_then(parse_duration_secs))
}

const HARD_QUOTA_MARKERS: &[&str] = &[
    "insufficient_quota",
    "billing_hard_limit",
    "billing",
    "credit balance",
    "insufficient balance",
    "insufficient_balance",
    "payment required",
    "usage limit",
];

/// Classifies a non-2xx HTTP response (spec §4 classification table).
#[must_use]
pub fn classify_http_error(status: u16, headers: &HeaderMap, body: &str) -> ProviderError {
    let message = error_message(body);
    let lower = body.to_ascii_lowercase();
    let hard_quota = HARD_QUOTA_MARKERS.iter().any(|m| lower.contains(m));
    match status {
        401 => ProviderError::Auth(message),
        402 => ProviderError::QuotaExceeded(message),
        403 if hard_quota => ProviderError::QuotaExceeded(message),
        403 => ProviderError::Auth(message),
        429 => {
            let retry = retry_after_secs(headers, body);
            if hard_quota {
                ProviderError::QuotaExceeded(message)
            } else if retry.is_some() {
                ProviderError::RateLimited { retry_after_secs: retry }
            } else if lower.contains("quota") {
                ProviderError::QuotaExceeded(message)
            } else {
                ProviderError::RateLimited { retry_after_secs: None }
            }
        }
        400 if hard_quota => ProviderError::QuotaExceeded(message),
        408 => ProviderError::Network(format!("request timeout: {message}")),
        400..=499 => ProviderError::BadRequest(format!("HTTP {status}: {message}")),
        _ => ProviderError::Server(format!("HTTP {status}: {message}")),
    }
}

/// Reads the next body chunk with an idle timeout.
pub async fn next_chunk<S, B>(stream: &mut S, idle: Duration) -> Result<Option<B>, ProviderError>
where
    S: Stream<Item = Result<B, reqwest::Error>> + Unpin,
{
    match tokio::time::timeout(idle, stream.next()).await {
        Err(_) => Err(ProviderError::Network(format!("stream idle for {}s", idle.as_secs()))),
        Ok(None) => Ok(None),
        Ok(Some(Ok(b))) => Ok(Some(b)),
        Ok(Some(Err(e))) => Err(map_reqwest_error(e)),
    }
}

/// Splits a byte stream into complete UTF-8 lines (never splits a code point,
/// because `\n` is ASCII). Trailing `\r` is removed.
#[derive(Debug, Default)]
pub struct LineBuffer {
    buf: Vec<u8>,
}

impl LineBuffer {
    /// Feeds bytes, returning every completed line.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            out.push(String::from_utf8_lossy(&line).into_owned());
        }
        out
    }

    /// Remaining partial line at end of stream.
    pub fn finish(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        let rest = std::mem::take(&mut self.buf);
        let s = String::from_utf8_lossy(&rest).trim_end_matches('\r').to_owned();
        (!s.trim().is_empty()).then_some(s)
    }
}

/// One server-sent event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// `event:` field (empty when absent).
    pub event: String,
    /// `data:` lines joined with `\n`.
    pub data: String,
}

/// Incremental SSE parser (WHATWG event-stream subset).
#[derive(Debug, Default)]
pub struct SseParser {
    lines: LineBuffer,
    event: String,
    data: Vec<String>,
}

impl SseParser {
    /// Feeds bytes, returning completed events.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        let lines = self.lines.push(bytes);
        let mut out = Vec::new();
        for line in lines {
            self.line(line, &mut out);
        }
        out
    }

    /// Flushes a trailing event without blank line.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if let Some(l) = self.lines.finish() {
            self.line(l, &mut out);
        }
        self.line(String::new(), &mut out);
        out
    }

    fn line(&mut self, line: String, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if !self.data.is_empty() {
                out.push(SseEvent { event: std::mem::take(&mut self.event), data: self.data.join("\n") });
                self.data.clear();
            } else {
                self.event.clear();
            }
            return;
        }
        if line.starts_with(':') {
            return; // comment / keep-alive
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f.to_owned(), v.strip_prefix(' ').unwrap_or(v).to_owned()),
            None => (line, String::new()),
        };
        match field.as_str() {
            "event" => self.event = value,
            "data" => self.data.push(value),
            _ => {}
        }
    }
}

/// Item of a driven stream.
pub trait StreamItem: Send + 'static {
    /// Whether this item ends the stream.
    fn is_terminal(&self) -> bool;
    /// Terminal error item.
    fn from_error(err: ProviderError) -> Self;
}

impl StreamItem for ChatEvent {
    fn is_terminal(&self) -> bool {
        ChatEvent::is_terminal(self)
    }
    fn from_error(err: ProviderError) -> Self {
        ChatEvent::Error(err)
    }
}

impl StreamItem for CodingAgentEvent {
    fn is_terminal(&self) -> bool {
        matches!(self, CodingAgentEvent::Chat(c) if c.is_terminal())
    }
    fn from_error(err: ProviderError) -> Self {
        CodingAgentEvent::Chat(ChatEvent::Error(err))
    }
}

/// Sender side of a driven stream. Drops everything after the first terminal item.
pub struct Emitter<T> {
    tx: mpsc::Sender<T>,
    terminal: Arc<AtomicBool>,
}

impl<T> Clone for Emitter<T> {
    fn clone(&self) -> Self {
        Self { tx: self.tx.clone(), terminal: Arc::clone(&self.terminal) }
    }
}

impl<T: StreamItem> Emitter<T> {
    /// Emits an item. Fails with `Cancelled` when the consumer went away.
    pub async fn emit(&self, item: T) -> Result<(), ProviderError> {
        if self.terminal.load(Ordering::SeqCst) {
            return Ok(());
        }
        if item.is_terminal() {
            self.terminal.store(true, Ordering::SeqCst);
        }
        self.tx.send(item).await.map_err(|_| ProviderError::Cancelled)
    }

    /// Whether a terminal item was emitted.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.terminal.load(Ordering::SeqCst)
    }
}

/// Runs `driver` on a task and returns its output as a stream that always
/// ends with exactly one terminal item. Cancellation emits `Cancelled`.
pub fn drive<T, F, Fut>(cancel: CancellationToken, driver: F) -> futures::stream::BoxStream<'static, T>
where
    T: StreamItem,
    F: FnOnce(Emitter<T>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), ProviderError>> + Send + 'static,
{
    let (tx, rx) = mpsc::channel(128);
    let emitter = Emitter { tx, terminal: Arc::new(AtomicBool::new(false)) };
    let inner = emitter.clone();
    tokio::spawn(async move {
        let result = tokio::select! {
            biased;
            () = cancel.cancelled() => Err(ProviderError::Cancelled),
            r = driver(inner) => r,
        };
        if !emitter.finished() {
            let err = match result {
                Ok(()) => ProviderError::Server("stream ended without a terminal event".into()),
                Err(e) => e,
            };
            let _ = emitter.emit(T::from_error(err)).await;
        }
    });
    ReceiverStream::new(rx).boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue};

    #[tokio::test]
    async fn hung_server_times_out_as_network_error() {
        // Accepts the TCP connection but never sends response headers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hold = tokio::spawn(async move {
            let mut conns = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                conns.push(sock);
            }
        });
        let rb = build_client().post(format!("http://{addr}/v1/chat/completions")).body("{}");
        let err = send_with_header_timeout(rb, Duration::from_millis(300)).await.unwrap_err();
        assert!(matches!(err, ProviderError::Network(_)), "{err:?}");
        assert!(err.is_fallback_eligible());
        hold.abort();
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn sse_parser_handles_split_chunks_and_multiline_data() {
        let mut p = SseParser::default();
        assert!(p.push(b"event: a\ndata: {\"x\"").is_empty());
        let ev = p.push(b":1}\n\n: keep-alive\n\ndata: l1\r\ndata: l2\r\n\r\n");
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0], SseEvent { event: "a".into(), data: "{\"x\":1}".into() });
        assert_eq!(ev[1].data, "l1\nl2");
        assert_eq!(ev[1].event, "");
        assert!(p.push(b"data: tail").is_empty());
        assert_eq!(p.finish()[0].data, "tail");
    }

    #[test]
    fn line_buffer_keeps_utf8_intact() {
        let mut lb = LineBuffer::default();
        let s = "héllo 🌍\n".as_bytes();
        let (a, b) = s.split_at(3); // splits inside 'é'
        assert!(lb.push(a).is_empty());
        assert_eq!(lb.push(b), vec!["héllo 🌍".to_owned()]);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration_secs("12"), Some(12));
        assert_eq!(parse_duration_secs("1.2"), Some(2));
        assert_eq!(parse_duration_secs("6m0s"), Some(360));
        assert_eq!(parse_duration_secs("20ms"), Some(1));
        assert_eq!(parse_duration_secs("30s"), Some(30));
        assert_eq!(parse_duration_secs("abc"), None);
    }

    #[test]
    fn classification_table() {
        let none = HeaderMap::new();
        assert!(matches!(classify_http_error(401, &none, "{}"), ProviderError::Auth(_)));
        assert!(matches!(
            classify_http_error(403, &none, r#"{"error":{"message":"forbidden"}}"#),
            ProviderError::Auth(_)
        ));
        assert_eq!(
            classify_http_error(429, &headers(&[("retry-after", "7")]), "{}"),
            ProviderError::RateLimited { retry_after_secs: Some(7) }
        );
        assert!(matches!(
            classify_http_error(
                429,
                &none,
                r#"{"error":{"code":"insufficient_quota","message":"You exceeded your current quota"}}"#
            ),
            ProviderError::QuotaExceeded(_)
        ));
        assert!(matches!(classify_http_error(402, &none, "Insufficient Balance"), ProviderError::QuotaExceeded(_)));
        assert!(matches!(
            classify_http_error(400, &none, r#"{"error":{"message":"Your credit balance is too low"}}"#),
            ProviderError::QuotaExceeded(_)
        ));
        assert!(matches!(classify_http_error(400, &none, "{}"), ProviderError::BadRequest(_)));
        assert!(matches!(classify_http_error(503, &none, "{}"), ProviderError::Server(_)));
        assert!(matches!(classify_http_error(529, &none, "{}"), ProviderError::Server(_)));
        let gemini = r#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED","message":"quota","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"31s"}]}}"#;
        assert_eq!(classify_http_error(429, &none, gemini), ProviderError::RateLimited { retry_after_secs: Some(31) });
        assert_eq!(
            classify_http_error(429, &headers(&[("x-ratelimit-reset-requests", "1m30s")]), "{}"),
            ProviderError::RateLimited { retry_after_secs: Some(90) }
        );
        assert!(matches!(
            classify_http_error(429, &none, r#"{"error":{"message":"daily quota reached"}}"#),
            ProviderError::QuotaExceeded(_)
        ));
    }

    #[test]
    fn error_message_extraction() {
        assert_eq!(error_message(r#"{"error":{"message":"boom"}}"#), "boom");
        assert_eq!(error_message(r#"{"error":"model not found"}"#), "model not found");
        assert_eq!(error_message("plain"), "plain");
        assert_eq!(error_message(""), "no details");
    }

    #[tokio::test]
    async fn drive_guarantees_single_terminal() {
        let s = drive::<ChatEvent, _, _>(CancellationToken::new(), |em| async move {
            em.emit(ChatEvent::TextDelta("a".into())).await?;
            Ok(())
        });
        let items: Vec<_> = s.collect().await;
        assert_eq!(items.len(), 2);
        assert!(matches!(items[1], ChatEvent::Error(ProviderError::Server(_))));

        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        let s = drive::<ChatEvent, _, _>(cancel, |_em| async move {
            c2.cancel();
            futures::future::pending::<()>().await;
            Ok(())
        });
        let items: Vec<_> = s.collect().await;
        assert_eq!(items, vec![ChatEvent::Error(ProviderError::Cancelled)]);
    }
}
