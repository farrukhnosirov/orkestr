//! `jobs.submit` / `jobs.list` / `jobs.get` through the core router with
//! in-process node snapshots (no links) and a recording dispatcher.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use orkestr_core::rpc::jobs_ui::{clear_hooks, install_hooks, local_snapshot, JobsUiHooks, NodeSnapshotSource};
use orkestr_core::rpc::{build_router, Principal, RpcContext, RpcRouter};
use orkestr_core::{CoreConfig, Services};
use orkestr_domain::{
    Arch, CorrelationId, ExecutorRef, GpuInfo, JobId, NodeId, NodeStatus, OsKind, SessionId, TaskStatus, TrustLevel,
};
use orkestr_protocol::methods::jobs::{
    GpuRequirement, JobKind, JobState, JobSubmitSpec, Placement, Requirements, SubmitJobParams,
};
use orkestr_protocol::methods::nodes::{NodeSlots, NodeSnapshot};
use orkestr_task_engine::jobs::JobDispatcher;
use serde_json::{json, Value};

const GIB: u64 = 1024 * 1024 * 1024;

struct Nodes(Mutex<Vec<NodeSnapshot>>);

#[async_trait]
impl NodeSnapshotSource for Nodes {
    async fn snapshots(&self) -> Vec<NodeSnapshot> {
        self.0.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<JobId>>);

impl JobDispatcher for Recorder {
    fn dispatch(&self, job_id: JobId) {
        self.0.lock().unwrap().push(job_id);
    }
}

struct Rpc {
    router: RpcRouter,
    services: Arc<Services>,
}

impl Rpc {
    async fn call(&self, method: &str, params: Value) -> Result<Value, Value> {
        let ctx = RpcContext {
            services: Arc::clone(&self.services),
            session_id: SessionId::new(),
            correlation_id: CorrelationId::new(),
            principal: Principal::Local,
        };
        self.router.dispatch(ctx, method, params).await.map_err(|e| serde_json::to_value(e).unwrap())
    }
    async fn ok(&self, method: &str, params: Value) -> Value {
        self.call(method, params).await.unwrap_or_else(|e| panic!("{method}: {e}"))
    }
    async fn task(&self, project: &str) -> String {
        self.ok("tasks.create", json!({"projectId": project, "title": "t"})).await["task"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    async fn submit(&self, task: &str, kind: JobKind, requirements: Requirements) -> Result<Value, Value> {
        let p = SubmitJobParams {
            task_id: task.parse().unwrap(),
            spec: JobSubmitSpec {
                kind,
                requirements,
                estimate: Default::default(),
                needs: Default::default(),
                affinity: Default::default(),
            },
        };
        self.call("jobs.submit", serde_json::to_value(p).unwrap()).await.map(|v| v["job"].clone())
    }
}

fn cmd() -> JobKind {
    JobKind::Command { program: "xcodebuild".into(), args: vec![], env: BTreeMap::new(), timeout_secs: None }
}

fn remote(base: &NodeSnapshot, name: &str, os: OsKind, arch: Arch) -> NodeSnapshot {
    let mut n = base.clone();
    n.node.id = NodeId::new();
    n.node.name = name.into();
    n.node.os = os;
    n.node.arch = arch;
    n.node.is_local = false;
    n.node.tools = vec!["git".into()];
    n.node.resources.gpus = vec![];
    n.is_coordinator = false;
    n.trust_level = TrustLevel::Standard;
    n.status = NodeStatus::Online;
    n.slots = NodeSlots { max: 2, used: 0 };
    n
}

#[tokio::test]
async fn submit_places_waits_and_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let core = orkestr_core::start(CoreConfig::with_data_dir(dir.path())).await.unwrap();
    let rpc = Rpc { router: build_router(), services: Arc::clone(&core.services) };
    let me = core.services.node_id;
    let pid = rpc.ok("projects.create", json!({"name": "Nodes"})).await["project"]["id"].as_str().unwrap().to_owned();

    // Default wiring (no workers enrolled): the local node (coordinator) takes a plain command.
    let t0 = rpc.task(&pid).await;
    let job = rpc.submit(&t0, cmd(), Requirements::default()).await.unwrap();
    assert_eq!(job["state"], "offered");
    assert_eq!(job["owner"]["nodeId"], me.to_string());

    // In-proc fleet: coordinator + mac + gpu.
    let local = local_snapshot(&core.services).await;
    let mut mac = remote(&local, "mac", OsKind::MacOs, Arch::Arm64);
    mac.node.tools.push("xcodebuild".into());
    let mut gpu = remote(&local, "gpu", OsKind::Linux, Arch::X64);
    gpu.node.resources.gpus = vec![GpuInfo { name: "NVIDIA GeForce RTX 4090".into(), vram_bytes: Some(24 * GIB) }];
    let nodes = Arc::new(Nodes(Mutex::new(vec![local.clone(), mac.clone(), gpu.clone()])));
    let rec = Arc::new(Recorder::default());
    install_hooks(
        me,
        JobsUiHooks {
            snapshots: Some(Arc::clone(&nodes) as Arc<dyn NodeSnapshotSource>),
            dispatcher: Some(Arc::clone(&rec) as Arc<dyn JobDispatcher>),
            weights: None,
        },
    );

    // Scenario F: macOS build → mac; explanation stored; owner set; dispatched.
    let t1 = rpc.task(&pid).await;
    let macos = Requirements { os: Some(OsKind::MacOs), tools: vec!["xcodebuild".into()], ..Requirements::default() };
    let job = rpc.submit(&t1, cmd(), macos.clone()).await.unwrap();
    assert_eq!(job["state"], "offered");
    assert_eq!(job["epoch"], 1);
    assert_eq!(job["owner"]["nodeId"], mac.node.id.to_string());
    let placement: Placement = serde_json::from_value(job["placement"].clone()).unwrap();
    let Placement::Place { node_id, explanation, .. } = placement else { panic!("{job}") };
    assert_eq!(node_id, mac.node.id);
    assert!(explanation[0].starts_with("place("), "{explanation:?}");
    assert!(explanation.iter().any(|l| l.starts_with("reject.os(")));
    let job_id: JobId = job["spec"]["jobId"].as_str().unwrap().parse().unwrap();
    assert_eq!(*rec.0.lock().unwrap(), vec![job_id]);
    let task = rpc.ok("tasks.get", json!({"taskId": t1})).await["task"].clone();
    let owner: ExecutorRef = serde_json::from_value(task["owner"].clone()).unwrap();
    assert_eq!(owner, ExecutorRef::Node { node_id: mac.node.id });
    let got = rpc.ok("jobs.get", json!({"jobId": job_id})).await["job"].clone();
    assert_eq!(got, job);
    let listed = rpc.ok("jobs.list", json!({"taskId": t1})).await["jobs"].clone();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(rpc.ok("jobs.list", json!({"nodeId": mac.node.id})).await["jobs"].as_array().unwrap().len(), 1);
    // A second job for the same task is refused.
    assert_eq!(rpc.submit(&t1, cmd(), macos.clone()).await.unwrap_err()["code"], "conflict");

    // Impossible GPU → Infeasible, task Blocked.
    let t2 = rpc.task(&pid).await;
    let big = Requirements {
        gpu: Some(GpuRequirement { min_vram_mb: Some(80 * 1024), vendor: None }),
        ..Requirements::default()
    };
    let job = rpc.submit(&t2, cmd(), big).await.unwrap();
    assert_eq!(job["state"], "infeasible");
    assert_eq!(job["placement"]["decision"], "infeasible");
    let task = rpc.ok("tasks.get", json!({"taskId": t2})).await["task"].clone();
    assert_eq!(task["status"], serde_json::to_value(TaskStatus::Blocked).unwrap());

    // Only mac fits and it is draining → Wait, job stays pending, not dispatched.
    nodes.0.lock().unwrap()[1].status = NodeStatus::Draining;
    let t3 = rpc.task(&pid).await;
    let job = rpc.submit(&t3, cmd(), macos).await.unwrap();
    assert_eq!(job["state"], serde_json::to_value(JobState::Pending).unwrap());
    assert_eq!(job["placement"]["decision"], "wait");
    assert_eq!(job["placement"]["reason"], "nodesUnavailable");
    assert_eq!(rec.0.lock().unwrap().len(), 1);

    // Validation.
    let t4 = rpc.task(&pid).await;
    let err = rpc.submit(&t4, JobKind::Service, Requirements::default()).await.unwrap_err();
    assert_eq!(err["data"]["unimplementedFeature"], "jobs.service");
    let empty = JobKind::AgentTask { template_id: None, model: None, prompt: " ".into() };
    assert_eq!(rpc.submit(&t4, empty, Requirements::default()).await.unwrap_err()["code"], "bad_request");
    assert_eq!(rpc.call("jobs.get", json!({"jobId": JobId::new()})).await.unwrap_err()["code"], "not_found");

    clear_hooks(me);
    core.shutdown().await;
}
