//! DefaultNodeScheduler: filter matrix, score weights, tie-break, Wait vs
//! Infeasible and golden explanation strings (node-protocol.md §5).

use std::collections::BTreeMap;

use orkestr_domain::{
    Arch, EnvironmentKind, GpuInfo, JobId, Node, NodeId, NodeResources, NodeStatus, OsKind, ProjectId, TaskId,
    TaskPriority, TrustLevel,
};
use orkestr_node::NodeScheduler;
use orkestr_protocol::methods::jobs::{
    Affinity, GpuRequirement, JobKind, JobSpec, JobWaitReason, Needs, Placement, PlacementPolicy, Requirements,
    ResourceEstimate, ScoreWeights,
};
use orkestr_protocol::methods::nodes::{NodeSlots, NodeSnapshot};
use orkestr_task_engine::node_scheduler::{gpu_vendor, required_trust, score_terms, weighted, DefaultNodeScheduler};

const GIB: u64 = 1024 * 1024 * 1024;

fn nid(n: u8) -> NodeId {
    NodeId::parse(&format!("00000000-0000-7000-8000-0000000000{n:02x}")).unwrap()
}

fn project() -> ProjectId {
    ProjectId::parse("00000000-0000-7000-8000-00000000aaaa").unwrap()
}

fn node(n: u8, name: &str, os: OsKind, arch: Arch) -> NodeSnapshot {
    NodeSnapshot {
        node: Node {
            id: nid(n),
            name: name.into(),
            hostname: name.into(),
            os,
            os_version: "1".into(),
            arch,
            core_version: "0.6.0".into(),
            status: NodeStatus::Online,
            resources: NodeResources {
                cpu_cores: 8,
                cpu_usage_percent: 50.0,
                memory_total_bytes: 32 * GIB,
                memory_used_bytes: 16 * GIB,
                disk_total_bytes: 500 * GIB,
                disk_free_bytes: 250 * GIB,
                gpus: Vec::new(),
            },
            tools: vec!["git".into(), "cargo".into()],
            local_models: Vec::new(),
            is_local: false,
            last_heartbeat_at: None,
        },
        status: NodeStatus::Online,
        trust_level: TrustLevel::Standard,
        allowed_projects: vec!["*".into()],
        slots: NodeSlots { max: 2, used: 0 },
        leases: Vec::new(),
        last_heartbeat_at: None,
        is_coordinator: false,
        labels: BTreeMap::new(),
        model_digests: Vec::new(),
    }
}

fn win() -> NodeSnapshot {
    let mut n = node(1, "win", OsKind::Windows, Arch::X64);
    n.is_coordinator = true;
    n.node.is_local = true;
    n.trust_level = TrustLevel::Trusted;
    n
}
fn mac() -> NodeSnapshot {
    let mut n = node(2, "mac", OsKind::MacOs, Arch::Arm64);
    n.node.tools.push("xcodebuild".into());
    n
}
fn gpu() -> NodeSnapshot {
    let mut n = node(3, "gpu", OsKind::Linux, Arch::X64);
    n.node.resources.gpus = vec![GpuInfo { name: "NVIDIA GeForce RTX 4090".into(), vram_bytes: Some(24 * GIB) }];
    n.node.local_models = vec!["llama3.1:8b".into()];
    n
}

fn command() -> JobSpec {
    JobSpec {
        job_id: JobId::new(),
        task_id: TaskId::new(),
        project_id: project(),
        kind: JobKind::Command { program: "make".into(), args: vec![], env: BTreeMap::new(), timeout_secs: None },
        requirements: Requirements::default(),
        estimate: ResourceEstimate::default(),
        needs: Needs::default(),
        priority: TaskPriority::default(),
        affinity: Affinity::default(),
    }
}

fn agent() -> JobSpec {
    JobSpec { kind: JobKind::AgentTask { template_id: None, model: None, prompt: "do it".into() }, ..command() }
}

