//! Knowledge tools: `record_memory`, `record_decision` (memory engine §20–21)
//! and `create_snapshot` (§25). Snapshots are also taken automatically before
//! an agent's first write (see `run.rs`).

use orkestr_domain::{
    Actor, DecisionAlternative, MemoryKind, MemoryScope, SnapshotReason, TaskSnapshotPhase, WorkspaceSnapshot,
};
use orkestr_protocol::event::{EventKind, SnapshotCreatedPayload};
use orkestr_protocol::methods::memory::{CreateDecisionParams, CreateMemoryParams};
use orkestr_protocol::NewEvent;
use orkestr_snapshots::CreateSnapshot;
use serde_json::Value;

use super::{arg_str, opt_list, opt_str, ToolEnv, ToolError, ToolOutput};

fn parse_kind(s: Option<&str>) -> MemoryKind {
    s.and_then(|k| serde_json::from_value(Value::String(k.to_owned())).ok()).unwrap_or(MemoryKind::Note)
}

/// `record_memory`.
pub async fn record_memory(args: &Value, env: &ToolEnv) -> Result<ToolOutput, ToolError> {
    let item = env
        .deps
        .memory
        .create_item(CreateMemoryParams {
            scope: MemoryScope::Project { project_id: env.project_id },
            kind: parse_kind(opt_str(args, "kind")),
            title: arg_str(args, "title")?.to_owned(),
            content: arg_str(args, "content")?.to_owned(),
            tags: Some(opt_list(args, "tags")),
            source: Some(format!("agent:{}", env.agent_id)),
            pinned: None,
            key: None,
        })
        .await
        .map_err(|e| ToolError::new(e.to_string()))?;
    Ok(ToolOutput::ok(format!("memory item {} recorded: {}", item.id, item.title), format!("memory: {}", item.title)))
}

/// `record_decision`.
pub async fn record_decision(args: &Value, env: &ToolEnv) -> Result<ToolOutput, ToolError> {
    let alternatives: Vec<DecisionAlternative> = opt_list(args, "alternatives")
        .into_iter()
        .map(|a| DecisionAlternative {
            title: a.chars().take(200).collect(),
            summary: a,
            proposed_by: Some(format!("agent:{}", env.agent_id)),
            rejected_reason: None,
        })
        .collect();
    let d = env
        .deps
        .memory
        .create_decision(CreateDecisionParams {
            project_id: env.project_id,
            title: arg_str(args, "title")?.to_owned(),
            decision: arg_str(args, "decision")?.to_owned(),
            context: opt_str(args, "context").map(str::to_owned),
            alternatives: (!alternatives.is_empty()).then_some(alternatives),
            reasoning_summary: opt_str(args, "rationale_summary").map(str::to_owned),
            consequences: None,
            status: None,
            task_ids: Some(vec![env.task_id]),
            files: None,
            decided_by: Some(Actor::Agent(env.agent_id)),
        })
        .await
        .map_err(|e| ToolError::new(e.to_string()))?;
    Ok(ToolOutput::ok(format!("decision {} recorded: {}", d.id, d.title), format!("decision: {}", d.title)))
}

/// Takes a snapshot of the agent root and publishes `SnapshotCreated`.
///
/// `task_files` (the paths the agent changed itself) is recorded when the
/// agent works in a root shared with other writers (no own branch): other
/// agents and the user may change files there meanwhile, and rolling the task
/// back must revert only the agent's own changes.
pub async fn snapshot(
    env: &ToolEnv,
    label: String,
    reason: SnapshotReason,
    phase: Option<TaskSnapshotPhase>,
    task_files: Option<&[String]>,
) -> Result<WorkspaceSnapshot, ToolError> {
    let root = env.root()?.to_path_buf();
    let req = CreateSnapshot {
        project_id: env.project_id,
        workspace_path: root,
        label: Some(label),
        reason,
        task_id: Some(env.task_id),
        agent_id: Some(env.agent_id),
        task_phase: phase,
        created_by: Actor::Agent(env.agent_id),
    };
    let snaps = &env.deps.snapshots;
    let created = match task_files {
        Some(files) if env.own_branch.is_none() => snaps.create_with_task_files(req, files).await,
        _ => snaps.create(req).await,
    };
    let snap = created.map_err(|e| ToolError::new(format!("snapshot failed: {e}")))?;
    env.deps
        .publish(
            NewEvent::new(EventKind::SnapshotCreated(SnapshotCreatedPayload { snapshot: snap.clone() }))
                .agent(env.agent_id),
        )
        .await;
    Ok(snap)
}

/// `create_snapshot`.
pub async fn create_snapshot(args: &Value, env: &ToolEnv) -> Result<ToolOutput, ToolError> {
    let label = opt_str(args, "label")
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map_or_else(|| "Agent checkpoint".to_owned(), |l| l.chars().take(120).collect());
    let snap = snapshot(env, label, SnapshotReason::TaskCheckpoint, None, None).await?;
    let mut out = ToolOutput::ok(
        format!("snapshot {} created ({} files)", snap.id, snap.file_count),
        format!("snapshot {}", snap.label),
    );
    out.snapshot_id = Some(snap.id);
    Ok(out)
}
