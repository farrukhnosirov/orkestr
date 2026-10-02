//! Static knowledge about provider kinds: display names, auth, defaults and a
//! fallback model catalog used when `list_models` is unavailable.

use orkestr_domain::{AuthMethod, Model, ModelCapabilities, ProviderId, ProviderKind};
use serde::{Deserialize, Serialize};

use crate::pricing::pricing_for;

/// Default Ollama endpoint.
pub const OLLAMA_DEFAULT_URL: &str = "http://localhost:11434";
/// OpenAI API base.
pub const OPENAI_DEFAULT_URL: &str = "https://api.openai.com/v1";
/// Anthropic API base.
pub const ANTHROPIC_DEFAULT_URL: &str = "https://api.anthropic.com";
/// Gemini API base.
pub const GEMINI_DEFAULT_URL: &str = "https://generativelanguage.googleapis.com";
/// DeepSeek API base.
pub const DEEPSEEK_DEFAULT_URL: &str = "https://api.deepseek.com";
/// Mock display name (mandatory label, spec §10).
pub const MOCK_DISPLAY_NAME: &str = "Mock (offline demo)";

/// Static information about a provider kind (shown by the provider UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindInfo {
    /// Kind.
    pub kind: ProviderKind,
    /// Display name.
    pub display_name: &'static str,
    /// One-line description.
    pub description: &'static str,
    /// Auth mechanism.
    pub auth_method: AuthMethod,
    /// Needs an API key.
    pub requires_api_key: bool,
    /// API key is optional (OpenAI-compatible).
    pub api_key_optional: bool,
    /// Base URL editable.
    pub supports_base_url: bool,
    /// Default base URL.
    pub default_base_url: Option<&'static str>,
    /// Custom headers editable.
    pub supports_custom_headers: bool,
    /// Runs locally.
    pub is_local: bool,
    /// Offers a coding-agent mode.
    pub coding_agent: bool,
    /// Official CLI binary name.
    pub binary: Option<&'static str>,
    /// Note about official sign-in availability.
    pub sign_in_note: Option<&'static str>,
    /// How the user logs in (CLI kinds).
    pub login_instructions: Option<&'static str>,
    /// Default model.
    pub default_model: Option<&'static str>,
}

/// All kinds offered in Phase 1, in UI order.
pub const PHASE1_KINDS: &[ProviderKind] = &[
    ProviderKind::OpenAi,
    ProviderKind::Anthropic,
    ProviderKind::ClaudeCode,
    ProviderKind::Codex,
    ProviderKind::Gemini,
    ProviderKind::DeepSeek,
    ProviderKind::Ollama,
    ProviderKind::OpenAiCompatible,
    ProviderKind::Mock,
];

