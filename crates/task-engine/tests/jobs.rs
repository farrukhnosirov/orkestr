//! Job table: CRUD, epoch fencing, task owner, JobAssigned.

use std::collections::BTreeMap;

use orkestr_domain::{
    now, CheckpointId, ExecutorRef, JobId, LeaseEpoch, NodeId, Project, ProjectId, TaskActor, TaskPriority,
};
use orkestr_events::{EventBus, Received};
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::jobs::{
    Affinity, JobKind, JobSpec, JobState, JobWaitReason, Needs, Placement, Requirements, ResourceEstimate,
};
use orkestr_protocol::methods::tasks::CreateTaskParams;
use orkestr_protocol::topic;
use orkestr_storage::{Db, ProjectRepository, SqliteMetadataStore};
use orkestr_task_engine::jobs::{check_epoch, JobError, JobFilter, JobStore};
use orkestr_task_engine::TaskService;

struct Env {
    _dir: tempfile::TempDir,
    tasks: TaskService,
    jobs: JobStore,
    events: EventBus,
    project: ProjectId,
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("j.db")).unwrap();
    let mut migrations = orkestr_storage::storage_migrations();
    migrations.extend(orkestr_task_engine::task_migrations());
    migrations.extend(orkestr_task_engine::job_migrations());
    db.migrate(migrations).await.unwrap();
    let p = Project::new("Demo", now());
    SqliteMetadataStore::new(db.clone()).insert(&p).await.unwrap();
    let events = EventBus::in_memory(1024);
    Env {
        _dir: dir,
        tasks: TaskService::new(db.clone(), events.clone()),
        jobs: JobStore::new(db, events.clone()),
        events,
        project: p.id,
    }
}

impl Env {
    async fn spec(&self) -> JobSpec {
        let t = self
            .tasks
            .create(
                CreateTaskParams { project_id: self.project, title: "job".into(), ..CreateTaskParams::default() },
                TaskActor::User,
            )
            .await
            .unwrap();
        JobSpec {
            job_id: JobId::new(),
            task_id: t.id,
            project_id: self.project,
            kind: JobKind::Command { program: "make".into(), args: vec![], env: BTreeMap::new(), timeout_secs: None },
            requirements: Requirements::default(),
            estimate: ResourceEstimate::default(),
            needs: Needs::default(),
            priority: TaskPriority::default(),
            affinity: Affinity::default(),
        }
    }
}

fn wait() -> Placement {
    Placement::Wait { reason: JobWaitReason::NoCapacity, explanation: vec!["wait.noCapacity(rejected=1): x".into()] }
}

#[tokio::test]
async fn crud_and_filters() {
    let e = env().await;
    let s1 = e.spec().await;
    let s2 = e.spec().await;
    let j1 = e.jobs.create(s1.clone(), Some(wait())).await.unwrap();
    assert_eq!(j1.state, JobState::Pending);
    assert_eq!(j1.epoch, LeaseEpoch(0));
    let j2 = e.jobs.create(s2.clone(), None).await.unwrap();
    assert_eq!(e.jobs.get(j1.spec.job_id).await.unwrap(), j1);
    let all = e.jobs.list(JobFilter::default()).await.unwrap();
    assert_eq!(all.len(), 2);
    let by_task = e.jobs.list(JobFilter { task_id: Some(s2.task_id), ..JobFilter::default() }).await.unwrap();
    assert_eq!(by_task, vec![j2.clone()]);
    assert_eq!(e.jobs.active_for_task(s1.task_id).await.unwrap().unwrap().spec.job_id, s1.job_id);
    assert!(matches!(e.jobs.get(JobId::new()).await, Err(JobError::NotFound(_))));

    // Infeasible placement makes the job terminal → no longer active.
    let inf = Placement::Infeasible { reason: "noFeasibleNode".into(), explanation: vec![] };
    let v = e.jobs.set_placement(s2.job_id, inf.clone()).await.unwrap();
    assert_eq!(v.state, JobState::Infeasible);
    assert_eq!(v.placement, Some(inf));
    assert!(e.jobs.active_for_task(s2.task_id).await.unwrap().is_none());
    let active = e.jobs.list(JobFilter { active_only: true, ..JobFilter::default() }).await.unwrap();
    assert_eq!(active.len(), 1);
}

