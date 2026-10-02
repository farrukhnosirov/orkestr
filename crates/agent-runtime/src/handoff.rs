//! Structured handoffs (§57, agent-runtime-spec §8): created on provider
//! fallback, stop, failure and completion. Built from the runtime's own
//! progress records — never from the raw chat history — and capped at ≈ 4k
//! tokens.

use orkestr_domain::{now, AgentInstance, ModelRef, SnapshotId, Task};
use orkestr_protocol::methods::agents::{
    Handoff, HandoffArtifact, HandoffBudget, HandoffDecision, HandoffFile, HandoffParty, HandoffReason, HandoffTask,
    HandoffTestState, HandoffWorkspace,
};

/// Max characters of the free-text context (≈ 1500 tokens).
pub const MAX_CONTEXT_CHARS: usize = 6_000;
/// Max characters of the rendered handoff (≈ 4k tokens).
pub const MAX_RENDERED_CHARS: usize = 16_000;
const MAX_ITEMS: usize = 20;
const MAX_ITEM_CHARS: usize = 300;

/// Work progress the runtime tracks while an agent runs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    /// Files written / edited (worktree-relative).
    pub files_changed: Vec<String>,
    /// Decisions recorded via `record_decision`.
    pub decisions: Vec<HandoffDecision>,
    /// Short descriptions of completed steps (successful mutating tool calls).
    pub done: Vec<String>,
    /// Last verification command.
    pub test_state: Option<HandoffTestState>,
    /// Latest checkpoint.
    pub snapshot_id: Option<SnapshotId>,
    /// Last visible assistant text (summary of where the agent is).
    pub last_text: String,
    /// Open issues reported by the agent.
    pub open_issues: Vec<String>,
}

impl Progress {
    /// Adds a changed file once.
    pub fn file(&mut self, path: &str) {
        if !self.files_changed.iter().any(|f| f == path) {
            self.files_changed.push(path.to_owned());
        }
    }

