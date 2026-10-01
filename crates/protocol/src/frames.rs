//! WebSocket frames (contract §4). JSON, tagged by `type`, camelCase fields.
//!
//! Client → server: `auth`, `req`, `sub`, `unsub`.
//! Server → client: `res`, `event`.
//!
//! Handshake: the client sends `{"type":"auth","token":"…"}` as its first frame
//! (browsers cannot set WS headers). The server answers with a `res` frame whose
//! `id` is the auth frame's `id` (default `"auth"`) and whose result is
//! [`AuthResult`]. Clients that authenticated with an `Authorization: Bearer`
//! header may still send the auth frame; it is validated again and answered the
//! same way. `sub`/`unsub` are answered with a `res` carrying [`SubResult`].

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::SessionId;

use crate::error::RpcError;
use crate::event::Event;

fn default_auth_id() -> String {
    "auth".to_owned()
}

/// Information a client may send about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClientInfo {
    /// `desktop`, `web`, `mobile`, `cli`, `test`.
    pub kind: String,
    /// Client version.
    pub version: String,
}

/// Frames sent by clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ClientFrame {
    /// Authenticate the connection.
    Auth {
        /// Bearer token.
        token: String,
        /// Request id echoed in the `res` frame (default `"auth"`).
        #[serde(default = "default_auth_id")]
        #[ts(as = "Option<String>", optional)]
        id: String,
        /// Optional client info.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        client: Option<ClientInfo>,
    },
    /// Invoke a method.
    Req {
        /// Client-unique request id.
        id: String,
        /// `<area>.<verb>`.
        method: String,
        /// Method params (`{}` when omitted).
        #[serde(default)]
        #[ts(as = "Option<serde_json::Value>", optional)]
        params: serde_json::Value,
    },
    /// Subscribe to topics.
    Sub {
        /// Request id.
        id: String,
        /// Topics to add.
        topics: Vec<String>,
    },
    /// Unsubscribe from topics.
    Unsub {
        /// Request id.
        id: String,
        /// Topics to remove.
        topics: Vec<String>,
    },
}

/// Response frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResFrame {
    /// Request id being answered.
    pub id: String,
    /// Success flag.
    pub ok: bool,
    /// Result on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl ResFrame {
    /// Successful response.
    pub fn ok(id: impl Into<String>, result: serde_json::Value) -> Self {
        Self { id: id.into(), ok: true, result: Some(result), error: None }
    }

    /// Error response.
    pub fn err(id: impl Into<String>, error: RpcError) -> Self {
        Self { id: id.into(), ok: false, result: None, error: Some(error) }
    }
}

/// Frames sent by the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ServerFrame {
    /// Response to `auth`, `req`, `sub` or `unsub`.
    Res(ResFrame),
    /// A published event matching the connection's subscriptions.
    Event {
        /// Same as `event.seq` (lets clients detect gaps cheaply).
        seq: u64,
        /// The event (boxed to keep the frame enum small).
        event: Box<Event>,
    },
}

/// Result of a successful `auth`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AuthResult {
    /// Session id assigned to this connection.
    pub session_id: SessionId,
    /// Core version.
    pub server_version: String,
    /// [`crate::PROTOCOL_VERSION`].
    pub protocol_version: u32,
    /// Sequence number of the last persisted event (for replay/gap detection).
    pub last_seq: u64,
}

/// Result of `sub` / `unsub`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SubResult {
    /// All topics the connection is subscribed to after the change.
    pub topics: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_contract_examples() {
        let f: ClientFrame = serde_json::from_str(r#"{"type":"auth","token":"abc"}"#).unwrap();
        assert_eq!(f, ClientFrame::Auth { token: "abc".into(), id: "auth".into(), client: None });

        let f: ClientFrame =
            serde_json::from_str(r#"{"type":"req","id":"1","method":"projects.list","params":{}}"#).unwrap();
        assert!(matches!(f, ClientFrame::Req { ref method, .. } if method == "projects.list"));

        let f: ClientFrame = serde_json::from_str(r#"{"type":"req","id":"2","method":"system.health"}"#).unwrap();
        assert!(matches!(f, ClientFrame::Req { params: serde_json::Value::Null, .. }));

        let f: ClientFrame = serde_json::from_str(r#"{"type":"sub","id":"..","topics":["global"]}"#).unwrap();
        assert!(matches!(f, ClientFrame::Sub { .. }));
    }

    #[test]
    fn res_frame_shapes() {
        let ok = ServerFrame::Res(ResFrame::ok("1", serde_json::json!({"a": 1})));
        assert_eq!(
            serde_json::to_value(&ok).unwrap(),
            serde_json::json!({"type": "res", "id": "1", "ok": true, "result": {"a": 1}})
        );
        let err = ServerFrame::Res(ResFrame::err("2", RpcError::not_found("x")));
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "not_found");
        assert!(v.get("result").is_none());
    }
}
