//! TaskService against a real (temp-file) SQLite database.

use std::sync::Arc;
use std::time::Duration;

use orkestr_domain::{
    now, DependencyKind, ExecutionStatus, Project, ProjectId, TaskActor, TaskId, TaskPriority, TaskStatus,
};
use orkestr_events::{EventBus, Received, Subscription};
use orkestr_protocol::event::{Event, EventKind};
use orkestr_protocol::methods::tasks::{CreateTaskParams, ListTasksParams, TaskPatch};
use orkestr_storage::{Db, ProjectRepository, SqliteMetadataStore};
use orkestr_task_engine::{ResourceSnapshot, Scheduler, SchedulerLimits, TaskError, TaskService};
use TaskStatus::*;

struct Env {
    _dir: tempfile::TempDir,
    db: Db,
    svc: TaskService,
    events: EventBus,
    project: ProjectId,
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("t.db")).unwrap();
    let mut migrations = orkestr_storage::storage_migrations();
    migrations.extend(orkestr_task_engine::task_migrations());
    db.migrate(migrations).await.unwrap();
    let meta = SqliteMetadataStore::new(db.clone());
    let p = Project::new("Demo", now());
    meta.insert(&p).await.unwrap();
    let events = EventBus::in_memory(1024);
    Env { _dir: dir, svc: TaskService::new(db.clone(), events.clone()), db, events, project: p.id }
}

impl Env {
    async fn create(&self, title: &str, deps: &[TaskId]) -> TaskId {
        self.svc
            .create(
                CreateTaskParams {
                    project_id: self.project,
                    title: title.into(),
                    depends_on: Some(deps.to_vec()),
                    ..CreateTaskParams::default()
                },
                TaskActor::User,
            )
            .await
            .unwrap()
            .id
    }

    async fn go(&self, id: TaskId, path: &[TaskStatus]) {
        for s in path {
            self.svc.transition(id, *s, TaskActor::User, None, None).await.unwrap_or_else(|e| panic!("→ {s:?}: {e}"));
        }
    }

    async fn status(&self, id: TaskId) -> TaskStatus {
        self.svc.get(id).await.unwrap().status
    }
}

async fn drain(sub: &mut Subscription) -> Vec<Arc<Event>> {
    let mut out = Vec::new();
    while let Ok(Some(r)) = tokio::time::timeout(Duration::from_millis(100), sub.recv()).await {
        if let Received::Event(e) = r {
            out.push(e);
        }
    }
    out
}

