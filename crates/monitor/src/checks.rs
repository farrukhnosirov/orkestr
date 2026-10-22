//! HTTP/TCP health checks with up/down hysteresis. **Owner: WP5.**
//!
//! State machine (`threshold` = `failure_threshold`, at least 1):
//! * `Unknown` → `Up` on the first success, → `Down` after `threshold`
//!   consecutive failures;
//! * `Up` → `Down` after `threshold` consecutive failures;
//! * `Down` → `Up` after `threshold` consecutive successes.
//!
//! A single flapping result therefore never toggles the state.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use orkestr_domain::Timestamp;
use orkestr_protocol::methods::monitor::{CheckProbe, CheckRunSample, CheckState, HealthCheck, HealthCheckStatus};

use crate::logs::redact_line;

/// Runs kept per check.
pub const HISTORY: usize = 60;
/// Body bytes inspected for `contains`.
const MAX_BODY: usize = 1024 * 1024;

/// Outcome of one probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutcome {
    /// Passed.
    pub ok: bool,
    /// Latency.
    pub latency_ms: u32,
    /// Failure reason (redacted).
    pub error: Option<String>,
}

/// Validates a probe definition.
pub fn validate(probe: &CheckProbe) -> Result<(), String> {
    match probe {
        CheckProbe::Http { url, method, .. } => {
            let u = reqwest::Url::parse(url).map_err(|e| format!("invalid url: {e}"))?;
            if !matches!(u.scheme(), "http" | "https") {
                return Err("url must be http(s)".into());
            }
            if let Some(m) = method {
                reqwest::Method::from_bytes(m.trim().to_ascii_uppercase().as_bytes())
                    .map_err(|_| format!("invalid method `{m}`"))?;
            }
            Ok(())
        }
        CheckProbe::Tcp { host, port } => {
            if host.trim().is_empty() || *port == 0 {
                Err("tcp probe needs host and port".into())
            } else {
                Ok(())
            }
        }
    }
}

fn elapsed_ms(start: Instant) -> u32 {
    u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX)
}

/// Runs a probe with `timeout`.
pub async fn probe(client: &reqwest::Client, probe: &CheckProbe, timeout: Duration) -> ProbeOutcome {
    let start = Instant::now();
    let res = tokio::time::timeout(timeout, run_probe(client, probe)).await;
    let latency_ms = elapsed_ms(start);
    match res {
        Ok(Ok(())) => ProbeOutcome { ok: true, latency_ms, error: None },
        Ok(Err(e)) => ProbeOutcome { ok: false, latency_ms, error: Some(redact_line(&e)) },
        Err(_) => {
            ProbeOutcome { ok: false, latency_ms, error: Some(format!("timed out after {} ms", timeout.as_millis())) }
        }
    }
}

async fn run_probe(client: &reqwest::Client, probe: &CheckProbe) -> Result<(), String> {
    match probe {
        CheckProbe::Tcp { host, port } => {
            tokio::net::TcpStream::connect((host.as_str(), *port)).await.map(drop).map_err(|e| format!("connect: {e}"))
        }
        CheckProbe::Http { url, method, expect_status, contains } => {
            let m = method
                .as_deref()
                .map(|m| reqwest::Method::from_bytes(m.trim().to_ascii_uppercase().as_bytes()))
                .transpose()
                .map_err(|_| "invalid method".to_owned())?
                .unwrap_or(reqwest::Method::GET);
            let resp = client.request(m, url).send().await.map_err(|e| format!("request: {}", e.without_url()))?;
            let status = resp.status();
            let status_ok = match expect_status {
                Some(s) => status.as_u16() == *s,
                None => status.is_success() || status.is_redirection(),
            };
            if !status_ok {
                return Err(format!("unexpected status {}", status.as_u16()));
            }
            if let Some(needle) = contains.as_deref().filter(|n| !n.is_empty()) {
                let mut body = Vec::new();
                let mut resp = resp;
                while let Some(chunk) = resp.chunk().await.map_err(|e| format!("body: {}", e.without_url()))? {
                    body.extend_from_slice(&chunk);
                    if body.len() >= MAX_BODY {
                        break;
                    }
                }
                if !String::from_utf8_lossy(&body).contains(needle) {
                    return Err("body does not contain the expected text".into());
                }
            }
            Ok(())
        }
    }
}

/// Live state of one check.
#[derive(Debug, Clone)]
pub struct CheckRuntime {
    /// Spec.
    pub check: HealthCheck,
    /// State.
    pub state: CheckState,
    /// State since.
    pub since: Option<Timestamp>,
    /// Consecutive failures.
    pub fails: u32,
    /// Consecutive successes.
    pub oks: u32,
    /// Last run.
    pub last_run: Option<Timestamp>,
    /// Last latency.
    pub last_latency_ms: Option<u32>,
    /// Last error.
    pub last_error: Option<String>,
    /// Recent runs.
    pub history: VecDeque<CheckRunSample>,
    /// Next scheduled run (monotonic).
    pub next_due: Instant,
    /// A probe is in flight.
    pub running: bool,
}

/// A state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    /// From.
    pub from: CheckState,
    /// To.
    pub to: CheckState,
}

impl CheckRuntime {
    /// Fresh runtime (due immediately).
    #[must_use]
    pub fn new(check: HealthCheck, state: CheckState, since: Option<Timestamp>) -> Self {
        Self {
            check,
            state,
            since,
            fails: 0,
            oks: 0,
            last_run: None,
            last_latency_ms: None,
            last_error: None,
            history: VecDeque::new(),
            next_due: Instant::now(),
            running: false,
        }
    }

