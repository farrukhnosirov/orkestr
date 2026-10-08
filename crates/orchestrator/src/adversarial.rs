//! Adversarial review (§8).
//!
//! A separate model — preferably a different provider/model than the one
//! that wrote the change (routing purpose `Adversarial Review`, the author's
//! model excluded) — is instructed to *refute* the change: security,
//! architecture weaknesses, race conditions, data loss, performance, edge
//! cases, wrong assumptions, UX. It must not simply agree; every finding
//! needs evidence. When no other model is available, the author's model is
//! used with the adversarial prompt and the review is flagged
//! `sameModelFallback`.
//!
//! Findings are then verified in one bounded second pass: the author model
//! concedes each finding (→ `Confirmed`) or rebuts it with evidence
//! (→ `Rebutted`). A rebuttal without evidence leaves the finding `Open`.
//! Unresolved critical findings block the quality gate
//! ([`crate::review::gate_of`]).

use orkestr_domain::{now, ModelRef, Project, Task};
use orkestr_protocol::methods::reviews::{
    AdversarialReviewParams, FindingSeverity, FindingStatus, Review, ReviewFinding, ReviewKind, ReviewResult,
    ReviewStatus, ReviewVerdict,
};
use orkestr_providers::adapters::mock_review::{ADVERSARIAL_MARKER, VERIFY_MARKER};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::error::{OrchResult, OrchestratorError};
use crate::review::{
    gather_change, parse_findings, parse_verdict, review_schema, role_chain, structured_call, task_brief, text_of,
    Change, InFlight,
};
use crate::service::{Inner, Manager};
use crate::util::one_line;

/// Max findings sent to the verification pass.
const MAX_VERIFY: usize = 30;

/// A prepared adversarial review.
#[derive(Debug, Clone)]
pub(crate) struct AdversaryPlan {
    /// The review document (status `Running`).
    pub review: Review,
    /// Adversary models (head first).
    pub chain: Vec<ModelRef>,
    /// Models for the verification pass (the author first).
    pub verifier: Vec<ModelRef>,
}

/// Puts models of other providers than `author`'s first (stable order).
#[must_use]
pub fn prefer_other_providers(chain: Vec<ModelRef>, author: Option<&ModelRef>) -> Vec<ModelRef> {
    let Some(a) = author else { return chain };
    let (mut other, same): (Vec<ModelRef>, Vec<ModelRef>) =
        chain.into_iter().partition(|m| m.provider_id != a.provider_id);
    other.extend(same);
    other
}

/// Appends the author's model as the final fallback of an adversary chain
/// (when it is not in the chain already).
#[must_use]
pub fn with_author_last(mut chain: Vec<ModelRef>, author: Option<&ModelRef>) -> Vec<ModelRef> {
    if let Some(a) = author {
        if !chain.contains(a) {
            chain.push(a.clone());
        }
    }
    chain
}

/// Chooses the adversary and the verifier and creates the review document.
pub(crate) async fn prepare(
    inner: &Inner,
    project: &Project,
    task: &Task,
    explicit: Option<&ModelRef>,
) -> OrchResult<AdversaryPlan> {
    let author = task.model.clone();
    let (chain, same) = if let Some(m) = explicit {
        let chain = role_chain(inner, project, "Adversarial Review", Some(m), &[]).await?;
        (chain, author.as_ref() == Some(m))
    } else {
        let exclude: Vec<ModelRef> = author.iter().cloned().collect();
        match role_chain(inner, project, "Adversarial Review", None, &exclude).await {
            Ok(chain) => (prefer_other_providers(chain, author.as_ref()), false),
            Err(OrchestratorError::NoModel(_)) if author.is_some() => {
                // Fallback: the author's model with the adversarial prompt.
                let chain = role_chain(inner, project, "Adversarial Review", None, &[]).await?;
                let same = chain.first() == author.as_ref();
                (chain, same)
            }
            Err(e) => return Err(e),
        }
    };
    let usable_author = author
        .as_ref()
        .filter(|a| inner.deps.models.provider(a.provider_id).is_some_and(|i| i.enabled && i.chat_capable()));
    // Last resort when every other model fails (limit, quota, outage): the
    // author's model with the adversarial prompt, flagged on the review.
    let chain = with_author_last(chain, explicit.is_none().then_some(usable_author).flatten());
    let verifier = match usable_author {
        Some(a) => match role_chain(inner, project, "Review", Some(a), &[]).await {
            Ok(c) => c,
            Err(_) => role_chain(inner, project, "Review", None, &[]).await?,
        },
        None => role_chain(inner, project, "Review", None, &[]).await?,
    };
    let mut review = inner.new_review(task, ReviewKind::Adversarial, chain.first().cloned());
    review.same_model_fallback = same;
    Ok(AdversaryPlan { review, chain, verifier })
}

