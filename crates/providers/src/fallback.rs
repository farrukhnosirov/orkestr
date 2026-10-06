//! Fallback chain executor (spec §4 retry policy, §7.3 structured handoff).
//!
//! Tries each model of a chain in order. A fallback-eligible failure
//! (RateLimited / QuotaExceeded / Auth / Network / Server / Unsupported) moves
//! to the next model, emitting `ProviderRateLimited` and `ProviderFallback`
//! events. A rate limit with a short retry-after is retried on the same model
//! first. When output was already delivered, the caller's [`HandoffHook`]
//! decides whether (and with which request) to continue — never silently.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use orkestr_domain::ModelRef;
use tokio_util::sync::CancellationToken;

use crate::adapter::ProviderAdapter;
use crate::http::{drive, Emitter};
use crate::types::{ChatEvent, ChatRequest, ChatStream, ProviderError, ToolCallPart};

/// Events the provider layer reports to the outside world (the core maps
/// them onto `ProviderStatusChanged` / `ProviderRateLimited` /
/// `ProviderFallback`).
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    /// Health / auth changed.
    StatusChanged {
        /// Provider.
        provider_id: orkestr_domain::ProviderId,
        /// Health.
        health: orkestr_domain::ProviderHealth,
        /// Wire auth state.
        auth_state: orkestr_domain::AuthState,
        /// Detail (never secret).
        detail: Option<String>,
    },
    /// A provider rate-limited us.
    RateLimited {
        /// Provider.
        provider_id: orkestr_domain::ProviderId,
        /// Model.
        model: Option<String>,
        /// Seconds until retry.
        retry_after_secs: Option<u64>,
    },
    /// The executor switched models.
    Fallback {
        /// Failed model.
        from: ModelRef,
        /// Next model.
        to: ModelRef,
        /// Error class.
        reason: String,
    },
}

/// Receives provider events.
pub trait ProviderEventSink: Send + Sync {
    /// Fire-and-forget.
    fn emit(&self, event: ProviderEvent);
}

/// Sink that drops everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl ProviderEventSink for NullSink {
    fn emit(&self, _event: ProviderEvent) {}
}

/// Outcome of one attempt (feeds circuit breaker, limits, latency).
#[derive(Debug, Clone, PartialEq)]
pub enum AttemptOutcome {
    /// Completed; time to first event.
    Success {
        /// Latency in ms.
        latency_ms: u64,
    },
    /// Failed.
    Failure(ProviderError),
}

/// Where the executor gets adapters and reports outcomes (the registry).
pub trait AdapterSource: Send + Sync {
    /// Adapter of a provider (None = removed / disabled).
    fn adapter(&self, provider: &orkestr_domain::ProviderId) -> Option<Arc<dyn ProviderAdapter>>;
    /// Whether an attempt on `model` may start now. The registry's circuit
    /// breaker admits a single probe while half-open; a refused attempt is
    /// skipped (fallback continues) and is not reported to [`Self::observe`].
    fn admit(&self, _model: &ModelRef) -> bool {
        true
    }
    /// Attempt outcome.
    fn observe(&self, model: &ModelRef, outcome: &AttemptOutcome);
    /// Event sink.
    fn emit(&self, event: ProviderEvent);
}

/// What the handoff hook sees.
#[derive(Debug, Clone, PartialEq)]
pub struct HandoffContext {
    /// Failed model.
    pub from: ModelRef,
    /// Next model.
    pub to: ModelRef,
    /// The failure.
    pub error: ProviderError,
    /// Text delivered before the failure.
    pub partial_text: String,
    /// Tool calls delivered before the failure.
    pub partial_tool_calls: Vec<ToolCallPart>,
    /// 1-based attempt number of the next model.
    pub attempt: u32,
}

impl HandoffContext {
    /// Whether the failed attempt had delivered output.
    #[must_use]
    pub fn had_output(&self) -> bool {
        !self.partial_text.is_empty() || !self.partial_tool_calls.is_empty()
    }
}