fn policy() -> PlacementPolicy {
    PlacementPolicy::default()
}

fn place(job: &JobSpec, nodes: &[NodeSnapshot]) -> Placement {
    DefaultNodeScheduler.place(job, nodes, &policy())
}

fn placed(p: &Placement) -> NodeId {
    match p {
        Placement::Place { node_id, .. } => *node_id,
        other => panic!("expected Place, got {other:?}"),
    }
}

fn all() -> Vec<NodeSnapshot> {
    vec![win(), mac(), gpu()]
}

/// Asserts the job is rejected by the single node `n` with explanation code `code`.
fn rejected_with(job: &JobSpec, n: NodeSnapshot, code: &str) {
    let p = place(job, &[n]);
    let Placement::Infeasible { explanation, .. } = &p else { panic!("expected Infeasible for {code}, got {p:?}") };
    assert!(explanation[1].starts_with(&format!("{code}(")), "{code}: {explanation:?}");
}

#[test]
fn filter_matrix() {
    // OS / arch
    let mut j = command();
    j.requirements.os = Some(OsKind::MacOs);
    assert_eq!(placed(&place(&j, &all())), nid(2));
    rejected_with(&j, gpu(), "reject.os");
    let mut j = command();
    j.requirements.arch = Some(Arch::Arm64);
    assert_eq!(placed(&place(&j, &all())), nid(2));
    rejected_with(&j, win(), "reject.arch");
    // tools
    let mut j = command();
    j.requirements.tools = vec!["xcodebuild".into()];
    assert_eq!(placed(&place(&j, &all())), nid(2));
    rejected_with(&j, gpu(), "reject.tool");
    // GPU presence / VRAM / vendor
    let mut j = command();
    j.requirements.gpu = Some(GpuRequirement { min_vram_mb: Some(16 * 1024), vendor: None });
    assert_eq!(placed(&place(&j, &all())), nid(3));
    rejected_with(&j, mac(), "reject.gpu");
    let mut small = gpu();
    small.node.resources.gpus[0].vram_bytes = Some(8 * GIB);
    rejected_with(&j, small, "reject.gpuVram");
    j.requirements.gpu = Some(GpuRequirement { min_vram_mb: None, vendor: Some("amd".into()) });
    rejected_with(&j, gpu(), "reject.gpuVendor");
    j.requirements.gpu = Some(GpuRequirement { min_vram_mb: None, vendor: Some("nvidia".into()) });
    assert_eq!(placed(&place(&j, &all())), nid(3));
    // local models (exact and :latest)
    let mut j = command();
    j.requirements.local_models = vec!["llama3.1:8b".into()];
    assert_eq!(placed(&place(&j, &all())), nid(3));
    rejected_with(&j, mac(), "reject.model");
    // labels
    let mut j = command();
    j.requirements.labels.insert("rack".into(), "a".into());
    let mut labelled = mac();
    labelled.labels.insert("rack".into(), "a".into());
    assert_eq!(placed(&place(&j, &[gpu(), labelled])), nid(2));
    rejected_with(&j, gpu(), "reject.label");
    // memory / cpu totals
    let mut j = command();
    j.requirements.min_mem_mb = Some(64 * 1024);
    rejected_with(&j, gpu(), "reject.memory");
    let mut j = command();
    j.estimate.cpu_cores = 16.0;
    rejected_with(&j, gpu(), "reject.cpu");
    // trust vs kind: restricted can't run AgentTask, can run a Command
    let mut r = gpu();
    r.trust_level = TrustLevel::Restricted;
    rejected_with(&agent(), r.clone(), "reject.trust");
    assert_eq!(placed(&place(&command(), std::slice::from_ref(&r))), nid(3));
    let mut j = command();
    j.needs.network = true;
    rejected_with(&j, r, "reject.trust");
    let mut j = command();
    j.needs.environment = Some(EnvironmentKind::Production);
    rejected_with(&j, gpu(), "reject.trust");
    assert_eq!(placed(&place(&j, &all())), nid(1));
    // allowed projects
    let mut n = gpu();
    n.allowed_projects = vec![ProjectId::new().to_string()];
    rejected_with(&command(), n.clone(), "reject.project");
    n.allowed_projects = vec![project().to_string()];
    assert_eq!(placed(&place(&command(), &[n])), nid(3));
    let mut n = gpu();
    n.allowed_projects = vec![];
    rejected_with(&command(), n, "reject.project");
    // secrets only on the coordinator
    let mut j = command();
    j.needs.secrets = vec!["API_KEY".into()];
    rejected_with(&j, gpu(), "reject.secrets");
    assert_eq!(placed(&place(&j, &all())), nid(1));
    // affinity forbidden, revoked
    let mut j = command();
    j.affinity.forbidden = vec![nid(1), nid(2)];
    assert_eq!(placed(&place(&j, &all())), nid(3));
    rejected_with(&j, win(), "reject.forbidden");
    let mut n = gpu();
    n.status = NodeStatus::Revoked;
    rejected_with(&command(), n, "reject.revoked");
    // service jobs
    let j = JobSpec { kind: JobKind::Service, ..command() };
    rejected_with(&j, win(), "reject.kind");
}