#[tokio::test]
async fn create_update_list_and_version_conflict() {
    let e = env().await;
    let mut sub = e.events.subscribe_all();
    let t = e
        .svc
        .create(
            CreateTaskParams {
                project_id: e.project,
                title: "  Build API ".into(),
                acceptance_criteria: Some(vec!["returns 200".into(), "  ".into()]),
                priority: Some(TaskPriority::High),
                files: Some(vec!["src\\api.rs".into()]),
                estimate_minutes: Some(30),
                ..CreateTaskParams::default()
            },
            TaskActor::User,
        )
        .await
        .unwrap();
    assert_eq!(t.title, "Build API");
    assert_eq!(t.status, Planned);
    assert_eq!(t.version, 0);
    assert_eq!(t.acceptance_criteria.len(), 1);
    assert_eq!(t.files_touched, vec!["src/api.rs"]);
    let evs = drain(&mut sub).await;
    assert_eq!(evs.len(), 1);
    assert!(matches!(evs[0].kind, EventKind::TaskCreated(_)));
    assert_eq!(evs[0].project_id, Some(e.project));
    assert_eq!(evs[0].task_id, Some(t.id));

    let patch =
        TaskPatch { description: Some("desc".into()), priority: Some(TaskPriority::Critical), ..TaskPatch::default() };
    let u = e.svc.update(t.id, Some(0), patch.clone()).await.unwrap();
    assert_eq!(u.version, 1);
    assert_eq!(u.priority, TaskPriority::Critical);
    let err = e.svc.update(t.id, Some(0), patch).await.unwrap_err();
    assert!(matches!(err, TaskError::VersionConflict { current: 1 }), "{err:?}");
    assert!(matches!(drain(&mut sub).await[0].kind, EventKind::TaskUpdated(_)));

    // Bad input.
    let err = e
        .svc
        .create(
            CreateTaskParams { project_id: e.project, title: " ".into(), ..CreateTaskParams::default() },
            TaskActor::User,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TaskError::Invalid(_)));
    let err = e
        .svc
        .create(
            CreateTaskParams { project_id: ProjectId::new(), title: "x".into(), ..CreateTaskParams::default() },
            TaskActor::User,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TaskError::ProjectNotFound(_)));

    // Filters.
    let t2 = e.create("Second", &[]).await;
    e.go(t2, &[Ready]).await;
    let all = e.svc.list(ListTasksParams { project_id: Some(e.project), ..ListTasksParams::default() }).await.unwrap();
    assert_eq!(all.iter().map(|t| t.id).collect::<Vec<_>>(), vec![t.id, t2]);
    let ready = e.svc.list(ListTasksParams { status: Some(vec![Ready]), ..ListTasksParams::default() }).await.unwrap();
    assert_eq!(ready.len(), 1);
    assert!(ready[0].ready_since.is_some());
    let none = e.svc.list(ListTasksParams { status: Some(vec![]), ..ListTasksParams::default() }).await.unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn transitions_history_attempts_and_events() {
    let e = env().await;
    let t = e.create("Work", &[]).await;
    let mut sub = e.events.subscribe_all();
    e.go(t, &[Ready, Running, Testing, Running, Failed, Ready, Running, Review, Completed]).await;
    let task = e.svc.get(t).await.unwrap();
    assert_eq!(task.status, Completed);
    assert_eq!(task.attempts, 2);
    assert_eq!(task.executions.len(), 2);
    assert!(task.finished_at.is_some());
    assert_eq!(task.version, 9);

    let (history, attempts) = e.svc.history(t).await.unwrap();
    assert_eq!(history.len(), 10, "creation + 9 transitions");
    assert_eq!(history[0].from, None);
    assert_eq!(history[0].reason.as_deref(), Some("created"));
    assert_eq!((history[9].from, history[9].to), (Some(Review), Completed));
    assert!(history.iter().all(|h| h.actor == TaskActor::User && !h.forced));
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].status, ExecutionStatus::Failed);
    assert_eq!(attempts[1].status, ExecutionStatus::Succeeded);
    assert_eq!((attempts[0].attempt, attempts[1].attempt), (1, 2));
    assert!(attempts.iter().all(|a| a.finished_at.is_some()));

    let evs = drain(&mut sub).await;
    let changed = evs.iter().filter(|e| matches!(e.kind, EventKind::TaskStatusChanged(_))).count();
    let updated = evs.iter().filter(|e| matches!(e.kind, EventKind::TaskUpdated(_))).count();
    assert_eq!((changed, updated), (9, 9));
    assert!(evs.iter().all(|ev| ev.project_id == Some(e.project) && ev.task_id == Some(t)));

    // Terminal: nothing allowed; invalid transitions leave no trace.
    let err = e.svc.transition(t, Running, TaskActor::User, None, None).await.unwrap_err();
    assert!(matches!(err, TaskError::InvalidTransition(_)));
    assert_eq!(e.svc.history(t).await.unwrap().0.len(), 10);
}

#[tokio::test]
async fn privileged_edges_only_via_recovery() {
    let e = env().await;
    let t = e.create("x", &[]).await;
    e.go(t, &[Ready, Running, Waiting]).await;
    for actor in [TaskActor::User, TaskActor::Manager, TaskActor::System] {
        let err = e.svc.transition(t, Interrupted, actor, None, None).await.unwrap_err();
        assert!(matches!(err, TaskError::InvalidTransition(_)), "{actor:?}");
    }
    let r = e.svc.recover_interrupted("crash_recovery").await.unwrap();
    assert_eq!(r.interrupted, vec![t]);
    let (h, a) = e.svc.history(t).await.unwrap();
    let last = h.last().unwrap();
    assert_eq!(
        (last.from, last.to, last.actor, last.forced),
        (Some(Waiting), Interrupted, TaskActor::SystemRecovery, true)
    );
    assert_eq!(a[0].status, ExecutionStatus::Interrupted);
}

