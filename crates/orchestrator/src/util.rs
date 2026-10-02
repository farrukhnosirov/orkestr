//! Shared helpers: model routing for the Manager, streamed chat collection,
//! usage accounting, text shaping.

use std::time::Instant;

use futures::StreamExt;
use orkestr_domain::{now, ManagerConfig, ModelRef, Project, ProjectId, UsageRecord, UsageRecordId, UsageSummary};
use orkestr_protocol::event::{EventKind, MessageDeltaPayload, MessageRole};
use orkestr_protocol::NewEvent;
use orkestr_providers::{ChatEvent, ChatRequest, RequiredCapabilities, RouteRequest, ToolCallPart, Usage};
use tokio_util::sync::CancellationToken;

use crate::deps::OrchestratorDeps;
use crate::error::{OrchResult, OrchestratorError};

/// Redacts registered secrets and secret-looking values.
#[must_use]
pub fn redact(text: &str) -> String {
    orkestr_security::redact::redact(text)
}

/// Collapses whitespace and caps to `n` characters.
#[must_use]
pub fn one_line(s: &str, n: usize) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    cap(&t, n)
}

/// Caps to `n` characters (with an ellipsis).
#[must_use]
pub fn cap(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Ordered models for a Manager request (head first): the explicit model
/// (request override or Manager setting), the Manager fallback chain, then
/// routing. Every explicit model must be allowed by the project's privacy
/// policy; CLI coding agents are excluded (the Manager needs tool calling
/// over the chat API).
pub async fn manager_chain(
    deps: &OrchestratorDeps,
    project: &Project,
    cfg: &ManagerConfig,
    explicit: Option<&ModelRef>,
    structured: bool,
) -> OrchResult<Vec<ModelRef>> {
    let mut chain: Vec<ModelRef> = Vec::new();
    for m in explicit.into_iter().chain(cfg.model.as_ref()).chain(cfg.fallback_chain.iter()) {
        let info = deps.models.provider(m.provider_id).ok_or_else(|| {
            OrchestratorError::Invalid(format!("provider {} of model `{}` is not configured", m.provider_id, m.model))
        })?;
        if !project.privacy.allows_provider(m.provider_id, info.is_local) {
            if explicit == Some(m) {
                return Err(OrchestratorError::Forbidden(format!(
                    "model `{}` is not allowed by the project's AI privacy policy",
                    m.model
                )));
            }
            continue;
        }
        if !chain.contains(m) {
            chain.push(m.clone());
        }
    }
    let routed = deps
        .models
        .route(&RouteRequest {
            purpose: Some("Manager".into()),
            required: RequiredCapabilities { tools: !structured, structured_output: false, ..Default::default() },
            preferred: chain.first().cloned(),
            fallback_chain: chain.iter().skip(1).cloned().collect(),
            policy: project.privacy.ai.clone(),
            max_cost_usd: None,
            est_input_tokens: 6_000,
            max_output_tokens: u64::from(deps.config.max_output_tokens),
            exclude: Vec::new(),
        })
        .await
        .map_err(OrchestratorError::NoModel)?;
    for m in routed {
        if !chain.contains(&m) {
            chain.push(m);
        }
    }
    chain.retain(|m| deps.models.provider(m.provider_id).is_some_and(|i| i.enabled && !i.is_cli_agent()));
    if chain.is_empty() {
        return Err(OrchestratorError::NoModel(
            "no chat model is available for the AI Manager — choose one in Settings → AI Manager or connect a provider (the offline Mock provider also works)"
                .into(),
        ));
    }
    Ok(chain)
}

/// Collected output of one streamed model call.
#[derive(Debug, Default)]
pub struct Collected {
    /// Visible text.
    pub text: String,
    /// Tool calls.
    pub calls: Vec<ToolCallPart>,
    /// Usage (last report).
    pub usage: Option<Usage>,
    /// Model that answered (after fallbacks).
    pub model: Option<ModelRef>,
    /// Fallback notes (`a → b (reason)`).
    pub switches: Vec<String>,
    /// Time to first event.
    pub first_ms: Option<u64>,
}

/// Where streamed text deltas go.
#[derive(Debug, Clone)]
pub struct DeltaSink {
    /// Project scope of the `ManagerMessage` events.
    pub project_id: ProjectId,
    /// Message id the deltas belong to.
    pub message_id: String,
}

/// Runs one streamed chat over `chain`, publishing text deltas as
/// `ManagerMessage` events when `sink` is set. Hidden reasoning has no
/// `ChatEvent` variant and is therefore never seen here.
pub async fn stream_chat(
    deps: &OrchestratorDeps,
    chain: Vec<ModelRef>,
    request: ChatRequest,
    sink: Option<&DeltaSink>,
    cancel: &CancellationToken,
) -> OrchResult<Collected> {
    let head = chain.first().cloned();
    let mut stream = deps.models.chat(chain, request, None, cancel.child_token());
    let started = Instant::now();
    let mut out = Collected { model: head, ..Collected::default() };
    let mut ended = false;
    while let Some(ev) = stream.next().await {
        out.first_ms.get_or_insert_with(|| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
        match ev {
            ChatEvent::TextDelta(t) => {
                out.text.push_str(&t);
                if let Some(s) = sink {
                    deps.publish(
                        NewEvent::new(EventKind::ManagerMessage(MessageDeltaPayload {
                            agent_id: None,
                            message_id: s.message_id.clone(),
                            role: MessageRole::Assistant,
                            delta: redact(&t),
                            done: false,
                        }))
                        .project(s.project_id),
                    )
                    .await;
                }
            }
            ChatEvent::ToolCall { id, name, arguments } => out.calls.push(ToolCallPart { id, name, arguments }),
            ChatEvent::Usage(u) => out.usage = Some(u),
            ChatEvent::Switched { from, to, reason } => {
                // The failed attempt's partial output is discarded by the executor.
                out.text.clear();
                out.calls.clear();
                out.switches.push(format!("{} → {} ({reason})", from.model, to.model));
                out.model = Some(to);
            }
            ChatEvent::Done { .. } => {
                ended = true;
                break;
            }
            ChatEvent::Error(e) => {
                if cancel.is_cancelled() || e == orkestr_providers::ProviderError::Cancelled {
                    return Err(OrchestratorError::Conflict("cancelled".into()));
                }
                return Err(OrchestratorError::Planning(format!("model call failed: {}", redact(&e.to_string()))));
            }
        }
    }
    if !ended {
        if cancel.is_cancelled() {
            return Err(OrchestratorError::Conflict("cancelled".into()));
        }
        return Err(OrchestratorError::Planning("the model stream ended unexpectedly".into()));
    }
    Ok(out)
}

/// Records the usage of a Manager call in the cost engine and returns it.
pub async fn record_usage(
    deps: &OrchestratorDeps,
    project: ProjectId,
    model: &ModelRef,
    usage: Usage,
    latency_ms: Option<u64>,
    success: bool,
) -> UsageSummary {
    let cost = orkestr_providers::estimate_cost(&model.model, &usage).unwrap_or(0.0);
    let rec = UsageRecord {
        id: UsageRecordId::new(),
        ts: now(),
        provider_id: model.provider_id,
        model: model.model.clone(),
        project_id: Some(project),
        task_id: None,
        agent_id: None,
        execution_id: None,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        cost_usd: cost,
        estimated: usage.estimated || usage.reported_cost_usd.is_none(),
        latency_ms,
        success,
    };
    if let Err(e) = deps.cost.ingest(&rec).await {
        tracing::warn!(error = %e, "manager usage ingest failed");
    }
    UsageSummary { input_tokens: usage.input_tokens, output_tokens: usage.output_tokens, cost_usd: cost }
}

/// Display name of a role (also the routing purpose).
#[must_use]
pub fn role_name(role: &orkestr_domain::AgentRole) -> String {
    orkestr_agent_runtime::prompt::role_name(role)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_helpers() {
        assert_eq!(one_line("a\n  b\tc", 10), "a b c");
        assert_eq!(cap("abcdef", 4), "abc…");
        assert_eq!(cap("abc", 4), "abc");
    }
}