#[test]
fn unavailable_and_busy_nodes_are_excluded() {
    for s in [NodeStatus::Draining, NodeStatus::Suspect, NodeStatus::Offline, NodeStatus::Degraded] {
        let mut m = mac();
        m.status = s;
        let mut j = command();
        j.requirements.tools = vec!["git".into()];
        j.affinity.forbidden = vec![nid(1)];
        assert_eq!(placed(&place(&j, &[m, gpu()])), nid(3), "{s:?} excluded");
    }
    let mut full = gpu();
    full.slots = NodeSlots { max: 2, used: 2 };
    let mut j = command();
    j.affinity.forbidden = vec![nid(1)];
    assert_eq!(placed(&place(&j, &[full, mac()])), nid(2));
    let mut j = command();
    j.estimate.mem_mb = 20 * 1024; // 16 GiB free on every node
    assert!(matches!(place(&j, &[gpu()]), Placement::Wait { reason: JobWaitReason::NoCapacity, .. }));
}

#[test]
fn wait_vs_infeasible() {
    let mut j = command();
    j.requirements.os = Some(OsKind::MacOs);
    // Only mac can run it; it is draining → Wait(nodesUnavailable).
    let mut m = mac();
    m.status = NodeStatus::Draining;
    let p = place(&j, &[win(), m, gpu()]);
    assert!(matches!(p, Placement::Wait { reason: JobWaitReason::NodesUnavailable, .. }), "{p:?}");
    // Busy → Wait(noCapacity).
    let mut m = mac();
    m.slots.used = 2;
    let p = place(&j, &[win(), m, gpu()]);
    assert!(matches!(p, Placement::Wait { reason: JobWaitReason::NoCapacity, .. }), "{p:?}");
    // No macOS node at all → Infeasible.
    let p = place(&j, &[win(), gpu()]);
    assert!(matches!(&p, Placement::Infeasible { reason, .. } if reason == "noFeasibleNode"), "{p:?}");
    // A revoked mac never counts as "could run later".
    let mut m = mac();
    m.status = NodeStatus::Revoked;
    assert!(matches!(place(&j, &[m]), Placement::Infeasible { .. }));
    // No nodes.
    assert!(matches!(&place(&j, &[]), Placement::Infeasible { reason, .. } if reason == "noNodes"));
}