fn adversary_system(language: &str, same_model: bool) -> String {
    let mut s = format!(
        "{ADVERSARIAL_MARKER}\nYou are the adversarial reviewer of Orkestr. Another AI model wrote the change below. \
         Your job is to REFUTE it: find the strongest reasons it is wrong, unsafe or incomplete.\n\
         Attack: security (injection, secrets, authz), architecture weaknesses, race conditions and concurrency, \
         data loss, performance, edge cases (empty / huge / malformed input, errors, timeouts), wrong assumptions \
         and UX problems.\n\
         Rules:\n\
         - Do NOT agree by default and do not praise. If you cannot break it, report the strongest remaining risk \
         as a low or info finding.\n\
         - Every finding needs concrete evidence: quote the code (file:line) or describe the exact input that \
         breaks it. Claims without evidence are worthless.\n\
         - Give the file (relative path) and the line in the NEW version of the file when possible.\n\
         - Severity: critical = must not ship; high = serious; medium; low; info.\n\
         - verdict: request_changes when any finding is high or critical, otherwise approve. \
         acceptanceCriteria may be an empty list.\n\
         - Write a short visible summary (no hidden reasoning).\n"
    );
    if same_model {
        s.push_str(
            "- No independent model was available: the change was written by a model like you. Be extra \
             sceptical and do not trust its own claims.\n",
        );
    }
    s.push('\n');
    s.push_str(&orkestr_agent_runtime::reply_language_instruction(language));
    s.push_str("- JSON field names and enum values stay in English.\n");
    s
}

fn verify_system(language: &str) -> String {
    format!(
        "{VERIFY_MARKER}\nYou wrote the change below. An adversarial reviewer reported findings against it. For EACH \
         finding answer honestly:\n\
         - \"concede\" when the finding is real (give the evidence that confirms it), or\n\
         - \"rebut\" when it is wrong — only with concrete evidence (code, tests, guarantees) that refutes it.\n\
         Do not defend the change without evidence; an unsupported rebuttal counts as unresolved.\n\n{}\
         - JSON field names and enum values stay in English.\n",
        orkestr_agent_runtime::reply_language_instruction(language)
    )
}

fn verify_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["responses"],
        "properties": {"responses": {"type": "array", "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["id", "verdict", "evidence"],
            "properties": {
                "id": {"type": "string"},
                "verdict": {"type": "string", "enum": ["concede", "rebut"]},
                "evidence": {"type": "string"}
            }
        }}}
    })
}

/// Parses the adversary's answer. Findings without evidence lose one severity level.
pub(crate) fn parse_adversarial(doc: &Value) -> Result<(Option<ReviewVerdict>, String, Vec<ReviewFinding>), String> {
    if !doc.get("findings").is_some_and(Value::is_array) {
        return Err("`findings` must be an array".into());
    }
    let mut findings = parse_findings(doc);
    for f in &mut findings {
        if f.evidence.trim().is_empty() {
            f.severity = match f.severity {
                FindingSeverity::Critical => FindingSeverity::High,
                FindingSeverity::High => FindingSeverity::Medium,
                FindingSeverity::Medium => FindingSeverity::Low,
                s => s,
            };
        }
    }
    Ok((parse_verdict(doc.get("verdict")), text_of(doc, &["summary"], 4_000), findings))
}

/// Applies the verification answers to the findings.
pub(crate) fn apply_verification(findings: &mut [ReviewFinding], doc: &Value) {
    let Some(items) = doc.get("responses").and_then(Value::as_array) else { return };
    for f in findings.iter_mut() {
        let Some(r) = items.iter().find(|r| r.get("id").and_then(Value::as_str) == Some(f.id.as_str())) else {
            continue;
        };
        let verdict = r.get("verdict").and_then(Value::as_str).unwrap_or_default().trim().to_ascii_lowercase();
        let evidence = text_of(r, &["evidence", "reason"], 2_000);
        match verdict.as_str() {
            "concede" | "conceded" | "agree" | "accept" => {
                f.status = FindingStatus::Confirmed;
                f.response = Some(evidence);
            }
            "rebut" | "rebutted" | "disagree" | "reject" if !evidence.is_empty() => {
                f.status = FindingStatus::Rebutted;
                f.response = Some(evidence);
            }
            _ => {
                f.response = Some(if evidence.is_empty() { "rebuttal without evidence".into() } else { evidence });
            }
        }
    }
}

