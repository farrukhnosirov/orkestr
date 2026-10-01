//! RPC errors with stable, machine-readable codes (contract §4).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Stable error codes. Never rename; append only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RpcErrorCode {
    /// Missing/invalid token.
    Unauthorized,
    /// Malformed frame, unknown method, invalid params.
    BadRequest,
    /// Entity not found.
    NotFound,
    /// State conflict (duplicate, invalid transition, stale write).
    Conflict,
    /// Denied by policy.
    Forbidden,
    /// Action needs a user approval first.
    ApprovalRequired,
    /// Too many requests (client or provider).
    RateLimited,
    /// Budget would be exceeded.
    BudgetExceeded,
    /// Upstream AI provider failure.
    ProviderError,
    /// Bug or unexpected failure. Details are logged, not leaked.
    Internal,
}

impl RpcErrorCode {
    /// The snake_case wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RpcErrorCode::Unauthorized => "unauthorized",
            RpcErrorCode::BadRequest => "bad_request",
            RpcErrorCode::NotFound => "not_found",
            RpcErrorCode::Conflict => "conflict",
            RpcErrorCode::Forbidden => "forbidden",
            RpcErrorCode::ApprovalRequired => "approval_required",
            RpcErrorCode::RateLimited => "rate_limited",
            RpcErrorCode::BudgetExceeded => "budget_exceeded",
            RpcErrorCode::ProviderError => "provider_error",
            RpcErrorCode::Internal => "internal",
        }
    }
}

/// Error payload of a failed `res` frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{}: {message}", code.as_str())]
#[ts(export)]
pub struct RpcError {
    /// Machine-readable code.
    pub code: RpcErrorCode,
    /// Human-readable, non-sensitive message.
    pub message: String,
    /// Optional structured details (e.g. validation errors, approval id).
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

impl RpcError {
    /// Creates an error without data.
    pub fn new(code: RpcErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    /// Attaches structured data.
    #[must_use]
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    /// `unauthorized`.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::Unauthorized, message)
    }
    /// `bad_request`.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::BadRequest, message)
    }
    /// `not_found`.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::NotFound, message)
    }
    /// `conflict`.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::Conflict, message)
    }
    /// `forbidden`.
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::Forbidden, message)
    }
    /// `internal`. Pass a generic message; log the cause separately.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::Internal, message)
    }
    /// The explicit "not implemented yet" error used by skeleton handlers.
    pub fn unimplemented_feature(feature: &str) -> Self {
        Self::new(RpcErrorCode::BadRequest, format!("`{feature}` is not implemented yet (unimplemented-feature)"))
            .with_data(serde_json::json!({ "unimplementedFeature": feature }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_serialize_snake_case() {
        for (code, s) in [
            (RpcErrorCode::Unauthorized, "unauthorized"),
            (RpcErrorCode::BadRequest, "bad_request"),
            (RpcErrorCode::ApprovalRequired, "approval_required"),
            (RpcErrorCode::BudgetExceeded, "budget_exceeded"),
            (RpcErrorCode::ProviderError, "provider_error"),
        ] {
            assert_eq!(serde_json::to_value(code).unwrap(), s);
            assert_eq!(code.as_str(), s);
        }
    }

    #[test]
    fn error_shape() {
        let e = RpcError::not_found("project x");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v, serde_json::json!({"code": "not_found", "message": "project x", "data": null}));
        assert_eq!(e.to_string(), "not_found: project x");
    }
}