#[tokio::test]
async fn assign_fencing_and_updates() {
    let e = env().await;
    let mut sub = e.events.subscribe([topic::JOBS]).unwrap();
    let s = e.spec().await;
    let id = s.job_id;
    let place = Placement::Place { node_id: NodeId::new(), score: 0.5, explanation: vec!["place(...): x".into()] };
    e.jobs.create(s.clone(), Some(place)).await.unwrap();
    let node = NodeId::new();

    // epoch must increase
    assert!(matches!(e.jobs.assign(id, node, LeaseEpoch(0)).await, Err(JobError::StaleEpoch { .. })));
    let v = e.jobs.assign(id, node, LeaseEpoch(1)).await.unwrap();
    assert_eq!(
        (v.state, v.epoch, v.owner),
        (JobState::Offered, LeaseEpoch(1), Some(ExecutorRef::Node { node_id: node }))
    );
    assert_eq!(e.jobs.list(JobFilter { node_id: Some(node), ..JobFilter::default() }).await.unwrap().len(), 1);
    assert_eq!(e.jobs.active_per_node().await.unwrap().get(&node), Some(&1));
    let Some(Received::Event(ev)) = tokio::time::timeout(std::time::Duration::from_secs(2), sub.recv()).await.unwrap()
    else {
        panic!("no event")
    };
    let EventKind::JobAssigned(p) = &ev.kind else { panic!("{ev:?}") };
    assert_eq!((p.job_id, p.node_id, p.epoch), (id, node, LeaseEpoch(1)));
    assert_eq!(p.explanation, vec!["place(...): x".to_owned()]);

    // fenced updates
    let stale = LeaseEpoch(0);
    assert!(matches!(e.jobs.set_state(id, stale, JobState::Running, None).await, Err(JobError::StaleEpoch { .. })));
    let v = e.jobs.set_state(id, LeaseEpoch(1), JobState::Running, None).await.unwrap();
    assert_eq!(v.state, JobState::Running);
    let cp = CheckpointId::new();
    assert_eq!(e.jobs.record_checkpoint(id, LeaseEpoch(1), cp).await.unwrap().last_checkpoint, Some(cp));
    let until = now() - chrono::TimeDelta::seconds(5);
    e.jobs.set_lease_until(id, LeaseEpoch(1), Some(until)).await.unwrap();
    assert!(e.jobs.lease_until(id).await.unwrap().is_some());
    let expired = e.jobs.expired_leases(now()).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert!(e.jobs.expired_leases(until - chrono::TimeDelta::seconds(1)).await.unwrap().is_empty());
    let v = e.jobs.update_fenced(id, LeaseEpoch(1), |v| v.progress = Some(0.5)).await.unwrap();
    assert_eq!(v.progress, Some(0.5));

    // bump: CAS, clears the lease, old epoch now stale
    assert!(matches!(e.jobs.bump_epoch(id, LeaseEpoch(7)).await, Err(JobError::StaleEpoch { .. })));
    assert_eq!(e.jobs.bump_epoch(id, LeaseEpoch(1)).await.unwrap(), LeaseEpoch(2));
    assert!(e.jobs.lease_until(id).await.unwrap().is_none());
    assert!(matches!(
        e.jobs.set_state(id, LeaseEpoch(1), JobState::Completed, None).await,
        Err(JobError::StaleEpoch { current: LeaseEpoch(2), got: LeaseEpoch(1) })
    ));
    // re-assignment to another node at a higher epoch
    let other = NodeId::new();
    let v = e.jobs.assign(id, other, LeaseEpoch(3)).await.unwrap();
    assert_eq!(v.owner, Some(ExecutorRef::Node { node_id: other }));
    assert!(check_epoch(LeaseEpoch(3), LeaseEpoch(3)).is_ok());
    assert!(check_epoch(LeaseEpoch(3), LeaseEpoch(2)).is_err());
}

#[tokio::test]
async fn task_owner_is_written_without_touching_status() {
    let e = env().await;
    let s = e.spec().await;
    let before = e.tasks.get(s.task_id).await.unwrap();
    let node = NodeId::new();
    let t = e.jobs.set_task_owner(s.task_id, Some(ExecutorRef::Node { node_id: node })).await.unwrap();
    assert_eq!(t.owner, Some(ExecutorRef::Node { node_id: node }));
    assert_eq!(t.status, before.status);
    assert_eq!(t.version, before.version + 1);
    assert_eq!(e.tasks.get(s.task_id).await.unwrap().owner, t.owner);
}
