//! Phase 6 WP-C: cross-node job execution over in-process `LoopbackLink`s
//! (coordinator core + two worker cores): command job with a tool shim +
//! artifact, planned migration A→B (diff + handoff, no repeated steps),
//! stale epochs fenced on both sides, lease expiry → unplanned migration,
//! side-effect gate (`needs_user_decision`), cancel + emergency stop broadcast.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_core::nodes::coordinator_jobs::{
    self, BlobTransfer, CoordinatorConfig, CoordinatorDeps, CoordinatorJobs, FirstFitPlacer, StaticLinks,
};
use orkestr_core::nodes::lease::{LeaseConfig, PolicySigner};
use orkestr_core::nodes::worker::{self, sha_digest, MemBlobStore, WorkerConfig, WorkerJobs};
use orkestr_core::nodes::LoopbackLink;
use orkestr_core::rpc::{build_router, Principal, RpcContext, RpcRouter};
use orkestr_core::{CoreConfig, CoreHandle};
use orkestr_domain::{
    CorrelationId, Digest, JobId, LeaseEpoch, NodeId, ProjectId, SessionId, TaskActor, TaskId, TaskStatus,
};
use orkestr_events::Received;
use orkestr_node::exec::ToolShims;
use orkestr_node::link::call_typed;
use orkestr_node::{BlobError, BlobStore, NodeLink};
use orkestr_protocol::event::{Event, EventKind};
use orkestr_protocol::methods::jobs::{
    self as jm, JobKind, JobProgressItem, JobSpec, JobState, PolicySnapshot, SideEffectKind, WorkerJobParams,
};
use orkestr_protocol::methods::nodes::LeasePhase;
use orkestr_protocol::topic;
use orkestr_vcs::{GitCli, RunOptions};
use parking_lot::Mutex;
use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(30);

/// Copies blobs directly between per-node stores (stands in for node.blob*).
#[derive(Default)]
struct DirectTransfer {
    stores: Mutex<HashMap<NodeId, Arc<dyn BlobStore>>>,
}

async fn copy(src: &dyn BlobStore, dst: &dyn BlobStore, d: &Digest) -> Result<(), BlobError> {
    if dst.has(d).await {
        return Ok(());
    }
    let p = src.path(d).ok_or_else(|| BlobError::NotFound(d.clone()))?;
    let bytes = std::fs::read(p)?;
    dst.put_chunk(d, 0, bytes.len() as u64, &bytes).await.map(|_| ())
}

#[async_trait]
impl BlobTransfer for DirectTransfer {
    async fn push(&self, src: &dyn BlobStore, link: &dyn NodeLink, d: &Digest) -> Result<(), BlobError> {
        let dst = self.stores.lock().get(&link.node_id()).cloned().ok_or(BlobError::Transfer("no store".into()))?;
        copy(src, dst.as_ref(), d).await
    }

    async fn fetch(&self, link: &dyn NodeLink, d: &Digest, dst: &dyn BlobStore) -> Result<(), BlobError> {
        let src = self.stores.lock().get(&link.node_id()).cloned().ok_or(BlobError::Transfer("no store".into()))?;
        copy(src.as_ref(), dst, d).await
    }
}

struct Worker {
    core: CoreHandle,
    host: Arc<WorkerJobs>,
    link: Arc<LoopbackLink>,
}

struct Harness {
    _dirs: Vec<tempfile::TempDir>,
    core: CoreHandle,
    router: Arc<RpcRouter>,
    coord: Arc<CoordinatorJobs>,
    placer: Arc<FirstFitPlacer>,
    workers: Vec<Worker>,
    alive: Arc<Mutex<HashSet<NodeId>>>,
    events: Arc<Mutex<Vec<Event>>>,
    shutdown: tokio_util::sync::CancellationToken,
}

