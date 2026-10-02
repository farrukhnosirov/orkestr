//! `tasks.*` handlers (§10–11). Thin layer over
//! [`orkestr_task_engine::TaskService`], which owns the state machine,
//! persistence, cascades and `Task*` events.
//!
//! Every RPC caller acts as [`TaskActor::User`]: clients can never use the
//! privileged `SystemRecovery` edges (those run only inside crash recovery).

use orkestr_domain::{DependencyKind, TaskActor};
use orkestr_protocol::methods::tasks::{
    self as m, AddDependencyParams, CreateTaskParams, DeleteTaskResult, ListTasksParams, ListTasksResult,
    RemoveDependencyParams, SingleTaskResult, TaskChangeResult, TaskGraphParams, TaskHistoryResult, TaskIdParams,
    TransitionTaskParams, UpdateTaskParams,
};
use orkestr_protocol::RpcError;
use orkestr_task_engine::service::TaskChange;
use orkestr_task_engine::{allowed_transitions, TaskError};
use serde_json::json;

use super::{RpcContext, RpcRouter};

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, p: ListTasksParams| async move {
            Ok(ListTasksResult { tasks: ctx.services.tasks.list(p).await.map_err(task_to_rpc)? })
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: TaskIdParams| async move {
            Ok(SingleTaskResult { task: ctx.services.tasks.get(p.task_id).await.map_err(task_to_rpc)? })
        })
        .method::<m::Create, _, _>(|ctx: RpcContext, p: CreateTaskParams| async move {
            Ok(SingleTaskResult { task: ctx.services.tasks.create(p, TaskActor::User).await.map_err(task_to_rpc)? })
        })
        .method::<m::Update, _, _>(|ctx: RpcContext, p: UpdateTaskParams| async move {
            let task = ctx.services.tasks.update(p.task_id, p.expected_version, p.patch).await.map_err(task_to_rpc)?;
            Ok(SingleTaskResult { task })
        })
        .method::<m::Delete, _, _>(|ctx: RpcContext, p: TaskIdParams| async move {
            Ok(DeleteTaskResult { task_id: ctx.services.tasks.delete(p.task_id).await.map_err(task_to_rpc)? })
        })
        .method::<m::Transition, _, _>(|ctx: RpcContext, p: TransitionTaskParams| async move {
            let change = ctx
                .services
                .tasks
                .transition(p.task_id, p.to, TaskActor::User, p.reason, p.expected_version)
                .await
                .map_err(task_to_rpc)?;
            Ok(change_result(change))
        })
        .method::<m::AddDependency, _, _>(|ctx: RpcContext, p: AddDependencyParams| async move {
            let change = ctx
                .services
                .tasks
                .add_dependency(p.task_id, p.depends_on, p.kind.unwrap_or(DependencyKind::FinishToStart))
                .await
                .map_err(task_to_rpc)?;
            Ok(change_result(change))
        })
        .method::<m::RemoveDependency, _, _>(|ctx: RpcContext, p: RemoveDependencyParams| async move {
            let change = ctx.services.tasks.remove_dependency(p.task_id, p.depends_on).await.map_err(task_to_rpc)?;
            Ok(change_result(change))
        })
        .method::<m::Graph, _, _>(|ctx: RpcContext, p: TaskGraphParams| async move {
            ctx.services.tasks.graph(p.project_id, p.root_id).await.map_err(task_to_rpc)
        })
        .method::<m::History, _, _>(|ctx: RpcContext, p: TaskIdParams| async move {
            let (entries, executions) = ctx.services.tasks.history(p.task_id).await.map_err(task_to_rpc)?;
            Ok(TaskHistoryResult { entries, executions })
        })
        .method::<m::Retry, _, _>(|ctx: RpcContext, p: TaskIdParams| async move {
            Ok(change_result(ctx.services.tasks.retry(p.task_id, TaskActor::User).await.map_err(task_to_rpc)?))
        });
}

fn change_result(c: TaskChange) -> TaskChangeResult {
    TaskChangeResult { task: c.task, changes: c.changes }
}

/// Maps task-engine errors to RPC errors (contract §4 codes).
pub fn task_to_rpc(err: TaskError) -> RpcError {
    match err {
        TaskError::InvalidTransition(t) => RpcError::conflict(t.to_string()).with_data(json!({
            "from": t.from,
            "to": t.to,
            "allowed": allowed_transitions(t.from),
        })),
        TaskError::NotFound(id) => RpcError::not_found(format!("task {id} not found")),
        TaskError::ProjectNotFound(id) => RpcError::not_found(format!("project {id} not found")),
        e @ TaskError::DependencyNotFound { .. } => RpcError::not_found(e.to_string()),
        TaskError::Cycle(path) => {
            let msg = TaskError::Cycle(path.clone()).to_string();
            RpcError::conflict(msg).with_data(json!({ "cycle": path }))
        }
        e @ TaskError::GuardFailed { guard, .. } => {
            RpcError::conflict(e.to_string()).with_data(json!({ "guard": guard }))
        }
        e @ TaskError::VersionConflict { current } => {
            RpcError::conflict(e.to_string()).with_data(json!({ "currentVersion": current }))
        }
        TaskError::Conflict(m) => RpcError::conflict(m),
        TaskError::Invalid(m) => RpcError::bad_request(m),
        TaskError::Unimplemented(f) => RpcError::unimplemented_feature(f),
        TaskError::Storage(e) => {
            tracing::error!(error = %e, "task storage failure while handling request");
            RpcError::internal("internal storage error")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::{TaskId, TaskStatus};

    #[test]
    fn error_mapping() {
        let e = task_to_rpc(TaskError::InvalidTransition(orkestr_task_engine::InvalidTransition {
            from: TaskStatus::Completed,
            to: TaskStatus::Running,
        }));
        assert_eq!(e.code.as_str(), "conflict");
        assert_eq!(e.data.as_ref().unwrap()["allowed"], json!([]));
        let id = TaskId::new();
        let e = task_to_rpc(TaskError::Cycle(vec![id, id]));
        assert_eq!(e.data.unwrap()["cycle"][0], json!(id.to_string()));
        assert_eq!(task_to_rpc(TaskError::NotFound(id)).code.as_str(), "not_found");
        assert_eq!(task_to_rpc(TaskError::Invalid("x".into())).code.as_str(), "bad_request");
        let e = task_to_rpc(TaskError::Storage("secret path".into()));
        assert!(!e.message.contains("secret"));
    }
}