#[tokio::test]
async fn crash_recovery_over_every_state() {
    let e = env().await;
    let paths: Vec<(TaskStatus, Vec<TaskStatus>)> = vec![
        (Planned, vec![]),
        (Ready, vec![Ready]),
        (Running, vec![Ready, Running]),
        (Waiting, vec![Ready, Running, Waiting]),
        (Blocked, vec![Blocked]),
        (Review, vec![Ready, Running, Review]),
        (Testing, vec![Ready, Running, Testing]),
        (Completed, vec![Ready, Running, Completed]),
        (Failed, vec![Ready, Running, Failed]),
        (Cancelled, vec![Cancelled]),
        (Interrupted, vec![Ready, Running, Interrupted]),
        (Recovering, vec![Ready, Running, Failed, Recovering]),
    ];
    let mut ids = Vec::new();
    for (expected, path) in &paths {
        let id = e.create(&format!("{expected:?}"), &[]).await;
        e.go(id, path).await;
        assert_eq!(e.status(id).await, *expected);
        ids.push((id, *expected));
    }
    let mut sub = e.events.subscribe_all();
    let report = e.svc.recover_interrupted("crash_recovery").await.unwrap();
    assert_eq!(report.interrupted.len(), 4);
    for (id, before) in ids {
        let after = e.status(id).await;
        if before.is_interruptible_on_crash() {
            assert_eq!(after, Interrupted, "{before:?}");
            assert!(report.interrupted.contains(&id));
        } else {
            assert_eq!(after, before, "{before:?} untouched");
        }
    }
    let evs = drain(&mut sub).await;
    let reasons: Vec<_> = evs
        .iter()
        .filter_map(|ev| match &ev.kind {
            EventKind::TaskStatusChanged(p) => Some(p.reason.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(reasons, vec![Some("crash_recovery".to_owned()); 4]);
    // Idempotent.
    assert!(e.svc.recover_interrupted("crash_recovery").await.unwrap().interrupted.is_empty());
}

#[tokio::test]
async fn dependencies_cycles_and_propagation() {
    let e = env().await;
    let a = e.create("A", &[]).await;
    let b = e.create("B", &[a]).await;
    let c = e.create("C", &[b]).await;
    assert_eq!(e.svc.get(c).await.unwrap().dependencies, vec![b]);

    // Cycle C ← A? (A depends on C closes A → C → B → A.)
    let err = e.svc.add_dependency(a, c, DependencyKind::FinishToStart).await.unwrap_err();
    match err {
        TaskError::Cycle(path) => assert_eq!(path, vec![a, c, b, a]),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        e.svc.add_dependency(a, a, DependencyKind::FinishToStart).await.unwrap_err(),
        TaskError::Cycle(_)
    ));

    // Guard: B cannot become Ready before A completes.
    let err = e.svc.transition(b, Ready, TaskActor::User, None, None).await.unwrap_err();
    assert!(matches!(err, TaskError::GuardFailed { guard: "dependencies_completed", .. }), "{err:?}");

    // A fails → B (Planned) gets Blocked; C stays Planned (B did not fail).
    e.go(a, &[Ready, Running, Failed]).await;
    assert_eq!(e.status(b).await, Blocked);
    assert_eq!(e.status(c).await, Planned);

    // Retry A, complete it → B auto-unblocks to Ready.
    let r = e.svc.retry(a, TaskActor::User).await.unwrap();
    assert_eq!(r.task.status, Ready);
    let done = e.svc.transition(a, Running, TaskActor::User, None, None).await.unwrap();
    assert_eq!(done.changes.len(), 1);
    let done = e.svc.transition(a, Completed, TaskActor::User, None, None).await.unwrap();
    assert_eq!(done.changes.len(), 2, "A completed + B promoted");
    assert_eq!(done.changes[1].task_id, b);
    assert_eq!((done.changes[1].from, done.changes[1].to), (Blocked, Ready));
    assert_eq!(done.changes[1].reason.as_deref(), Some("dependencies_completed"));

    // Planned dependents are auto-promoted when all deps complete.
    e.go(b, &[Running, Completed]).await;
    assert_eq!(e.status(c).await, Ready);
    let (h, _) = e.svc.history(c).await.unwrap();
    assert_eq!(h.last().unwrap().actor, TaskActor::System);

    // Adding a pending dependency to a Ready task blocks it; removing unblocks.
    let d = e.create("D", &[]).await;
    let r = e.svc.add_dependency(c, d, DependencyKind::FinishToStart).await.unwrap();
    assert_eq!(r.task.status, Blocked);
    assert_eq!(r.changes[0].reason.as_deref(), Some("dependency_pending"));
    let r = e.svc.remove_dependency(c, d).await.unwrap();
    assert_eq!(r.task.status, Ready);
    assert!(matches!(e.svc.remove_dependency(c, d).await.unwrap_err(), TaskError::DependencyNotFound { .. }));

    // Soft edges never block.
    let r = e.svc.add_dependency(c, d, DependencyKind::Soft).await.unwrap();
    assert_eq!(r.task.status, Ready);

    // In-flight tasks reject new dependencies; cross-project edges rejected.
    e.go(c, &[Running]).await;
    assert!(matches!(
        e.svc.add_dependency(c, a, DependencyKind::FinishToStart).await.unwrap_err(),
        TaskError::Conflict(_)
    ));
    let meta = SqliteMetadataStore::new(e.db.clone());
    let other = Project::new("Other", now());
    meta.insert(&other).await.unwrap();
    let x = e
        .svc
        .create(
            CreateTaskParams { project_id: other.id, title: "X".into(), ..CreateTaskParams::default() },
            TaskActor::User,
        )
        .await
        .unwrap();
    assert!(matches!(
        e.svc.add_dependency(d, x.id, DependencyKind::FinishToStart).await.unwrap_err(),
        TaskError::Invalid(_)
    ));

    // Cancelled dependency blocks Ready dependents.
    let f = e.create("F", &[]).await;
    let g = e.create("G", &[]).await;
    e.go(g, &[Ready]).await;
    e.svc.add_dependency(g, f, DependencyKind::FinishToStart).await.unwrap();
    assert_eq!(e.status(g).await, Blocked);
    let r = e.svc.transition(f, Cancelled, TaskActor::User, None, None).await.unwrap();
    assert_eq!(r.changes.len(), 1, "already blocked: no extra change");
}

#[tokio::test]
async fn delete_rules_and_graph() {
    let e = env().await;
    let epic = e.create("Epic", &[]).await;
    let a = e.create("A", &[]).await;
    let b = e.create("B", &[a]).await;
    let c = e.create("C", &[a]).await;
    for id in [a, b] {
        e.svc.update(id, None, TaskPatch { parent_id: Some(epic), ..TaskPatch::default() }).await.unwrap();
    }
    e.svc.update(c, None, TaskPatch { estimate_minutes: Some(10), ..TaskPatch::default() }).await.unwrap();

    let g = e.svc.graph(e.project, None).await.unwrap();
    assert_eq!(g.nodes.len(), 4);
    assert_eq!(g.edges.len(), 2);
    assert_eq!(g.ready, vec![epic, a], "planned tasks with satisfied deps");
    assert_eq!(g.critical_path, vec![a, c]);
    assert_eq!(g.critical_path_length, 11);
    assert_eq!(g.topo_order.first(), Some(&epic));
    let sub = e.svc.graph(e.project, Some(epic)).await.unwrap();
    let mut ids: Vec<_> = sub.nodes.iter().map(|n| n.id).collect();
    ids.sort();
    let mut expected = vec![epic, a, b];
    expected.sort();
    assert_eq!(ids, expected);
    assert_eq!(sub.edges.len(), 1);

    // Parent cycle rejected.
    let err = e.svc.update(epic, None, TaskPatch { parent_id: Some(a), ..TaskPatch::default() }).await.unwrap_err();
    assert!(matches!(err, TaskError::Cycle(_)), "{err:?}");

    // Only Planned/Cancelled can be deleted.
    e.go(a, &[Ready]).await;
    assert!(matches!(e.svc.delete(a).await.unwrap_err(), TaskError::Conflict(_)));
    e.go(a, &[Cancelled]).await;
    assert_eq!(e.status(b).await, Blocked, "dependency cancelled");
    assert_eq!(e.status(c).await, Blocked);
    let mut sub = e.events.subscribe_all();
    e.svc.delete(a).await.unwrap();
    assert!(matches!(e.svc.get(a).await.unwrap_err(), TaskError::NotFound(_)));
    // B and C were blocked only by A → both become Ready.
    assert_eq!(e.status(b).await, Ready);
    assert_eq!(e.status(c).await, Ready);
    assert!(e.svc.get(b).await.unwrap().dependencies.is_empty());
    let evs = drain(&mut sub).await;
    assert!(evs.iter().any(|ev| matches!(&ev.kind, EventKind::TaskDeleted(p) if p.task_id == a)));

    // Deleting a parent turns children into roots.
    e.svc.delete(epic).await.unwrap();
    assert_eq!(e.svc.get(b).await.unwrap().parent_id, None);
}

#[tokio::test]
async fn hierarchy_depth_and_parent_completion_guard() {
    let e = env().await;
    let mut parent = None;
    let mut chain = Vec::new();
    for i in 0..6 {
        let t = e
            .svc
            .create(
                CreateTaskParams {
                    project_id: e.project,
                    title: format!("L{i}"),
                    parent_id: parent,
                    ..CreateTaskParams::default()
                },
                TaskActor::User,
            )
            .await
            .unwrap();
        parent = Some(t.id);
        chain.push(t.id);
    }
    let err = e
        .svc
        .create(
            CreateTaskParams {
                project_id: e.project,
                title: "too deep".into(),
                parent_id: parent,
                ..CreateTaskParams::default()
            },
            TaskActor::User,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TaskError::Invalid(_)), "{err:?}");

    // A parent cannot complete while a child is open.
    let root = chain[0];
    e.go(root, &[Ready, Running]).await;
    let err = e.svc.transition(root, Completed, TaskActor::User, None, None).await.unwrap_err();
    assert!(matches!(err, TaskError::GuardFailed { guard: "children_completed", .. }));
}

#[tokio::test]
async fn retry_interrupted_and_scheduler_view() {
    let e = env().await;
    let a = e.create("A", &[]).await;
    e.go(a, &[Ready, Running, Interrupted]).await;
    let r = e.svc.retry(a, TaskActor::User).await.unwrap();
    assert_eq!(r.task.status, Ready);
    assert_eq!(r.changes.iter().map(|c| c.to).collect::<Vec<_>>(), vec![Recovering, Ready]);
    let planned = e.create("P", &[]).await;
    assert!(matches!(e.svc.retry(planned, TaskActor::User).await.unwrap_err(), TaskError::Conflict(_)));

    let b = e.create("B", &[]).await;
    e.svc
        .update(
            b,
            None,
            TaskPatch {
                priority: Some(TaskPriority::Critical),
                files_touched: Some(vec!["x.rs".into()]),
                ..TaskPatch::default()
            },
        )
        .await
        .unwrap();
    e.go(b, &[Ready]).await;
    let running = e.create("R", &[]).await;
    e.svc
        .update(running, None, TaskPatch { files_touched: Some(vec!["x.rs".into()]), ..TaskPatch::default() })
        .await
        .unwrap();
    e.go(running, &[Ready, Running]).await;
    let plan = e
        .svc
        .next_runnable(Scheduler::new(SchedulerLimits::default()), ResourceSnapshot::unconstrained())
        .await
        .unwrap();
    assert_eq!(plan.start, vec![a]);
    assert_eq!(plan.deferred.len(), 1);
    assert_eq!(plan.deferred[0].0, b, "critical task waits for the file conflict");
}

#[tokio::test]
async fn migrations_are_reversible() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("m.db")).unwrap();
    let mut migrations = orkestr_storage::storage_migrations();
    migrations.extend(orkestr_task_engine::task_migrations());
    let n = migrations.len();
    db.migrate(migrations).await.unwrap();
    let reverted = db.call(move |c| orkestr_storage::Migrator::rollback_last(c, n)).await.unwrap();
    assert_eq!(reverted.len(), n);
    let tables: i64 = db
        .call(|c| Ok(c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'task%'", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(tables, 0);
}
