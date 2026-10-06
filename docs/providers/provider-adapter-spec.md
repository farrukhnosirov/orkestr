# Provider Adapter Specification

Status: Phase 0 deliverable #7 (§65). Binding companion to
`docs/ENGINEERING_CONTRACT.md` §7. Crate: `orkestr-providers`.
Spec references: §3, §4, §5, §7, §8, §22, §55, §56, §57, §60.5, §62.6–7, §63.B, §67.

> Rule zero (§60.5): **no provider-specific logic outside `orkestr-providers`.**
> The agent runtime, orchestrator, cost engine and UI see only the types in this
> document. "Claude is replaced by GPT without rewriting the Agent Runtime" (§67)
> is the acceptance test of this spec.

---

## 0. Implementation status and reconciliation with the code (2026-10-02)

The Phase 0 foundation contains a **skeleton** of this crate
(`crates/providers/src/lib.rs`) and the wire types in `orkestr-domain`; no
adapter exists yet, not even `mock`. This spec is the Phase 1 target. Where the
skeleton and the spec disagree, the Phase 1 owner reconciles them as follows
(wire types in `orkestr-domain` are already exported to TypeScript and change
only additively):

| Concept | As built | This spec | Resolution |
|---|---|---|---|
| `ProviderKind` (wire) | `OpenAi \| Anthropic \| Gemini \| DeepSeek \| Ollama \| OpenAiCompatible \| ClaudeCode \| Codex \| Mock \| Custom` (PascalCase) | adapter ids `openai`, `anthropic`, …, `mock` | Both: the snake_case names are module/adapter ids, the enum is the wire value (`../architecture/data-model.md` §1.2). `Custom` carries no string yet |
| `ProviderId` | UUID v7 newtype | "UUID v7 newtype" (§1) — but ADR-0020 says slug | Spec matches the code; ADR-0020's slug decision is open (ADR-0020 notes) |
| `ModelRef` | `{ providerId, model }` | `{ providerId, modelId }` | Use the built field name `model` |
| `Provider` (wire) | `{ id, kind, name, enabled, baseUrl, authMethod, credentialRefId, isLocal, defaultModel, settings, createdAt, updatedAt }` | config with `credentialSecretRefId` (protocol §8.13) | `credentialRefId` (a `CredentialRefId` pointing at a `ProviderCredentialReference`) is the built name |
| Model list item | `orkestr_domain::Model { providerId, id, displayName, contextWindow, maxOutputTokens, capabilities: ModelCapabilities, pricing, isLocal }` | `ModelInfo` (§3.1) with latency class, deprecation, capability overrides | Keep `Model` as the wire type; `ModelInfo` may exist inside the crate and map onto it, or its extra fields are added to `Model` additively |
| Capabilities | `ModelCapabilities { tools, vision, structuredOutput, streaming, reasoning, codingAgent }` (booleans) | `ProviderCapabilities` with enums | The rich struct is crate-internal; the wire keeps booleans plus additive fields |
| Pricing | `ModelPricing { inputPerMtokUsd, outputPerMtokUsd, cachedInputPerMtokUsd }` (`f64`, `cost()` helper) | §8 | Matches in substance |
| `ProviderDescriptor` | `{ kind, display_name, is_local, capabilities: ModelCapabilities, max_context_tokens, coding_agent }` | `{ kind, display_name, locality, auth_modes, capabilities, default_models, cost_model, privacy }` | Extend the built struct (crate-internal, not on the wire) |
| Auth state | `orkestr_domain::AuthState { Unconfigured, Unverified, Authenticated, Invalid }` (wire) | rich `AuthState { Ready, NotConfigured, NotInstalled, LoginRequired, Invalid, Unreachable, Unknown }` | The name `AuthState` is taken by the wire enum: introduce the rich type under another name (e.g. `AuthStatus`) and map it onto the 4-variant wire enum (+ `detail` text in `ProviderStatusChanged`) |
| Auth modes | `orkestr_domain::AuthMethod { None, ApiKey, OAuth, CliSession, CloudCredentials, LocalEndpoint, CustomHeaders }` | `AuthMode` with data | `AuthMethod` is the wire tag; the data-carrying `AuthMode` stays crate-internal |
| Health | `orkestr_domain::ProviderHealth { Unknown, Healthy, Degraded, RateLimited, Down }` | `HealthReport` | `health()` returns `ProviderHealth` today; a richer `HealthReport` may wrap it |
| `ChatEvent` | `TextDelta(String) \| ToolCall{id, name, arguments} \| Usage{input_tokens, output_tokens} \| Done \| Error(ProviderError)` | §3.4 (+ `StopReason`, `Usage` struct, content parts) | Extend additively; keep the variant names |
| `ProviderError` | `RateLimited{retry_after_secs} \| QuotaExceeded \| Auth(String) \| Network(String) \| BadRequest(String) \| Server(String) \| Cancelled \| Unimplemented(String)` | §4 with structured reasons | Add the structured fields; keep the contract §7 class names |
| Coding agents | `run_coding_agent(CodingAgentRequest{prompt, cwd, model, max_turns}, cancel) -> ChatStream` | `-> CodingAgentStream` of `CodingAgentEvent` | Switch to the dedicated stream type when the CLI adapters land |
| `id()` | returns `ProviderId` by value | returns `&ProviderId` | Either is fine (`ProviderId` is `Copy`) |
| Registry, router, limits, fallback, mock | not built | §5, §7, §9, §10 | Phase 1 |