/// Builds the structured handoff for the next model (agent runtime, §57).
/// Return `None` to stop the chain and surface the error.
#[async_trait]
pub trait HandoffHook: Send + Sync {
    /// Called before every switch.
    async fn handoff(&self, ctx: &HandoffContext, request: ChatRequest) -> Option<ChatRequest>;
}

/// Executor options.
#[derive(Debug, Clone, Copy)]
pub struct FallbackOptions {
    /// Max wait for a same-model retry after a rate limit.
    pub max_rate_limit_wait: Duration,
    /// Same-model retries for rate limits (only before any output).
    pub same_model_retries: u32,
}

impl Default for FallbackOptions {
    fn default() -> Self {
        Self { max_rate_limit_wait: Duration::from_secs(5), same_model_retries: 1 }
    }
}

enum AttemptEnd {
    Done,
    Failed {
        error: ProviderError,
        text: String,
        calls: Vec<ToolCallPart>,
    },
    /// Not attempted (circuit half-open, probe in flight).
    Skipped {
        error: ProviderError,
    },
}

async fn attempt(
    source: &dyn AdapterSource,
    model: &ModelRef,
    request: &ChatRequest,
    em: &Emitter<ChatEvent>,
    cancel: &CancellationToken,
) -> Result<AttemptEnd, ProviderError> {
    if !source.admit(model) {
        return Ok(AttemptEnd::Skipped {
            error: ProviderError::Unsupported(format!(
                "provider {} is recovering (circuit half-open, a probe request is in flight)",
                model.provider_id
            )),
        });
    }
    let Some(adapter) = source.adapter(&model.provider_id) else {
        return Ok(AttemptEnd::Failed {
            error: ProviderError::Unsupported(format!("provider {} is not available", model.provider_id)),
            text: String::new(),
            calls: vec![],
        });
    };
    let mut req = request.clone();
    req.model.clone_from(&model.model);
    let start = Instant::now();
    let mut first_event: Option<u64> = None;
    let mut stream = match adapter.chat(req, cancel.child_token()).await {
        Ok(s) => s,
        Err(error) => return Ok(AttemptEnd::Failed { error, text: String::new(), calls: vec![] }),
    };
    let mut text = String::new();
    let mut calls = Vec::new();
    while let Some(ev) = stream.next().await {
        first_event.get_or_insert_with(|| start.elapsed().as_millis() as u64);
        match ev {
            ChatEvent::Error(error) => return Ok(AttemptEnd::Failed { error, text, calls }),
            ChatEvent::Done { .. } => {
                source.observe(model, &AttemptOutcome::Success { latency_ms: first_event.unwrap_or(0) });
                em.emit(ev).await?;
                return Ok(AttemptEnd::Done);
            }
            ChatEvent::TextDelta(ref t) => {
                text.push_str(t);
                em.emit(ev).await?;
            }
            ChatEvent::ToolCall { ref id, ref name, ref arguments } => {
                calls.push(ToolCallPart { id: id.clone(), name: name.clone(), arguments: arguments.clone() });
                em.emit(ev).await?;
            }
            other => em.emit(other).await?,
        }
    }
    Ok(AttemptEnd::Failed { error: ProviderError::Server("stream ended without a terminal event".into()), text, calls })
}

