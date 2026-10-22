//! Deterministic node scheduler (Phase 6, node-protocol.md §5). **Owner: WP-B.**
//!
//! Filter (hard constraints) → score (weighted, normalized) → tie-break
//! (coordinator, then lowest `NodeId`); explanation strings are stored on the
//! job (`JobView.placement`) and shown in the UI.
//!
//! ## Filter classes
//! Every node is checked in a fixed order. *Permanent* rejections (the node can
//! never run the job: revoked, forbidden, project not allowed, trust, secrets on
//! a remote node, OS, arch, CPU/RAM/GPU totals, tools, labels, local models)
//! are checked before *transient* ones (health Offline/Suspect/Draining/
//! Degraded, no free slot, not enough free RAM/disk now). Result:
//! * some node passes everything → [`Placement::Place`];
//! * none passes, but at least one fails only transiently → [`Placement::Wait`]
//!   (`noCapacity` if any of those is merely busy, else `nodesUnavailable`);
//! * otherwise → [`Placement::Infeasible`] (task goes `Blocked`).
//!
//! ## Explanation lines
//! Each line is `code(k=v, …): English text`. `code` + params are stable and
//! i18n-key friendly (UI: `nodes.explain.<code>` with the params); the English
//! text is a fallback. Lines: the decision first, then the winner's score
//! terms, the tie-break (if any), other feasible candidates, then one line per
//! rejected node — nodes in ascending `NodeId` order. Golden-tested.

use std::cmp::Ordering;

use orkestr_domain::{EnvironmentKind, GpuInfo, NodeId, NodeStatus, OsKind, TrustLevel};
use orkestr_node::NodeScheduler;
use orkestr_protocol::methods::jobs::{JobKind, JobSpec, JobWaitReason, Placement, PlacementPolicy, ScoreWeights};
use orkestr_protocol::methods::nodes::NodeSnapshot;

/// Scores closer than this are a tie.
const TIE_EPSILON: f64 = 1e-9;
const MIB: u64 = 1024 * 1024;

/// The default scheduler.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultNodeScheduler;

/// One explanation line: machine code + params + English fallback text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    /// Stable machine code (`reject.os`, `wait.noCapacity`, …).
    pub code: &'static str,
    /// Ordered params.
    pub params: Vec<(&'static str, String)>,
    /// English fallback.
    pub text: String,
}

impl Reason {
    fn new(code: &'static str, params: Vec<(&'static str, String)>, text: impl Into<String>) -> Self {
        Self { code, params, text: text.into() }
    }

    /// Renders `code(k=v, …): text`.
    #[must_use]
    pub fn render(&self) -> String {
        let params: Vec<String> = self.params.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{}({}): {}", self.code, params.join(", "), self.text)
    }
}

/// Outcome of filtering one node.
#[derive(Debug, Clone, PartialEq)]
enum Verdict {
    Eligible,
    /// Can never run the job.
    Permanent(Reason),
    /// Could run it, but not now; `busy` = capacity (vs. health).
    Transient {
        reason: Reason,
        busy: bool,
    },
}

/// Trust level a job needs (explicit `needs.minTrust` or derived from kind/needs).
#[must_use]
pub fn required_trust(job: &JobSpec) -> TrustLevel {
    let mut t = TrustLevel::Restricted;
    if matches!(job.kind, JobKind::AgentTask { .. } | JobKind::Service)
        || job.needs.network
        || !job.needs.secrets.is_empty()
    {
        t = TrustLevel::Standard;
    }
    if matches!(job.needs.environment, Some(EnvironmentKind::Production)) {
        t = TrustLevel::Trusted;
    }
    job.needs.min_trust.map_or(t, |m| m.max(t))
}

fn os_name(o: OsKind) -> &'static str {
    match o {
        OsKind::Windows => "windows",
        OsKind::MacOs => "macos",
        OsKind::Linux => "linux",
        OsKind::Other => "other",
    }
}

fn arch_name(a: orkestr_domain::Arch) -> &'static str {
    match a {
        orkestr_domain::Arch::X64 => "x64",
        orkestr_domain::Arch::Arm64 => "arm64",
        orkestr_domain::Arch::Other => "other",
    }
}

fn trust_name(t: TrustLevel) -> &'static str {
    match t {
        TrustLevel::Restricted => "restricted",
        TrustLevel::Standard => "standard",
        TrustLevel::Trusted => "trusted",
    }
}