### 0.1 Phase 1 implementation (branch `phase1/providers`)

Built in `crates/providers` (modules `types`, `adapter`, `adapters/{openai,anthropic,gemini,ollama,cli,claude_code,codex,mock}`,
`http`, `registry`, `router`, `fallback`, `limits`, `pricing`, `catalog`, `factory`, `store`, `credentials`):

- `ChatEvent` keeps the contract names: `TextDelta(String)`, `ToolCall{id,name,arguments}`, `Usage(Usage)`,
  `Done{stop_reason}`, `Error(ProviderError)`, plus `Switched{from,to,reason}` emitted by the fallback executor.
- `ProviderError` keeps the seven classes with string detail (`QuotaExceeded(String)`), plus `Unsupported`
  and `Unimplemented`. Structured sub-reasons (`LimitScope`, `AuthFailure`, …) are **not** built yet.
- Rich auth state is `AuthStatus` (crate) mapped onto the wire `AuthState` + detail; live status is the
  new wire type `orkestr_domain::ProviderStatus`. `run_coding_agent` returns `CodingAgentStream`.
- Wire methods: `providers.{list,get,create,update,delete,setCredential,clearCredential,health,models,test,kinds,getManager,setManager,getRouting,setRouting}`;
  Manager (§5) and routing (§7) settings are `orkestr_domain::{ManagerConfig, RoutingConfig}`.
- Not built yet: per-call permission bridge for Claude Code (`--permission-prompt-tool` MCP server — the
  caller passes a static `CliToolPolicy` that never pre-approves `Bash`; every reported CLI command is
  classified after the fact by the dangerous-action guard and a Dangerous/Forbidden one stops the run),
  Codex session resume, per-(provider, model) concurrency
  semaphores, opaque thinking-block echo for Anthropic extended thinking with tools (thinking is only
  enabled without tools), prompt-caching hints, benchmark-weighted routing (Phase 2).

RPC names follow `../api/protocol.md` §8.13/§8.27 (`providers.delete`,
`providers.health`; the code also reserves `providers.create`,
`providers.update`, `providers.setCredential`, which §8.27 maps).

---

## 1. Concepts

| Term | Meaning |
|---|---|
| `ProviderKind` | Adapter implementation: `openai`, `anthropic`, `gemini`, `deepseek`, `ollama`, `openai_compatible`, `claude_code`, `codex`, `mock`. Closed enum in Phase 1; plugin-supplied kinds (§48) are Phase 7 and use `Custom(String)`. |
| `ProviderId` | A **configured instance** of a kind (UUID v7 newtype in `orkestr-domain`). A user may have two `openai_compatible` instances (e.g. LM Studio and a company gateway). |
| `ModelRef` | `{ providerId, model }` (`orkestr_domain::ModelRef`) — the only way the rest of the system names a model. |
| Descriptor | Static-ish self-description (capabilities, context, local/cloud, cost model). |
| Auth state | Dynamic: is the provider usable right now and how it is authenticated. |
| Limit state | Dynamic: rate-limit / quota / circuit-breaker status (owned by the registry, fed by adapters). |

Two execution styles exist and are both first-class:

1. **Chat style** (`chat`) — Orkestr owns the tool loop (agent-runtime). Used for
   all HTTP providers and `mock`.
2. **Coding-agent style** (`run_coding_agent`) — an official CLI (Claude Code,
   Codex) owns its inner tool loop inside a worktree; Orkestr supervises,
   constrains and observes it. See §6.

---

## 2. Trait (Rust, object-safe)

```rust
// crates/providers/src/adapter.rs
use async_trait::async_trait;
use futures::stream::BoxStream;
use tokio_util::sync::CancellationToken;

pub type ChatStream = BoxStream<'static, ChatEvent>;
pub type CodingAgentStream = BoxStream<'static, CodingAgentEvent>;

#[async_trait]
pub trait ProviderAdapter: Send + Sync + 'static {
    /// Configured instance id (stable across restarts).
    fn id(&self) -> &ProviderId;

    /// Capabilities/context/local/cost model. Cheap, no I/O. May change only
    /// after `refresh()` (e.g. model list discovered) — registry re-reads it.
    fn descriptor(&self) -> ProviderDescriptor;

    /// Current auth state. MAY do cheap I/O (keyring lookup, `--version`
    /// probe) but MUST NOT spend tokens. Must complete < 3 s (registry
    /// applies a timeout and reports `AuthState::Unknown` on timeout).
    async fn auth_state(&self) -> AuthState;

    /// Models available to *this* credential. Cached by the registry (TTL 10 min).
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>;

    /// Liveness/latency probe without spending tokens where the API allows it
    /// (e.g. GET /models). Never invoked more than once per 30 s per instance.
    async fn health(&self) -> HealthReport;

    /// Streaming chat completion. The returned stream MUST end with exactly one
    /// terminal item: `ChatEvent::Done` or `ChatEvent::Error`. Pre-flight
    /// failures (bad config, auth missing) are returned as `Err` instead.
    async fn chat(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError>;

    /// Official CLI coding agents only. Default: unsupported.
    async fn run_coding_agent(
        &self,
        _req: CodingAgentRequest,
        _cancel: CancellationToken,
    ) -> Result<CodingAgentStream, ProviderError> {
        Err(ProviderError::bad_request(BadRequestReason::Unsupported,
            "this provider does not offer a coding-agent mode"))
    }

    /// Re-read config/credentials after the user edited them (hot-swap §9).
    async fn refresh(&self) -> Result<(), ProviderError> { Ok(()) }
}
```