/// Runs `request` over `chain` with fallback. The returned stream ends with
/// exactly one terminal item; `ChatEvent::Switched` marks every model switch.
pub fn chat_with_fallback(
    source: Arc<dyn AdapterSource>,
    chain: Vec<ModelRef>,
    request: ChatRequest,
    hook: Option<Arc<dyn HandoffHook>>,
    options: FallbackOptions,
    cancel: CancellationToken,
) -> ChatStream {
    let inner_cancel = cancel.clone();
    drive(cancel, move |em: Emitter<ChatEvent>| async move {
        if chain.is_empty() {
            return Err(ProviderError::Unsupported(
                "no model available for this request (check providers, routing and privacy policy)".into(),
            ));
        }
        let mut request = request;
        let mut index = 0usize;
        let mut retries = 0u32;
        loop {
            let Some(model) = chain.get(index).cloned() else {
                return Err(ProviderError::Server("fallback chain exhausted".into()));
            };
            let end = attempt(source.as_ref(), &model, &request, &em, &inner_cancel).await?;
            let (error, text, calls) = match end {
                AttemptEnd::Done => return Ok(()),
                AttemptEnd::Failed { error, text, calls } => {
                    source.observe(&model, &AttemptOutcome::Failure(error.clone()));
                    (error, text, calls)
                }
                AttemptEnd::Skipped { error } => (error, String::new(), Vec::new()),
            };
            tracing::warn!(provider = %model.provider_id, model = %model.model, class = error.class(), "provider attempt failed");
            if error == ProviderError::Cancelled {
                return Err(error);
            }
            let had_output = !text.is_empty() || !calls.is_empty();
            if let ProviderError::RateLimited { retry_after_secs } = error {
                source.emit(ProviderEvent::RateLimited {
                    provider_id: model.provider_id,
                    model: Some(model.model.clone()),
                    retry_after_secs,
                });
                let wait = Duration::from_secs(retry_after_secs.unwrap_or(1));
                if !had_output && retries < options.same_model_retries && wait <= options.max_rate_limit_wait {
                    retries += 1;
                    tokio::select! {
                        () = tokio::time::sleep(wait) => {}
                        () = inner_cancel.cancelled() => return Err(ProviderError::Cancelled),
                    }
                    continue;
                }
            }
            if !error.is_fallback_eligible() {
                return Err(error);
            }
            let Some(next) = chain.get(index + 1).cloned() else {
                return Err(error);
            };
            let ctx = HandoffContext {
                from: model.clone(),
                to: next.clone(),
                error: error.clone(),
                partial_text: text,
                partial_tool_calls: calls,
                attempt: (index + 2) as u32,
            };
            match &hook {
                Some(h) => match h.handoff(&ctx, request.clone()).await {
                    Some(r) => request = r,
                    None => return Err(error),
                },
                // Without a hook, partial output cannot be handed over safely.
                None if had_output => return Err(error),
                None => {}
            }
            let reason = error.class().to_owned();
            source.emit(ProviderEvent::Fallback { from: model.clone(), to: next.clone(), reason: reason.clone() });
            em.emit(ChatEvent::Switched { from: model, to: next, reason }).await?;
            index += 1;
            retries = 0;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::mock::{MockAdapter, MockOptions};
    use crate::types::{ChatMessage, StopReason};
    use orkestr_domain::ProviderId;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Source {
        adapters: HashMap<ProviderId, Arc<dyn ProviderAdapter>>,
        outcomes: Mutex<Vec<(ModelRef, AttemptOutcome)>>,
        events: Mutex<Vec<ProviderEvent>>,
        refused: Vec<String>,
    }

    impl AdapterSource for Source {
        fn adapter(&self, p: &ProviderId) -> Option<Arc<dyn ProviderAdapter>> {
            self.adapters.get(p).cloned()
        }
        fn admit(&self, m: &ModelRef) -> bool {
            !self.refused.contains(&m.model)
        }
        fn observe(&self, m: &ModelRef, o: &AttemptOutcome) {
            self.outcomes.lock().push((m.clone(), o.clone()));
        }
        fn emit(&self, e: ProviderEvent) {
            self.events.lock().push(e);
        }
    }

    fn setup() -> (Arc<Source>, ProviderId) {
        let id = ProviderId::new();
        let mut s = Source::default();
        s.adapters.insert(
            id,
            Arc::new(MockAdapter::new(id, "Mock (offline demo)", MockOptions { chunk_delay: Duration::ZERO })),
        );
        (Arc::new(s), id)
    }

    fn m(p: ProviderId, model: &str) -> ModelRef {
        ModelRef { provider_id: p, model: model.into() }
    }

    fn fast() -> FallbackOptions {
        FallbackOptions { max_rate_limit_wait: Duration::from_millis(0), same_model_retries: 0 }
    }

    #[tokio::test]
    async fn refused_attempt_is_skipped_without_counting_as_failure() {
        let id = ProviderId::new();
        let mut s = Source::default();
        s.adapters.insert(
            id,
            Arc::new(MockAdapter::new(id, "Mock (offline demo)", MockOptions { chunk_delay: Duration::ZERO })),
        );
        s.refused = vec!["mock-server-error".into()];
        let src = Arc::new(s);
        let chain = vec![m(id, "mock-server-error"), m(id, "mock-echo")];
        let req = ChatRequest::new("ignored", vec![ChatMessage::user("hello")]);
        let out: Vec<ChatEvent> =
            chat_with_fallback(src.clone(), chain, req, None, fast(), CancellationToken::new()).collect().await;
        assert!(matches!(out.last(), Some(ChatEvent::Done { .. })), "{out:?}");
        assert!(out.iter().any(|e| matches!(e, ChatEvent::Switched { .. })));
        // Only the attempted model reports an outcome: the refused one must
        // neither re-open nor release the half-open probe of another request.
        let outcomes = src.outcomes.lock();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].0.model, "mock-echo");
    }

    #[tokio::test]
    async fn falls_back_on_rate_limit_and_quota_and_emits_events() {
        let (src, p) = setup();
        let chain = vec![m(p, "mock-rate-limit"), m(p, "mock-quota"), m(p, "mock-echo")];
        let req = ChatRequest::new("ignored", vec![ChatMessage::user("hello")]);
        let out: Vec<ChatEvent> =
            chat_with_fallback(src.clone(), chain.clone(), req, None, fast(), CancellationToken::new()).collect().await;
        let switches: Vec<_> = out.iter().filter(|e| matches!(e, ChatEvent::Switched { .. })).collect();
        assert_eq!(switches.len(), 2);
        assert!(matches!(&out[0], ChatEvent::Switched { reason, .. } if reason == "rate_limited"));
        assert!(matches!(&out[1], ChatEvent::Switched { reason, .. } if reason == "quota_exceeded"));
        let text: String =
            out.iter().filter_map(|e| if let ChatEvent::TextDelta(t) = e { Some(t.as_str()) } else { None }).collect();
        assert_eq!(text, "hello");
        assert_eq!(out.last(), Some(&ChatEvent::Done { stop_reason: StopReason::EndTurn }));
        let events = src.events.lock().clone();
        assert!(matches!(&events[0], ProviderEvent::RateLimited { retry_after_secs: Some(1), .. }));
        assert!(matches!(&events[1], ProviderEvent::Fallback { from, to, .. } if *from == chain[0] && *to == chain[1]));
        assert!(matches!(&events[2], ProviderEvent::Fallback { to, .. } if *to == chain[2]));
        let outcomes = src.outcomes.lock().clone();
        assert!(matches!(outcomes.last(), Some((mr, AttemptOutcome::Success { .. })) if *mr == chain[2]));
    }

    #[tokio::test]
    async fn non_eligible_errors_and_exhaustion_surface() {
        let (src, p) = setup();
        let req = ChatRequest::new("x", vec![ChatMessage::user("hi")]);
        let out: Vec<ChatEvent> = chat_with_fallback(
            src.clone(),
            vec![m(p, "mock-server-error")],
            req.clone(),
            None,
            fast(),
            CancellationToken::new(),
        )
        .collect()
        .await;
        assert!(matches!(out.as_slice(), [ChatEvent::Error(ProviderError::Server(_))]));
        let out: Vec<ChatEvent> =
            chat_with_fallback(src, vec![], req, None, fast(), CancellationToken::new()).collect().await;
        assert!(matches!(out.as_slice(), [ChatEvent::Error(ProviderError::Unsupported(_))]));
    }

    #[tokio::test]
    async fn rate_limit_retries_same_model_when_short() {
        let (src, p) = setup();
        let opts = FallbackOptions { max_rate_limit_wait: Duration::from_secs(1), same_model_retries: 1 };
        let req = ChatRequest::new("x", vec![ChatMessage::user("hi")]);
        let start = Instant::now();
        let out: Vec<ChatEvent> = chat_with_fallback(
            src.clone(),
            vec![m(p, "mock-rate-limit"), m(p, "mock-echo")],
            req,
            None,
            opts,
            CancellationToken::new(),
        )
        .collect()
        .await;
        assert!(start.elapsed() >= Duration::from_millis(900), "waited retry-after once");
        let fails = src.outcomes.lock().iter().filter(|(_, o)| matches!(o, AttemptOutcome::Failure(_))).count();
        assert_eq!(fails, 2, "one retry on the same model before falling back");
        assert_eq!(out.last(), Some(&ChatEvent::Done { stop_reason: StopReason::EndTurn }));
    }

    struct RecordingHook {
        seen: Mutex<Vec<HandoffContext>>,
        allow: bool,
    }

    #[async_trait]
    impl HandoffHook for RecordingHook {
        async fn handoff(&self, ctx: &HandoffContext, mut request: ChatRequest) -> Option<ChatRequest> {
            self.seen.lock().push(ctx.clone());
            if !self.allow {
                return None;
            }
            request.messages.push(ChatMessage::user(format!(
                "HANDOFF from {} ({})",
                ctx.from.model,
                ctx.error.class()
            )));
            Some(request)
        }
    }

    #[tokio::test]
    async fn handoff_hook_shapes_the_next_request_or_stops() {
        let (src, p) = setup();
        let hook = Arc::new(RecordingHook { seen: Mutex::new(vec![]), allow: true });
        let req = ChatRequest::new("x", vec![ChatMessage::user("hi")]);
        let out: Vec<ChatEvent> = chat_with_fallback(
            src.clone(),
            vec![m(p, "mock-quota"), m(p, "mock-echo")],
            req.clone(),
            Some(hook.clone()),
            fast(),
            CancellationToken::new(),
        )
        .collect()
        .await;
        let text: String =
            out.iter().filter_map(|e| if let ChatEvent::TextDelta(t) = e { Some(t.as_str()) } else { None }).collect();
        assert_eq!(text, "HANDOFF from mock-quota (quota_exceeded)");
        let seen = hook.seen.lock().clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].attempt, 2);
        assert!(!seen[0].had_output());

        let stop = Arc::new(RecordingHook { seen: Mutex::new(vec![]), allow: false });
        let out: Vec<ChatEvent> = chat_with_fallback(
            src,
            vec![m(p, "mock-quota"), m(p, "mock-echo")],
            req,
            Some(stop),
            fast(),
            CancellationToken::new(),
        )
        .collect()
        .await;
        assert!(matches!(out.as_slice(), [ChatEvent::Error(ProviderError::QuotaExceeded(_))]));
    }

    #[tokio::test]
    async fn cancellation_is_terminal() {
        let id = ProviderId::new();
        let mut s = Source::default();
        s.adapters.insert(
            id,
            Arc::new(MockAdapter::new(
                id,
                "Mock (offline demo)",
                MockOptions { chunk_delay: Duration::from_millis(50) },
            )),
        );
        let cancel = CancellationToken::new();
        let req = ChatRequest::new("x", vec![ChatMessage::user("a long message that streams slowly in many chunks")]);
        let mut stream = chat_with_fallback(Arc::new(s), vec![m(id, "mock-echo")], req, None, fast(), cancel.clone());
        let first = stream.next().await;
        assert!(matches!(first, Some(ChatEvent::TextDelta(_))));
        cancel.cancel();
        let rest: Vec<ChatEvent> = stream.collect().await;
        assert_eq!(rest.last(), Some(&ChatEvent::Error(ProviderError::Cancelled)));
    }
}
