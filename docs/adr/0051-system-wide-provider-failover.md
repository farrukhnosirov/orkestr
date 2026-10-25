# ADR-0051: System-wide AI source failover

Status: Accepted (wave 9, v1.2.0)

## Context
The user asked: "everywhere in the system, if a subscription or API key stops
working, switch immediately to the next one by priority". Orkestr already had
per-purpose routing, fallback chains (`chat_with_fallback`), a circuit breaker
and rate-limit / quota windows. But it had no single source order, CLI agents
failed over only on usage limits, and a failure in the middle of a stream
without a handoff hook ended the call.

## Decision
* **One priority list of AI sources.** Every provider instance is one source:
  each subscription (Claude Code, ChatGPT/Codex) and each API key (Anthropic,
  OpenAI, Gemini, Ollama, …). The list is stored in `RoutingConfig`
  (`sourcePriority`, `purposeSources`, `failover`; all additive with serde
  defaults). Providers missing from the stored list are appended in the
  default order: subscriptions, then cloud API keys, then local models, with
  Mock last. Each source has capabilities (chat, tools, vision, computer use,
  embeddings). Defaults come from its kind and the user can override them.
  A source can also be enabled or disabled.
* **Per-purpose overrides** (`manager`, `agents`, `reviews`, `voice`, `jarvis`,
  `cursor`, `detective`, `productManager`) replace the global order for that
  purpose. Otherwise a purpose uses the global order, filtered by the
  capability it needs. The function `purpose_group` maps routing purposes to
  these groups.
* **The registry applies it to every chain.** `ProviderRegistry::route` calls
  `failover::prioritize` after the existing router. Explicit picks stay at
  the head (preferred model, routing rule, caller fallbacks). Next comes one
  model per source, in priority order. Remaining routed models come after
  that. Sources in cooldown move to the end of the chain but stay in it. All
  callers already use `route` and then `chat`, so they all pick this up
  without changes.
* **One classification table.** `failover::classify(&ProviderError) ->
  FailureKind` covers these failures: auth invalid, usage limit, rate limit,
  quota, network, timeout, 5xx, model unavailable, CLI missing, CLI crashed
  and unsupported. All of them fail over. Bad requests, content-policy
  refusals and cancellation never fail over. `chat_with_fallback`, the
  `ProviderChain` runner and CLI agent runs all use this table.
* **Cooldown.** `limits::Cooldown` applies to auth, network, timeout, 5xx,
  model-unavailable and CLI failures. It starts at 30 s and doubles up to
  30 min. A successful call clears it, and so does a passing health probe
  (the health loop recovers sources automatically). Rate limits and quotas
  keep their own windows. When a subscription usage-limit message carries a
  reset time (`…|<unix>`), the quota window uses it.
* **Streaming.** A failure before the first token switches sources
  transparently. If output was already delivered, the existing handoff hook
  continues the work (agents use the structured handoff). When there is no
  hook, the next source restarts the answer and its system prompt gets a
  note. Consumers already discard partial output on `ChatEvent::Switched`.
* **Visibility.** Every switch emits `ProviderFallback`, and the registry adds
  `fromName` and `toName` to it. The toast reads «Claude Code недоступен (лимит)
  — переключился на ChatGPT». The event log keeps it, which serves as the
  audit trail. The registry keeps a log of the last 100 switches, available
  through `providers.sources`. The setting is in Settings → «Приоритет
  ИИ-источников».
* **Generic runner.** For calls that are not a plain streamed chat,
  `ProviderRegistry::chain(&RouteRequest) -> ProviderChain` and
  `ProviderChain::run(|SourceAttempt| async …)` try sources by priority, with
  the same classification, cooldown and events. `ProviderRegistry::complete`
  is a convenience for collected text.

## Consequences
* A user who pinned a model still gets automatic failover after it. Setting
  `failover: false` turns that off.
* Mock is always the last resort. Its scripted failure models are routed only
  when referenced explicitly.