/// Verdict of an adversarial review after verification.
#[must_use]
pub fn adversarial_verdict(findings: &[ReviewFinding]) -> ReviewVerdict {
    let serious = findings
        .iter()
        .any(|f| f.is_blocking() || (f.severity == FindingSeverity::High && f.status == FindingStatus::Confirmed));
    if serious {
        ReviewVerdict::RequestChanges
    } else {
        ReviewVerdict::Approve
    }
}

/// Runs an adversarial review + its verification pass to the end (errors
/// land in the document).
pub(crate) async fn run(
    inner: &Inner,
    plan: AdversaryPlan,
    project: &Project,
    task: &Task,
    change: &Change,
    cancel: &CancellationToken,
) -> Review {
    let AdversaryPlan { mut review, chain, verifier } = plan;
    review.diff = change.stats.clone();
    inner.save_review(&review).await;
    let language = inner.deps.config.ui_language.get();
    let brief = task_brief(inner, project, task, change).await;
    let attack = structured_call(
        inner,
        project.id,
        Some(task.id),
        chain,
        adversary_system(&language, review.same_model_fallback),
        format!("{brief}\nTry to refute this change now."),
        "orkestr_adversarial_review",
        review_schema(),
        parse_adversarial,
        cancel,
    )
    .await;
    let (stated, summary, mut findings) = match attack {
        Ok(((v, s, f), out)) => {
            if task.model.as_ref() == Some(&out.model) {
                // Every other model failed; the author's model answered.
                review.same_model_fallback = true;
            }
            review.model = Some(out.model);
            review.usage.add(&out.usage);
            (v, s, f)
        }
        Err(e) => {
            review.status = ReviewStatus::Failed;
            review.error = Some(one_line(&e.to_string(), 500));
            review.updated_at = now();
            review.finished_at = Some(review.updated_at);
            inner.save_review(&review).await;
            return review;
        }
    };
    review.summary = summary;
    review.findings = findings.clone();
    review.status = ReviewStatus::Verifying;
    review.updated_at = now();
    inner.save_review(&review).await;

    if !findings.is_empty() {
        let list: Vec<Value> = findings
            .iter()
            .take(MAX_VERIFY)
            .map(|f| {
                json!({"id": f.id, "severity": format!("{:?}", f.severity).to_ascii_lowercase(), "category": f.category,
                       "file": f.file, "line": f.line, "title": f.title, "detail": f.detail, "evidence": f.evidence})
            })
            .collect();
        let user = format!(
            "{brief}\n## Findings to verify\n{}\n\nAnswer every finding now.",
            serde_json::to_string_pretty(&Value::Array(list)).unwrap_or_default()
        );
        match structured_call(
            inner,
            project.id,
            Some(task.id),
            verifier,
            verify_system(&language),
            user,
            "orkestr_review_verification",
            verify_schema(),
            |doc| {
                if doc.get("responses").is_some_and(Value::is_array) {
                    Ok(())
                } else {
                    Err("`responses` must be an array".to_owned())
                }
            },
            cancel,
        )
        .await
        {
            Ok(((), out)) => {
                apply_verification(&mut findings, &out.doc);
                review.verifier_model = Some(out.model);
                review.usage.add(&out.usage);
            }
            Err(e) => {
                tracing::warn!(review_id = %review.id, error = %e, "adversarial verification failed");
                review.summary.push_str(&format!(
                    "\n\n(Verification by the author model failed: {}. Findings stay open.)",
                    one_line(&e.to_string(), 200)
                ));
            }
        }
    }
    review.findings = findings;
    let computed = adversarial_verdict(&review.findings);
    review.verdict = Some(match (stated, computed) {
        (Some(ReviewVerdict::Reject), ReviewVerdict::RequestChanges) => ReviewVerdict::Reject,
        _ => computed,
    });
    review.status = ReviewStatus::Finished;
    review.updated_at = now();
    review.finished_at = Some(review.updated_at);
    inner.save_review(&review).await;
    inner.review_finished(&review).await;
    tracing::info!(review_id = %review.id, task_id = %task.id, findings = review.findings.len(),
        blocking = review.blocking().len(), same_model = review.same_model_fallback, "adversarial review finished");
    review
}