#[test]
fn score_weights_drive_the_choice() {
    // Two remote, otherwise identical nodes; idle one wins on headroom/load.
    let mut a = node(5, "a", OsKind::Linux, Arch::X64);
    let mut b = node(6, "b", OsKind::Linux, Arch::X64);
    a.node.resources.cpu_usage_percent = 90.0;
    b.node.resources.cpu_usage_percent = 10.0;
    let j = command();
    assert_eq!(placed(&place(&j, &[a.clone(), b.clone()])), nid(6));
    // Preference outweighs load with a preference-only policy.
    let mut j2 = command();
    j2.affinity.preferred = vec![nid(5)];
    let only_pref = PlacementPolicy {
        weights: ScoreWeights {
            headroom: 0.0,
            cache_warmth: 0.0,
            load: 0.0,
            network: 0.0,
            preference: 1.0,
            energy: 0.0,
        },
        coordinator_node_id: None,
    };
    assert_eq!(placed(&DefaultNodeScheduler.place(&j2, &[a.clone(), b.clone()], &only_pref)), nid(5));
    // Default weights sum to 1 and terms are normalized.
    let t = score_terms(&j, &b);
    assert!((t.headroom - (0.9 + 0.5 + 1.0) / 3.0).abs() < 1e-9);
    assert!((t.load - 0.9).abs() < 1e-9);
    assert!((t.preference - 0.5).abs() < 1e-9);
    let s = weighted(&t, &ScoreWeights::default());
    let manual = 0.30 * t.headroom + 0.25 * 0.0 + 0.15 * t.load + 0.10 * 0.5 + 0.15 * 0.5 + 0.05 * 1.0;
    assert!((s - manual).abs() < 1e-9, "{s} vs {manual}");
    // Energy: a node on battery loses against an identical one on AC.
    let mut on_battery = b.clone();
    on_battery.node.id = nid(4);
    on_battery.labels.insert("power".into(), "battery".into());
    assert_eq!(placed(&place(&j, &[on_battery, b])), nid(6));
}

#[test]
fn tie_break_prefers_coordinator_then_lowest_id() {
    let w = PlacementPolicy {
        weights: ScoreWeights {
            headroom: 0.0,
            cache_warmth: 0.0,
            load: 0.0,
            network: 0.0,
            preference: 1.0,
            energy: 0.0,
        },
        coordinator_node_id: None,
    };
    let j = command();
    let (x, y, z) = (
        node(9, "x", OsKind::Linux, Arch::X64),
        node(7, "y", OsKind::Linux, Arch::X64),
        node(8, "z", OsKind::Linux, Arch::X64),
    );
    // All tie on preference: lowest id wins regardless of input order.
    assert_eq!(placed(&DefaultNodeScheduler.place(&j, &[x.clone(), y.clone(), z.clone()], &w)), nid(7));
    assert_eq!(placed(&DefaultNodeScheduler.place(&j, &[z.clone(), x.clone(), y.clone()], &w)), nid(7));
    // Coordinator (via policy) wins the tie even with the highest id.
    let wc = PlacementPolicy { coordinator_node_id: Some(nid(9)), ..w };
    let p = DefaultNodeScheduler.place(&j, &[y, z, x], &wc);
    assert_eq!(placed(&p), nid(9));
    let Placement::Place { explanation, .. } = p else { unreachable!() };
    assert!(explanation.iter().any(|l| l.starts_with("tieBreak(rule=coordinator")), "{explanation:?}");
}

#[test]
fn derived_trust_and_vendor() {
    assert_eq!(required_trust(&command()), TrustLevel::Restricted);
    assert_eq!(required_trust(&agent()), TrustLevel::Standard);
    let mut j = command();
    j.needs.min_trust = Some(TrustLevel::Trusted);
    assert_eq!(required_trust(&j), TrustLevel::Trusted);
    let g = |n: &str| gpu_vendor(&GpuInfo { name: n.into(), vram_bytes: None });
    assert_eq!(g("NVIDIA GeForce RTX 4090"), Some("nvidia"));
    assert_eq!(g("AMD Radeon RX 7900"), Some("amd"));
    assert_eq!(g("Apple M3 Max"), Some("apple"));
    assert_eq!(g("Intel(R) UHD Graphics"), Some("intel"));
    assert_eq!(g("Matrox"), None);
}