fn status_name(s: NodeStatus) -> &'static str {
    match s {
        NodeStatus::Online => "online",
        NodeStatus::Busy => "busy",
        NodeStatus::Degraded => "degraded",
        NodeStatus::Offline => "offline",
        NodeStatus::Suspect => "suspect",
        NodeStatus::Draining => "draining",
        NodeStatus::Revoked => "revoked",
    }
}

fn kind_name(k: &JobKind) -> &'static str {
    match k {
        JobKind::Command { .. } => "command",
        JobKind::AgentTask { .. } => "agentTask",
        JobKind::Service => "service",
    }
}

/// Vendor of a GPU inferred from its marketing name (`nvidia`, `amd`, `apple`, `intel`).
#[must_use]
pub fn gpu_vendor(gpu: &GpuInfo) -> Option<&'static str> {
    let n = gpu.name.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["nvidia", "geforce", "rtx", "gtx", "quadro", "tesla"]) {
        Some("nvidia")
    } else if has(&["amd", "radeon"]) {
        Some("amd")
    } else if has(&["apple"]) || ["m1", "m2", "m3", "m4"].iter().any(|p| n.starts_with(p)) {
        Some("apple")
    } else if has(&["intel", "arc "]) {
        Some("intel")
    } else {
        None
    }
}

fn mib(bytes: u64) -> u64 {
    bytes / MIB
}

fn project_allowed(allowed: &[String], project: &str) -> bool {
    allowed.iter().any(|p| p == "*" || p == project)
}

fn has_model(models: &[String], want: &str) -> bool {
    models
        .iter()
        .any(|m| m == want || (!want.contains(':') && m.strip_prefix(want).is_some_and(|rest| rest == ":latest")))
}

/// Node label used in params: the id (stable) — names go into the text only.
fn base(n: &NodeSnapshot) -> Vec<(&'static str, String)> {
    vec![("node", n.node.id.to_string())]
}

