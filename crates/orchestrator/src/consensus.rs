//! Consensus Mode (§8, §68.13).
//!
//! 2–5 models independently propose a solution to one question — in
//! parallel, each with its own isolated context (no participant sees another
//! one's answer) — then a synthesizer (the AI Manager model by default)
//! compares, scores and decides. Alternatives, summaries, scores and the
//! decision are stored ([`ConsensusRun`]) and recorded in the project's
//! Decision Log (`DecisionRecorded`) with every proposal as an alternative.
//!
//! Participants come from the request (`models`) or from routing: the
//! routing rule of `purpose` (model + fallbacks, e.g. `Security` → Claude +
//! GPT, §7) and then other ready models, distinct providers first.
//!
//! Bounds: 2–5 participants, one repair round and a timeout per call, the
//! project's emergency token; at least two successful proposals are needed.

use std::sync::Arc;

use orkestr_domain::{now, Actor, ConsensusId, DecisionAlternative, DecisionStatus, ModelRef, Project, UsageSummary};
use orkestr_protocol::event::{ConsensusPayload, EventKind};
use orkestr_protocol::methods::consensus::{
    ConsensusDecision, ConsensusProposal, ConsensusResult, ConsensusRun, ConsensusStatus, ListConsensusParams,
    ListConsensusResult, ProposalStatus, RunConsensusParams, MAX_CONTEXT, MAX_PARTICIPANTS, MAX_QUESTION,
    MIN_PARTICIPANTS,
};
use orkestr_protocol::methods::memory::CreateDecisionParams;
use orkestr_protocol::NewEvent;
use orkestr_providers::adapters::mock_review::{PROPOSAL_MARKER, SYNTHESIS_MARKER};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::error::{OrchResult, OrchestratorError};
use crate::review::{list_of, role_chain, structured_call, text_of};
use crate::service::{Inner, Manager, MessageExtras};
use crate::util::{cap, is_ru, one_line};

/// Perspectives given to participants (rotated) so a panel of one model
/// still explores different solutions.
const PERSPECTIVES: [&str; 5] = [
    "pragmatic delivery: the simplest thing that fully works",
    "long-term architecture and maintainability",
    "risk, security and failure modes",
    "performance and scalability",
    "minimal change to the existing code",
];

/// Picks `n` participants from an ordered chain: distinct providers first,
/// then distinct models, then (when fewer models exist) reuse with a warning.
#[must_use]
pub fn pick_participants(chain: &[ModelRef], n: usize) -> (Vec<ModelRef>, Option<String>) {
    let mut out: Vec<ModelRef> = Vec::new();
    for m in chain {
        if out.len() < n && !out.iter().any(|o| o.provider_id == m.provider_id) {
            out.push(m.clone());
        }
    }
    for m in chain {
        if out.len() < n && !out.contains(m) {
            out.push(m.clone());
        }
    }
    let distinct = out.len();
    let mut i = 0;
    while out.len() < n && distinct > 0 {
        out.push(out[i % distinct].clone());
        i += 1;
    }
    let warning = (distinct < n && distinct > 0).then(|| {
        format!("only {distinct} distinct model(s) available — participants reuse them with different perspectives")
    });
    (out, warning)
}

fn proposal_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "summary", "approach", "pros", "cons", "risks", "confidence"],
        "properties": {
            "title": {"type": "string"},
            "summary": {"type": "string"},
            "approach": {"type": "string"},
            "pros": {"type": "array", "items": {"type": "string"}},
            "cons": {"type": "array", "items": {"type": "string"}},
            "risks": {"type": "array", "items": {"type": "string"}},
            "confidence": {"type": "number"}
        }
    })
}

fn synthesis_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "decision", "reasoningSummary", "consequences", "chosenProposal", "agreement", "scores"],
        "properties": {
            "title": {"type": "string"},
            "decision": {"type": "string"},
            "reasoningSummary": {"type": "string"},
            "consequences": {"type": "string"},
            "chosenProposal": {"type": ["integer", "null"]},
            "agreement": {"type": "number"},
            "scores": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["proposal", "score", "note"],
                "properties": {
                    "proposal": {"type": "integer"},
                    "score": {"type": "number"},
                    "note": {"type": "string"}
                }
            }}
        }
    })
}