Implementation rules:

- Adapters are **stateless with respect to conversations**. All conversation
  state lives in `ChatRequest` (built by agent-runtime + context engine). The only
  exception is the CLI session id for resumable coding agents (§6.4), which is
  returned to the caller and passed back explicitly.
- Adapters never touch SQLite, the event bus, the permission engine or the file
  system outside their documented needs. They return data; the registry/runtime
  emits events (`ProviderStatusChanged`, `ProviderRateLimited`, `ProviderFallback`).
- HTTP: one shared `reqwest::Client` (rustls, HTTP/2, connect timeout 10 s,
  no global request timeout — streams are governed by idle timeout, §5.4).
- Secrets: adapters receive a `SecretReference` and resolve it via
  `orkestr-security::SecretStore` at call time; the plaintext lives only in the
  request builder's stack frame and is wrapped in `secrecy::SecretString`.
  `Debug`/`Display` of every request type redacts headers named
  `authorization`, `x-api-key`, `x-goog-api-key`, `api-key`, and any custom header
  flagged `secret: true`.
- Cancellation: on `cancel.cancelled()` the adapter drops the HTTP response /
  kills the CLI process tree (via `orkestr-platform` process API) and emits
  `ChatEvent::Error(ProviderError::Cancelled)` as the terminal item within 2 s.

---

## 3. Data types

All types live in `orkestr-providers::types` unless they cross the wire, in which
case they are defined in `orkestr-protocol::methods::providers` (camelCase, `TS`).

### 3.1 Descriptor & capabilities

```rust
pub struct ProviderDescriptor {
    pub kind: ProviderKind,
    pub display_name: String,          // "Mock (offline demo)" for mock — mandatory label
    pub locality: Locality,            // Local | Cloud | SelfHosted
    pub auth_modes: Vec<AuthMode>,     // supported; the active one is in AuthState
    pub capabilities: ProviderCapabilities,
    pub default_models: Vec<ModelInfo>,// static fallback when list_models fails
    pub cost_model: CostModelKind,      // see §8
    pub privacy: PrivacyTraits,        // for §56 policy evaluation
}

pub struct ProviderCapabilities {
    pub streaming: bool,
    pub tool_calling: ToolCallingSupport,  // None | Native | NativeParallel
    pub structured_output: StructuredOutput, // None | JsonMode | JsonSchema
    pub vision: bool,
    pub system_prompt: bool,
    pub prompt_caching: bool,
    pub reasoning_control: ReasoningControl, // None | Effort{levels} | BudgetTokens
    pub coding_agent: Option<CodingAgentCapabilities>, // Some only for CLI kinds
    pub max_concurrency_hint: Option<u32>, // e.g. Ollama: 1 per loaded model
}

pub struct ModelInfo {
    pub id: String,                    // provider-native id
    pub display_name: String,
    pub context_window: u32,           // tokens
    pub max_output_tokens: u32,
    pub capabilities_override: Option<ProviderCapabilities>, // per-model deltas
    pub pricing: Option<ModelPricing>, // None = unknown / subscription
    pub latency_class: LatencyClass,   // Fast | Standard | Slow (benchmarks refine §22)
    pub deprecated: bool,
}

pub struct PrivacyTraits {
    pub data_leaves_machine: bool,     // false for Ollama/local, mock
    pub provider_region: Option<String>,
    pub training_opt_out_known: bool,  // informational only
}
```

Model capabilities = provider capabilities overridden field-by-field by
`capabilities_override`. The router (§7) reads the effective set.

### 3.2 Auth

```rust
pub enum AuthMode {
    ApiKey { secret: SecretReference },
    CustomHeaders { headers: Vec<HeaderSpec> },          // values may be SecretReference
    CliSession { binary: CliBinary },                    // official CLI, its own login
    OfficialOAuth,                                       // reserved; only where the vendor publishes it for 3rd parties
    CloudCredentials { profile: String },                // Bedrock/Vertex/Azure — Phase 7
    LocalEndpoint { base_url: Url },                     // Ollama, LM Studio...
    None,                                                // mock
}

pub enum AuthState {
    Ready { mode: AuthModeTag, account_label: Option<String> }, // label is user-visible, never a token
    NotConfigured,                         // no key / endpoint set
    NotInstalled { binary: String, hint: String }, // CLI missing
    LoginRequired { how: LoginInstruction },// CLI installed but not logged in
    Invalid { reason: String },            // 401/403 seen recently
    Unreachable { reason: String },        // local endpoint down
    Unknown,                               // probe timed out / not yet probed
}
```

### 3.3 Per-provider auth matrix

