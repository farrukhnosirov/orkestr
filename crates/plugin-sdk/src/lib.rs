//! Orkestr plugin SDK, guest side (ADR-0034). **Owner: WP1.**
//!
//! Build a plugin with `cargo build --target wasm32-unknown-unknown --release`
//! (see `examples/plugins/hello-tool`). ABI v1: the module exports `memory`,
//! `orkestr_alloc(len) -> ptr` and `orkestr_call(ptr, len) -> i64`
//! (`ptr << 32 | len` of the JSON result) and imports
//! `"orkestr" "host_call"(ptr, len) -> i64`.
//!
//! ```ignore
//! use orkestr_plugin_sdk::{orkestr_plugin, host, CallRequest};
//! use serde_json::{json, Value};
//!
//! fn handle(req: CallRequest) -> Result<Value, String> {
//!     host::log("info", &format!("tool {} called", req.tool))?;
//!     Ok(json!({ "hello": req.input["name"] }))
//! }
//! orkestr_plugin!(handle);
//! ```
//!
//! * Call payload: [`CallRequest`] `{"tool", "input"}`; result envelope
//!   `{"ok": value}` or `{"error": "message"}` ([`encode_result`]).
//! * Host calls: `{"op", "input"}` → `{"ok"}` / `{"error"}` ([`host`] on
//!   wasm32; [`encode_host_request`] / [`decode_host_reply`] everywhere).
//!
//! Memory: every plugin call runs in a fresh instance, so the SDK allocates
//! with the global allocator and never frees payloads (the instance is
//! dropped after the call).
//!
//! On non-wasm targets only the target-independent helpers compile, so the
//! host workspace build stays green.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// ABI version this SDK implements.
pub const ABI_VERSION: u32 = 1;

/// Packs `(ptr, len)` into the ABI's `i64`.
#[must_use]
pub fn pack(ptr: u32, len: u32) -> i64 {
    ((u64::from(ptr) << 32) | u64::from(len)).cast_signed()
}

/// Unpacks the ABI's `i64` into `(ptr, len)`.
#[must_use]
pub fn unpack(v: i64) -> (u32, u32) {
    let v = v.cast_unsigned();
    ((v >> 32) as u32, (v & 0xffff_ffff) as u32)
}

/// A tool call request delivered to `orkestr_call`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRequest {
    /// Tool name.
    pub tool: String,
    /// JSON input.
    #[serde(default)]
    pub input: Value,
}

/// Host operations (mirror of the host's `HostOp`).
pub mod ops {
    /// Plugin log line.
    pub const LOG: &str = "Log";
    /// HTTP request to a granted host.
    pub const HTTP_FETCH: &str = "HttpFetch";
    /// Plugin event.
    pub const EMIT_EVENT: &str = "EmitEvent";
    /// Private KV read.
    pub const KV_GET: &str = "KvGet";
    /// Private KV write.
    pub const KV_SET: &str = "KvSet";
    /// Granted Read-class RPC method.
    pub const RPC_READ: &str = "RpcRead";
    /// Granted secret.
    pub const SECRET_GET: &str = "SecretGet";
}

/// Encodes a host request.
#[must_use]
pub fn encode_host_request(op: &str, input: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({ "op": op, "input": input })).unwrap_or_default()
}

/// Decodes a host reply envelope.
///
/// # Errors
/// The host refused / failed the call, or the reply is malformed.
pub fn decode_host_reply(bytes: &[u8]) -> Result<Value, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| format!("invalid host reply: {e}"))?;
    if let Some(ok) = v.get("ok") {
        return Ok(ok.clone());
    }
    Err(v.get("error").and_then(Value::as_str).unwrap_or("malformed host reply").to_owned())
}

/// Encodes a tool result envelope.
#[must_use]
pub fn encode_result(r: &Result<Value, String>) -> Vec<u8> {
    let v = match r {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "error": e }),
    };
    serde_json::to_vec(&v).unwrap_or_else(|_| b"{\"error\":\"unserializable result\"}".to_vec())
}

/// Decodes the call payload.
///
/// # Errors
/// Malformed request.
pub fn decode_request(bytes: &[u8]) -> Result<CallRequest, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("invalid call request: {e}"))
}

/// FFI glue (wasm32 only).
#[cfg(target_arch = "wasm32")]
mod ffi {
    #[link(wasm_import_module = "orkestr")]
    extern "C" {
        #[link_name = "host_call"]
        pub fn host_call(ptr: i32, len: i32) -> i64;
    }

    /// Guest allocator export used by the host to pass payloads in.
    #[no_mangle]
    pub extern "C" fn orkestr_alloc(len: i32) -> i32 {
        let mut v = Vec::<u8>::with_capacity(usize::try_from(len).unwrap_or(0).max(1));
        let p = v.as_mut_ptr();
        core::mem::forget(v);
        p as i32
    }

    /// Leaks `bytes` and returns the packed `(ptr, len)`.
    pub fn leak(bytes: Vec<u8>) -> i64 {
        let b = bytes.into_boxed_slice();
        let len = b.len() as u32;
        let ptr = Box::leak(b).as_ptr() as u32;
        super::pack(ptr, len)
    }

