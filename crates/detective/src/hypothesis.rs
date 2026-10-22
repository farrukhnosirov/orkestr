//! Root-cause hypothesis: prompt, JSON schema, strict validation and a
//! bounded repair loop (the invalid answer and the validation error are sent
//! back; every attempt's spend counts against the incident budget).

use orkestr_protocol::methods::detective::{Hypothesis, Incident, IncidentTrigger};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::collectors::cap;
use crate::ports::{HypothesisModel, ModelRequest};
use crate::DetectiveError;

/// Attempts (first answer + repairs).
pub const MAX_ATTEMPTS: u32 = 3;

/// Output schema (`fixPlan` is accepted as an alias of `proposedFix`).
#[must_use]
pub fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["summary", "rootCause", "confidence", "evidenceRefs", "suspectedFiles", "proposedFix"],
        "properties": {
            "summary": {"type": "string", "description": "One-line summary of the failure."},
            "rootCause": {"type": "string", "description": "Why it fails, grounded in the evidence."},
            "confidence": {"type": "number", "minimum": 0, "maximum": 1},
            "evidenceRefs": {"type": "array", "items": {"type": "string"}, "description": "Evidence ids (ev1, ev2, ...) supporting the hypothesis."},
            "suspectedFiles": {"type": "array", "items": {"type": "string"}, "description": "Project-relative path[:line]."},
            "proposedFix": {"type": "string", "description": "Concrete fix plan."},
            "reproduction": {"type": ["string", "null"], "description": "Shell command that reproduces the failure (a failing test), or null."}
        }
    })
}

/// System prompt.
pub const SYSTEM: &str = "You are Orkestr's Bug Detective. Find the root cause of a production incident from the \
evidence only. Cite evidence ids exactly as given (ev1, ev2, ...). Never invent files, commits or log lines. \
Be calibrated: confidence 0.9+ only when a stack trace or diff points directly at the defect. \
Reply with ONE JSON object matching the schema — no prose, no markdown.";

/// User prompt with the trigger and the evidence.
#[must_use]
pub fn prompt(inc: &Incident, max_bytes: usize) -> String {
    let trigger = match &inc.trigger {
        IncidentTrigger::Alert { alert_id } => format!("monitoring alert {alert_id}"),
        IncidentTrigger::LogError { source_id, line } => format!("error line in log source {source_id}: {line}"),
        IncidentTrigger::TestFailure { run_id } => format!("failing test run {run_id}"),
        IncidentTrigger::Manual { description } => format!("reported by the user: {description}"),
    };
    let mut out = format!("Incident: {}\nTrigger: {trigger}\n\nEvidence:\n", inc.title);
    for e in &inc.evidence {
        let block = format!(
            "\n### {} [{:?}] {}{}\n{}\n",
            e.id,
            e.kind,
            e.title,
            e.reference.as_deref().map(|r| format!(" ({r})")).unwrap_or_default(),
            e.content
        );
        if out.len() + block.len() > max_bytes {
            out.push_str(&format!("\n(further evidence omitted: prompt budget of {max_bytes} bytes)\n"));
            break;
        }
        out.push_str(&block);
    }
    out.push_str("\nAnswer with the JSON hypothesis.");
    out
}

/// Extracts the first JSON object of `text` (whole text, a fenced block, or
/// the outermost `{...}` span).
#[must_use]
pub fn extract_json(text: &str) -> Option<Value> {
    let t = text.trim();
    if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(t) {
        return Some(v);
    }
    if let Some(open) = t.find("```") {
        let after = &t[open + 3..];
        let body = &after[after.find('\n').map_or(0, |n| n + 1)..];
        if let Some(close) = body.find("```") {
            if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(body[..close].trim()) {
                return Some(v);
            }
        }
    }
    let (a, b) = (t.find('{')?, t.rfind('}')?);
    if a < b {
        if let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(&t[a..=b]) {
            return Some(v);
        }
    }
    None
}

fn string_field(v: &Value, key: &str, alias: Option<&str>) -> Result<String, String> {
    let raw = v.get(key).or_else(|| alias.and_then(|a| v.get(a)));
    match raw {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.trim().to_owned()),
        Some(Value::String(_)) => Err(format!("`{key}` must not be empty")),
        Some(_) => Err(format!("`{key}` must be a string")),
        None => Err(format!("`{key}` is missing")),
    }
}

fn string_list(v: &Value, key: &str) -> Result<Vec<String>, String> {
    match v.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| x.as_str().map(|s| s.trim().to_owned()).ok_or_else(|| format!("`{key}` must contain strings")))
            .collect(),
        Some(_) => Err(format!("`{key}` must be an array of strings")),
        None => Err(format!("`{key}` is missing")),
    }
}