fn unit(v: Option<&Value>) -> f64 {
    let x = v.and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.trim().trim_end_matches('%').parse().ok())));
    match x {
        Some(x) if x > 1.0 && x <= 100.0 => x / 100.0,
        Some(x) if x.is_finite() => x.clamp(0.0, 1.0),
        _ => 0.5,
    }
}

/// Parsed participant answer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedProposal {
    pub title: String,
    pub summary: String,
    pub approach: String,
    pub pros: Vec<String>,
    pub cons: Vec<String>,
    pub risks: Vec<String>,
    pub confidence: f64,
}

pub(crate) fn parse_proposal(doc: &Value) -> Result<ParsedProposal, String> {
    let summary = text_of(doc, &["summary", "solution"], 4_000);
    let approach = text_of(doc, &["approach", "solution", "details", "plan"], 12_000);
    if summary.is_empty() && approach.is_empty() {
        return Err("the proposal needs a `summary` and an `approach`".into());
    }
    let mut title = text_of(doc, &["title", "name"], 200);
    if title.is_empty() {
        title = one_line(if summary.is_empty() { &approach } else { &summary }, 80);
    }
    Ok(ParsedProposal {
        title,
        summary: if summary.is_empty() { one_line(&approach, 400) } else { summary },
        approach,
        pros: list_of(doc, "pros", 10),
        cons: list_of(doc, "cons", 10),
        risks: list_of(doc, "risks", 10),
        confidence: unit(doc.get("confidence")),
    })
}

/// Parsed synthesizer answer: decision + `(proposal number, score, note)`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedSynthesis {
    pub decision: ConsensusDecision,
    pub scores: Vec<(u32, f64, String)>,
}

pub(crate) fn parse_synthesis(doc: &Value, question: &str, numbers: &[u32]) -> Result<ParsedSynthesis, String> {
    let decision = text_of(doc, &["decision", "finalDecision"], 12_000);
    if decision.is_empty() {
        return Err("`decision` must not be empty".into());
    }
    let mut title = text_of(doc, &["title"], 180);
    if title.is_empty() {
        title = one_line(question, 120);
    }
    let chosen = doc
        .get("chosenProposal")
        .or_else(|| doc.get("chosen"))
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| numbers.contains(n));
    let scores = doc
        .get("scores")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    let n = u32::try_from(s.get("proposal")?.as_u64()?).ok().filter(|n| numbers.contains(n))?;
                    let score = s.get("score").and_then(Value::as_f64).filter(|x| x.is_finite())?.clamp(0.0, 10.0);
                    Some((n, score, text_of(s, &["note", "reason"], 500)))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ParsedSynthesis {
        decision: ConsensusDecision {
            title,
            decision,
            reasoning_summary: text_of(doc, &["reasoningSummary", "reasoning_summary", "reasoning"], 8_000),
            consequences: text_of(doc, &["consequences", "followUps"], 8_000),
            chosen_index: chosen.map(|n| n - 1),
            agreement: unit(doc.get("agreement")),
        },
        scores,
    })
}

impl Inner {
    /// Saves + publishes a consensus run (failures are logged).
    pub(crate) async fn save_consensus(&self, run: &ConsensusRun) {
        if let Err(e) = self.reviews().save_consensus(run).await {
            tracing::error!(consensus_id = %run.id, error = %e, "failed to persist a consensus run");
            return;
        }
        let mut ev =
            NewEvent::new(EventKind::ConsensusUpdated(ConsensusPayload { run: run.clone() })).project(run.project_id);
        if let Some(t) = run.task_id {
            ev = ev.task(t);
        }
        self.deps.publish(ev).await;
    }

    fn check_model(&self, project: &Project, m: &ModelRef) -> OrchResult<()> {
        let info = self.deps.models.provider(m.provider_id).ok_or_else(|| {
            OrchestratorError::Invalid(format!("provider {} of model `{}` is not configured", m.provider_id, m.model))
        })?;
        if !project.effective_privacy().allows_provider(m.provider_id, info.is_local) {
            return Err(OrchestratorError::Forbidden(format!(
                "model `{}` is not allowed by the project's AI privacy policy",
                m.model
            )));
        }
        if !info.enabled || !info.chat_capable() {
            return Err(OrchestratorError::Invalid(format!("model `{}` is disabled or not a chat model", m.model)));
        }
        Ok(())
    }

