//! One `codex app-server` process: JSON-RPC 2.0 (without the `jsonrpc`
//! field, as the CLI writes it) over stdin / stdout lines.
//!
//! Responses are matched to requests by id; notifications and the CLI's own
//! requests (approvals, …) arrive on the [`Inbound`] channel returned by
//! [`AppServer::start`]. Dropping the last handle stops the process tree.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::adapters::claude_chat::{ChatProc, ProcSpawner};
use crate::adapters::cli;
use crate::types::ProviderError;

/// Default timeout of a request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Timeout of `thread/start` / `thread/resume` (MCP servers start first).
pub const THREAD_TIMEOUT: Duration = Duration::from_secs(180);

/// Something the app-server sent that is not a response.
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    /// A notification.
    Notification {
        /// Method.
        method: String,
        /// Params.
        params: Value,
    },
    /// A request the CLI sends to Orkestr (approvals, …): answer with
    /// [`AppServer::respond`] / [`AppServer::respond_error`].
    Request {
        /// JSON-RPC id (number or string).
        id: Value,
        /// Method.
        method: String,
        /// Params.
        params: Value,
    },
    /// The process ended.
    Closed,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, ProviderError>>>>>;

/// A running app-server.
pub struct AppServer {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicI64,
    alive: Arc<AtomicBool>,
    _guard: Box<dyn Send + Sync>,
}

impl std::fmt::Debug for AppServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppServer").field("alive", &self.is_alive()).finish_non_exhaustive()
    }
}

/// Error of a JSON-RPC error object.
#[must_use]
pub fn rpc_error(err: &Value) -> ProviderError {
    let msg = err.get("message").and_then(Value::as_str).unwrap_or("app-server error").to_owned();
    cli::classify_cli_message(&msg).unwrap_or(ProviderError::Server(msg))
}

impl AppServer {
    /// Starts `program app-server` with `spawner`, runs the `initialize`
    /// handshake and returns the server with its inbound channel.
    pub async fn start(
        spawner: &dyn ProcSpawner,
        program: &Path,
        cwd: &Path,
    ) -> Result<(Arc<Self>, mpsc::UnboundedReceiver<Inbound>), ProviderError> {
        let proc = spawner.spawn(program, &["app-server".to_owned()], cwd)?;
        let (server, rx) = Self::from_proc(proc);
        let init = json!({
            "clientInfo": { "name": "orkestr", "title": "Orkestr", "version": env!("CARGO_PKG_VERSION") },
        });
        server.request_with("initialize", init, Duration::from_secs(30)).await?;
        server.notify("initialized", None);
        Ok((server, rx))
    }

