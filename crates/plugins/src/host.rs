//! Host API exposed to plugins (ADR-0034). Every call is checked against the
//! plugin's grants and audited; denials publish `PluginCallDenied`.

use async_trait::async_trait;
use orkestr_domain::PluginId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::PluginError;

/// Operations a plugin may request through `orkestr.host_call`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HostOp {
    /// Write to the plugin log (always allowed, rate limited).
    Log,
    /// HTTP request to a granted `Network` scope host.
    HttpFetch,
    /// Publish a plugin event.
    EmitEvent,
    /// Read from the plugin's private KV store.
    KvGet,
    /// Write to the plugin's private KV store.
    KvSet,
    /// Read-class RPC method (granted per method).
    RpcRead,
    /// Read a granted secret reference.
    SecretGet,
}

/// Host side of the plugin ABI.
#[async_trait]
pub trait HostApi: Send + Sync {
    /// Executes `op` for `caller` after the capability check.
    async fn call(&self, caller: &PluginId, op: HostOp, input: Value) -> Result<Value, PluginError>;
}

/// Denies everything (safe default before the real host is wired).
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyAllHost;

#[async_trait]
impl HostApi for DenyAllHost {
    async fn call(&self, _caller: &PluginId, op: HostOp, _input: Value) -> Result<Value, PluginError> {
        Err(PluginError::Denied(format!("host op {op:?} is not available")))
    }
}
