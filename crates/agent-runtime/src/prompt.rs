//! System prompt + initial context (§20 context optimizer, §57 handoff).

use std::path::Path;

use orkestr_domain::{AgentInstance, AgentRole, AgentTemplate, Project, Task, WorktreePolicy};

use crate::deps::RuntimeDeps;

/// Display name of a role (also the routing `purpose`).
#[must_use]
pub fn role_name(role: &AgentRole) -> String {
    match role {
        AgentRole::Custom(s) => s.clone(),
        AgentRole::UiUx => "UI/UX".into(),
        AgentRole::Qa => "QA".into(),
        AgentRole::ProductManager => "Product Manager".into(),
        AgentRole::DevOps => "DevOps".into(),
        other => format!("{other:?}"),
    }
}

/// Substitutes `{{project.name}}`, `{{task.title}}`, `{{agent.role}}`, `{{agent.name}}`.
#[must_use]
pub fn substitute(text: &str, project: &Project, task: &Task, agent: &AgentInstance) -> String {
    text.replace("{{project.name}}", &project.name)
        .replace("{{task.title}}", &task.title)
        .replace("{{agent.role}}", &role_name(&agent.role))
        .replace("{{agent.name}}", &agent.name)
}

fn policy_text(policy: WorktreePolicy) -> &'static str {
    match policy {
        WorktreePolicy::DedicatedWorktree => {
            "your own git worktree on your own branch — commit there with git_commit; nobody else edits it"
        }
        WorktreePolicy::SharedWorktree => {
            "the shared project checkout — keep changes focused; git_commit is not available"
        }
        WorktreePolicy::ReadOnlyMain => "the main checkout, read-only — you cannot modify files or run commands",
        WorktreePolicy::None => "no filesystem access",
    }
}

/// The full system prompt: template prompt + runtime rules + reply language
/// (`language`: the user's UI language, e.g. `ru`).
#[must_use]
pub fn system_prompt(
    template: &AgentTemplate,
    project: &Project,
    task: &Task,
    agent: &AgentInstance,
    tools: &[String],
    language: &str,
) -> String {
    let mut s = substitute(&template.system_prompt, project, task, agent);
    s.push_str("\n\n## Orkestr runtime\n");
    s.push_str(&format!(
        "You are agent \"{}\" (role {}) running inside Orkestr. You work alone on one task with real tools.\n",
        agent.name,
        role_name(&agent.role)
    ));
    match agent.worktree.as_ref() {
        Some(w) if agent.worktree_policy != WorktreePolicy::None => s.push_str(&format!(
            "- Workspace: {} ({}). All paths are relative to it; anything outside it is refused.\n",
            w.path,
            policy_text(agent.worktree_policy)
        )),
        _ => s.push_str("- Workspace: none — you have no filesystem access.\n"),
    }
    s.push_str(&format!("- Tools available: {}.\n", tools.join(", ")));
    s.push_str(
        "- Every action is checked by the permission engine. A denied call returns an error: adapt, do not repeat the same call.\n\
         - Dangerous commands and some writes need the user's approval; you wait automatically until they decide.\n\
         - Never print, store or commit secrets (keys, tokens, passwords). Do not include hidden reasoning in files or messages.\n",
    );
    s.push_str(&format!(
        "- You have at most {} turns. Work efficiently: explore, change, verify.\n\
         - When finished (or blocked), call submit_result exactly once with status (done/partial/blocked/failed), \
         summary, files_changed, tests_run and open_issues.\n",
        agent.max_turns
    ));
    s.push_str("\n## Language\n");
    s.push_str(&crate::language::reply_language_instruction(language));
    s
}

/// Builds the initial user message: task, acceptance criteria and the
/// context bundle (rules, memory, relevant files, previous handoff).
pub async fn initial_message(
    deps: &RuntimeDeps,
    template: &AgentTemplate,
    project: &Project,
    task: &Task,
    root: Option<&Path>,
    handoff: Option<String>,
) -> String {
    // Full Context Optimizer (§20): cached index + import graph, churn, rule
    // precedence, memory relevance, model-aware budget, compressed handoff.
    let opts = crate::context_plan::ContextOptions { summarize: true, ..crate::context_plan::ContextOptions::default() };
    let planned =
        crate::context_plan::plan_context(deps, template.memory_access, project, task, root, handoff, &opts).await;
    let mut out = planned.task_header;
    out.push_str("\n# Context (selected by Orkestr)\n");
    out.push_str(&planned.plan.bundle.render());
    out.push_str("\nStart by exploring what you need, then do the work and finish with submit_result.\n");
    out
}