    async fn project_brief(&self, project: &Project) -> String {
        let mut s = format!("## Project\nName: {}\n", project.name);
        if !project.description.trim().is_empty() {
            s.push_str(&format!("Description: {}\n", one_line(&project.description, 500)));
        }
        if let Ok(rules) = self.deps.memory.effective_rules(Some(project.id), None).await {
            for r in rules.iter().take(10) {
                s.push_str(&format!("- Rule {}: {}\n", r.item.title, one_line(&r.item.content, 240)));
            }
        }
        s
    }

    async fn propose(
        &self,
        run: &Arc<tokio::sync::Mutex<ConsensusRun>>,
        index: usize,
        total: usize,
        model: ModelRef,
        user: String,
        cancel: &CancellationToken,
    ) {
        let language = self.deps.config.ui_language.get();
        let system = format!(
            "{PROPOSAL_MARKER}\nYou are one member of an Orkestr consensus panel. Independently propose ONE complete \
             solution to the question. You do not see the other members' answers.\n\
             Participant {} of {total}.\n\
             Emphasise this perspective: {}.\n\
             - Give a short title, a summary, the approach (concrete steps, markdown), pros, cons, risks and your \
             confidence (0..1).\n\
             - Visible reasoning only; no hidden chain-of-thought.\n\n{}\
             - JSON field names stay in English.\n",
            index + 1,
            PERSPECTIVES[index % PERSPECTIVES.len()],
            orkestr_agent_runtime::reply_language_instruction(&language)
        );
        let (project_id, task_id) = {
            let r = run.lock().await;
            (r.project_id, r.task_id)
        };
        let out = structured_call(
            self,
            project_id,
            task_id,
            vec![model.clone()],
            system,
            user,
            "orkestr_consensus_proposal",
            proposal_schema(),
            parse_proposal,
            cancel,
        )
        .await;
        let snapshot = {
            let mut r = run.lock().await;
            if let Some(p) = r.proposals.get_mut(index) {
                match out {
                    Ok((parsed, s)) => {
                        p.status = ProposalStatus::Done;
                        p.answered_by = Some(s.model);
                        p.title = parsed.title;
                        p.summary = parsed.summary;
                        p.approach = parsed.approach;
                        p.pros = parsed.pros;
                        p.cons = parsed.cons;
                        p.risks = parsed.risks;
                        p.confidence = parsed.confidence;
                        p.usage = s.usage;
                    }
                    Err(e) => {
                        p.status = ProposalStatus::Failed;
                        p.error = Some(one_line(&e.to_string(), 400));
                    }
                }
            }
            r.usage = r.proposals.iter().fold(UsageSummary::default(), |mut acc, p| {
                acc.add(&p.usage);
                acc
            });
            r.updated_at = now();
            r.clone()
        };
        self.save_consensus(&snapshot).await;
    }