/// Static info for a kind.
#[must_use]
pub fn kind_info(kind: ProviderKind) -> KindInfo {
    let base = KindInfo {
        kind,
        display_name: "",
        description: "",
        auth_method: AuthMethod::ApiKey,
        requires_api_key: true,
        api_key_optional: false,
        supports_base_url: false,
        default_base_url: None,
        supports_custom_headers: false,
        is_local: false,
        coding_agent: false,
        binary: None,
        sign_in_note: None,
        login_instructions: None,
        default_model: None,
    };
    match kind {
        ProviderKind::OpenAi => KindInfo {
            display_name: "OpenAI",
            description: "OpenAI API (Chat Completions, streaming, tools, structured output).",
            supports_base_url: true,
            default_base_url: Some(OPENAI_DEFAULT_URL),
            sign_in_note: Some(
                "ChatGPT account sign-in is not offered for third-party apps; use an API key, or the Codex CLI for subscription access.",
            ),
            default_model: Some("gpt-4.1-mini"),
            ..base
        },
        ProviderKind::Anthropic => KindInfo {
            display_name: "Anthropic",
            description: "Anthropic Messages API (streaming, tool use).",
            supports_base_url: true,
            default_base_url: Some(ANTHROPIC_DEFAULT_URL),
            sign_in_note: Some("Claude subscription access goes through the official Claude Code CLI provider."),
            default_model: Some("claude-sonnet-4-5"),
            ..base
        },
        ProviderKind::Gemini => KindInfo {
            display_name: "Google Gemini",
            description: "Gemini API (streamGenerateContent, function calling).",
            supports_base_url: true,
            default_base_url: Some(GEMINI_DEFAULT_URL),
            default_model: Some("gemini-2.5-flash"),
            ..base
        },
        ProviderKind::DeepSeek => KindInfo {
            display_name: "DeepSeek",
            description: "DeepSeek API (OpenAI-compatible).",
            supports_base_url: true,
            default_base_url: Some(DEEPSEEK_DEFAULT_URL),
            default_model: Some("deepseek-chat"),
            ..base
        },
        ProviderKind::Ollama => KindInfo {
            display_name: "Ollama",
            description: "Local models via Ollama — nothing leaves this machine.",
            auth_method: AuthMethod::LocalEndpoint,
            requires_api_key: false,
            supports_base_url: true,
            default_base_url: Some(OLLAMA_DEFAULT_URL),
            is_local: true,
            ..base
        },
        ProviderKind::OpenAiCompatible | ProviderKind::Custom => KindInfo {
            display_name: "OpenAI-compatible",
            description: "Any OpenAI-compatible endpoint (LM Studio, vLLM, gateways) with optional key and headers.",
            auth_method: AuthMethod::CustomHeaders,
            requires_api_key: false,
            api_key_optional: true,
            supports_base_url: true,
            supports_custom_headers: true,
            ..base
        },
        ProviderKind::ClaudeCode => KindInfo {
            display_name: "Claude Code",
            description: "Official Claude Code CLI using your own logged-in session (subscription or API).",
            auth_method: AuthMethod::CliSession,
            requires_api_key: false,
            is_local: false,
            coding_agent: true,
            binary: Some("claude"),
            login_instructions: Some(
                "Install the official CLI (npm install -g @anthropic-ai/claude-code), run `claude` in a terminal and complete its login (/login). Orkestr never reads the CLI's credentials.",
            ),
            default_model: Some("sonnet"),
            ..base
        },
        ProviderKind::Codex => KindInfo {
            display_name: "OpenAI Codex",
            description: "Official Codex CLI using your own logged-in session (ChatGPT plan or API key login).",
            auth_method: AuthMethod::CliSession,
            requires_api_key: false,
            coding_agent: true,
            binary: Some("codex"),
            login_instructions: Some(
                "Install the official CLI (npm install -g @openai/codex), then run `codex login` in a terminal. Orkestr never reads the CLI's credentials.",
            ),
            ..base
        },
        ProviderKind::Mock => KindInfo {
            display_name: MOCK_DISPLAY_NAME,
            description: "Deterministic offline demo provider — scripted plans, tool calls and structured output. No network, no keys.",
            auth_method: AuthMethod::None,
            requires_api_key: false,
            is_local: true,
            default_model: Some("mock-smart"),
            ..base
        },
    }
}

/// snake_case adapter id of a kind (`openai`, `claude_code`, …).
#[must_use]
pub fn kind_slug(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenAi => "openai",
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Gemini => "gemini",
        ProviderKind::DeepSeek => "deepseek",
        ProviderKind::Ollama => "ollama",
        ProviderKind::OpenAiCompatible => "openai_compatible",
        ProviderKind::ClaudeCode => "claude_code",
        ProviderKind::Codex => "codex",
        ProviderKind::Mock => "mock",
        ProviderKind::Custom => "custom",
    }
}

/// Inverse of [`kind_slug`] (also accepts the PascalCase wire name).
#[must_use]
pub fn parse_kind(s: &str) -> Option<ProviderKind> {
    let all = [
        ProviderKind::OpenAi,
        ProviderKind::Anthropic,
        ProviderKind::Gemini,
        ProviderKind::DeepSeek,
        ProviderKind::Ollama,
        ProviderKind::OpenAiCompatible,
        ProviderKind::ClaudeCode,
        ProviderKind::Codex,
        ProviderKind::Mock,
        ProviderKind::Custom,
    ];
    all.into_iter().find(|k| kind_slug(*k) == s || format!("{k:?}") == s)
}

struct Spec {
    id: &'static str,
    name: &'static str,
    ctx: u32,
    out: u32,
    tools: bool,
    vision: bool,
    structured: bool,
    reasoning: bool,
}

const fn spec(id: &'static str, name: &'static str, ctx: u32, out: u32, vision: bool, reasoning: bool) -> Spec {
    Spec { id, name, ctx, out, tools: true, vision, structured: true, reasoning }
}