#[test]
fn golden_explanations() {
    // Scenario F: macOS build goes to mac.
    let mut j = command();
    j.requirements.os = Some(OsKind::MacOs);
    j.requirements.tools = vec!["xcodebuild".into()];
    let Placement::Place { explanation, .. } = place(&j, &all()) else { panic!() };
    let golden = [
        "place(node=00000000-0000-7000-8000-000000000002, score=0.450): placed on mac (score 0.450)",
        "score(node=00000000-0000-7000-8000-000000000002, headroom=0.667, cacheWarmth=0.000, load=0.500, network=0.500, preference=0.500, energy=1.000): headroom 0.667, cache 0.000, 1-load 0.500, network 0.500, preference 0.500, energy 1.000",
        "reject.os(node=00000000-0000-7000-8000-000000000001, required=macos, actual=windows): win runs windows, job requires macos",
        "reject.os(node=00000000-0000-7000-8000-000000000003, required=macos, actual=linux): gpu runs linux, job requires macos",
    ];
    assert_eq!(explanation, golden);

    // Impossible GPU → Infeasible.
    let mut j = command();
    j.requirements.gpu = Some(GpuRequirement { min_vram_mb: Some(80 * 1024), vendor: None });
    let Placement::Infeasible { reason, explanation } = place(&j, &all()) else { panic!() };
    assert_eq!(reason, "noFeasibleNode");
    let golden = [
        "infeasible.noFeasibleNode(rejected=3): no enrolled node can ever satisfy this job",
        "reject.gpu(node=00000000-0000-7000-8000-000000000001): win has no GPU",
        "reject.gpu(node=00000000-0000-7000-8000-000000000002): mac has no GPU",
        "reject.gpuVram(node=00000000-0000-7000-8000-000000000003, requiredMb=81920, actualMb=24576): gpu has at most 24576 MiB VRAM, job needs 81920 MiB",
    ];
    assert_eq!(explanation, golden);

    // Restricted node refuses AgentTask; draining mac → Wait.
    let mut g = gpu();
    g.trust_level = TrustLevel::Restricted;
    let mut m = mac();
    m.status = NodeStatus::Draining;
    let mut j = agent();
    j.affinity.forbidden = vec![nid(1)];
    let Placement::Wait { reason, explanation } = place(&j, &[win(), m, g]) else { panic!() };
    assert_eq!(reason, JobWaitReason::NodesUnavailable);
    let golden = [
        "wait.nodesUnavailable(rejected=3): suitable nodes exist but none is online now",
        "reject.forbidden(node=00000000-0000-7000-8000-000000000001): win is forbidden for this job",
        "unavailable.status(node=00000000-0000-7000-8000-000000000002, status=draining): mac is draining",
        "reject.trust(node=00000000-0000-7000-8000-000000000003, required=standard, actual=restricted, kind=agentTask): gpu has trust level restricted; agentTask jobs need standard",
    ];
    assert_eq!(explanation, golden);

    // Plain command: coordinator wins (cache + network), others listed as candidates.
    let Placement::Place { node_id, explanation, .. } = place(&command(), &all()) else { panic!() };
    assert_eq!(node_id, nid(1));
    assert_eq!(
        explanation[0],
        "place(node=00000000-0000-7000-8000-000000000001, score=0.750): placed on win (score 0.750)"
    );
    assert_eq!(
        &explanation[2..],
        [
            "candidate(node=00000000-0000-7000-8000-000000000002, score=0.450): mac was also suitable (score 0.450)",
            "candidate(node=00000000-0000-7000-8000-000000000003, score=0.450): gpu was also suitable (score 0.450)",
        ]
    );
}