    /// Runs proposals + synthesis + Decision Log entry (never fails: errors land in the document).
    async fn drive_consensus(
        &self,
        run: ConsensusRun,
        project: Project,
        synthesizer: Vec<ModelRef>,
        record: bool,
        cancel: CancellationToken,
    ) -> ConsensusRun {
        let total = run.proposals.len();
        let mut user = format!("Question: {}\n\n", run.question);
        if !run.context.trim().is_empty() {
            user.push_str(&format!("## Context\n{}\n\n", run.context));
        }
        user.push_str(&self.project_brief(&project).await);
        user.push_str("\nPropose your solution now.");
        let models: Vec<ModelRef> = run.proposals.iter().map(|p| p.model.clone()).collect();
        let shared = Arc::new(tokio::sync::Mutex::new(run));
        let calls =
            models.into_iter().enumerate().map(|(i, m)| self.propose(&shared, i, total, m, user.clone(), &cancel));
        futures::future::join_all(calls).await;

        let mut run = shared.lock().await.clone();
        let done: Vec<&ConsensusProposal> = run.proposals.iter().filter(|p| p.status == ProposalStatus::Done).collect();
        if cancel.is_cancelled() || done.len() < MIN_PARTICIPANTS {
            run.status = ConsensusStatus::Failed;
            run.error = Some(if cancel.is_cancelled() {
                "cancelled".into()
            } else {
                format!("only {} proposal(s) succeeded — consensus needs at least {MIN_PARTICIPANTS}", done.len())
            });
            run.updated_at = now();
            run.finished_at = Some(run.updated_at);
            self.save_consensus(&run).await;
            return run;
        }
        // Synthesis.
        let numbers: Vec<u32> = done.iter().map(|p| p.index + 1).collect();
        let mut s = format!("Question: {}\n\n", run.question);
        if !run.context.trim().is_empty() {
            s.push_str(&format!("## Context\n{}\n\n", cap(&run.context, 8_000)));
        }
        s.push_str("## Independent proposals\n");
        for p in &done {
            s.push_str(&format!(
                "\n### Proposal {} — {} (confidence {:.2})\nSummary: {}\nApproach:\n{}\nPros: {}\nCons: {}\nRisks: {}\n",
                p.index + 1,
                p.title,
                p.confidence,
                p.summary,
                cap(&p.approach, 6_000),
                p.pros.join("; "),
                p.cons.join("; "),
                p.risks.join("; ")
            ));
        }
        s.push_str("\nCompare the proposals and decide now.");
        run.status = ConsensusStatus::Synthesizing;
        run.updated_at = now();
        self.save_consensus(&run).await;
        let language = self.deps.config.ui_language.get();
        let system = format!(
            "{SYNTHESIS_MARKER}\nYou are the AI Manager of Orkestr synthesising a consensus decision. Several models \
             answered the same question independently. Compare the proposals on correctness, risk, cost and fit; \
             score each one 0–10 with a one-line note; then write the final decision — adopt one proposal or combine \
             the best parts — with a reasoning summary and consequences. chosenProposal is the number of the proposal \
             the decision is mainly based on (null when it is a new combination); agreement (0..1) says how much the \
             proposals agree.\n- Visible reasoning only; no hidden chain-of-thought.\n\n{}\
             - JSON field names stay in English.\n",
            orkestr_agent_runtime::reply_language_instruction(&language)
        );
        let question = run.question.clone();
        let out = structured_call(
            self,
            run.project_id,
            run.task_id,
            synthesizer,
            system,
            s,
            "orkestr_consensus_synthesis",
            synthesis_schema(),
            |doc| parse_synthesis(doc, &question, &numbers),
            &cancel,
        )
        .await;
        match out {
            Ok((parsed, o)) => {
                for (n, score, note) in parsed.scores {
                    if let Some(p) = run.proposals.get_mut((n - 1) as usize) {
                        p.score = Some(score);
                        p.score_note = (!note.is_empty()).then_some(note);
                    }
                }
                run.synthesizer = Some(o.model);
                run.usage.add(&o.usage);
                run.decision = Some(parsed.decision);
                run.status = ConsensusStatus::Finished;
            }
            Err(e) => {
                run.status = ConsensusStatus::Failed;
                run.error = Some(format!("synthesis failed: {}", one_line(&e.to_string(), 400)));
            }
        }
        if run.status == ConsensusStatus::Finished && record {
            self.record_decision(&mut run).await;
        }
        run.updated_at = now();
        run.finished_at = Some(run.updated_at);
        self.save_consensus(&run).await;
        tracing::info!(consensus_id = %run.id, status = ?run.status, proposals = run.proposals.len(), "consensus finished");
        run
    }

