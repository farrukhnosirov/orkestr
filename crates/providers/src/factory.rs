//! Builds adapters from persisted provider configs and validates config input.

use std::sync::Arc;
use std::time::Duration;

use orkestr_domain::{Provider, ProviderKind};

use crate::adapter::ProviderAdapter;
use crate::adapters::anthropic::AnthropicAdapter;
use crate::adapters::claude_code::ClaudeCodeAdapter;
use crate::adapters::codex::CodexAdapter;
use crate::adapters::gemini::GeminiAdapter;
use crate::adapters::mock::{MockAdapter, MockOptions};
use crate::adapters::ollama::{is_loopback_url, OllamaAdapter};
use crate::adapters::openai::{OpenAiAdapter, OpenAiFlavor};
use crate::catalog::kind_info;
use crate::credentials::CredentialResolver;
use crate::http::{HttpConfig, DEFAULT_IDLE_TIMEOUT};
use crate::types::ProviderError;

/// Header names that carry credentials: they must go through the secret
/// vault (API key field), never through plaintext custom headers.
pub const SECRET_HEADER_NAMES: &[&str] =
    &["authorization", "proxy-authorization", "x-api-key", "api-key", "x-goog-api-key", "cookie"];

/// Dependencies shared by all adapters.
#[derive(Clone)]
pub struct AdapterDeps {
    /// Secret resolution.
    pub credentials: Arc<dyn CredentialResolver>,
    /// Shared HTTP client.
    pub client: reqwest::Client,
    /// Mock tuning.
    pub mock: MockOptions,
    /// Stream idle timeout.
    pub idle_timeout: Duration,
}

impl std::fmt::Debug for AdapterDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterDeps").field("idle_timeout", &self.idle_timeout).finish_non_exhaustive()
    }
}

impl AdapterDeps {
    /// Default deps around a resolver.
    #[must_use]
    pub fn new(credentials: Arc<dyn CredentialResolver>) -> Self {
        Self {
            credentials,
            client: crate::http::build_client(),
            mock: MockOptions::default(),
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }
}

/// Validates an `http(s)://` base URL and normalizes it (no trailing slash).
pub fn validate_base_url(url: &str) -> Result<String, ProviderError> {
    let u = url.trim();
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://"));
    match rest {
        Some(r) if !r.is_empty() && !r.starts_with('/') && !u.contains(char::is_whitespace) => {
            Ok(u.trim_end_matches('/').to_owned())
        }
        _ => Err(ProviderError::BadRequest(format!("base URL must start with http:// or https://: `{u}`"))),
    }
}

/// Validated non-secret custom headers from `settings.headers`.
pub fn headers_from_settings(settings: &serde_json::Value) -> Result<Vec<(String, String)>, ProviderError> {
    let Some(obj) = settings.get("headers") else { return Ok(vec![]) };
    let obj = obj.as_object().ok_or_else(|| ProviderError::BadRequest("settings.headers must be an object".into()))?;
    let mut out = Vec::new();
    for (k, v) in obj {
        let name = k.trim();
        let value = v.as_str().ok_or_else(|| ProviderError::BadRequest(format!("header `{name}` must be a string")))?;
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(ProviderError::BadRequest(format!("invalid header name `{name}`")));
        }
        if SECRET_HEADER_NAMES.contains(&name.to_ascii_lowercase().as_str()) {
            return Err(ProviderError::BadRequest(format!(
                "header `{name}` carries a credential — put the key in the API key field (stored in the secret vault)"
            )));
        }
        if value.contains(['\r', '\n']) {
            return Err(ProviderError::BadRequest(format!("header `{name}` contains a line break")));
        }
        out.push((name.to_owned(), value.to_owned()));
    }
    Ok(out)
}

fn binary_override(settings: &serde_json::Value) -> Option<String> {
    settings.get("binaryPath").and_then(|v| v.as_str()).map(str::to_owned).filter(|s| !s.trim().is_empty())
}

/// Effective base URL of a provider (configured or kind default).
#[must_use]
pub fn effective_base_url(p: &Provider) -> Option<String> {
    p.base_url
        .clone()
        .filter(|u| !u.trim().is_empty())
        .or_else(|| kind_info(p.kind).default_base_url.map(str::to_owned))
}

/// Whether a provider config runs locally (privacy routing).
#[must_use]
pub fn is_local(p: &Provider) -> bool {
    match p.kind {
        ProviderKind::Mock => true,
        ProviderKind::Ollama | ProviderKind::OpenAiCompatible | ProviderKind::Custom => {
            effective_base_url(p).is_some_and(|u| is_loopback_url(&u))
        }
        _ => false,
    }
}

/// Builds the adapter for a provider config.
pub fn build_adapter(p: &Provider, deps: &AdapterDeps) -> Result<Arc<dyn ProviderAdapter>, ProviderError> {
    let http = || -> Result<HttpConfig, ProviderError> {
        let base = effective_base_url(p)
            .ok_or_else(|| ProviderError::BadRequest(format!("{} requires a base URL", p.name)))?;
        Ok(HttpConfig {
            id: p.id,
            name: p.name.clone(),
            base_url: validate_base_url(&base)?,
            headers: headers_from_settings(&p.settings)?,
            secret: p.secret_ref_id,
            credentials: Arc::clone(&deps.credentials),
            client: deps.client.clone(),
            default_model: p.default_model.clone(),
            idle_timeout: deps.idle_timeout,
        })
    };
    Ok(match p.kind {
        ProviderKind::OpenAi => Arc::new(OpenAiAdapter::new(http()?, OpenAiFlavor::OpenAi)),
        ProviderKind::DeepSeek => Arc::new(OpenAiAdapter::new(http()?, OpenAiFlavor::DeepSeek)),
        ProviderKind::OpenAiCompatible | ProviderKind::Custom => {
            Arc::new(OpenAiAdapter::new(http()?, OpenAiFlavor::Compatible { local: is_local(p) }))
        }
        ProviderKind::Anthropic => Arc::new(AnthropicAdapter::new(http()?)),
        ProviderKind::Gemini => Arc::new(GeminiAdapter::new(http()?)),
        ProviderKind::Ollama => Arc::new(OllamaAdapter::new(http()?)),
        ProviderKind::ClaudeCode => {
            Arc::new(ClaudeCodeAdapter::new(p.id, p.name.clone(), binary_override(&p.settings)))
        }
        ProviderKind::Codex => Arc::new(CodexAdapter::new(p.id, p.name.clone(), binary_override(&p.settings))),
        ProviderKind::Mock => Arc::new(MockAdapter::new(p.id, p.name.clone(), deps.mock)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn base_urls() {
        assert_eq!(validate_base_url(" http://localhost:1234/v1/ ").unwrap(), "http://localhost:1234/v1");
        assert!(validate_base_url("ftp://x").is_err());
        assert!(validate_base_url("http://").is_err());
        assert!(validate_base_url("https://a b").is_err());
    }

    #[test]
    fn headers_reject_secrets_and_injection() {
        assert_eq!(
            headers_from_settings(&json!({"headers": {"X-Team": "a"}})).unwrap(),
            vec![("X-Team".to_owned(), "a".to_owned())]
        );
        assert!(headers_from_settings(&json!({"headers": {"Authorization": "Bearer x"}})).is_err());
        assert!(headers_from_settings(&json!({"headers": {"X-A": "a\r\nInjected: 1"}})).is_err());
        assert!(headers_from_settings(&json!({"headers": {"bad name": "a"}})).is_err());
        assert!(headers_from_settings(&json!({})).unwrap().is_empty());
    }
}