/// Validates one answer against the schema and the incident's evidence ids.
pub fn validate(text: &str, evidence_ids: &[String]) -> Result<Hypothesis, String> {
    let v = extract_json(text).ok_or_else(|| "the answer is not a JSON object".to_owned())?;
    let summary = string_field(&v, "summary", None)?;
    let root_cause = string_field(&v, "rootCause", Some("root_cause"))?;
    let proposed_fix = string_field(&v, "proposedFix", Some("fixPlan"))?;
    let confidence =
        v.get("confidence").and_then(Value::as_f64).ok_or_else(|| "`confidence` must be a number".to_owned())?;
    if !(0.0..=1.0).contains(&confidence) || confidence.is_nan() {
        return Err(format!("`confidence` must be within 0..1 (got {confidence})"));
    }
    let evidence_refs = string_list(&v, "evidenceRefs")?;
    if let Some(bad) = evidence_refs.iter().find(|r| !evidence_ids.contains(r)) {
        return Err(format!("`evidenceRefs` cites unknown evidence `{bad}` (known: {})", evidence_ids.join(", ")));
    }
    if evidence_refs.is_empty() && !evidence_ids.is_empty() {
        return Err("`evidenceRefs` must cite at least one evidence id".into());
    }
    let mut suspected_files = string_list(&v, "suspectedFiles")?;
    suspected_files.retain(|f| !f.is_empty());
    if suspected_files.iter().any(|f| f.contains("..")) {
        return Err("`suspectedFiles` must be project-relative paths without `..`".into());
    }
    let reproduction = match v.get("reproduction") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Some(_) => return Err("`reproduction` must be a string or null".into()),
    };
    Ok(Hypothesis {
        summary: cap(&summary, 400),
        root_cause: cap(&root_cause, 8_000),
        confidence,
        evidence_refs,
        suspected_files,
        proposed_fix: cap(&proposed_fix, 8_000),
        reproduction,
    })
}

/// Result of [`generate`].
#[derive(Debug, Clone, PartialEq)]
pub struct Generated {
    /// The validated hypothesis.
    pub hypothesis: Hypothesis,
    /// Total spend of all attempts.
    pub cost_usd: f64,
    /// Attempts used.
    pub attempts: u32,
}

/// Asks the model, validates, and repairs up to [`MAX_ATTEMPTS`]. `charge`
/// is called after every attempt with its spend and may abort (budget cap).
pub async fn generate(
    model: &dyn HypothesisModel,
    inc: &Incident,
    prompt_bytes: usize,
    cancel: &CancellationToken,
    mut charge: impl FnMut(f64) -> Result<(), DetectiveError>,
) -> Result<Generated, DetectiveError> {
    let ids: Vec<String> = inc.evidence.iter().map(|e| e.id.clone()).collect();
    let mut messages = vec![("user".to_owned(), prompt(inc, prompt_bytes))];
    let mut total = 0.0;
    let mut last_err = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        if cancel.is_cancelled() {
            return Err(DetectiveError::Cancelled);
        }
        let req = ModelRequest {
            project_id: inc.project_id,
            system: SYSTEM.to_owned(),
            messages: messages.clone(),
            schema: schema(),
        };
        let reply = tokio::select! {
            r = model.complete(req, cancel.clone()) => r?,
            () = cancel.cancelled() => return Err(DetectiveError::Cancelled),
        };
        total += reply.cost_usd;
        charge(reply.cost_usd)?;
        match validate(&reply.text, &ids) {
            Ok(hypothesis) => return Ok(Generated { hypothesis, cost_usd: total, attempts: attempt }),
            Err(e) => {
                tracing::warn!(incident = %inc.id, attempt, error = %e, "invalid hypothesis answer");
                last_err = e.clone();
                messages.push(("assistant".into(), cap(&reply.text, 16_000)));
                messages.push((
                    "user".into(),
                    format!("That answer was invalid: {e}. Reply with only the corrected JSON object."),
                ));
            }
        }
    }
    Err(DetectiveError::Invalid(format!("the model did not produce a valid hypothesis: {last_err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Vec<String> {
        vec!["ev1".into(), "ev2".into()]
    }

    const GOOD: &str = r#"{"summary":"nil user","rootCause":"load() unwraps a missing row","confidence":0.8,"evidenceRefs":["ev1"],"suspectedFiles":["src/user.rs:42"],"proposedFix":"return 404","reproduction":"cargo test user"}"#;

    #[test]
    fn accepts_valid_fenced_and_alias() {
        let h = validate(GOOD, &ids()).unwrap();
        assert_eq!(h.confidence, 0.8);
        assert_eq!(h.reproduction.as_deref(), Some("cargo test user"));
        let fenced = format!("Sure!\n```json\n{GOOD}\n```");
        assert!(validate(&fenced, &ids()).is_ok());
        let alias = GOOD.replace("proposedFix", "fixPlan");
        assert_eq!(validate(&alias, &ids()).unwrap().proposed_fix, "return 404");
    }

    #[test]
    fn rejects_bad_answers() {
        assert!(validate("not json", &ids()).is_err());
        assert!(validate(&GOOD.replace("0.8", "1.5"), &ids()).unwrap_err().contains("confidence"));
        assert!(validate(&GOOD.replace("0.8", "\"high\""), &ids()).unwrap_err().contains("confidence"));
        assert!(validate(&GOOD.replace("\"ev1\"", "\"ev9\""), &ids()).unwrap_err().contains("ev9"));
        assert!(validate(&GOOD.replace("\"ev1\"", ""), &ids()).unwrap_err().contains("at least one"));
        assert!(validate(&GOOD.replace("nil user", " "), &ids()).unwrap_err().contains("summary"));
        assert!(validate(&GOOD.replace("src/user.rs:42", "../etc/passwd"), &ids()).is_err());
        assert!(validate(&GOOD.replace("\"cargo test user\"", "5"), &ids()).is_err());
    }
}