fn specs(kind: ProviderKind) -> Vec<Spec> {
    match kind {
        ProviderKind::OpenAi => vec![
            spec("gpt-5", "GPT-5", 400_000, 128_000, true, true),
            spec("gpt-5-mini", "GPT-5 mini", 400_000, 128_000, true, true),
            spec("gpt-4.1", "GPT-4.1", 1_047_576, 32_768, true, false),
            spec("gpt-4.1-mini", "GPT-4.1 mini", 1_047_576, 32_768, true, false),
            spec("gpt-4o", "GPT-4o", 128_000, 16_384, true, false),
            spec("o4-mini", "o4-mini", 200_000, 100_000, true, true),
        ],
        ProviderKind::Anthropic => vec![
            spec("claude-opus-4-1", "Claude Opus 4.1", 200_000, 32_000, true, true),
            spec("claude-sonnet-4-5", "Claude Sonnet 4.5", 200_000, 64_000, true, true),
            spec("claude-haiku-4-5", "Claude Haiku 4.5", 200_000, 64_000, true, true),
        ],
        ProviderKind::Gemini => vec![
            spec("gemini-2.5-pro", "Gemini 2.5 Pro", 1_048_576, 65_536, true, true),
            spec("gemini-2.5-flash", "Gemini 2.5 Flash", 1_048_576, 65_536, true, true),
            spec("gemini-2.5-flash-lite", "Gemini 2.5 Flash-Lite", 1_048_576, 65_536, true, false),
        ],
        ProviderKind::DeepSeek => vec![
            spec("deepseek-chat", "DeepSeek Chat", 128_000, 8_192, false, false),
            Spec {
                structured: true,
                tools: false,
                ..spec("deepseek-reasoner", "DeepSeek Reasoner", 128_000, 64_000, false, true)
            },
        ],
        ProviderKind::ClaudeCode => vec![
            Spec {
                structured: false,
                ..spec("sonnet", "Claude Code · Sonnet (CLI default)", 200_000, 64_000, false, true)
            },
            Spec { structured: false, ..spec("opus", "Claude Code · Opus", 200_000, 32_000, false, true) },
        ],
        ProviderKind::Codex => vec![Spec {
            structured: false,
            ..spec("default", "Codex · CLI default model", 200_000, 64_000, false, true)
        }],
        ProviderKind::Mock => vec![
            spec("mock-smart", "Mock · smart demo (plans, tools, JSON)", 128_000, 8_192, false, false),
            spec("mock-echo", "Mock · echo", 128_000, 8_192, false, false),
            spec("mock-tool-roundtrip", "Mock · tool round-trip", 128_000, 8_192, false, false),
            spec("mock-slow", "Mock · slow stream", 128_000, 8_192, false, false),
            spec("mock-rate-limit", "Mock · always rate-limited", 128_000, 8_192, false, false),
            spec("mock-quota", "Mock · quota exceeded", 128_000, 8_192, false, false),
            spec("mock-server-error", "Mock · server error", 128_000, 8_192, false, false),
        ],
        ProviderKind::Ollama | ProviderKind::OpenAiCompatible | ProviderKind::Custom => vec![],
    }
}

/// Static fallback model list for a kind.
#[must_use]
pub fn default_models(kind: ProviderKind, provider_id: ProviderId, is_local: bool) -> Vec<Model> {
    let coding_agent = matches!(kind, ProviderKind::ClaudeCode | ProviderKind::Codex);
    let priced =
        matches!(kind, ProviderKind::OpenAi | ProviderKind::Anthropic | ProviderKind::Gemini | ProviderKind::DeepSeek);
    specs(kind)
        .into_iter()
        .map(|s| Model {
            provider_id,
            id: s.id.to_owned(),
            display_name: s.name.to_owned(),
            context_window: s.ctx,
            max_output_tokens: Some(s.out),
            capabilities: ModelCapabilities {
                tools: s.tools,
                vision: s.vision,
                structured_output: s.structured,
                streaming: true,
                reasoning: s.reasoning,
                coding_agent,
            },
            pricing: if priced { pricing_for(s.id) } else { None },
            is_local,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_roundtrip() {
        for k in PHASE1_KINDS {
            assert_eq!(parse_kind(kind_slug(*k)), Some(*k));
            assert_eq!(parse_kind(&format!("{k:?}")), Some(*k));
        }
        assert_eq!(parse_kind("nope"), None);
    }

    #[test]
    fn mock_is_clearly_labelled() {
        assert_eq!(kind_info(ProviderKind::Mock).display_name, "Mock (offline demo)");
        assert!(kind_info(ProviderKind::Ollama).is_local);
        assert_eq!(kind_info(ProviderKind::ClaudeCode).binary, Some("claude"));
    }

    #[test]
    fn default_models_have_pricing_for_cloud() {
        let pid = ProviderId::new();
        let m = default_models(ProviderKind::OpenAi, pid, false);
        assert!(m.iter().all(|m| m.pricing.is_some()));
        assert!(default_models(ProviderKind::Mock, pid, true).iter().all(|m| m.pricing.is_none()));
    }
}