    async fn record_decision(&self, run: &mut ConsensusRun) {
        let Some(d) = run.decision.clone() else { return };
        // The Decision Log is user-facing data: write it in the UI language.
        let ru = is_ru(&self.deps.config.ui_language.get());
        let alternatives: Vec<DecisionAlternative> = run
            .proposals
            .iter()
            .filter(|p| p.status == ProposalStatus::Done)
            .map(|p| {
                let mut summary = p.summary.clone();
                if let Some(score) = p.score {
                    let pct = p.confidence * 100.0;
                    summary.push_str(&if ru {
                        format!("\n\nОценка: {}/10 · уверенность {pct:.0}\u{a0}%", crate::util::ru_decimal(score, 1))
                    } else {
                        format!("\n\nScore: {score:.1}/10 · confidence {pct:.0}%")
                    });
                }
                summary.push_str(&format!("\n\n{}", p.approach));
                DecisionAlternative {
                    title: cap(&p.title, 180),
                    summary: cap(&summary, 16_000),
                    proposed_by: Some(cap(&p.answered_by.as_ref().unwrap_or(&p.model).model, 400)),
                    rejected_reason: (d.chosen_index != Some(p.index)).then(|| {
                        p.score_note.clone().unwrap_or_else(|| {
                            if ru {
                                "арбитр выбрал другой вариант"
                            } else {
                                "not chosen by the synthesizer"
                            }
                            .to_owned()
                        })
                    }),
                }
            })
            .collect();
        let n = alternatives.len();
        let mut context = if ru {
            format!("Консенсус моделей ({n}) по вопросу: {}", run.question)
        } else {
            format!("Consensus of {n} models on: {}", run.question)
        };
        if !run.context.trim().is_empty() {
            context.push_str(&format!("\n\n{}", cap(&run.context, 16_000)));
        }
        let agreement = d.agreement * 100.0;
        let reasoning = if ru {
            format!("{}\n\nСогласие участников: {agreement:.0}\u{a0}%", d.reasoning_summary)
        } else {
            format!("{}\n\nAgreement: {agreement:.0}%", d.reasoning_summary)
        };
        let params = CreateDecisionParams {
            project_id: run.project_id,
            title: cap(&d.title, 180),
            decision: d.decision.clone(),
            context: Some(context),
            alternatives: Some(alternatives),
            reasoning_summary: Some(reasoning),
            consequences: Some(d.consequences.clone()),
            status: Some(DecisionStatus::Accepted),
            task_ids: run.task_id.map(|t| vec![t]),
            files: None,
            decided_by: Some(Actor::Manager),
        };
        match self.deps.memory.create_decision(params).await {
            Ok(dec) => {
                run.decision_id = Some(dec.id);
                let text = if ru {
                    format!("Решение консенсуса записано в журнал решений (№{}): {}", dec.number, dec.title)
                } else {
                    format!("Consensus decision recorded in the Decision Log (#{}): {}", dec.number, dec.title)
                };
                self.system_event(run.project_id, &text, MessageExtras::default()).await;
            }
            Err(e) => {
                tracing::warn!(consensus_id = %run.id, error = %e, "could not record the consensus decision");
                run.warnings.push(format!("the decision could not be recorded in the Decision Log: {e}"));
            }
        }
    }
}