fn with(n: &NodeSnapshot, extra: &[(&'static str, String)]) -> Vec<(&'static str, String)> {
    let mut p = base(n);
    p.extend(extra.iter().cloned());
    p
}

#[allow(clippy::too_many_lines)]
fn filter(job: &JobSpec, n: &NodeSnapshot) -> Verdict {
    let name = &n.node.name;
    let req = &job.requirements;
    let perm = |code, extra: &[(&'static str, String)], text: String| {
        Verdict::Permanent(Reason::new(code, with(n, extra), text))
    };

    // --- permanent ----------------------------------------------------------
    if n.status == NodeStatus::Revoked {
        return perm("reject.revoked", &[], format!("{name} is revoked"));
    }
    if job.affinity.forbidden.contains(&n.node.id) {
        return perm("reject.forbidden", &[], format!("{name} is forbidden for this job"));
    }
    if matches!(job.kind, JobKind::Service) {
        return perm("reject.kind", &[("kind", "service".into())], "service jobs are not supported yet".into());
    }
    let project = job.project_id.to_string();
    if !project_allowed(&n.allowed_projects, &project) {
        return perm("reject.project", &[("project", project)], format!("{name} is not enrolled for this project"));
    }
    let need = required_trust(job);
    if n.trust_level < need {
        return perm(
            "reject.trust",
            &[
                ("required", trust_name(need).into()),
                ("actual", trust_name(n.trust_level).into()),
                ("kind", kind_name(&job.kind).into()),
            ],
            format!(
                "{name} has trust level {}; {} jobs need {}",
                trust_name(n.trust_level),
                kind_name(&job.kind),
                trust_name(need)
            ),
        );
    }
    if !job.needs.secrets.is_empty() && !n.is_coordinator {
        return perm(
            "reject.secrets",
            &[("count", job.needs.secrets.len().to_string())],
            format!("{name} is remote; secrets cannot be delivered to remote nodes yet"),
        );
    }
    if let Some(os) = req.os {
        if n.node.os != os {
            return perm(
                "reject.os",
                &[("required", os_name(os).into()), ("actual", os_name(n.node.os).into())],
                format!("{name} runs {}, job requires {}", os_name(n.node.os), os_name(os)),
            );
        }
    }
    if let Some(arch) = req.arch {
        if n.node.arch != arch {
            return perm(
                "reject.arch",
                &[("required", arch_name(arch).into()), ("actual", arch_name(n.node.arch).into())],
                format!("{name} is {}, job requires {}", arch_name(n.node.arch), arch_name(arch)),
            );
        }
    }
    let res = &n.node.resources;
    if job.estimate.cpu_cores > 0.0 && f64::from(job.estimate.cpu_cores) > f64::from(res.cpu_cores) {
        return perm(
            "reject.cpu",
            &[("required", format!("{}", job.estimate.cpu_cores)), ("actual", res.cpu_cores.to_string())],
            format!("{name} has {} cores, job needs {}", res.cpu_cores, job.estimate.cpu_cores),
        );
    }
    let mem_need = req.min_mem_mb.unwrap_or(0).max(job.estimate.mem_mb);
    if mem_need > 0 && mib(res.memory_total_bytes) < mem_need {
        return perm(
            "reject.memory",
            &[("requiredMb", mem_need.to_string()), ("actualMb", mib(res.memory_total_bytes).to_string())],
            format!("{name} has {} MiB RAM, job needs {mem_need} MiB", mib(res.memory_total_bytes)),
        );
    }
    let mut tools: Vec<&String> = req.tools.iter().collect();
    tools.sort();
    for t in tools {
        if !n.node.tools.iter().any(|have| have.eq_ignore_ascii_case(t)) {
            return perm("reject.tool", &[("tool", t.clone())], format!("{name} has no {t}"));
        }
    }
    for (k, v) in &req.labels {
        if n.labels.get(k) != Some(v) {
            return perm(
                "reject.label",
                &[("label", k.clone()), ("required", v.clone())],
                format!("{name} lacks label {k}={v}"),
            );
        }
    }
    if req.gpu.is_some() || job.estimate.vram_mb > 0 {
        let gpu = req.gpu.clone().unwrap_or_default();
        let vram_need = gpu.min_vram_mb.unwrap_or(0).max(job.estimate.vram_mb);
        let gpus = &res.gpus;
        if gpus.is_empty() {
            return perm("reject.gpu", &[], format!("{name} has no GPU"));
        }
        let vendor_ok: Vec<&GpuInfo> = match &gpu.vendor {
            Some(v) => gpus.iter().filter(|g| gpu_vendor(g).is_some_and(|gv| gv.eq_ignore_ascii_case(v))).collect(),
            None => gpus.iter().collect(),
        };
        if vendor_ok.is_empty() {
            let v = gpu.vendor.clone().unwrap_or_default();
            return perm("reject.gpuVendor", &[("required", v.clone())], format!("{name} has no {v} GPU"));
        }
        let best = vendor_ok.iter().filter_map(|g| g.vram_bytes).map(mib).max().unwrap_or(0);
        if vram_need > 0 && best < vram_need {
            return perm(
                "reject.gpuVram",
                &[("requiredMb", vram_need.to_string()), ("actualMb", best.to_string())],
                format!("{name} has at most {best} MiB VRAM, job needs {vram_need} MiB"),
            );
        }
    }
    let mut models: Vec<&String> = req.local_models.iter().collect();
    models.sort();
    for m in models {
        if !has_model(&n.node.local_models, m) {
            return perm("reject.model", &[("model", m.clone())], format!("{name} does not have local model {m}"));
        }
    }

    // --- transient ----------------------------------------------------------
    if !matches!(n.status, NodeStatus::Online | NodeStatus::Busy) {
        let s = status_name(n.status);
        return Verdict::Transient {
            reason: Reason::new("unavailable.status", with(n, &[("status", s.into())]), format!("{name} is {s}")),
            busy: false,
        };
    }
    if n.slots.used >= n.slots.max || n.status == NodeStatus::Busy {
        return Verdict::Transient {
            reason: Reason::new(
                "busy.slots",
                with(n, &[("used", n.slots.used.to_string()), ("max", n.slots.max.to_string())]),
                format!("{name} has no free job slot ({}/{})", n.slots.used, n.slots.max),
            ),
            busy: true,
        };
    }
    let mem_free = mib(res.memory_total_bytes.saturating_sub(res.memory_used_bytes));
    if job.estimate.mem_mb > 0 && mem_free < job.estimate.mem_mb {
        return Verdict::Transient {
            reason: Reason::new(
                "busy.memory",
                with(n, &[("requiredMb", job.estimate.mem_mb.to_string()), ("freeMb", mem_free.to_string())]),
                format!("{name} has {mem_free} MiB RAM free, job needs {} MiB", job.estimate.mem_mb),
            ),
            busy: true,
        };
    }
    let disk_free = mib(res.disk_free_bytes);
    if job.estimate.disk_mb > 0 && disk_free < job.estimate.disk_mb {
        return Verdict::Transient {
            reason: Reason::new(
                "busy.disk",
                with(n, &[("requiredMb", job.estimate.disk_mb.to_string()), ("freeMb", disk_free.to_string())]),
                format!("{name} has {disk_free} MiB disk free, job needs {} MiB", job.estimate.disk_mb),
            ),
            busy: true,
        };
    }
    Verdict::Eligible
}

/// Normalized score terms of one eligible node (each in `[0, 1]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreTerms {
    /// Free resources (mean of free CPU, free RAM, free slots).
    pub headroom: f64,
    /// Project cache warmth (the coordinator holds the workspace: 1; others 0).
    pub cache_warmth: f64,
    /// `1 − load` (load = max(cpu, slot usage)).
    pub load: f64,
    /// Network (coordinator 1, remote 0.5 until RTT is measured).
    pub network: f64,
    /// Preference (preferred 1, neutral 0.5).
    pub preference: f64,
    /// Energy (label `power=battery` → 0, else 1).
    pub energy: f64,
}

fn clamp01(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(0.0, 1.0)
    }
}

#[allow(clippy::cast_precision_loss)]
fn fraction(num: u64, den: u64) -> f64 {
    if den == 0 {
        0.0
    } else {
        clamp01(num as f64 / den as f64)
    }
}

/// Score terms of `n` for `job`.
#[must_use]
pub fn score_terms(job: &JobSpec, n: &NodeSnapshot) -> ScoreTerms {
    let r = &n.node.resources;
    let cpu_free = clamp01(1.0 - f64::from(r.cpu_usage_percent) / 100.0);
    let mem_free = fraction(r.memory_total_bytes.saturating_sub(r.memory_used_bytes), r.memory_total_bytes);
    let slots_free = fraction(u64::from(n.slots.max.saturating_sub(n.slots.used)), u64::from(n.slots.max));
    let slot_load = fraction(u64::from(n.slots.used), u64::from(n.slots.max));
    ScoreTerms {
        headroom: (cpu_free + mem_free + slots_free) / 3.0,
        cache_warmth: if n.is_coordinator { 1.0 } else { 0.0 },
        load: 1.0 - (1.0 - cpu_free).max(slot_load),
        network: if n.is_coordinator { 1.0 } else { 0.5 },
        preference: if job.affinity.preferred.contains(&n.node.id) { 1.0 } else { 0.5 },
        energy: if n.labels.get("power").is_some_and(|p| p == "battery") { 0.0 } else { 1.0 },
    }
}

/// Weighted, normalized score in `[0, 1]` (weights are divided by their sum).
#[must_use]
pub fn weighted(t: &ScoreTerms, w: &ScoreWeights) -> f64 {
    let ws = [w.headroom, w.cache_warmth, w.load, w.network, w.preference, w.energy].map(|x| x.max(0.0));
    let total: f64 = ws.iter().sum();
    if total <= 0.0 {
        return 0.0;
    }
    let ts = [t.headroom, t.cache_warmth, t.load, t.network, t.preference, t.energy];
    clamp01(ws.iter().zip(ts).map(|(w, t)| w * t).sum::<f64>() / total)
}

fn f3(x: f64) -> String {
    format!("{x:.3}")
}

/// Ranking order: higher score first; ties → coordinator, then lowest id.
fn rank(a: &(NodeSnapshot, f64), b: &(NodeSnapshot, f64)) -> Ordering {
    if (a.1 - b.1).abs() >= TIE_EPSILON {
        return b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal);
    }
    b.0.is_coordinator.cmp(&a.0.is_coordinator).then_with(|| a.0.node.id.cmp(&b.0.node.id))
}