| Kind | Modes (Phase) | How auth is obtained | Probe used by `auth_state()` |
|---|---|---|---|
| `openai` | ApiKey (P1) | User pastes key → `SecretStore`. Header `Authorization: Bearer`. Optional `OpenAI-Organization`/`OpenAI-Project` headers. | Keyring entry exists → `Ready`; `health()` = `GET /v1/models`. |
| `anthropic` | ApiKey (P1) | Key in `SecretStore`, header `x-api-key`, `anthropic-version` pinned in adapter. | Same; `GET /v1/models`. |
| `gemini` | ApiKey (P1); CloudCredentials/Vertex (P7) | `x-goog-api-key` header (never as URL query, so it cannot leak into logs). | `GET /v1beta/models`. |
| `deepseek` | ApiKey (P1) | OpenAI-compatible wire, Bearer. Implemented by reusing the `openai_compatible` codec with a fixed base URL. | `GET /models`. |
| `ollama` | LocalEndpoint (P1) | Default `http://localhost:11434` (configurable; remote Ollama requires explicit opt-in and is then `Locality::SelfHosted`). | `GET /api/tags` (also = `list_models`). |
| `openai_compatible` | ApiKey / CustomHeaders / LocalEndpoint (P1) | Base URL + optional key + custom headers. | `GET {base}/models`, tolerate 404 (→ `Ready` with static models). |
| `claude_code` | CliSession (P1) | **Official `claude` CLI only.** The user logs in with the CLI's own flow (Orkestr opens an owned terminal running the CLI's documented login command so the user completes it themselves). Orkestr never reads, copies, parses or forwards the CLI's credential files or keychain entries, never scrapes cookies, never proxies the session. | `claude --version` → installed. Login state is reported as `Unknown` until the first real run returns success or an auth error, unless the installed CLI version exposes a documented non-token-spending status command (detected by the Phase 0 spike, not assumed). |
| `codex` | CliSession (P1) | **Official `codex` CLI only**, logged in by the user via the CLI (ChatGPT account sign-in or the CLI's own API-key login). Same prohibitions as above (e.g. never read `~/.codex/auth.json`). | `codex --version`; `codex login status` if supported by the detected version (spike-verified). |
| `mock` | None (P1) | — | Always `Ready`. |

Subscription vs API billing (§4) are **different provider instances**: "Claude
(Claude Code subscription)" and "Anthropic API" appear as two providers with
different cost models. The UI (§55 onboarding) offers `[Claude Code Login]` and
`[Anthropic API Key]` as separate buttons.

CLI binary discovery goes through `orkestr-platform` (PATH lookup incl. Windows
`.cmd`/`.exe`/`.ps1` shims created by npm; user can pin an absolute path). The
process is spawned **without a shell** (no `cmd /c`), with an explicit argument
vector — never string concatenation.

### 3.4 Chat request & events

```rust
pub struct ChatRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,           // role: User | Assistant | Tool
    pub tools: Vec<ToolSpec>,                 // { name, description, input_schema: JSON Schema }
    pub tool_choice: ToolChoice,              // Auto | None | Required | Specific(name)
    pub response_format: ResponseFormat,      // Text | JsonObject | JsonSchema(Value)
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub reasoning: Option<ReasoningLevel>,    // Low | Medium | High — mapped per provider, ignored if unsupported
    pub stop: Vec<String>,
    pub cache_hints: Vec<CacheHint>,          // prompt-caching breakpoints; ignored if unsupported
    pub metadata: RequestMetadata,            // correlationId, projectId, taskId, agentId (NOT sent to provider except opaque user id hash where APIs ask for one)
}

pub enum ContentPart { Text(String), Image { media_type: String, data: ObjectRef }, ToolResult { call_id: String, content: String, is_error: bool }, ToolUse { call_id: String, name: String, arguments: serde_json::Value }, Opaque(OpaqueBlock) }

/// Provider-private blocks that MUST be echoed back within the same tool loop
/// (e.g. Anthropic signed thinking blocks when extended thinking + tools are used).
/// In-memory only: never persisted, never emitted on the event bus, never shown.
pub struct OpaqueBlock { pub provider: ProviderKind, pub bytes: bytes::Bytes }

pub enum ChatEvent {
    TextDelta { text: String },
    ToolCall { id: String, name: String, arguments: serde_json::Value }, // complete, adapter-accumulated
    Usage(Usage),               // may arrive more than once; last value wins (cumulative)
    Done { stop_reason: StopReason, opaque: Vec<OpaqueBlock> },
    Error(ProviderError),
}

pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,     // count only (billing); content is never kept
    pub reported_cost_usd: Option<f64>, // when the provider/CLI reports it directly
    pub estimated: bool,           // true when counts are heuristic (provider reported none)
}

pub enum StopReason { EndTurn, ToolUse, MaxTokens, StopSequence, ContentFilter, Other(String) }
```

Normalisation rules every adapter must satisfy (verified by the shared
conformance test suite `crates/providers/tests/conformance.rs`, run against
`mock` and recorded HTTP fixtures):

1. Tool-call arguments are emitted **once, complete, parsed** as JSON. If the model
   produced invalid JSON, the adapter emits `ToolCall` with
   `arguments = {"__invalid_json": "<raw>"}`; the agent runtime returns a tool error
   to the model (it does not crash the loop).
2. Text deltas are UTF-8 safe (never split a code point).
3. Reasoning / thinking content (Anthropic `thinking`, OpenAI reasoning items,
   DeepSeek `reasoning_content`, Codex `reasoning` items, Gemini thought parts) is
   **dropped at the adapter boundary** — there is no `ChatEvent` variant for it
   (§1.14, §21). Only token counts survive in `Usage.reasoning_tokens`. Signed
   blocks required for continuity travel as `OpaqueBlock` (see above).
4. Exactly one terminal item (`Done` or `Error`); nothing after it.
5. `Usage` must be emitted before `Done` whenever the provider reports usage; if
   the provider does not (some OpenAI-compatible servers), the adapter estimates
   (chars/4 heuristic, or a tokenizer when one is bundled for that model family)
   and sets `Usage.estimated = true`; `orkestr-cost` records it as estimated.

### 3.5 Provider → ChatEvent mapping (summary)

| Provider wire | TextDelta | ToolCall | Usage | Done |
|---|---|---|---|---|
| OpenAI (Responses or Chat Completions SSE) | `output_text.delta` / `choices[].delta.content` | accumulate `function_call` arguments deltas by index/call id | final `usage` (`stream_options.include_usage=true`) | `response.completed` / `finish_reason` |
| Anthropic Messages SSE | `content_block_delta` type `text_delta` | `input_json_delta` accumulated per `tool_use` block, emitted at `content_block_stop` | `message_start.usage` + `message_delta.usage` | `message_stop` + `stop_reason` |
| Gemini `streamGenerateContent?alt=sse` | `parts[].text` (non-thought) | `parts[].functionCall` (complete) | `usageMetadata` | `finishReason` |
| DeepSeek / OpenAI-compatible | as OpenAI Chat Completions | as OpenAI | as OpenAI (if present) | `finish_reason` |
| Ollama `/api/chat` NDJSON | `message.content` | `message.tool_calls` | `prompt_eval_count`, `eval_count` | `done: true` |

Adapters pick the wire API version they implement and pin it in code; changing it
is a code change with fixture updates, not configuration.

---

## 4. Error taxonomy

The contract fixes seven classes. Detail travels in typed sub-reasons so the
classes never multiply.

```rust
#[derive(thiserror::Error, Debug, Clone, Serialize, Deserialize)]
pub enum ProviderError {
    #[error("rate limited")]       RateLimited { retry_after: Option<Duration>, scope: LimitScope },
    #[error("quota exceeded")]     QuotaExceeded { resets_at: Option<DateTime<Utc>>, scope: LimitScope },
    #[error("auth: {reason:?}")]   Auth { reason: AuthFailure },
    #[error("network: {message}")] Network { message: String, retryable: bool },
    #[error("bad request: {reason:?}")] BadRequest { reason: BadRequestReason, message: String },
    #[error("server: {status:?}")] Server { status: Option<u16>, message: String },
    #[error("cancelled")]          Cancelled,
}

pub enum LimitScope { Requests, InputTokens, OutputTokens, Tokens, Concurrency, Subscription, Unknown }
pub enum AuthFailure { MissingCredential, InvalidCredential, Forbidden, NotInstalled, LoginRequired, Expired }
pub enum BadRequestReason { ContextLengthExceeded, InvalidSchema, ContentFiltered, ModelNotFound, Unsupported, Other }
```

Classification table (adapters MUST map exactly like this):

| Observation | Class |
|---|---|
| HTTP 429 with rate-limit semantics; `retry-after`, `x-ratelimit-reset-*`, `anthropic-ratelimit-*-reset`, Gemini `RESOURCE_EXHAUSTED` with retry info | `RateLimited` |
| HTTP 429/402/403 with quota/billing semantics (`insufficient_quota`, credit balance too low, daily quota), CLI "usage limit reached / resets at …" | `QuotaExceeded` |
| 401; 403 permission errors; CLI exits with not-logged-in message; binary missing | `Auth` |
| DNS/TLS/connect/reset errors; stream idle timeout; Ollama not running | `Network` (`retryable = true` except TLS validation failures) |
| 400/404/413/422; context too long; schema rejected; safety refusal at API level | `BadRequest` |
| 5xx, 529 overloaded, malformed stream, CLI crash with non-zero exit and no classifiable message | `Server` |
| Our `CancellationToken` fired | `Cancelled` |

Mapping to wire error codes (contract §4): `RateLimited` → `rate_limited`;
`QuotaExceeded` → `provider_error` with `data.class = "quota_exceeded"` (NOT
`budget_exceeded`, which is reserved for Orkestr's own budgets, §8); all other
classes → `provider_error` with `data.class` = snake_case class name;
`Cancelled` is not an RPC error (the operation result reports cancellation).

Retry policy (applied by the router, never inside adapters, so behaviour is uniform):

| Class | Same-provider retry | Fallback (§7) |
|---|---|---|
| RateLimited | yes, after `retry_after` (or exp. backoff 1 s→30 s, jitter ±20 %), max 3, only if wait ≤ `max_rate_limit_wait` (default 60 s) | yes when wait exceeds limit or retries exhausted |
| QuotaExceeded | no | yes |
| Auth | no (mark instance `AuthState::Invalid`) | yes |
| Network (retryable) | yes, max 3, backoff 0.5 s→8 s | yes after exhaustion |
| BadRequest::ContextLengthExceeded | no; agent runtime may compact context once and retry | yes, to a model with larger window |
| BadRequest (other) | no | no (a different model would most likely fail too) — surfaced to agent as tool-loop failure |
| Server | yes, max 2 | yes |
| Cancelled | never | never |

A retry is only allowed if **no output was yet delivered to the caller** for the
current request (no TextDelta/ToolCall emitted). After partial output, failure is
surfaced and the agent runtime decides (normally: fallback with handoff).

---

## 5. Rate-limit and quota handling

### 5.1 Limit state (registry-owned)

```rust
pub struct LimitState {
    pub provider_id: ProviderId,
    pub model: Option<String>,
    pub status: LimitStatus,          // Ok | Throttled{until} | QuotaExhausted{resets_at} | CircuitOpen{until}
    pub remaining_requests: Option<u64>,
    pub remaining_tokens: Option<u64>,
    pub reset_at: Option<DateTime<Utc>>,
    pub in_flight: u32,
    pub max_concurrency: u32,
}
```

- Adapters report header-derived limits via `RateLimitObservation` attached to
  `Usage`/errors; the registry merges them (`ProviderRegistry::observe`).
- A per-`(ProviderId, model)` **concurrency semaphore** (`max_concurrency` default:
  cloud 4, Ollama 1, CLI 2 — user-configurable) gates `chat`/`run_coding_agent`.
  Waiting for a permit is visible as agent state `Waiting` (provider).
- Proactive throttling: when `remaining_* < 5 %` the router prefers alternatives.
- **Circuit breaker**: 5 consecutive `Server`/`Network` failures within 60 s ⇒
  `CircuitOpen` for 30 s (doubling up to 5 min); half-open allows one probe.
- Every transition of `LimitStatus` emits `ProviderRateLimited` (when entering
  Throttled/QuotaExhausted) or `ProviderStatusChanged` (everything else).

### 5.2 Task impact

A request blocked by limits for longer than `max_rate_limit_wait` and with no
fallback available moves the task `Running → Waiting` (reason
`provider_limit`), and back to `Running` when the limit resets (task-engine §3).

### 5.3 Subscription CLIs

Claude Code / Codex subscription limits are opaque. The adapter parses the
CLI's terminal result / error message for known limit phrases (patterns kept in
`crates/providers/src/claude_code/limits.rs` with tests, version-tagged) and maps
them to `QuotaExceeded{scope: Subscription, resets_at}`; unknown → `Server`.

### 5.4 Timeouts

- Connect: 10 s. First byte: 60 s (120 s for reasoning models). Stream idle
  (no bytes): 90 s → `Network{retryable: true}`.
- CLI coding agent: wall-clock limit from the agent budget (agent-runtime spec);
  idle (no JSON line) 10 min → killed, `Server`.

---

## 6. Coding-agent providers (Claude Code CLI, Codex CLI)

### 6.1 Principles

- **Official, documented CLI invocations only.** No private APIs, no token
  extraction, no session hijacking (§4, §62.6–7).
- The CLI runs **inside the agent's worktree** (cwd = worktree root) as a child
  process owned by Orkestr, spawned via `orkestr-platform` (Windows: Job Object so
  the whole tree dies on cancel; Unix: process group + SIGTERM→SIGKILL after 5 s).
- Environment is **allow-listed** (PATH, HOME/USERPROFILE, APPDATA/LOCALAPPDATA,
  TEMP/TMP, SystemRoot, LANG, and the CLI's own documented config vars). Project
  secrets are never injected unless an explicit `Secrets` grant exists for that
  agent and environment.
- A snapshot (`orkestr-snapshots`) of the worktree is taken before every run.

### 6.2 Request & events

```rust
pub struct CodingAgentRequest {
    pub model: Option<String>,
    pub prompt: String,                 // task + handoff, built by agent-runtime
    pub system_append: Option<String>,
    pub worktree: PathBuf,              // validated: inside <data_dir>/worktrees
    pub resume_session: Option<String>, // CLI session id from a previous run
    pub max_turns: Option<u32>,
    pub tool_policy: CliToolPolicy,     // derived from PermissionEngine grants
    pub permission_bridge: Option<PermissionBridgeEndpoint>, // §6.3
}

pub enum CodingAgentEvent {
    Chat(ChatEvent),                                 // assistant text, usage, done/error
    ToolStarted { call_id: String, tool: String, summary: String },   // → ToolCallStarted
    ToolFinished { call_id: String, ok: bool, summary: String },      // → ToolCallFinished
    FileChanged { path: PathBuf, change: FileChangeKind },            // → FileChanged
    Session { session_id: String },                  // persisted on AgentInstance for resume
}
```

### 6.3 Keeping the PermissionEngine in the loop

The contract (§6) requires every agent tool call to pass
`PermissionEngine::check`. For CLI agents whose inner loop we do not own:

| CLI | Mechanism | Coverage |
|---|---|---|
| Claude Code | Run non-interactively (`claude -p … --output-format stream-json --verbose`) with `--permission-prompt-tool mcp__orkestr__permission` and `--mcp-config` pointing at a **local Orkestr MCP server** (stdio, spawned per run, scoped to this agent). Every tool use the CLI wants to perform is sent to that tool, which calls `PermissionEngine::check` (and the approval flow) and answers allow/deny. Additionally `--allowedTools`/`--disallowedTools` encode static grants, `--max-turns` encodes the budget. | Per tool call (equivalent to chat style). |
| Codex | `codex exec --json -C <worktree> --sandbox workspace-write` (network off by default in that sandbox) with non-interactive approval policy. Codex's sandbox is the enforcement boundary; per-command gating by Orkestr is **not available**. | Coarse. |

Consequences:

- Codex coding-agent mode requires the agent to hold `Write` + `Terminal`
  grants for its worktree **and** a one-time explicit user opt-in per project
  ("Codex runs commands inside its own sandbox; Orkestr cannot approve each
  command"). `Network`, `Deploy`, `Secrets`, `Ssh`, `Docker` grants are never
  passed through. Codex `danger-full-access` sandbox mode is **hard-denied**.
- Post-run, Orkestr diffs the worktree, emits `FileChanged` events, re-runs the
  DangerousActionGuard over the CLI-reported executed commands for audit, and
  flags any write outside the worktree (detected via snapshot of project root
  metadata) as a security incident (`Notification` + task → `Failed`).
- Exact flag names/JSON event names vary by CLI version. The adapter detects the
  version (`--version`), keeps a **version-tagged mapping table**, and refuses to
  run on versions below the tested minimum (`BadRequest{reason: Unsupported}`
  with an upgrade hint; `auth_state()` reports `Invalid{reason: "cli version
  < minimum"}`). The Phase 0 spike records the minimum
  versions and the actual flag set; until then flags above are the design intent
  and **unverified**.

### 6.4 Stream mapping

| CLI JSON line | Orkestr event |
|---|---|
| Claude `system/init` | `Session{session_id}` |
| Claude `assistant` message text blocks | `Chat(TextDelta)` (whole block as one delta) |
| Claude `assistant` `tool_use` | `ToolStarted` |
| Claude `user` `tool_result` | `ToolFinished` |
| Claude `result` (`usage`, `total_cost_usd`, `is_error`, `subtype`) | `Chat(Usage)` + `Chat(Done)` / `Chat(Error)` |
| Codex `thread.started` | `Session` |
| Codex `item.*` `agent_message` | `Chat(TextDelta)` |
| Codex `item.*` `command_execution` / `file_change` / `mcp_tool_call` | `ToolStarted`/`ToolFinished`/`FileChanged` |
| Codex `item.*` `reasoning` | **dropped** (no-CoT rule) |
| Codex `turn.completed` (usage) / `turn.failed` / `error` | `Chat(Usage)` + `Done` / `Error` |

Unknown line types are logged at `debug` (redacted) and ignored, never fatal.

---

## 7. Routing & fallback (§5, §7, §56, §63.B)

### 7.1 Inputs

```rust
pub struct RouteRequest {
    pub purpose: RoutePurpose,            // Orchestration | AgentRole(Role) | Classification | Review | Consensus | Embedding(P2)
    pub required: RequiredCapabilities,   // tools, vision, json_schema, min_context, coding_agent
    pub preferred: Option<ModelRef>,      // agent template preferred model / manual pick
    pub fallback_chain: Vec<ModelRef>,    // template/manager configured
    pub privacy: PrivacyPolicy,           // §56: CloudAllowed | SelectedProviders(set) | LocalOnly | SelfHostedOnly
    pub data_class: DataClass,            // Public | Internal | Confidential | Secret
    pub budget: BudgetView,               // remaining $ for task/agent/project (orkestr-cost)
    pub exclude: Vec<ModelRef>,           // already failed in this attempt chain
}
```

### 7.2 Algorithm (deterministic; Phase 1 = rules, Phase 2 adds benchmark scoring)

1. **Candidates** = `[preferred] ++ fallback_chain` (manual mode) or, in auto
   mode, all models of `Ready` providers.
2. **Hard filters** (drop): privacy/data-class violation (e.g. `LocalOnly` keeps
   only `Locality::Local`; `Secret` data never leaves the machine unless the
   project policy explicitly allows a named provider); missing required
   capability; context window < estimated prompt + `max_output_tokens`;
   `LimitStatus` ∉ {Ok, Throttled with wait ≤ max}; `AuthState` ≠ Ready;
   estimated cost > remaining budget (unless budget is `Unrestricted`, §22);
   in `exclude`.
3. **Order**: manual mode keeps chain order. Auto mode sorts by score
   `w_q·quality(purpose) + w_l·latency + w_c·cost + w_h·health` where
   quality comes from the user's routing table (§7 examples:
   Architecture→Claude, Backend→Codex, UI→Gemini, Classification→Ollama) in
   Phase 1 and from `orkestr-cost` benchmarks (§22) in Phase 2.
4. Return the ordered list; the caller tries the head.

`Security` purpose with consensus (§8) is Phase 2 and is implemented in the
orchestrator by issuing multiple independent routes, not in the router.

### 7.3 Fallback with structured handoff (§7, §57)

When an attempt fails with a fallback-eligible class (§4 table):

1. Router picks the next candidate (adding the failed one to `exclude`).
2. If the failed attempt produced partial work, the agent runtime builds a
   **Handoff** (`docs/agents/agent-runtime-spec.md` §8) from persisted turns,
   tool results and worktree diff — never the raw chat transcript of the old
   model, never provider-opaque blocks (they are provider-bound and are discarded).
3. Emit `ProviderFallback` event:

```json
{ "kind": "ProviderFallback", "taskId": "...", "agentId": "...",
  "payload": { "from": {"providerId":"...","modelId":"..."},
               "to":   {"providerId":"...","modelId":"..."},
               "reason": "quota_exceeded", "detail": "resets at 2026-10-02T18:00:00Z",
               "handoffId": "...", "attempt": 2 } }
```

4. The UI shows it as a user-visible notice (§7 "явное событие пользователю").
5. If the chain is exhausted: task `Running → Waiting` (reason
   `no_provider_available`) + `Notification`; it resumes automatically when a
   limit resets or the user picks a model.

Fallback across execution styles (CLI coding agent → chat-style API model) is
allowed: the handoff is style-independent, and the worktree is the shared state.

---

## 8. Cost model (§22)

```rust
pub enum CostModelKind {
    PerToken,                 // ModelPricing applies
    Subscription { plan_label: Option<String> }, // $ marginal = 0, usage tracked, limit opaque
    Local,                    // $0; track tokens + duration + energy proxy (GPU time) for benchmarks
    Unknown,                  // custom endpoints without pricing
}

pub struct ModelPricing {               // USD per 1M tokens
    pub input: f64,
    pub output: f64,
    pub cached_input: Option<f64>,
    pub cache_write: Option<f64>,
    pub reasoning_billed_as_output: bool,
    pub effective_from: NaiveDate,
}
```

- Pricing tables live in `orkestr-cost` (versioned JSON resource), **not** hard-coded
  in adapters; adapters may supply defaults via `ModelInfo.pricing`, users can
  override per model. Estimated cost is always labelled "estimated".
- `UsageRecord` (§51) per provider call: `{ providerId, modelId, projectId,
  taskId, agentId, usage, costUsd, costSource: Computed|Reported|Estimated,
  latencyMs, outcome }`.
- Pre-flight estimate (used by the router's budget filter): `input_estimate ×
  input_price + max_output_tokens × output_price`.
- When reported (`reported_cost_usd`, e.g. Claude Code `total_cost_usd`) and the
  instance is `Subscription`, it is stored as *API-equivalent* value for
  benchmarking but excluded from spend totals.
- Crossing 50/80/100 % of any budget emits `BudgetThreshold`; 100 % makes the
  router filter out paid models for that scope (`budget_exceeded` RPC error on
  explicit requests).

---

## 9. Registry & hot-swap (§4)

```rust
pub struct ProviderRegistry {
    adapters: arc_swap::ArcSwap<HashMap<ProviderId, Arc<dyn ProviderAdapter>>>,
    limits: DashMap<(ProviderId, Option<String>), LimitState>,
    // ...
}
impl ProviderRegistry {
    pub fn get(&self, id: &ProviderId) -> Option<Arc<dyn ProviderAdapter>>;
    pub async fn upsert(&self, cfg: ProviderConfig) -> Result<(), ProviderError>; // build → health → swap
    pub async fn remove(&self, id: &ProviderId) -> Result<(), ProviderError>;
    pub fn route(&self, req: &RouteRequest) -> Vec<ModelRef>;
    pub fn observe(&self, id: &ProviderId, obs: RateLimitObservation);
}
```

- **Hot-swap**: `upsert` constructs the new adapter, runs `auth_state` + `health`
  (non-fatal), then atomically swaps the map. In-flight streams hold their own
  `Arc` and finish on the old instance; new requests use the new one. No restart.
- `remove` with in-flight work: marks instance `Disabled` (router excludes it) and
  lets in-flight finish; `force` cancels them (their tasks go through fallback).
- Provider configs (non-secret) persist in table `providers`; credentials only as
  `SecretReference` (§24, contract §6).
- Background health loop: every 60 s for providers used in the last hour, every
  10 min otherwise; never for CLIs (version probe only on demand).

RPC surface (area `providers`, Phase 1): `providers.list`, `providers.get`,
`providers.upsert`, `providers.delete`, `providers.health` (health + auth probe),
`providers.models`, `providers.limits`, `providers.setRouting`. Credentials are
written via `secrets.put` (security area) and referenced by id.

---

## 10. Mock provider

- Kind `mock`, display name exactly **"Mock (offline demo)"**, `Locality::Local`,
  `CostModelKind::Local`.
- Deterministic: response is a pure function of (model id, last user message,
  tool list). Scripted scenarios selectable by model id: `mock-echo`,
  `mock-tool-roundtrip` (calls the first declared tool once with schema-valid
  args), `mock-rate-limit` (fails with `RateLimited{retry_after: 1s}` on the first
  call per request id), `mock-quota`, `mock-slow` (5 s stream). Used by
  conformance, router, fallback and agent-runtime tests and for the offline demo.

---

## 11. Phasing

| Item | Phase |
|---|---|
| Trait, types, registry, router (rules), fallback + handoff, error taxonomy, limits/circuit breaker, mock | 1 |
| Adapters: openai, anthropic, ollama, openai_compatible, deepseek, gemini | 1 |
| Adapters: claude_code, codex (after Phase 0 spike confirms flags) | 1 |
| Benchmark-weighted auto-routing, consensus routing, embeddings | 2 |
| Plugin-provided adapters, Bedrock/Vertex/Azure | 7 |
| Remote node provider execution (route to a provider on another node) | 6 |

## 12. Test obligations

- Conformance suite (all adapters, fixtures + mock): stream invariants of §3.4.
- Error classification table §4: one test per row per HTTP adapter (fixtures).
- Router: privacy filters, budget filter, chain order, exclusion, exhaustion.
- Fallback: partial output → no retry; handoff built; `ProviderFallback` emitted.
- Redaction: `format!("{:?}", req)` never contains a key (property test).
- CLI adapters: JSON-line parser over recorded transcripts; unknown lines ignored;
  reasoning items dropped; cancel kills process tree (integration test, Windows + Unix).