impl Manager {
    /// `reviews.adversarial`: adversarial review of a task by a different
    /// model, with a verification pass. Does not move the task; unresolved
    /// critical findings block the quality gate.
    pub async fn adversarial_review(&self, p: AdversarialReviewParams) -> OrchResult<ReviewResult> {
        let inner = &self.inner;
        let task = inner.deps.tasks.get(p.task_id).await?;
        let project = inner.project(task.project_id).await?;
        let cancel = inner.quality_token(project.id)?;
        let plan = prepare(inner, &project, &task, p.model.as_ref()).await?;
        let guard = InFlight::acquire(task.id, true)?;
        let mut review = plan.review.clone();
        inner.save_review(&review).await;
        let mgr = self.clone();
        let handle = tokio::spawn(async move {
            let _guard = guard;
            let change = gather_change(&mgr.inner, &project, &task).await;
            run(&mgr.inner, plan, &project, &task, &change, &cancel).await;
        });
        if p.wait.unwrap_or(false) {
            handle.await.map_err(|e| OrchestratorError::Internal(format!("review task failed: {e}")))?;
            review = inner.reviews().review(review.id).await?;
        }
        Ok(ReviewResult { review, adversarial: None })
    }
}

#[cfg(test)]
mod unit {
    use super::*;
    use orkestr_domain::ProviderId;

    fn finding(id: &str, sev: FindingSeverity, evidence: &str) -> ReviewFinding {
        ReviewFinding {
            id: id.into(),
            severity: sev,
            category: "security".into(),
            file: None,
            line: None,
            title: "t".into(),
            detail: String::new(),
            suggestion: String::new(),
            evidence: evidence.into(),
            status: FindingStatus::Open,
            response: None,
            resolution_note: None,
            resolved_at: None,
        }
    }

    #[test]
    fn author_is_the_last_resort_of_the_adversary_chain() {
        let (a, b) = (ProviderId::new(), ProviderId::new());
        let m = |p: ProviderId, n: &str| ModelRef { provider_id: p, model: n.into() };
        let author = m(a, "a1");
        assert_eq!(with_author_last(vec![m(b, "b1")], Some(&author)), vec![m(b, "b1"), author.clone()]);
        assert_eq!(with_author_last(vec![author.clone(), m(b, "b1")], Some(&author)), vec![author.clone(), m(b, "b1")]);
        assert_eq!(with_author_last(vec![m(b, "b1")], None), vec![m(b, "b1")]);
    }

    #[test]
    fn other_providers_first() {
        let (a, b) = (ProviderId::new(), ProviderId::new());
        let m = |p, n: &str| ModelRef { provider_id: p, model: n.into() };
        let out = prefer_other_providers(vec![m(a, "a2"), m(b, "b1"), m(a, "a3")], Some(&m(a, "a1")));
        assert_eq!(out[0].model, "b1");
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn claims_without_evidence_are_downgraded() {
        let doc = json!({"verdict": "request_changes", "summary": "s", "findings": [
            {"severity": "critical", "category": "security", "title": "x", "evidence": ""},
            {"severity": "critical", "category": "security", "title": "y", "evidence": "a.js:3 eval(x)"}
        ]});
        let (_, _, f) = parse_adversarial(&doc).unwrap();
        assert_eq!(f[0].severity, FindingSeverity::High);
        assert_eq!(f[1].severity, FindingSeverity::Critical);
        assert!(parse_adversarial(&json!({"summary": "nothing"})).is_err());
    }

    #[test]
    fn verification_concedes_rebuts_and_keeps_unsupported_rebuttals_open() {
        let mut f = vec![
            finding("f1", FindingSeverity::Critical, "e"),
            finding("f2", FindingSeverity::Critical, "e"),
            finding("f3", FindingSeverity::Critical, "e"),
        ];
        let doc = json!({"responses": [
            {"id": "f1", "verdict": "concede", "evidence": "yes"},
            {"id": "f2", "verdict": "rebut", "evidence": "tests cover it"},
            {"id": "f3", "verdict": "rebut", "evidence": ""}
        ]});
        apply_verification(&mut f, &doc);
        assert_eq!(f[0].status, FindingStatus::Confirmed);
        assert_eq!(f[1].status, FindingStatus::Rebutted);
        assert_eq!(f[2].status, FindingStatus::Open);
        assert_eq!(adversarial_verdict(&f), ReviewVerdict::RequestChanges);
        assert_eq!(adversarial_verdict(&f[1..2]), ReviewVerdict::Approve);
    }
}