    /// Reads `(ptr, len)` from linear memory.
    ///
    /// # Safety
    /// `ptr..ptr+len` must be a host-written payload.
    pub unsafe fn read(ptr: u32, len: u32) -> Vec<u8> {
        core::slice::from_raw_parts(ptr as *const u8, len as usize).to_vec()
    }
}

/// Host API (wasm32 only).
#[cfg(target_arch = "wasm32")]
pub mod host {
    use serde_json::{json, Value};

    /// Raw host call.
    ///
    /// # Errors
    /// Refused / failed by the host.
    pub fn call(op: &str, input: &Value) -> Result<Value, String> {
        let req = super::encode_host_request(op, input);
        // SAFETY: the host reads `req` (kept alive for the call) and returns a
        // payload it wrote with `orkestr_alloc`.
        let packed = unsafe { super::ffi::host_call(req.as_ptr() as i32, req.len() as i32) };
        let (p, l) = super::unpack(packed);
        // SAFETY: (p, l) was written by the host into memory from `orkestr_alloc`.
        super::decode_host_reply(&unsafe { super::ffi::read(p, l) })
    }

    /// Writes a plugin log line (`trace|debug|info|warn|error`).
    ///
    /// # Errors
    /// Host failure.
    pub fn log(level: &str, message: &str) -> Result<(), String> {
        call(super::ops::LOG, &json!({ "level": level, "message": message })).map(|_| ())
    }

    /// HTTP GET to a granted host: `{status, headers, body}`.
    ///
    /// # Errors
    /// Not granted / network failure.
    pub fn http_get(url: &str) -> Result<Value, String> {
        call(super::ops::HTTP_FETCH, &json!({ "url": url }))
    }

    /// Reads the plugin's private KV store.
    ///
    /// # Errors
    /// Host failure.
    pub fn kv_get(key: &str) -> Result<Value, String> {
        call(super::ops::KV_GET, &json!({ "key": key }))
    }

    /// Writes the plugin's private KV store (`Value::Null` deletes).
    ///
    /// # Errors
    /// Host failure / quota.
    pub fn kv_set(key: &str, value: &Value) -> Result<(), String> {
        call(super::ops::KV_SET, &json!({ "key": key, "value": value })).map(|_| ())
    }

    /// Reads a granted secret.
    ///
    /// # Errors
    /// Not granted / missing.
    pub fn secret_get(name: &str) -> Result<String, String> {
        let v = call(super::ops::SECRET_GET, &json!({ "name": name }))?;
        Ok(v.get("value").and_then(Value::as_str).unwrap_or_default().to_owned())
    }

    /// Calls a granted Read-class RPC method.
    ///
    /// # Errors
    /// Not granted / method error.
    pub fn rpc_read(method: &str, params: &Value) -> Result<Value, String> {
        call(super::ops::RPC_READ, &json!({ "method": method, "params": params }))
    }

    /// Publishes a plugin event.
    ///
    /// # Errors
    /// Host failure.
    pub fn emit_event(kind: &str, payload: &Value) -> Result<(), String> {
        call(super::ops::EMIT_EVENT, &json!({ "kind": kind, "payload": payload })).map(|_| ())
    }
}

/// Implementation of `orkestr_call` used by [`orkestr_plugin!`].
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub fn __dispatch(ptr: i32, len: i32, handler: fn(CallRequest) -> Result<Value, String>) -> i64 {
    // SAFETY: the host wrote `len` bytes at `ptr` (memory from `orkestr_alloc`).
    let bytes = unsafe { ffi::read(ptr as u32, len as u32) };
    let result = decode_request(&bytes).and_then(handler);
    ffi::leak(encode_result(&result))
}

/// Declares the plugin entry point: `orkestr_plugin!(handler)` where
/// `handler: fn(CallRequest) -> Result<serde_json::Value, String>`.
#[macro_export]
macro_rules! orkestr_plugin {
    ($handler:path) => {
        #[no_mangle]
        pub extern "C" fn orkestr_call(ptr: i32, len: i32) -> i64 {
            $crate::__dispatch(ptr, len, $handler)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_roundtrip() {
        assert_eq!(super::unpack(super::pack(7, 9)), (7, 9));
    }

    #[test]
    fn envelopes() {
        let req = encode_host_request(ops::LOG, &json!({"message": "x"}));
        let v: Value = serde_json::from_slice(&req).unwrap();
        assert_eq!(v["op"], "Log");
        assert_eq!(decode_host_reply(br#"{"ok":{"a":1}}"#).unwrap()["a"], 1);
        assert_eq!(decode_host_reply(br#"{"error":"denied: no"}"#).unwrap_err(), "denied: no");
        assert_eq!(encode_result(&Ok(json!(1))), br#"{"ok":1}"#.to_vec());
        assert_eq!(encode_result(&Err("bad".into())), br#"{"error":"bad"}"#.to_vec());
        assert_eq!(decode_request(br#"{"tool":"t","input":{"n":1}}"#).unwrap().tool, "t");
    }
}
