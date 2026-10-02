//! System prompt + initial context (§20 context optimizer, §57 handoff).

use std::path::Path;

use orkestr_context::{build_bundle, ContextRequest, FileIndex, FsReader, IndexOptions};
use orkestr_domain::{AgentInstance, AgentRole, AgentTemplate, MemoryAccess, Project, Task, WorktreePolicy};
use orkestr_protocol::methods::memory::SearchMemoryParams;

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
    let mut task_text = format!("# Task: {}\n", task.title);
    if !task.description.trim().is_empty() {
        task_text.push_str(&format!("\n{}\n", task.description.trim()));
    }
    if !task.acceptance_criteria.is_empty() {
        task_text.push_str("\nAcceptance criteria:\n");
        for c in &task.acceptance_criteria {
            task_text.push_str(&format!("- [{}] {}\n", if c.met { "x" } else { " " }, c.text));
        }
    }

    let (rules, memory) = if template.memory_access == MemoryAccess::None {
        (Vec::new(), Vec::new())
    } else {
        let rules = match deps.memory.effective_rules(Some(project.id), None).await {
            Ok(r) => r.into_iter().map(|r| r.item).collect(),
            Err(e) => {
                tracing::warn!(error = %e, "agent context: effective rules unavailable");
                Vec::new()
            }
        };
        let hits = deps
            .memory
            .search(SearchMemoryParams {
                query: task.title.clone(),
                project_id: Some(project.id),
                include_global: Some(template.memory_access != MemoryAccess::ProjectOnly),
                kinds: None,
                limit: Some(8),
            })
            .await
            .map(|h| h.into_iter().map(|h| h.item).collect())
            .unwrap_or_default();
        (rules, hits)
    };

    let req = ContextRequest {
        task: format!("{}\n{}", task.title, task.description),
        token_budget: deps.config.context_token_budget,
        rules,
        memory,
        handoff,
        max_files: 8,
    };
    let bundle = match root.map(Path::to_path_buf) {
        Some(root) => tokio::task::spawn_blocking(move || {
            let index = FileIndex::build(&root, IndexOptions::default()).unwrap_or_default();
            build_bundle(&index, &FsReader::new(&root), &req)
        })
        .await
        .ok(),
        None => {
            let empty = FileIndex::default();
            Some(build_bundle(&empty, &FsReader::new(std::env::temp_dir()), &req))
        }
    };
    let mut out = task_text;
    if let Some(b) = bundle {
        out.push_str("\n# Context (selected by Orkestr)\n");
        out.push_str(&b.render());
    }
    out.push_str("\nStart by exploring what you need, then do the work and finish with submit_result.\n");
    out
}