    /// Wraps a started process (tests use a fake one).
    #[must_use]
    pub fn from_proc(proc: ChatProc) -> (Arc<Self>, mpsc::UnboundedReceiver<Inbound>) {
        let ChatProc { stdin, mut stdout, _guard } = proc;
        let pending: Pending = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (p, a) = (Arc::clone(&pending), Arc::clone(&alive));
        tokio::spawn(async move {
            while let Some(line) = stdout.recv().await {
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    tracing::debug!("ignoring non-JSON line from codex app-server");
                    continue;
                };
                let method = v.get("method").and_then(Value::as_str).map(str::to_owned);
                let id = v.get("id").cloned().filter(|i| !i.is_null());
                match (method, id) {
                    (Some(method), Some(id)) => {
                        let params = v.get("params").cloned().unwrap_or(Value::Null);
                        let _ = in_tx.send(Inbound::Request { id, method, params });
                    }
                    (Some(method), None) => {
                        let params = v.get("params").cloned().unwrap_or(Value::Null);
                        let _ = in_tx.send(Inbound::Notification { method, params });
                    }
                    (None, Some(id)) => {
                        let Some(n) = id.as_i64() else { continue };
                        if let Some(tx) = p.lock().remove(&n) {
                            let r = match v.get("error") {
                                Some(e) if !e.is_null() => Err(rpc_error(e)),
                                _ => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                            };
                            let _ = tx.send(r);
                        }
                    }
                    (None, None) => {}
                }
            }
            a.store(false, Ordering::SeqCst);
            // Fail every pending request.
            for (_, tx) in p.lock().drain() {
                let _ = tx.send(Err(ProviderError::Server("the Codex app-server ended".into())));
            }
            let _ = in_tx.send(Inbound::Closed);
        });
        let server = Arc::new(Self { out: stdin, pending, next_id: AtomicI64::new(1), alive, _guard });
        (server, in_rx)
    }

    /// Whether the process is still running.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst) && !self.out.is_closed()
    }

    fn send(&self, v: &Value) -> bool {
        self.out.send(v.to_string()).is_ok()
    }

    /// Sends a request and waits for its result ([`REQUEST_TIMEOUT`]).
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, ProviderError> {
        self.request_with(method, params, REQUEST_TIMEOUT).await
    }

    /// Sends a request and waits for its result.
    pub async fn request_with(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, ProviderError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        let msg = if params.is_null() {
            json!({ "id": id, "method": method })
        } else {
            json!({ "id": id, "method": method, "params": params })
        };
        if !self.send(&msg) {
            self.pending.lock().remove(&id);
            return Err(ProviderError::Server("the Codex app-server is not running".into()));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(ProviderError::Server("the Codex app-server ended".into())),
            Err(_) => {
                self.pending.lock().remove(&id);
                Err(ProviderError::Network(format!("Codex did not answer `{method}` within {}s", timeout.as_secs())))
            }
        }
    }

    /// Sends a notification.
    pub fn notify(&self, method: &str, params: Option<Value>) {
        let msg = match params {
            Some(p) => json!({ "method": method, "params": p }),
            None => json!({ "method": method }),
        };
        self.send(&msg);
    }

    /// Answers a request of the CLI.
    pub fn respond(&self, id: &Value, result: Value) {
        self.send(&json!({ "id": id, "result": result }));
    }

    /// Answers a request of the CLI with an error.
    pub fn respond_error(&self, id: &Value, message: &str) {
        self.send(&json!({ "id": id, "error": { "code": -32601, "message": message } }));
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A scripted fake `codex app-server` for tests.
    use super::*;

    /// How the fake answers one request: `(method, params) -> lines to emit`.
    pub type Script = Arc<dyn Fn(&str, &Value, i64) -> Vec<Value> + Send + Sync>;

    /// Spawns fake app-servers driven by a script; records requests and
    /// responses sent by Orkestr.
    pub struct FakeCodex {
        /// Script.
        pub script: Script,
        /// Every message Orkestr sent (`method` or `response`).
        pub seen: Arc<Mutex<Vec<Value>>>,
        /// Processes started.
        pub spawned: std::sync::atomic::AtomicUsize,
    }

    impl FakeCodex {
        /// Creates the fake.
        pub fn new(script: Script) -> Arc<Self> {
            Arc::new(Self { script, seen: Arc::default(), spawned: std::sync::atomic::AtomicUsize::new(0) })
        }
    }

    impl ProcSpawner for FakeCodex {
        fn spawn(&self, _program: &Path, _args: &[String], _cwd: &Path) -> Result<ChatProc, ProviderError> {
            self.spawned.fetch_add(1, Ordering::SeqCst);
            let (in_tx, mut in_rx) = mpsc::unbounded_channel::<String>();
            let (out_tx, out_rx) = mpsc::unbounded_channel::<String>();
            let script = Arc::clone(&self.script);
            let seen = Arc::clone(&self.seen);
            tokio::spawn(async move {
                while let Some(line) = in_rx.recv().await {
                    let v: Value = serde_json::from_str(&line).unwrap();
                    seen.lock().push(v.clone());
                    let method = v.get("method").and_then(Value::as_str).unwrap_or("response").to_owned();
                    let id = v.get("id").and_then(Value::as_i64).unwrap_or(-1);
                    let params = if method == "response" {
                        v.clone()
                    } else {
                        v.get("params").cloned().unwrap_or(Value::Null)
                    };
                    let lines = if method == "initialize" {
                        vec![json!({"id": id, "result": {"userAgent": "fake"}})]
                    } else {
                        script(&method, &params, id)
                    };
                    for l in lines {
                        if l.get("__exit").is_some() {
                            return;
                        }
                        let _ = out_tx.send(l.to_string());
                    }
                }
            });
            Ok(ChatProc { stdin: in_tx, stdout: out_rx, _guard: Box::new(()) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeCodex;
    use super::*;

    #[tokio::test]
    async fn requests_are_matched_and_notifications_forwarded() {
        let fake = FakeCodex::new(Arc::new(|method, _p, id| match method {
            "account/read" => vec![
                json!({"method": "account/updated", "params": {"authMode": "chatgpt"}}),
                json!({"id": id, "result": {"account": {"type": "chatgpt", "email": "a@b.c", "planType": "plus"}}}),
            ],
            "initialized" => vec![],
            "boom" => vec![json!({"id": id, "error": {"code": 1, "message": "You've hit your usage limit"}})],
            _ => vec![json!({"__exit": true})],
        }));
        let (srv, mut rx) = AppServer::start(&*fake, Path::new("codex"), Path::new(".")).await.unwrap();
        let acc = srv.request("account/read", json!({})).await.unwrap();
        assert_eq!(acc["account"]["planType"], "plus");
        assert!(matches!(rx.recv().await, Some(Inbound::Notification { method, .. }) if method == "account/updated"));
        assert!(matches!(srv.request("boom", Value::Null).await, Err(ProviderError::QuotaExceeded(_))));
        // The fake exits: pending requests fail, the channel reports Closed.
        assert!(srv.request("exit", Value::Null).await.is_err());
        assert!(matches!(rx.recv().await, Some(Inbound::Closed)));
        assert!(!srv.is_alive());
        let seen = fake.seen.lock();
        assert_eq!(seen[0]["method"], "initialize");
        assert_eq!(seen[1]["method"], "initialized");
    }
}
