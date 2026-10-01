//! # orkestr-orchestrator — **SKELETON** (Phase 1–2 fill it in)
//!
//! The AI Manager (MASTER_SPEC §1, §5, §8, §9, §16, §17):
//!
//! 1. study the project + memory, 2. build a [`Plan`], 3. decompose into
//!    tasks with dependencies, 4. decide how many agents / which models,
//!    5. create branches/worktrees, 6. run independent tasks in parallel,
//!    7. watch resources/limits/budget, 8. structured handoffs, 9. build →
//!    test → review → fix loops (bounded), 10. merge via the Merge Manager,
//!    11. ask the user only for decisions/approvals, 12. keep useful history
//!    (never hidden chain-of-thought).
//!
//! Also: consensus mode (independent proposals → synthesis, alternatives kept
//! in the decision log) and adversarial review. The orchestrator composes
//! engines through traits; it is not a god service (§64).
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use async_trait::async_trait;
use orkestr_domain::{AgentRole, ModelRef, ProjectId, TaskPriority};
use serde::{Deserialize, Serialize};

/// Errors of the orchestrator.
#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    /// Planning failed.
    #[error("planning failed: {0}")]
    Planning(String),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

/// A planned unit of work before it becomes a `Task`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedTask {
    /// Local key used for dependencies inside the plan.
    pub key: String,
    /// Title.
    pub title: String,
    /// Description.
    pub description: String,
    /// Acceptance criteria.
    pub acceptance_criteria: Vec<String>,
    /// Keys of prerequisite planned tasks.
    pub depends_on: Vec<String>,
    /// Suggested role.
    pub role: AgentRole,
    /// Suggested model.
    pub model: Option<ModelRef>,
    /// Priority.
    pub priority: TaskPriority,
}

/// A Manager plan (§1 step 2–4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// Project.
    pub project_id: ProjectId,
    /// User goal.
    pub goal: String,
    /// Summary shown to the user.
    pub summary: String,
    /// Tasks.
    pub tasks: Vec<PlannedTask>,
    /// Max parallel agents.
    pub max_parallel_agents: u32,
}

/// The AI Manager.
#[async_trait]
pub trait Manager: Send + Sync {
    /// Builds a plan for a goal (no side effects).
    async fn plan(&self, project: ProjectId, goal: &str) -> Result<Plan, OrchestratorError>;
    /// Executes an approved plan (creates tasks, spawns agents, …).
    async fn execute(&self, plan: Plan) -> Result<(), OrchestratorError>;
}

/// Validates that plan dependencies reference existing keys and contain no cycle.
pub fn validate_plan(plan: &Plan) -> Result<(), OrchestratorError> {
    use std::collections::{HashMap, HashSet};
    let keys: HashSet<&str> = plan.tasks.iter().map(|t| t.key.as_str()).collect();
    if keys.len() != plan.tasks.len() {
        return Err(OrchestratorError::Planning("duplicate task keys".into()));
    }
    let deps: HashMap<&str, Vec<&str>> =
        plan.tasks.iter().map(|t| (t.key.as_str(), t.depends_on.iter().map(String::as_str).collect())).collect();
    for (k, ds) in &deps {
        if let Some(missing) = ds.iter().find(|d| !keys.contains(*d)) {
            return Err(OrchestratorError::Planning(format!("{k} depends on unknown {missing}")));
        }
    }
    // Kahn's algorithm.
    let mut indegree: HashMap<&str, usize> = deps.iter().map(|(k, d)| (*k, d.len())).collect();
    let mut ready: Vec<&str> = indegree.iter().filter(|(_, n)| **n == 0).map(|(k, _)| *k).collect();
    let mut seen = 0;
    while let Some(k) = ready.pop() {
        seen += 1;
        for (other, ds) in &deps {
            if ds.contains(&k) {
                let n = indegree.get_mut(other).expect("key exists");
                *n -= 1;
                if *n == 0 {
                    ready.push(other);
                }
            }
        }
    }
    if seen == plan.tasks.len() {
        Ok(())
    } else {
        Err(OrchestratorError::Planning("dependency cycle".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(key: &str, deps: &[&str]) -> PlannedTask {
        PlannedTask {
            key: key.into(),
            title: key.into(),
            description: String::new(),
            acceptance_criteria: vec![],
            depends_on: deps.iter().map(|d| (*d).to_owned()).collect(),
            role: AgentRole::Backend,
            model: None,
            priority: TaskPriority::Normal,
        }
    }

    fn plan(tasks: Vec<PlannedTask>) -> Plan {
        Plan { project_id: ProjectId::new(), goal: "g".into(), summary: "s".into(), tasks, max_parallel_agents: 2 }
    }

    #[test]
    fn plan_validation() {
        validate_plan(&plan(vec![t("a", &[]), t("b", &["a"]), t("c", &["a", "b"])])).unwrap();
        assert!(validate_plan(&plan(vec![t("a", &["b"]), t("b", &["a"])])).is_err());
        assert!(validate_plan(&plan(vec![t("a", &["zzz"])])).is_err());
        assert!(validate_plan(&plan(vec![t("a", &[]), t("a", &[])])).is_err());
    }
}
