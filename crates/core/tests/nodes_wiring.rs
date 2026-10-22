//! Phase 6 integration wiring (`nodes::wiring`): a started core has the jobs
//! coordinator installed; a job placed on the coordinator node itself runs on
//! its own worker host (in-process link, local lease renewals) and completes;
//! a worker reports runs interrupted by a restart, which the coordinator
//! fences (stale epoch) and revokes; lease recovery happens once.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use orkestr_core::nodes::coordinator_jobs;
use orkestr_core::nodes::lease::{Lease, LeaseBook};
use orkestr_core::nodes::worker::{WorkerConfig, WorkerJobs};
use orkestr_core::rpc::{build_router, Principal, RpcContext, RpcRouter};
use orkestr_core::{CoreConfig, Services};
use orkestr_domain::{CorrelationId, JobId, LeaseEpoch, NodeId, SessionId};
use orkestr_node::exec::ToolShims;
use orkestr_protocol::methods::jobs::{JobKind, JobSubmitSpec, Requirements, SubmitJobParams};
use orkestr_protocol::methods::nodes::LeasePhase;
use serde_json::{json, Value};

struct Rpc {
    router: RpcRouter,
    services: Arc<Services>,
}

impl Rpc {
    async fn ok(&self, method: &str, params: Value) -> Value {
        let ctx = RpcContext {
            services: Arc::clone(&self.services),
            session_id: SessionId::new(),
            correlation_id: CorrelationId::new(),
            principal: Principal::Local,
        };
        self.router.dispatch(ctx, method, params).await.unwrap_or_else(|e| panic!("{method}: {e:?}"))
    }
}

#[tokio::test]
async fn local_job_runs_on_the_coordinators_own_worker_host() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::for_tests(dir.path())).await.unwrap();
    let me = core.services.node_id;
    assert!(coordinator_jobs::coordinator(me).is_some(), "coordinator wired at start");
    let rpc = Rpc { router: build_router(), services: Arc::clone(&core.services) };
    let pid = rpc.ok("projects.create", json!({"name": "Local"})).await["project"]["id"].as_str().unwrap().to_owned();
    let task = rpc.ok("tasks.create", json!({"projectId": pid, "title": "local job"})).await["task"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let p = SubmitJobParams {
        task_id: task.parse().unwrap(),
        spec: JobSubmitSpec {
            kind: JobKind::Command {
                program: "node".into(),
                args: vec!["-e".into(), "console.log('local ok')".into()],
                env: BTreeMap::new(),
                timeout_secs: Some(60),
            },
            requirements: Requirements::default(),
            estimate: Default::default(),
            needs: Default::default(),
            affinity: Default::default(),
        },
    };
    let job = rpc.ok("jobs.submit", serde_json::to_value(p).unwrap()).await["job"].clone();
    assert_eq!(job["placement"]["nodeId"], me.to_string(), "{job}");
    let id = job["spec"]["jobId"].as_str().unwrap().to_owned();
    let mut last = Value::Null;
    for _ in 0..300 {
        last = rpc.ok("jobs.get", json!({"jobId": id})).await["job"].clone();
        if ["completed", "failed", "cancelled"].contains(&last["state"].as_str().unwrap_or_default()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(last["state"], "completed", "{last}");
    let t = rpc.ok("tasks.get", json!({"taskId": task})).await["task"].clone();
    assert_eq!(t["status"], "Completed");
    assert!(t["placement"].as_array().is_some_and(|e| !e.is_empty()), "placement explanation on the task: {t}");
    core.shutdown().await;
}

#[tokio::test]
async fn interrupted_runs_are_reported_per_coordinator_and_removed_on_revoke() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::for_tests(dir.path())).await.unwrap();
    let s = Arc::clone(&core.services);
    let jobs_root = dir.path().join("wjobs");
    let (coord, other, job) = (NodeId::new(), NodeId::new(), JobId::new());
    // a run of `job` (epoch 3) left behind by a crash
    let run = jobs_root.join(job.to_string());
    std::fs::create_dir_all(run.join("e3")).unwrap();
    std::fs::write(run.join("e3.lease"), json!({"coordinator": coord, "epoch": 3}).to_string()).unwrap();
    let host = WorkerJobs::new(
        WorkerConfig {
            node_id: s.node_id,
            jobs_root,
            blobs: orkestr_core::rpc::models::blob_store(&s).unwrap(),
            shims: ToolShims::default(),
            max_concurrent: 2,
            checkpoint_every: None,
        },
        Arc::clone(s.vcs.git()),
        s.events.clone(),
    );
    let mine = host.leases_of(Some(coord));
    assert_eq!(mine.len(), 1);
    assert_eq!((mine[0].job_id, mine[0].epoch, mine[0].phase), (job, LeaseEpoch(3), LeasePhase::Failed));
    assert!(host.leases_of(Some(other)).is_empty(), "other coordinators never see it");
    host.revoke_of(Some(other), &[job]);
    assert_eq!(host.leases_of(None).len(), 1, "another coordinator cannot revoke it");
    host.revoke_of(Some(coord), &[job]);
    assert!(host.leases_of(None).is_empty());
    assert!(!run.join("e3").exists() && !run.join("e3.lease").exists(), "leftover removed");
    core.shutdown().await;
}

#[test]
fn a_lost_lease_is_recovered_once() {
    let book = LeaseBook::new();
    let (job, a, b) = (JobId::new(), NodeId::new(), NodeId::new());
    let old = Lease { node_id: a, epoch: LeaseEpoch(1), until: chrono::Utc::now() };
    book.grant(job, old);
    assert!(book.release_if(job, &old), "first recovery takes it");
    assert!(!book.release_if(job, &old), "a second report of the same loss is ignored");
    book.grant(job, Lease { node_id: b, epoch: LeaseEpoch(2), until: chrono::Utc::now() });
    assert!(!book.release_if(job, &old), "never releases the successor lease");
    assert!(book.is_current(job, b, LeaseEpoch(2)));
}
