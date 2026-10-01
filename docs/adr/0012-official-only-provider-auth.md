# ADR-0012: Official-only provider authentication; CLI coding agents (Claude Code, Codex)

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 1
- **Spec:** §4, §5, §7, §55, §62.6, §62.7, §64 (no provider lock-in), §67 ("Claude replaced by GPT without rewriting Agent Runtime")
- **Review:** RR-A9, RR-C8, RR-G16
- **Spikes:** SP-06 (Claude Code), SP-07 (Codex), SP-08 (auth & subscription constraints), SP-09 (Ollama)
- **Risks:** R-06, R-07, R-18

## Context

§4 lists the auth modes (official OAuth/sign-in, API key, authenticated CLI session, official SDK, cloud credentials, local endpoint, custom headers) and forbids cookie scraping, session hijacking and extracting private tokens of other applications. For OpenAI/Codex and Claude Code account-based usage, only officially supported mechanisms are allowed. Subscription access and API billing are different ways to connect.

## Decision drivers

1. Legal and ToS compliance (§4, §62.6, §62.7) comes before features.
2. Orkestr never holds or reads third-party subscription tokens.
3. The provider abstraction hides every difference from the Agent Runtime (§60.5, §67).
4. Every agent tool call is gated (contract §6), including those of CLI agents (RR-C8).

## Considered options

| Option | Verdict |
|---|---|
| **A. HTTP adapters with user API keys (SecretStore) + CLI adapters that spawn the user's installed, user-logged-in official CLI (`claude`, `codex`) in non-interactive mode** | **Chosen** |
| B. Read `~/.claude/.credentials.json` or `~/.codex/auth.json` and call the APIs directly with the subscription tokens | **Forbidden** (§4, ToS) |
| C. Embed an OAuth client that impersonates the CLI | **Forbidden** |
| D. API keys only | Rejected. §4 and §55 require subscription/CLI sign-in where it is officially supported |

## Decision

### `ProviderAdapter` (contract §7, in `orkestr-providers`)

```rust
#[async_trait::async_trait]
pub trait ProviderAdapter: Send + Sync + 'static {
    fn id(&self) -> &ProviderId;
    fn descriptor(&self) -> &ProviderDescriptor;
    async fn auth_state(&self) -> AuthState;          // NotConfigured | Configured | Authenticated{account_hint?} | Expired | Error{msg} | Unknown
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>;
    async fn health(&self) -> ProviderHealth;          // { status: Up|Degraded|Down, latencyMs?, rateLimit?: RateLimitState }
    async fn chat(&self, req: ChatRequest, cancel: CancellationToken)
        -> Result<BoxStream<'static, Result<ChatEvent, ProviderError>>, ProviderError>;
    /// Only for descriptor.kind == CodingAgentCli. Default: Err(ProviderError::Unsupported).
    async fn run_coding_agent(&self, req: CodingAgentRequest, gate: Arc<dyn ToolGate>, cancel: CancellationToken)
        -> Result<BoxStream<'static, Result<CodingAgentEvent, ProviderError>>, ProviderError> { Err(ProviderError::Unsupported) }
}

pub struct ProviderDescriptor {
    pub kind: ProviderKind,                 // HttpApi | CodingAgentCli | Local | Mock
    pub display_name: String,               // "Mock (offline demo)" for mock
    pub auth_modes: Vec<AuthMode>,          // ApiKey | CliSession | OAuthOfficial | CloudCredentials | LocalEndpoint | CustomHeaders
    pub capabilities: ModelCapabilities,    // tools, vision, structuredOutput, streaming, codingAgent
    pub max_context_tokens: u32,
    pub local: bool,                        // privacy routing (RR-G5)
    pub max_concurrency: u32,               // RR-G16
    pub cost_model: CostModel,              // PerToken{input_micros_per_mtok, output_…} | Subscription | Free
}

pub enum ChatEvent { TextDelta(String), ToolCall(ToolCallRequest), Usage(Usage), Done{ stop: StopReason }, Error(ProviderError) }
// No reasoning/thinking variant by design (RR-A5).

pub enum ProviderError { RateLimited{ retry_after: Option<Duration> }, QuotaExceeded, Auth(String), Network(String),
                         BadRequest(String), Server(String), Cancelled, Unsupported }

/// Callback the CLI adapter uses to gate each tool call of the CLI agent.
#[async_trait::async_trait]
pub trait ToolGate: Send + Sync {
    async fn authorize(&self, call: &ExternalToolCall) -> GateDecision; // Allow | Deny{reason}; may suspend for user approval
}
pub enum CodingAgentEvent { Text(String), ToolStarted(ExternalToolCall), ToolFinished{ id: String, ok: bool, summary: String },
                            FileChanged(PathBuf), Usage(Usage), Done{ summary: String, session_ref: Option<String> } }
```

### Auth matrix

| Provider | Phase 1 modes | Notes |
|---|---|---|
| OpenAI API | API key (SecretStore) | `reqwest`+rustls, SSE streaming |
| OpenAI Codex | CLI session (`codex login`, user-run in the Orkestr terminal) | `codex exec --json` |
| Anthropic API | API key | Messages API, SSE |
| Claude Code | CLI session (`claude` login, user-run) | `claude -p --output-format stream-json` |
| Gemini | API key | |
| DeepSeek | API key (OpenAI-compatible) | |
| Ollama | Local endpoint `http://localhost:11434` (configurable) | `descriptor.local = true`; models from `/api/tags` |
| OpenAI-compatible / custom | Base URL + API key or custom headers (headers stored as secrets) | |
| Mock | none | deterministic, labelled "Mock (offline demo)" |

