//! Native *Stop All AI* for the tray (§52).
//!
//! The tray's emergency stop is the last-resort control, so it must not
//! depend on the main webview (which may be hidden, crashed, hung or still
//! reconnecting its RPC socket). The shell stops agents itself:
//! * **embedded core** — calls the same service path as the
//!   `emergency.stopAll` RPC handler ([`orkestr_core::rpc::emergency::stop_all`]);
//! * **external `orkestrd`** — sends `emergency.stopAll` over a short-lived
//!   RPC connection with the known `{url, token}`.
//!
//! The webview only gets the outcome for UI feedback.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orkestr_core::Services;
use serde::Serialize;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

/// How long a one-shot RPC call may take (connect + auth + call).
pub const RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the stop goes.
pub enum StopTarget {
    /// The core runs in this process.
    Embedded(Arc<Services>),
    /// An already running `orkestrd`.
    External {
        /// RPC WebSocket URL.
        url: String,
        /// Bearer token.
        token: String,
    },
}

/// Outcome of the tray's *Stop All AI*, sent to the main webview for feedback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopAllOutcome {
    /// Whether the stop was executed.
    pub ok: bool,
    /// Agents stopped.
    pub agents_stopped: u32,
    /// Pending approvals cancelled.
    pub approvals_cancelled: u32,
    /// Why the stop failed (`ok == false`).
    pub error: Option<String>,
}

impl StopAllOutcome {
    /// A failed stop.
    #[must_use]
    pub fn failed(error: impl Into<String>) -> Self {
        Self { ok: false, agents_stopped: 0, approvals_cancelled: 0, error: Some(error.into()) }
    }
}

/// Executes Stop All AI against the core.
pub async fn stop_all(target: StopTarget) -> StopAllOutcome {
    match target {
        StopTarget::Embedded(services) => match orkestr_core::rpc::emergency::stop_all(&services).await {
            Ok(r) => StopAllOutcome {
                ok: true,
                agents_stopped: r.agents_stopped,
                approvals_cancelled: r.approvals_cancelled,
                error: None,
            },
            Err(e) => StopAllOutcome::failed(e.message),
        },
        StopTarget::External { url, token } => {
            match rpc_call_once(&url, &token, "emergency.stopAll", json!({}), RPC_TIMEOUT).await {
                Ok(result) => StopAllOutcome {
                    ok: true,
                    agents_stopped: count(&result, "agentsStopped"),
                    approvals_cancelled: count(&result, "approvalsCancelled"),
                    error: None,
                },
                Err(e) => StopAllOutcome::failed(e),
            }
        }
    }
}

fn count(v: &Value, key: &str) -> u32 {
    v.get(key).and_then(Value::as_u64).map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
}

/// One RPC call over a fresh WebSocket connection: auth frame, request,
/// response (events and pings in between are skipped). Returns `result`.
pub async fn rpc_call_once(
    url: &str,
    token: &str,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, String> {
    tokio::time::timeout(timeout, rpc_call_inner(url, token, method, params))
        .await
        .map_err(|_| format!("{method}: the core did not answer within {}s", timeout.as_secs()))?
}

async fn rpc_call_inner(url: &str, token: &str, method: &str, params: Value) -> Result<Value, String> {
    const AUTH_ID: &str = "shell-auth";
    const REQ_ID: &str = "shell-req";
    let (mut ws, _) =
        tokio_tungstenite::connect_async(url).await.map_err(|e| format!("cannot reach the core: {e}"))?;
    let frames = [
        json!({"type": "auth", "id": AUTH_ID, "token": token}),
        json!({"type": "req", "id": REQ_ID, "method": method, "params": params}),
    ];
    for frame in frames {
        ws.send(Message::Text(frame.to_string().into())).await.map_err(|e| format!("cannot reach the core: {e}"))?;
    }
    let result = loop {
        let msg = match ws.next().await {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => break Err(format!("connection to the core failed: {e}")),
            None => break Err("the core closed the connection".to_owned()),
        };
        let text = match msg {
            Message::Text(text) => text,
            Message::Close(_) => break Err("the core closed the connection".to_owned()),
            _ => continue,
        };
        let Ok(frame) = serde_json::from_str::<Value>(text.as_str()) else { continue };
        if frame["type"] != "res" {
            continue;
        }
        let ok = frame["ok"] == true;
        let error = || frame["error"]["message"].as_str().unwrap_or("unknown error").to_owned();
        match frame["id"].as_str() {
            Some(AUTH_ID) if !ok => break Err(format!("the core rejected the shell: {}", error())),
            Some(REQ_ID) if ok => break Ok(frame["result"].clone()),
            Some(REQ_ID) => break Err(error()),
            _ => {}
        }
    };
    let _ = ws.close(None).await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tray stop works without any webview: directly through the
    /// embedded services and over a one-shot RPC call to an external core.
    #[tokio::test]
    async fn stops_natively_in_embedded_and_external_mode() {
        let dir = tempfile::tempdir().unwrap();
        let core = orkestr_core::start(orkestr_core::CoreConfig::with_data_dir(dir.path().join("data"))).await.unwrap();

        let embedded = stop_all(StopTarget::Embedded(Arc::clone(&core.services))).await;
        assert!(embedded.ok, "{embedded:?}");
        assert!(core.services.security.emergency.flags().all_stopped, "the emergency flag is set");

        let external = stop_all(StopTarget::External { url: core.url.clone(), token: core.token.clone() }).await;
        assert!(external.ok, "{external:?}");
        assert_eq!(external.error, None);

        let rejected = stop_all(StopTarget::External { url: core.url.clone(), token: "wrong".into() }).await;
        assert!(!rejected.ok, "a bad token must fail visibly, not report success");
        assert!(rejected.error.is_some());

        core.shutdown().await;
        let unreachable = stop_all(StopTarget::External { url: closed_url(), token: "t".into() }).await;
        assert!(!unreachable.ok);
    }

    fn closed_url() -> String {
        // Bind and drop a listener: nothing listens on that port any more.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        format!("ws://127.0.0.1:{port}/rpc")
    }
}
