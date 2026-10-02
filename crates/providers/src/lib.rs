//! # orkestr-providers
//!
//! The AI Provider Hub (MASTER_SPEC §4, §7; contract §7;
//! `docs/providers/provider-adapter-spec.md`):
//!
//! * [`ProviderAdapter`] — object-safe async trait every provider implements
//!   (descriptor, auth state, models, health, streaming `chat`, optional
//!   `run_coding_agent` for official CLI agents).
//! * Adapters ([`adapters`]): `openai`, `anthropic`, `gemini`, `deepseek`
//!   (OpenAI-compatible preset), `ollama`, `openai_compatible`, `claude_code`
//!   (official `claude` CLI), `codex` (official `codex` CLI) and `mock`
//!   ("Mock (offline demo)", deterministic).
//! * [`ProviderRegistry`] — persisted configs, hot-swap, health polling,
//!   model cache, limit state + circuit breaker.
//! * [`router`] — manual routing table / automatic scoring under the privacy
//!   policy; [`fallback`] — fallback chain executor with structured handoff.
//! * [`pricing`] — static **estimated** price table + cost helpers.
//! * [`CredentialResolver`] — secrets are resolved by reference at call time;
//!   this crate never persists keys.
//!
//! **No provider-specific logic may leak outside this crate (§60.5).** Hidden
//! reasoning is dropped at the adapter boundary (no `ChatEvent` variant).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod adapter;
pub mod adapters;
pub mod catalog;
pub mod credentials;
pub mod factory;
pub mod fallback;
pub mod http;
pub mod limits;
pub mod pricing;
pub mod registry;
pub mod router;
pub mod store;
pub mod types;

pub use adapter::ProviderAdapter;
pub use catalog::{kind_info, kind_slug, parse_kind, KindInfo, MOCK_DISPLAY_NAME, PHASE1_KINDS};
pub use credentials::{CredentialError, CredentialResolver, MemoryCredentials};
pub use factory::AdapterDeps;
pub use fallback::{
    chat_with_fallback, AdapterSource, AttemptOutcome, FallbackOptions, HandoffContext, HandoffHook, NullSink,
    ProviderEvent, ProviderEventSink,
};
pub use pricing::{estimate_cost, pricing_for, PRICING_TABLE_DATE};
pub use registry::{
    migrations, ModelsOutcome, NewProvider, ProviderPatch, ProviderRegistry, ProviderView, RegistryError,
    RegistryOptions, RegistryResult, TestOutcome,
};
pub use router::{Candidate, RequiredCapabilities, RouteRequest};
pub use types::*;