impl DefaultNodeScheduler {
    /// Places `job` and returns the decision with structured reasons (rendered
    /// into [`Placement`] explanation strings by [`NodeScheduler::place`]).
    #[must_use]
    pub fn decide(&self, job: &JobSpec, nodes: &[NodeSnapshot], policy: &PlacementPolicy) -> (Placement, Vec<Reason>) {
        let mut sorted: Vec<&NodeSnapshot> = nodes.iter().collect();
        sorted.sort_by_key(|n| n.node.id);
        // The coordinator flag in the snapshot wins; the policy can also name it.
        let mark = |n: &NodeSnapshot| {
            let mut n = n.clone();
            n.is_coordinator = n.is_coordinator || policy.coordinator_node_id == Some(n.node.id);
            n
        };

        let mut eligible: Vec<(NodeSnapshot, f64)> = Vec::new();
        let mut rejected: Vec<Reason> = Vec::new();
        let (mut any_busy, mut any_transient) = (false, false);
        for n in sorted {
            let n = mark(n);
            match filter(job, &n) {
                Verdict::Eligible => {
                    let s = weighted(&score_terms(job, &n), &policy.weights);
                    eligible.push((n, s));
                }
                Verdict::Permanent(r) => rejected.push(r),
                Verdict::Transient { reason, busy } => {
                    any_transient = true;
                    any_busy |= busy;
                    rejected.push(reason);
                }
            }
        }

        if eligible.is_empty() {
            let mut lines = Vec::new();
            let placement_reason;
            let placement = if nodes.is_empty() {
                lines.push(Reason::new("infeasible.noNodes", vec![], "no nodes are enrolled"));
                placement_reason = "noNodes";
                None
            } else if any_transient {
                let (code, reason, text) = if any_busy {
                    (
                        "wait.noCapacity",
                        JobWaitReason::NoCapacity,
                        "suitable nodes exist but none has free capacity now",
                    )
                } else {
                    (
                        "wait.nodesUnavailable",
                        JobWaitReason::NodesUnavailable,
                        "suitable nodes exist but none is online now",
                    )
                };
                lines.push(Reason::new(code, vec![("rejected", rejected.len().to_string())], text));
                placement_reason = "";
                Some(reason)
            } else {
                lines.push(Reason::new(
                    "infeasible.noFeasibleNode",
                    vec![("rejected", rejected.len().to_string())],
                    "no enrolled node can ever satisfy this job",
                ));
                placement_reason = "noFeasibleNode";
                None
            };
            lines.extend(rejected);
            let explanation = lines.iter().map(Reason::render).collect();
            let p = match placement {
                Some(reason) => Placement::Wait { reason, explanation },
                None => Placement::Infeasible { reason: placement_reason.to_owned(), explanation },
            };
            return (p, lines);
        }

        eligible.sort_by(rank);
        let (win, score) = eligible[0].clone();
        let t = score_terms(job, &win);
        let mut lines = vec![
            Reason::new(
                "place",
                vec![("node", win.node.id.to_string()), ("score", f3(score))],
                format!("placed on {} (score {})", win.node.name, f3(score)),
            ),
            Reason::new(
                "score",
                vec![
                    ("node", win.node.id.to_string()),
                    ("headroom", f3(t.headroom)),
                    ("cacheWarmth", f3(t.cache_warmth)),
                    ("load", f3(t.load)),
                    ("network", f3(t.network)),
                    ("preference", f3(t.preference)),
                    ("energy", f3(t.energy)),
                ],
                format!(
                    "headroom {}, cache {}, 1-load {}, network {}, preference {}, energy {}",
                    f3(t.headroom),
                    f3(t.cache_warmth),
                    f3(t.load),
                    f3(t.network),
                    f3(t.preference),
                    f3(t.energy)
                ),
            ),
        ];
        if let Some((second, s2)) = eligible.get(1) {
            if (score - s2).abs() < TIE_EPSILON {
                let rule = if win.is_coordinator && !second.is_coordinator { "coordinator" } else { "lowestId" };
                let text = if rule == "coordinator" {
                    format!("tie with {}; the coordinator is preferred", second.node.name)
                } else {
                    format!("tie with {}; the lowest node id is preferred", second.node.name)
                };
                lines.push(Reason::new(
                    "tieBreak",
                    vec![("rule", rule.into()), ("over", second.node.id.to_string())],
                    text,
                ));
            }
        }
        let mut others: Vec<&(NodeSnapshot, f64)> = eligible.iter().skip(1).collect();
        others.sort_by_key(|(n, _)| n.node.id);
        for (n, s) in others {
            lines.push(Reason::new(
                "candidate",
                vec![("node", n.node.id.to_string()), ("score", f3(*s))],
                format!("{} was also suitable (score {})", n.node.name, f3(*s)),
            ));
        }
        lines.extend(rejected);
        let explanation = lines.iter().map(Reason::render).collect();
        (Placement::Place { node_id: win.node.id, score, explanation }, lines)
    }
}

impl NodeScheduler for DefaultNodeScheduler {
    fn place(&self, job: &JobSpec, nodes: &[NodeSnapshot], policy: &PlacementPolicy) -> Placement {
        self.decide(job, nodes, policy).0
    }
}

/// Explanation of a placement (empty for none).
#[must_use]
pub fn explanation_of(p: &Placement) -> &[String] {
    match p {
        Placement::Place { explanation, .. }
        | Placement::Wait { explanation, .. }
        | Placement::Infeasible { explanation, .. } => explanation,
    }
}

/// The chosen node, when placed.
#[must_use]
pub fn placed_node(p: &Placement) -> Option<NodeId> {
    match p {
        Placement::Place { node_id, .. } => Some(*node_id),
        _ => None,
    }
}
