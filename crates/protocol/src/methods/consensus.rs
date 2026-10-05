//! `consensus.*` — Consensus Mode (§8, §68.13). Phase 2.
//!
//! N (2–5) models independently propose a solution to one question (in
//! parallel, each in its own isolated context); a synthesizer (the AI
//! Manager model by default) compares the proposals, scores them and writes
//! the final decision. Alternatives, summaries, scores and the decision are
//! stored and — by default — recorded in the project's Decision Log
//! (`DecisionRecorded`).
//!
//! Participants are chosen per request (`models`) or per route: the routing
//! rule of `purpose` (e.g. `Security`) supplies its model plus fallbacks
//! (§7: "Security → Claude + GPT consensus").
//!
//! Events (topic `project:<id>`): `ConsensusUpdated` on every change and
//! `DecisionRecorded` when the decision lands in the Decision Log.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{ConsensusId, DecisionId, ModelRef, ProjectId, TaskId, Timestamp, UsageSummary};

use super::method;

/// Minimum participants.
pub const MIN_PARTICIPANTS: usize = 2;
/// Maximum participants.
pub const MAX_PARTICIPANTS: usize = 5;
/// Max characters of the question.
pub const MAX_QUESTION: usize = 8_000;
/// Max characters of extra context.
pub const MAX_CONTEXT: usize = 40_000;

/// Lifecycle of a consensus run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ConsensusStatus {
    /// Participants are proposing.
    Proposing,
    /// The synthesizer is deciding.
    Synthesizing,
    /// Decided.
    Finished,
    /// Could not decide (too few proposals, no model, cancelled).
    Failed,
}

impl ConsensusStatus {
    /// Whether the run is over.
    #[must_use]
    pub fn is_final(self) -> bool {
        matches!(self, Self::Finished | Self::Failed)
    }
}

/// Status of one proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ProposalStatus {
    /// Waiting for the model.
    Pending,
    /// Proposed.
    Done,
    /// The model failed or answered invalidly.
    Failed,
}

/// One participant's independent proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConsensusProposal {
    /// Position (0-based; also the participant number − 1).
    pub index: u32,
    /// Model requested for this participant.
    pub model: ModelRef,
    /// Model that answered (after fallbacks).
    pub answered_by: Option<ModelRef>,
    /// Status.
    pub status: ProposalStatus,
    /// Short name of the approach.
    pub title: String,
    /// Summary (visible output only).
    pub summary: String,
    /// The proposed solution (markdown).
    pub approach: String,
    /// Advantages.
    pub pros: Vec<String>,
    /// Disadvantages.
    pub cons: Vec<String>,
    /// Risks.
    pub risks: Vec<String>,
    /// Self-reported confidence 0..=1.
    pub confidence: f64,
    /// Synthesizer score 0..=10.
    pub score: Option<f64>,
    /// Synthesizer note on this proposal.
    pub score_note: Option<String>,
    /// Failure reason.
    pub error: Option<String>,
    /// Usage of this call.
    pub usage: UsageSummary,
}

/// The synthesized decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConsensusDecision {
    /// Decision title.
    pub title: String,
    /// The decision (markdown).
    pub decision: String,
    /// Reasoning summary (never hidden chain-of-thought).
    pub reasoning_summary: String,
    /// Consequences / follow-ups.
    pub consequences: String,
    /// Proposal the decision is mainly based on (index), if any.
    pub chosen_index: Option<u32>,
    /// How strongly the participants agreed (0..=1).
    pub agreement: f64,
}

/// One consensus run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConsensusRun {
    /// Id.
    pub id: ConsensusId,
    /// Project.
    pub project_id: ProjectId,
    /// Related task.
    pub task_id: Option<TaskId>,
    /// The question / design task.
    pub question: String,
    /// Extra context given to every participant.
    pub context: String,
    /// Routing purpose used to pick participants (e.g. `Security`).
    pub purpose: Option<String>,
    /// Status.
    pub status: ConsensusStatus,
    /// Proposals (one per participant).
    pub proposals: Vec<ConsensusProposal>,
    /// Synthesizer model.
    pub synthesizer: Option<ModelRef>,
    /// The decision.
    pub decision: Option<ConsensusDecision>,
    /// Decision Log entry.
    pub decision_id: Option<DecisionId>,
    /// Total usage.
    pub usage: UsageSummary,
    /// Notes (participant fallbacks, duplicates removed, …).
    pub warnings: Vec<String>,
    /// Failure reason.
    pub error: Option<String>,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
    /// Finished.
    pub finished_at: Option<Timestamp>,
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

/// Params of `consensus.run`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunConsensusParams {
    /// Project.
    pub project_id: ProjectId,
    /// The question or design task.
    pub question: String,
    /// Extra context (constraints, excerpts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context: Option<String>,
    /// Participants (2–5). Default: the routing rule of `purpose` (model + fallbacks), then routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub models: Option<Vec<ModelRef>>,
    /// Routing purpose that picks default participants (e.g. `Security`, `Architect`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub purpose: Option<String>,
    /// Number of participants when `models` is not given (default 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub participants: Option<u32>,
    /// Synthesizer (default: the AI Manager model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub synthesizer: Option<ModelRef>,
    /// Related task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Record the decision in the Decision Log (default: true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub record_decision: Option<bool>,
    /// Wait for the decision before answering (default: false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wait: Option<bool>,
}

/// Result of `consensus.run` / `consensus.get`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConsensusResult {
    /// The run.
    pub run: ConsensusRun,
}

/// Params of `consensus.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListConsensusParams {
    /// Project.
    pub project_id: ProjectId,
    /// Max runs (default 50, max 500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `consensus.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListConsensusResult {
    /// Runs, newest first.
    pub runs: Vec<ConsensusRun>,
}

/// Params of `consensus.get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GetConsensusParams {
    /// Run.
    pub consensus_id: ConsensusId,
}

method!(
    /// `consensus.run` — independent proposals from 2–5 models + a synthesized decision.
    Run, "consensus.run", RunConsensusParams, ConsensusResult
);
method!(
    /// `consensus.list` — consensus runs of a project.
    List, "consensus.list", ListConsensusParams, ListConsensusResult
);
method!(
    /// `consensus.get` — one consensus run.
    Get, "consensus.get", GetConsensusParams, ConsensusResult
);