    /// Adds a completed step (bounded).
    pub fn step(&mut self, s: impl Into<String>) {
        self.done.push(s.into());
        if self.done.len() > 200 {
            self.done.remove(0);
        }
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

fn clip_list(items: &[String]) -> Vec<String> {
    let start = items.len().saturating_sub(MAX_ITEMS);
    items[start..].iter().map(|s| clip(s, MAX_ITEM_CHARS)).collect()
}

/// Inputs of [`build`].
#[derive(Debug, Clone)]
pub struct HandoffInput<'a> {
    /// Producing agent.
    pub agent: &'a AgentInstance,
    /// Task.
    pub task: &'a Task,
    /// Why.
    pub reason: HandoffReason,
    /// Receiving model (fallback).
    pub to_model: Option<ModelRef>,
    /// Progress.
    pub progress: &'a Progress,
    /// Free-text state (e.g. failure reason).
    pub note: Option<String>,
    /// Turns left.
    pub turns_left: u32,
    /// USD left (agent cost limit).
    pub cost_left: Option<f64>,
}

/// Builds a capped handoff.
#[must_use]
pub fn build(input: &HandoffInput<'_>) -> Handoff {
    let p = input.progress;
    let mut context = String::new();
    if let Some(n) = &input.note {
        context.push_str(n.trim());
        context.push_str("\n\n");
    }
    if !p.last_text.trim().is_empty() {
        context.push_str("Latest agent message: ");
        context.push_str(p.last_text.trim());
    }
    let remaining: Vec<String> =
        input.task.acceptance_criteria.iter().filter(|c| !c.met).map(|c| c.text.clone()).collect();
    let mut artifacts = Vec::new();
    if let Some(s) = p.snapshot_id {
        artifacts.push(HandoffArtifact {
            kind: "snapshot".into(),
            reference: s.to_string(),
            label: "Latest checkpoint".into(),
        });
    }
    let mut h = Handoff {
        handoff_id: uuid::Uuid::now_v7().to_string(),
        schema_version: 1,
        created_at: now(),
        from: HandoffParty {
            agent_id: Some(input.agent.id),
            role: Some(input.agent.role.clone()),
            model: input.agent.model.clone(),
        },
        to: input.to_model.clone().map(|m| HandoffParty {
            agent_id: Some(input.agent.id),
            role: Some(input.agent.role.clone()),
            model: Some(m),
        }),
        reason: input.reason,
        task: HandoffTask {
            task_id: input.task.id,
            title: input.task.title.clone(),
            goal: clip(&input.task.description, 2_000),
            acceptance_criteria: clip_list(
                &input.task.acceptance_criteria.iter().map(|c| c.text.clone()).collect::<Vec<_>>(),
            ),
        },
        context: clip(&context, MAX_CONTEXT_CHARS),
        relevant_files: clip_list(&p.files_changed)
            .into_iter()
            .map(|path| HandoffFile { path, why: "changed by the agent".into() })
            .collect(),
        decisions: p.decisions.iter().rev().take(MAX_ITEMS).rev().cloned().collect(),
        done: clip_list(&p.done),
        remaining: clip_list(&remaining),
        unresolved_issues: clip_list(&p.open_issues),
        test_state: p.test_state.clone(),
        artifacts,
        workspace: HandoffWorkspace {
            branch: input.agent.worktree.as_ref().map(|w| w.branch.clone()).filter(|b| !b.is_empty()),
            worktree: input.agent.worktree.as_ref().map(|w| w.path.clone()),
            snapshot_id: p.snapshot_id,
            files_changed: clip_list(&p.files_changed),
        },
        budget_remaining: HandoffBudget { turns: input.turns_left, cost_usd: input.cost_left },
    };
    // Enforce the global cap by shrinking the largest free-text parts.
    while render(&h).chars().count() > MAX_RENDERED_CHARS && (h.context.len() > 200 || h.done.len() > 3) {
        if h.context.len() > 200 {
            let keep = h.context.chars().count() / 2;
            h.context = clip(&h.context, keep);
        } else {
            h.done.drain(..h.done.len() / 2);
        }
    }
    h
}

fn list(out: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    out.push_str(&format!("{title}:\n"));
    for i in items {
        out.push_str(&format!("- {i}\n"));
    }
}

/// Concise text form for prompts.
#[must_use]
pub fn render(h: &Handoff) -> String {
    let mut out = format!("Handoff ({:?}) for task \"{}\"\n", h.reason, h.task.title);
    if let Some(m) = &h.from.model {
        out.push_str(&format!("Previous model: {}\n", m.model));
    }
    if !h.context.is_empty() {
        out.push_str(&format!("State: {}\n", h.context));
    }
    list(&mut out, "Done", &h.done);
    list(&mut out, "Remaining acceptance criteria", &h.remaining);
    list(&mut out, "Files changed", &h.workspace.files_changed);
    let decisions: Vec<String> = h.decisions.iter().map(|d| format!("{}: {}", d.title, d.choice)).collect();
    list(&mut out, "Decisions", &decisions);
    list(&mut out, "Unresolved issues", &h.unresolved_issues);
    if let Some(t) = &h.test_state {
        out.push_str(&format!(
            "Last check: `{}` {} — {}\n",
            t.command,
            if t.passed { "passed" } else { "FAILED" },
            t.summary
        ));
    }
    if let Some(b) = &h.workspace.branch {
        out.push_str(&format!("Branch: {b}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::{
        AcceptanceCriterion, AgentId, AgentRole, AgentState, ProjectId, UsageSummary, WorktreePolicy,
    };

    fn agent() -> AgentInstance {
        AgentInstance {
            id: AgentId::new(),
            template_id: None,
            project_id: ProjectId::new(),
            task_id: None,
            team_id: None,
            parent_agent_id: None,
            name: "Backend #1".into(),
            role: AgentRole::Backend,
            state: AgentState::Thinking,
            model: None,
            worktree: None,
            usage: UsageSummary::default(),
            turns_used: 3,
            last_error: None,
            created_at: now(),
            started_at: None,
            stopped_at: None,
            last_activity_at: None,
            template_version: None,
            worktree_policy: WorktreePolicy::SharedWorktree,
            max_turns: 10,
            budget: None,
            execution_id: None,
            execution_style: None,
            cli_session: None,
            activity: None,
        }
    }

    #[test]
    fn handoff_is_capped_and_structured() {
        let a = agent();
        let mut t = Task::new(a.project_id, "Payroll export", now());
        t.acceptance_criteria = vec![
            AcceptanceCriterion { text: "CSV export".into(), met: true },
            AcceptanceCriterion { text: "Unit tests".into(), met: false },
        ];
        let mut p = Progress { last_text: "x".repeat(50_000), ..Progress::default() };
        for i in 0..500 {
            p.step(format!("step {i} {}", "y".repeat(400)));
            p.file(&format!("src/f{}.rs", i % 7));
        }
        p.file("src/f1.rs");
        let h = build(&HandoffInput {
            agent: &a,
            task: &t,
            reason: HandoffReason::ProviderFallback,
            to_model: None,
            progress: &p,
            note: Some("rate limited".into()),
            turns_left: 7,
            cost_left: Some(1.5),
        });
        assert!(render(&h).chars().count() <= MAX_RENDERED_CHARS);
        assert!(h.context.chars().count() <= MAX_CONTEXT_CHARS + 1);
        assert_eq!(h.remaining, vec!["Unit tests".to_owned()]);
        assert_eq!(h.workspace.files_changed.len(), 7, "deduplicated");
        assert!(h.done.len() <= MAX_ITEMS);
        assert_eq!(h.budget_remaining.turns, 7);
        let json = serde_json::to_value(&h).unwrap();
        assert_eq!(json["schemaVersion"], 1);
        assert_eq!(json["reason"], "ProviderFallback");
        assert!(render(&h).contains("Payroll export"));
    }
}