### CLI adapters: rules

1. **Detect, don't extract.** Resolve the executable through `PlatformAdapter::shell().which`. Auth state comes **only** from documented CLI commands or exit codes (e.g. `codex login status`; for Claude Code, whatever status command SP-06 confirms, otherwise `Unknown` until the first run). Orkestr **never opens or parses** the CLI's credential files.
2. **Login is user-driven.** "Sign in" opens an Orkestr terminal running the CLI's own login command. Orkestr does not see the OAuth exchange.
3. **Invocation (verified in SP-06/SP-07; flags may change with CLI versions, so the adapter checks `--version` against a tested range):**
   - **Claude Code:** `claude -p "<prompt>" --output-format stream-json --verbose --permission-mode default --permission-prompt-tool mcp__orkestr__permission --mcp-config <tmp.json> [--allowedTools …] [--disallowedTools …] [--model …] [--resume <session>]`, cwd = the agent worktree. `<tmp.json>` registers the `orkestr` MCP stdio server (`orkestrd mcp-bridge --session <one-time token>`), whose `permission` tool calls `ToolGate::authorize`, which calls `PermissionEngine::check`.
   - **Codex:** `codex exec --json --sandbox workspace-write --cd <worktree> [--model …] "<prompt>"` (network kept off unless `Network` is granted; config override flags confirmed in SP-07). Codex `exec` has no per-call approval callback, so gating is **launch-time** (see 4).
4. **Gating levels (RR-C8)**, shown in the UI as a badge:
   - `perCall` (Claude Code via the permission-prompt tool): the full PermissionEngine applies.
   - `launchTime` (Codex): launching requires `Write`+`Terminal` on the worktree to be **Allow** in the effective mode. In `Ask` mode the launch is an approval. Orkestr takes a snapshot first (ADR-0021). The OS-level sandbox is the CLI's own `workspace-write`. Each reported tool event is audited and classified after the fact; a Dangerous classification raises a security finding and pauses the task.
   - If SP-06 shows that `perCall` cannot be guaranteed for some tool class, the adapter downgrades to `launchTime` for that class.
5. **Isolation:** the CLI process is spawned with a minimal env (PATH, HOME/USERPROFILE, locale, plus explicitly bound secrets), inside the platform's process-containment primitive (Job Object on Windows, process group elsewhere, ADR-0022), with stdout parsed line by line. The `stderr` tail is kept for errors.
6. **Error mapping:** CLI rate-limit or usage-limit messages and exit codes map to `RateLimited`/`QuotaExceeded`. This feeds the fallback chain (§7, §63B).
7. **No hidden reasoning stored:** thinking blocks in the stream are dropped. Only text, tool events, usage and the final summary are kept (RR-A5).

### Router and fallback (Phase 1 basic, Phase 2 scoring)

- `ProviderRouter::select(req: &RouteRequest) -> Result<Vec<ProviderId /* fallback chain */>, RouteError>`. It filters by privacy (`local` only for `localOnly`), capability (tools, vision, codingAgent), auth state, health, budget and concurrency.
- Phase 1 applies the manual chain from the template, Manager or project settings.
- On `RateLimited`/`QuotaExceeded`/`Down`: emit `ProviderFallback{from, to, reason}`, build a structured handoff (§57: task, concise context, files, decisions, unresolved issues, test state), and continue on the next provider. Phase 2 adds benchmark-based scoring (ADR-0023).

## Consequences

- **Positive:** compliant; providers stay hot-swappable (§4); replacing Claude with GPT is a template/model change (§67); CLI agents are usable without Orkestr touching their credentials.
- **Negative:** CLI flags and stream formats change between versions. The adapters pin a tested version range, run a `--version` check, and fail loudly with a `provider_error` explaining the version mismatch (never degraded silently). Contract tests use recorded JSONL fixtures.
- **Negative:** Codex runs with launch-time gating only. This is shown to the user, needs a snapshot, and is refused in FullAuto unless the user explicitly opts in per project (`providers.codex.allowFullAuto`).
- **Negative:** subscription usage limits are opaque, so cost tracking for subscription providers records usage (tokens, if reported) with `costModel = Subscription`, not dollars.

## Implementation notes (Phase 0 foundation, 2026-10-02)

Not implemented yet; no provider adapter (including `mock`) exists. `orkestr-providers` has the skeleton `ProviderAdapter` trait (`run_coding_agent` returns a `ChatStream` and defaults to `ProviderError::Unimplemented`). Wire enums as built: `ProviderKind`, `AuthMethod { None, ApiKey, OAuth, CliSession, CloudCredentials, LocalEndpoint, CustomHeaders }`, `AuthState { Unconfigured, Unverified, Authenticated, Invalid }`, `ProviderHealth { Unknown, Healthy, Degraded, RateLimited, Down }`. `ProviderId` is a UUID v7 (see ADR-0020 notes). Money is `f64` USD in `ModelPricing`/`UsageRecord`. SP-05…SP-09 have not run.