    /// Records an outcome; returns the transition when the state changed.
    pub fn record(&mut self, at: Timestamp, o: &ProbeOutcome) -> Option<Transition> {
        let threshold = self.check.failure_threshold.max(1);
        self.last_run = Some(at);
        self.last_latency_ms = Some(o.latency_ms);
        self.last_error.clone_from(&o.error);
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(CheckRunSample {
            ts: at,
            ok: o.ok,
            latency_ms: Some(o.latency_ms),
            error: o.error.clone(),
        });
        if o.ok {
            self.oks = self.oks.saturating_add(1);
            self.fails = 0;
        } else {
            self.fails = self.fails.saturating_add(1);
            self.oks = 0;
        }
        let next = match self.state {
            CheckState::Unknown if o.ok => CheckState::Up,
            CheckState::Unknown | CheckState::Up if self.fails >= threshold => CheckState::Down,
            CheckState::Down if self.oks >= threshold => CheckState::Up,
            s => s,
        };
        (next != self.state).then(|| {
            let t = Transition { from: self.state, to: next };
            self.state = next;
            self.since = Some(at);
            t
        })
    }

    /// Wire view.
    #[must_use]
    pub fn status(&self) -> HealthCheckStatus {
        HealthCheckStatus {
            check: self.check.clone(),
            state: self.state,
            since: self.since,
            last_run: self.last_run,
            last_latency_ms: self.last_latency_ms,
            last_error: self.last_error.clone(),
            history: self.history.iter().cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::HealthCheckId;

    fn rt(threshold: u32) -> CheckRuntime {
        CheckRuntime::new(
            HealthCheck {
                id: HealthCheckId::new(),
                project_id: None,
                environment_id: None,
                name: "c".into(),
                probe: CheckProbe::Tcp { host: "127.0.0.1".into(), port: 1 },
                interval_secs: 30,
                timeout_ms: 1000,
                failure_threshold: threshold,
                enabled: true,
            },
            CheckState::Unknown,
            None,
        )
    }

    fn out(ok: bool) -> ProbeOutcome {
        ProbeOutcome { ok, latency_ms: 1, error: (!ok).then(|| "x".into()) }
    }

    #[test]
    fn up_down_transitions_with_hysteresis() {
        let mut r = rt(3);
        let now = orkestr_domain::now();
        assert_eq!(r.record(now, &out(true)), Some(Transition { from: CheckState::Unknown, to: CheckState::Up }));
        // Two failures stay Up, a success resets the counter.
        assert_eq!(r.record(now, &out(false)), None);
        assert_eq!(r.record(now, &out(false)), None);
        assert_eq!(r.record(now, &out(true)), None);
        assert_eq!(r.record(now, &out(false)), None);
        assert_eq!(r.record(now, &out(false)), None);
        assert_eq!(r.record(now, &out(false)), Some(Transition { from: CheckState::Up, to: CheckState::Down }));
        assert_eq!(r.record(now, &out(false)), None);
        // Recovery needs threshold successes too.
        assert_eq!(r.record(now, &out(true)), None);
        assert_eq!(r.record(now, &out(true)), None);
        assert_eq!(r.record(now, &out(false)), None);
        assert_eq!(r.record(now, &out(true)), None);
        assert_eq!(r.record(now, &out(true)), None);
        assert_eq!(r.record(now, &out(true)), Some(Transition { from: CheckState::Down, to: CheckState::Up }));
        assert_eq!(r.status().history.len(), 14);
    }

    #[test]
    fn unknown_goes_down_after_threshold() {
        let mut r = rt(2);
        let now = orkestr_domain::now();
        assert_eq!(r.record(now, &out(false)), None);
        assert_eq!(r.record(now, &out(false)), Some(Transition { from: CheckState::Unknown, to: CheckState::Down }));
    }

    #[tokio::test]
    async fn http_and_tcp_probes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let (code, body) = if req.starts_with("GET /ok") {
                        ("200 OK", "status: healthy")
                    } else {
                        ("503 Unavailable", "down")
                    };
                    let resp =
                        format!("HTTP/1.1 {code}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        let client = reqwest::Client::new();
        let t = Duration::from_secs(5);
        let http = |path: &str, contains: Option<&str>, expect: Option<u16>| CheckProbe::Http {
            url: format!("http://127.0.0.1:{port}{path}"),
            method: None,
            expect_status: expect,
            contains: contains.map(str::to_owned),
        };
        assert!(probe(&client, &http("/ok", Some("healthy"), None), t).await.ok);
        let bad_body = probe(&client, &http("/ok", Some("nope"), None), t).await;
        assert!(!bad_body.ok && bad_body.error.unwrap().contains("body"));
        let bad = probe(&client, &http("/fail", None, None), t).await;
        assert_eq!(bad.error.as_deref(), Some("unexpected status 503"));
        assert!(probe(&client, &http("/fail", None, Some(503)), t).await.ok);
        assert!(probe(&client, &CheckProbe::Tcp { host: "127.0.0.1".into(), port }, t).await.ok);
        // A port nobody listens on (bound then dropped).
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(!probe(&client, &CheckProbe::Tcp { host: "127.0.0.1".into(), port: free }, t).await.ok);
        assert!(validate(&CheckProbe::Http {
            url: "ftp://x".into(),
            method: None,
            expect_status: None,
            contains: None
        })
        .is_err());
        assert!(validate(&CheckProbe::Http {
            url: "http://x".into(),
            method: Some("bad method".into()),
            expect_status: None,
            contains: None
        })
        .is_err());
    }
}
