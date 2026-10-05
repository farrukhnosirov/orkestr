//! One structured model call with validation and a single repair round,
//! shared by the team lead decomposition and the AI Product Manager
//! (provider structured output when the model supports it; robust JSON
//! extraction otherwise). Only visible output is used — never reasoning.

use orkestr_domain::{ModelRef, Project, UsageSummary};
use orkestr_providers::{ChatMessage, ChatRequest, ResponseFormat};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::error::{OrchResult, OrchestratorError};
use crate::service::Inner;
use crate::util::{cap, manager_chain, record_usage, stream_chat};

/// Extracts the first JSON object that has `key`: the whole text, a fenced
/// block, or the span between the first `{` and the last `}`.
#[must_use]
pub fn extract_object(text: &str, key: &str) -> Option<Value> {
    let ok = |v: &Value| v.get(key).is_some();
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        if ok(&v) {
            return Some(v);
        }
    }
    let mut rest = t;
    while let Some(open) = rest.find("```") {
        let after = &rest[open + 3..];
        let body_start = after.find('\n').map_or(0, |n| n + 1);
        let body = &after[body_start..];
        let Some(close) = body.find("```") else { break };
        if let Ok(v) = serde_json::from_str::<Value>(body[..close].trim()) {
            if ok(&v) {
                return Some(v);
            }
        }
        rest = &body[close + 3..];
    }
    let (Some(a), Some(b)) = (t.find('{'), t.rfind('}')) else { return None };
    if a < b {
        if let Ok(v) = serde_json::from_str::<Value>(&t[a..=b]) {
            if ok(&v) {
                return Some(v);
            }
        }
    }
    None
}

/// What one call needs.
pub struct StructuredCall<'a> {
    /// Project (privacy policy, usage attribution).
    pub project: &'a Project,
    /// Explicit model (member / request override).
    pub model: Option<&'a ModelRef>,
    /// System prompt (contains the behaviour marker).
    pub system: String,
    /// User message.
    pub user: String,
    /// Structured-output schema name.
    pub schema_name: &'a str,
    /// Schema.
    pub schema: Value,
    /// Key the answer object must have.
    pub key: &'a str,
}

/// Result of a structured call.
pub struct StructuredOutcome<T> {
    /// Validated value.
    pub value: T,
    /// Model that answered.
    pub model: Option<ModelRef>,
    /// Usage of all attempts.
    pub usage: UsageSummary,
    /// The first answer was invalid and was repaired.
    pub repaired: bool,
}

impl Inner {
    /// Runs the call; `validate` turns the JSON into `T` or explains why not
    /// (that explanation is sent back once for repair).
    pub(crate) async fn structured_call<T>(
        &self,
        call: StructuredCall<'_>,
        cancel: &CancellationToken,
        validate: impl Fn(&Value) -> OrchResult<T>,
    ) -> OrchResult<StructuredOutcome<T>> {
        let cfg = self.config().await;
        let head = call.model.cloned().or_else(|| cfg.model.clone());
        let structured = head.as_ref().is_some_and(|m| self.deps.hub.supports_structured_output(m));
        let chain = manager_chain(&self.deps, call.project, &cfg, call.model, true).await?;
        let structured = structured && chain.first() == head.as_ref();
        let mut messages = vec![ChatMessage::user(call.user.clone())];
        let mut usage_total = UsageSummary::default();
        let mut last_err = String::new();
        for attempt in 0..2 {
            let mut req = ChatRequest::new(chain[0].model.clone(), messages.clone());
            let mut system = call.system.clone();
            if !structured {
                system.push_str(&format!(
                    "\nAnswer with ONE JSON object only (no prose, no markdown) matching this JSON schema:\n{}\n",
                    call.schema
                ));
            }
            req.system = Some(system);
            req.response_format = if structured {
                ResponseFormat::JsonSchema { name: call.schema_name.into(), schema: call.schema.clone() }
            } else {
                ResponseFormat::Text
            };
            req.max_output_tokens = Some(self.deps.config.max_output_tokens);
            req.reasoning = Some(cfg.reasoning);
            let out = stream_chat(&self.deps, chain.clone(), req, None, cancel).await?;
            let model = out.model.clone().unwrap_or_else(|| chain[0].clone());
            let usage = out.usage.unwrap_or_else(|| {
                orkestr_providers::Usage::estimate(messages.iter().map(|m| m.content.len()).sum(), out.text.len())
            });
            usage_total.add(&record_usage(&self.deps, call.project.id, &model, usage, out.first_ms, true).await);
            let parsed = extract_object(&out.text, call.key)
                .ok_or_else(|| OrchestratorError::Planning(format!("the answer contained no JSON object with `{}`", call.key)))
                .and_then(|v| validate(&v));
            match parsed {
                Ok(value) => {
                    return Ok(StructuredOutcome { value, model: Some(model), usage: usage_total, repaired: attempt > 0 })
                }
                Err(e) => {
                    last_err = e.to_string();
                    tracing::warn!(attempt, error = %last_err, schema = call.schema_name, "invalid structured answer");
                    messages.push(ChatMessage::assistant(cap(&out.text, 20_000)));
                    messages.push(ChatMessage::user(format!(
                        "That answer was not valid: {last_err}. Reply with only the corrected JSON object."
                    )));
                }
            }
        }
        Err(OrchestratorError::Planning(format!("the model did not produce a valid answer: {last_err}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_objects() {
        assert!(extract_object(r#"{"epic": {}}"#, "epic").is_some());
        assert!(extract_object("Here:\n```json\n{\"subtasks\": []}\n```", "subtasks").is_some());
        assert!(extract_object("text {\"subtasks\": [1]} more", "subtasks").is_some());
        assert!(extract_object("{\"other\": 1}", "subtasks").is_none());
        assert!(extract_object("nothing", "x").is_none());
    }
}