impl Manager {
    /// `consensus.run`: independent proposals from 2–5 models in parallel,
    /// then a synthesized decision (recorded in the Decision Log by default).
    /// Returns at once (status `Proposing`) unless `wait` is set.
    pub async fn run_consensus(&self, p: RunConsensusParams) -> OrchResult<ConsensusResult> {
        let inner = &self.inner;
        let question = p.question.trim().to_owned();
        if question.is_empty() || question.chars().count() > MAX_QUESTION {
            return Err(OrchestratorError::Invalid(format!("the question must be 1..={MAX_QUESTION} characters")));
        }
        let context = p.context.as_deref().unwrap_or_default().trim().to_owned();
        if context.chars().count() > MAX_CONTEXT {
            return Err(OrchestratorError::Invalid(format!("the context must be at most {MAX_CONTEXT} characters")));
        }
        let project = inner.project(p.project_id).await?;
        if let Some(t) = p.task_id {
            let task = inner.deps.tasks.get(t).await?;
            if task.project_id != project.id {
                return Err(OrchestratorError::Invalid("the task belongs to another project".into()));
            }
        }
        let cancel = inner.quality_token(project.id)?;
        let purpose = p.purpose.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| cap(s, 60));
        let mut warnings = Vec::new();
        let models = match &p.models {
            Some(models) => {
                if !(MIN_PARTICIPANTS..=MAX_PARTICIPANTS).contains(&models.len()) {
                    return Err(OrchestratorError::Invalid(format!(
                        "consensus needs {MIN_PARTICIPANTS}–{MAX_PARTICIPANTS} models"
                    )));
                }
                for m in models {
                    inner.check_model(&project, m)?;
                }
                let mut seen: Vec<&ModelRef> = Vec::new();
                for m in models {
                    if !seen.contains(&m) {
                        seen.push(m);
                    }
                }
                let distinct = seen.len();
                if distinct < models.len() {
                    warnings.push(
                        "the same model was picked more than once — it answers with different perspectives".into(),
                    );
                }
                models.clone()
            }
            None => {
                let n = p.participants.map_or(3, |n| n as usize).clamp(MIN_PARTICIPANTS, MAX_PARTICIPANTS);
                let chain = role_chain(inner, &project, purpose.as_deref().unwrap_or("Consensus"), None, &[]).await?;
                let (picked, warn) = pick_participants(&chain, n);
                warnings.extend(warn);
                picked
            }
        };
        let cfg = inner.config().await;
        let synthesizer = match &p.synthesizer {
            Some(m) => role_chain(inner, &project, "Manager", Some(m), &[]).await?,
            None => match role_chain(inner, &project, "Manager", cfg.model.as_ref(), &[]).await {
                Ok(c) => c,
                Err(_) => role_chain(inner, &project, "Manager", None, &[]).await?,
            },
        };
        let at = now();
        let run = ConsensusRun {
            id: ConsensusId::new(),
            project_id: project.id,
            task_id: p.task_id,
            question,
            context,
            purpose,
            status: ConsensusStatus::Proposing,
            proposals: models
                .into_iter()
                .enumerate()
                .map(|(i, model)| ConsensusProposal {
                    index: u32::try_from(i).unwrap_or(u32::MAX),
                    model,
                    answered_by: None,
                    status: ProposalStatus::Pending,
                    title: String::new(),
                    summary: String::new(),
                    approach: String::new(),
                    pros: Vec::new(),
                    cons: Vec::new(),
                    risks: Vec::new(),
                    confidence: 0.0,
                    score: None,
                    score_note: None,
                    error: None,
                    usage: UsageSummary::default(),
                })
                .collect(),
            synthesizer: synthesizer.first().cloned(),
            decision: None,
            decision_id: None,
            usage: UsageSummary::default(),
            warnings,
            error: None,
            created_at: at,
            updated_at: at,
            finished_at: None,
        };
        inner.save_consensus(&run).await;
        let record = p.record_decision.unwrap_or(true);
        let mgr = self.clone();
        let (doc, proj) = (run.clone(), project);
        let handle =
            tokio::spawn(async move { mgr.inner.drive_consensus(doc, proj, synthesizer, record, cancel).await });
        if p.wait.unwrap_or(false) {
            let run = handle.await.map_err(|e| OrchestratorError::Internal(format!("consensus task failed: {e}")))?;
            return Ok(ConsensusResult { run });
        }
        Ok(ConsensusResult { run })
    }

    /// `consensus.list`.
    pub async fn list_consensus(&self, p: ListConsensusParams) -> OrchResult<ListConsensusResult> {
        self.inner.project(p.project_id).await?;
        let limit = p.limit.unwrap_or(50).clamp(1, 500);
        Ok(ListConsensusResult { runs: self.inner.reviews().consensus_runs(p.project_id, limit).await? })
    }

    /// `consensus.get`.
    pub async fn get_consensus(&self, id: ConsensusId) -> OrchResult<ConsensusResult> {
        Ok(ConsensusResult { run: self.inner.reviews().consensus(id).await? })
    }
}

#[cfg(test)]
mod unit {
    use super::*;
    use orkestr_domain::ProviderId;

    #[test]
    fn participants_prefer_distinct_providers_and_reuse_with_a_warning() {
        let (a, b) = (ProviderId::new(), ProviderId::new());
        let m = |p, n: &str| ModelRef { provider_id: p, model: n.into() };
        let (out, w) = pick_participants(&[m(a, "a1"), m(a, "a2"), m(b, "b1")], 2);
        assert_eq!(out, vec![m(a, "a1"), m(b, "b1")]);
        assert!(w.is_none());
        let (out, w) = pick_participants(&[m(a, "a1")], 3);
        assert_eq!(out.len(), 3);
        assert!(w.is_some());
    }

    #[test]
    fn proposals_and_synthesis_parse_leniently() {
        let p =
            parse_proposal(&json!({"summary": "s", "approach": "a", "pros": "- x\n- y", "confidence": 80})).unwrap();
        assert_eq!(p.pros, vec!["x", "y"]);
        assert!((p.confidence - 0.8).abs() < 1e-9);
        assert!(parse_proposal(&json!({"title": "t"})).is_err());
        let s = parse_synthesis(
            &json!({"decision": "d", "chosenProposal": 2, "agreement": 0.7, "scores": [{"proposal": 2, "score": 12, "note": "n"}, {"proposal": 9, "score": 1}]}),
            "q?",
            &[1, 2],
        )
        .unwrap();
        assert_eq!(s.decision.chosen_index, Some(1));
        assert_eq!(s.scores, vec![(2, 10.0, "n".to_owned())]);
        assert!(parse_synthesis(&json!({"title": "x"}), "q", &[1]).is_err());
    }
}