fn shims() -> ToolShims {
    let script = std::env::temp_dir().join("orkestr-wpc-fake-xcodebuild.js");
    std::fs::write(
        &script,
        r#"const fs = require('fs');
const out = process.env.ORKESTR_JOB_OUTPUT_DIR;
fs.writeFileSync(out + '/App.ipa', 'IPA:' + process.argv.slice(2).join(' '));
console.log('built ' + process.argv.slice(2).join(' '));
console.log('::orkestr-side-effect {"action":"upload","kind":"nonIdempotent","detail":"testflight"}');
"#,
    )
    .unwrap();
    ToolShims::from_map(BTreeMap::from([(
        "xcodebuild".to_owned(),
        vec!["node".to_owned(), script.to_string_lossy().into_owned()],
    )]))
}

impl Harness {
    async fn new(ttl: Duration, checkpoint_every: Option<Duration>) -> Self {
        let mut dirs = Vec::new();
        let d = tempfile::tempdir().unwrap();
        let core = orkestr_core::start(CoreConfig::for_tests(d.path())).await.unwrap();
        dirs.push(d);
        let signer = PolicySigner::new(p256::ecdsa::SigningKey::random(&mut rand_core06::OsRng));
        let links = Arc::new(StaticLinks::default());
        let transfer = Arc::new(DirectTransfer::default());
        let placer = Arc::new(FirstFitPlacer::default());
        let cblobs: Arc<dyn BlobStore> = Arc::new(MemBlobStore::default());
        let coord_id = core.services.node_id;
        let mut workers = Vec::new();
        for _ in 0..2 {
            let d = tempfile::tempdir().unwrap();
            let wcore = orkestr_core::start(CoreConfig::for_tests(d.path())).await.unwrap();
            dirs.push(d);
            let s = Arc::clone(&wcore.services);
            let blobs: Arc<dyn BlobStore> = Arc::new(MemBlobStore::default());
            let cfg = WorkerConfig {
                node_id: s.node_id,
                jobs_root: s.paths.data_dir.join("jobs"),
                blobs: Arc::clone(&blobs),
                shims: shims(),
                max_concurrent: 4,
                checkpoint_every,
            };
            let host = Arc::new(WorkerJobs::new(cfg, Arc::clone(s.vcs.git()), s.events.clone()));
            host.trust_coordinator(coord_id, signer.verifying_key());
            worker::install(s.node_id, Arc::clone(&host));
            let link =
                Arc::new(LoopbackLink::new(s.node_id, Arc::new(build_router()), Arc::clone(&s), Principal::Local));
            transfer.stores.lock().insert(s.node_id, blobs);
            links.add(Arc::clone(&link) as Arc<dyn NodeLink>, None);
            placer.0.lock().push(s.node_id);
            workers.push(Worker { core: wcore, host, link });
        }
        let coord = CoordinatorJobs::new(CoordinatorDeps {
            services: Arc::clone(&core.services),
            cfg: CoordinatorConfig {
                node_id: coord_id,
                lease: LeaseConfig { ttl, heartbeat: Duration::from_millis(100) },
                policy: PolicySnapshot { approval_mode: "ask".into(), ..PolicySnapshot::default() },
            },
            links: links.clone(),
            placer: placer.clone(),
            transfer,
            blobs: cblobs,
            signer,
        });
        coordinator_jobs::install(coord_id, Arc::clone(&coord));
        let shutdown = tokio_util::sync::CancellationToken::new();
        coord.spawn_lease_loop(shutdown.clone());
        // heartbeat stand-in: renew leases of alive workers (WP-A's loop in production)
        let alive: Arc<Mutex<HashSet<NodeId>>> =
            Arc::new(Mutex::new(workers.iter().map(|w| w.link.node_id()).collect()));
        {
            let (alive, coord, sd) = (Arc::clone(&alive), Arc::clone(&coord), shutdown.clone());
            let hosts: Vec<(NodeId, Arc<WorkerJobs>)> =
                workers.iter().map(|w| (w.link.node_id(), Arc::clone(&w.host))).collect();
            tokio::spawn(async move {
                while !sd.is_cancelled() {
                    for (node, host) in &hosts {
                        if !alive.lock().contains(node) {
                            continue;
                        }
                        let (renew, revoke) = coord.heartbeat_params(*node);
                        host.revoke(&revoke);
                        let states = host.renew(&renew);
                        coord.heartbeat_ok(*node, &renew, &revoke, &states);
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });
        }
        let events = Arc::new(Mutex::new(Vec::new()));
        {
            let mut sub = core.services.events.subscribe([topic::JOBS]).unwrap();
            let ev = Arc::clone(&events);
            tokio::spawn(async move {
                while let Some(r) = sub.recv().await {
                    if let Received::Event(e) = r {
                        ev.lock().push((*e).clone());
                    }
                }
            });
        }
        Self { _dirs: dirs, core, router: Arc::new(build_router()), coord, placer, workers, alive, events, shutdown }
    }

    fn ctx(&self) -> RpcContext {
        RpcContext {
            services: Arc::clone(&self.core.services),
            session_id: SessionId::new(),
            correlation_id: CorrelationId::new(),
            principal: Principal::Local,
        }
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value, orkestr_protocol::RpcError> {
        self.router.dispatch(self.ctx(), method, params).await
    }

    async fn task(&self) -> (ProjectId, TaskId) {
        let p = self.rpc("projects.create", json!({"name": format!("Nodes {}", JobId::new())})).await.unwrap();
        let pid: ProjectId = serde_json::from_value(p["project"]["id"].clone()).unwrap();
        let t = self.rpc("tasks.create", json!({"projectId": pid, "title": "remote work"})).await.unwrap();
        let tid: TaskId = serde_json::from_value(t["task"]["id"].clone()).unwrap();
        self.core.services.tasks.transition(tid, TaskStatus::Ready, TaskActor::User, None, None).await.unwrap();
        (pid, tid)
    }

    fn spec(pid: ProjectId, tid: TaskId, kind: JobKind) -> JobSpec {
        JobSpec {
            job_id: JobId::new(),
            task_id: tid,
            project_id: pid,
            kind,
            requirements: Default::default(),
            estimate: Default::default(),
            needs: Default::default(),
            priority: Default::default(),
            affinity: Default::default(),
        }
    }

    async fn wait_state(&self, job: JobId, state: JobState) -> jm::JobView {
        self.coord
            .wait_for(job, WAIT, |v| v.state == state)
            .await
            .unwrap_or_else(|| panic!("job never reached {state:?}: {:?}", self.coord.get(job)))
    }

    async fn task_status(&self, tid: TaskId) -> TaskStatus {
        self.core.services.tasks.get(tid).await.unwrap().status
    }

    fn steps_from(&self, node: NodeId, job: JobId) -> Vec<String> {
        self.events
            .lock()
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::JobProgress(p) if p.node_id == node && p.job_id == job => Some(p.items.clone()),
                _ => None,
            })
            .flatten()
            .filter_map(|i| match i {
                JobProgressItem::Step { name } => Some(name),
                _ => None,
            })
            .collect()
    }

    fn has_event(&self, f: impl Fn(&EventKind) -> bool) -> bool {
        self.events.lock().iter().any(|e| f(&e.kind))
    }

    async fn finish(self) {
        self.shutdown.cancel();
        for w in self.workers {
            w.core.shutdown().await;
        }
        self.core.shutdown().await;
    }
}

async fn wait_until(mut f: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !f() {
        assert!(tokio::time::Instant::now() < deadline, "condition not reached");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn sh(g: &GitCli, dir: &Path, args: &[&str]) -> String {
    g.runner().run_ok(dir, args, RunOptions::write()).await.unwrap().text()
}

async fn repo(g: &GitCli) -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().to_path_buf();
    g.init(&p, Some("main")).await.unwrap();
    for (k, v) in [("user.name", "T"), ("user.email", "t@example.invalid"), ("core.autocrlf", "false")] {
        sh(g, &p, &["config", k, v]).await;
    }
    std::fs::write(p.join("README.md"), "hello\n").unwrap();
    sh(g, &p, &["add", "-A"]).await;
    sh(g, &p, &["commit", "-qm", "init"]).await;
    // uncommitted change on the coordinator that must travel with the job
    std::fs::write(p.join("README.md"), "hello\nlocal edit\n").unwrap();
    (d, p)
}

fn node_available() -> bool {
    orkestr_platform::process::which("node").is_some()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn command_job_with_tool_shim_produces_artifact() {
    if !node_available() {
        eprintln!("node not installed — skipping");
        return;
    }
    let h = Harness::new(Duration::from_secs(10), None).await;
    let a = &h.workers[0];
    let (pid, tid) = h.task().await;
    let spec = Harness::spec(
        pid,
        tid,
        JobKind::Command {
            program: "xcodebuild".into(),
            args: vec!["-scheme".into(), "App".into()],
            env: BTreeMap::new(),
            timeout_secs: Some(60),
        },
    );
    let job = spec.job_id;
    h.coord.submit(spec, None).await.unwrap();
    let v = h.wait_state(job, JobState::Completed).await;
    assert_eq!(v.epoch, LeaseEpoch(1));
    assert_eq!(v.artifacts.len(), 1, "{v:?}");
    let expected = sha_digest(b"IPA:-scheme App");
    assert_eq!(v.artifacts[0], expected, "artifact is hash-verified and identical");
    assert_eq!(h.task_status(tid).await, TaskStatus::Completed);
    let task = h.core.services.tasks.get(tid).await.unwrap();
    assert_eq!(task.owner, Some(orkestr_domain::ExecutorRef::Node { node_id: a.link.node_id() }));
    // output streamed with the worker's node id + stdout side-effect marker parsed
    assert!(h.has_event(|k| matches!(k, EventKind::JobProgress(p) if p.items.iter().any(|i| matches!(i, JobProgressItem::Output { text, .. } if text.contains("built -scheme App"))))));
    let cp = h.coord.last_checkpoint(job).unwrap();
    assert!(cp.side_effects.iter().any(|s| s.action == "upload" && s.kind == SideEffectKind::NonIdempotent));
    // job row persisted through the WP-B store
    let row = orkestr_core::rpc::jobs_ui::job_store(&h.core.services).get(job).await.unwrap();
    assert_eq!(row.state, JobState::Completed);
    // audit entries with nodeId
    let audit = h.rpc("audit.list", json!({})).await.map(|v| v.to_string()).unwrap_or_default();
    for action in ["jobs.offer", "jobs.accept", "jobs.complete"] {
        assert!(audit.contains(action), "audit lacks {action}");
    }
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planned_migration_restores_diff_and_handoff_and_fences_stale_epochs() {
    let g = GitCli::discover();
    if !g.is_available() {
        return;
    }
    let h = Harness::new(Duration::from_secs(10), Some(Duration::from_secs(3600))).await;
    let (_d, wt) = repo(&g).await;
    let (pid, tid) = h.task().await;
    let prompt = "write a.txt one\nsleep 600\nwrite b.txt two\nsleep 600\nwrite c.txt three";
    let spec = Harness::spec(pid, tid, JobKind::AgentTask { template_id: None, model: None, prompt: prompt.into() });
    let job = spec.job_id;
    h.coord.submit(spec, Some(wt.clone())).await.unwrap();
    let (a, b) = (h.workers[0].link.node_id(), h.workers[1].link.node_id());
    h.coord.wait_for(job, WAIT, |v| v.progress.is_some_and(|p| p > 0.0)).await.expect("first step done on A");
    let v: jm::JobResult =
        serde_json::from_value(h.rpc("jobs.migrate", json!({"jobId": job, "targetNodeId": b})).await.unwrap()).unwrap();
    assert_eq!(v.job.epoch, LeaseEpoch(2));
    let v = h.wait_state(job, JobState::Completed).await;
    assert_eq!(v.owner, Some(orkestr_domain::ExecutorRef::Node { node_id: b }));
    // result diff applied into the coordinator worktree (local edit preserved)
    assert_eq!(std::fs::read_to_string(wt.join("a.txt")).unwrap(), "one\n");
    assert_eq!(std::fs::read_to_string(wt.join("b.txt")).unwrap(), "two\n");
    assert_eq!(std::fs::read_to_string(wt.join("c.txt")).unwrap(), "three\n");
    assert_eq!(std::fs::read_to_string(wt.join("README.md")).unwrap(), "hello\nlocal edit\n");
    // B resumed from the handoff and did not repeat A's steps
    let steps_a = h.steps_from(a, job);
    let steps_b = h.steps_from(b, job);
    assert!(steps_b.iter().any(|s| s.starts_with("resumed from handoff")), "{steps_b:?}");
    for s in steps_a.iter().filter(|s| !s.starts_with("resumed")) {
        assert!(!steps_b.contains(s), "step {s} repeated on B");
    }
    assert!(h.has_event(|k| matches!(k, EventKind::JobMigrated(m) if m.from_node_id == a && m.to_node_id == b && m.epoch == LeaseEpoch(2))));
    assert!(h.has_event(|k| matches!(k, EventKind::JobCheckpointed(c) if c.checkpoint.node_id == a && c.checkpoint.handoff.is_some() && c.checkpoint.agent.as_ref().is_some_and(|ag| !ag.completed_steps.is_empty()))));
    assert_eq!(h.task_status(tid).await, TaskStatus::Completed);

    // worker side: an offer / cancel with an old epoch is rejected
    let offer_b = h.workers[1].host.leases().into_iter().find(|l| l.job_id == job).unwrap();
    assert_eq!(offer_b.epoch, LeaseEpoch(2));
    let err = call_typed::<jm::JobCancel>(
        h.workers[1].link.as_ref(),
        &WorkerJobParams { job_id: job, epoch: LeaseEpoch(1), immediate: true },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("stale epoch"), "{err}");
    let audit_b = h.workers[1].core.services.security.audit.query(Default::default()).await.unwrap();
    assert!(format!("{audit_b:?}").contains("jobs.staleEpochRejected"));
    // coordinator side: progress from A with epoch 1 is dropped
    let before = h.coord.stale_dropped();
    h.workers[0]
        .link
        .event_sender()
        .send(Event {
            seq: 0,
            id: orkestr_domain::EventId::new(),
            ts: chrono::Utc::now(),
            project_id: None,
            task_id: None,
            agent_id: None,
            node_id: Some(a),
            correlation_id: None,
            kind: EventKind::JobProgress(jm::JobProgressPayload {
                job_id: job,
                task_id: tid,
                node_id: a,
                epoch: LeaseEpoch(1),
                items: vec![JobProgressItem::Finished {
                    success: false,
                    exit_code: Some(1),
                    artifacts: vec![],
                    detail: None,
                }],
            }),
        })
        .unwrap();
    wait_until(|| h.coord.stale_dropped() > before).await;
    assert_eq!(h.coord.get(job).unwrap().state, JobState::Completed, "stale message had no effect");
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lease_expiry_migrates_and_deploy_side_effect_needs_user_decision() {
    let g = GitCli::discover();
    if !g.is_available() {
        return;
    }
    let h = Harness::new(Duration::from_millis(1200), Some(Duration::from_secs(3600))).await;
    let (a, b) = (h.workers[0].link.node_id(), h.workers[1].link.node_id());
    // 1) idempotent job: A dies after its first checkpoint → resumes on B
    let (_d, wt) = repo(&g).await;
    let (pid, tid) = h.task().await;
    let prompt = "write a.txt one\nsleep 1500\nwrite b.txt two\nwrite c.txt three";
    let spec = Harness::spec(pid, tid, JobKind::AgentTask { template_id: None, model: None, prompt: prompt.into() });
    let job = spec.job_id;
    h.coord.submit(spec, Some(wt.clone())).await.unwrap();
    wait_until(|| h.coord.last_checkpoint(job).is_some()).await;
    h.alive.lock().remove(&a); // node A stops heartbeating
    let v = h.coord.wait_for(job, WAIT, |v| v.state == JobState::Completed).await.expect("completed on B");
    assert_eq!(v.owner, Some(orkestr_domain::ExecutorRef::Node { node_id: b }));
    assert!(v.epoch >= LeaseEpoch(2));
    assert_eq!(std::fs::read_to_string(wt.join("c.txt")).unwrap(), "three\n");
    let (hist, _) = h.core.services.tasks.history(tid).await.unwrap();
    let path: Vec<TaskStatus> = hist.iter().map(|e| e.to).collect();
    let pos = |s| path.iter().position(|x| *x == s).unwrap_or_else(|| panic!("{s:?} missing in {path:?}"));
    assert!(pos(TaskStatus::Interrupted) < pos(TaskStatus::Recovering));
    assert!(path.iter().filter(|s| **s == TaskStatus::Running).count() >= 2, "{path:?}");
    assert!(h.has_event(
        |k| matches!(k, EventKind::JobMigrated(m) if m.from_node_id == a && m.reason.contains("node_lost"))
    ));
    // A's lease self-fenced: its job ended without touching the coordinator
    wait_until(|| h.workers[0].host.leases().iter().any(|l| l.job_id == job && l.phase != LeasePhase::Running)).await;

    // 2) deploy since the last checkpoint → Failed(needs_user_decision) + approval
    h.alive.lock().insert(a);
    h.placer.0.lock().retain(|n| *n == b);
    h.placer.0.lock().push(a);
    let (pid2, tid2) = h.task().await;
    let prompt = "write a.txt x\ndeploy production\nsleep 5000\nwrite b.txt y";
    let spec = Harness::spec(pid2, tid2, JobKind::AgentTask { template_id: None, model: None, prompt: prompt.into() });
    let job2 = spec.job_id;
    h.coord.submit(spec, None).await.unwrap();
    wait_until(|| h.steps_from(b, job2).iter().any(|s| s.contains("deploy"))).await;
    h.alive.lock().remove(&b);
    let v = h.wait_state(job2, JobState::Failed).await;
    assert_eq!(v.detail.as_deref(), Some("needs_user_decision"));
    assert_eq!(h.task_status(tid2).await, TaskStatus::Failed);
    let approvals = h.core.services.security.approvals.list(&Default::default()).await.unwrap();
    assert!(approvals.iter().any(|ap| format!("{ap:?}").contains("needs_user_decision")), "{approvals:?}");
    h.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_and_emergency_stop_broadcast() {
    let h = Harness::new(Duration::from_secs(10), None).await;
    let (a, b) = (h.workers[0].link.node_id(), h.workers[1].link.node_id());
    let (pid, tid) = h.task().await;
    let long = || JobKind::AgentTask { template_id: None, model: None, prompt: "sleep 20000\nwrite x.txt late".into() };
    let s1 = Harness::spec(pid, tid, long());
    let j1 = s1.job_id;
    h.coord.submit(s1, None).await.unwrap();
    let (pid2, tid2) = h.task().await;
    h.placer.0.lock().reverse();
    let s2 = Harness::spec(pid2, tid2, long());
    let j2 = s2.job_id;
    h.coord.submit(s2, None).await.unwrap();
    assert_eq!(h.coord.get(j1).unwrap().owner, Some(orkestr_domain::ExecutorRef::Node { node_id: a }));
    assert_eq!(h.coord.get(j2).unwrap().owner, Some(orkestr_domain::ExecutorRef::Node { node_id: b }));
    // jobs.cancel (UI)
    let r = h.rpc("jobs.cancel", json!({"jobId": j1, "immediate": false})).await.unwrap();
    assert_eq!(r["job"]["state"], "cancelled");
    wait_until(|| h.workers[0].host.leases().iter().any(|l| l.job_id == j1 && l.phase == LeasePhase::Cancelled)).await;
    assert_eq!(h.task_status(tid).await, TaskStatus::Cancelled);
    // emergency stop on the coordinator reaches every node
    h.rpc("emergency.stopAll", json!({})).await.unwrap();
    h.wait_state(j2, JobState::Cancelled).await;
    wait_until(|| h.workers[1].host.leases().iter().any(|l| l.job_id == j2 && l.phase == LeasePhase::Cancelled)).await;
    wait_until(|| h.workers.iter().all(|w| w.core.services.security.emergency.flags().all_stopped)).await;
    h.finish().await;
}
