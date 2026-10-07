#!/usr/bin/env node
// Phase 1 acceptance (§61, §63): drives the real app end to end over the
// WebSocket RPC with the offline Mock provider.
//
//   1. onboarding-equivalent setup (profile, approval mode, Mock as Manager + agent routing)
//   2. manager.chat → plan card → manager.approvePlan → supervised run
//      (Ready → Running → Completed, two tasks in parallel, one worktree per agent,
//      files written + committed, `node --test` run by QA, final report)
//   3. a dangerous command (`git clean -fd`) pauses an agent on ApprovalRequested;
//      approvals.resolve lets it continue
//   4. snapshots exist; snapshots.rollbackTask restores the pre-task state and
//      leaves an unrelated file the user edited meanwhile alone (§63 J)
//   5. emergency.stopAll stops a live run
//   6. crash recovery: the process is killed mid-run; after a restart the
//      in-flight tasks are Interrupted and so is the run
//
// Usage (repo root, after `cargo build -p orkestr-desktop`):
//   node scripts/smoke/phase1-acceptance.mjs              # target/debug/orkestr-desktop(.exe)
//   node scripts/smoke/phase1-acceptance.mjs --headless   # target/debug/orkestrd(.exe)
//   ORKESTR_SMOKE_EXE=path/to/exe node scripts/smoke/phase1-acceptance.mjs
//   --keep  keeps the temp dirs (printed at start) for inspection
//
// Requires Node >= 22 (global WebSocket), `git` and `node` on PATH. Nothing
// touches the real data directory or the OS keyring.

import { spawn, execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const isWin = process.platform === "win32";
const headless = process.argv.includes("--headless");
const keep = process.argv.includes("--keep");
const exe =
  process.env.ORKESTR_SMOKE_EXE ??
  join(repo, "target", "debug", `${headless ? "orkestrd" : "orkestr-desktop"}${isWin ? ".exe" : ""}`);

const results = [];
let failures = 0;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const norm = (p) => resolve(p).replace(/\\/g, "/").toLowerCase();

function check(name, ok, detail = "") {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  — ${detail}` : ""}`);
  if (!ok) throw new Error(`check failed: ${name} ${detail}`);
}
function section(title) {
  console.log(`\n── ${title} ${"─".repeat(Math.max(0, 70 - title.length))}`);
}

// ── temp dirs + project repo ──────────────────────────────────────────────
const base = mkdtempSync(join(tmpdir(), "orkestr-accept-"));
const home = join(base, "home");
const repoDir = join(base, "repo");
mkdirSync(home, { recursive: true });
mkdirSync(join(repoDir, "notes"), { recursive: true });
const git = (cwd, ...args) => execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
git(repoDir, "init", "-q", "-b", "main");
git(repoDir, "config", "user.email", "accept@orkestr.local");
git(repoDir, "config", "user.name", "Orkestr Acceptance");
writeFileSync(join(repoDir, "README.md"), "# Acceptance repo\n");
writeFileSync(join(repoDir, "package.json"), '{ "name": "accept", "private": true, "type": "module" }\n');
writeFileSync(join(repoDir, "notes", "todo.md"), "- original note\n");
git(repoDir, "add", "-A");
git(repoDir, "commit", "-q", "-m", "initial");
console.log(`ORKESTR_HOME=${home}\nproject=${repoDir}`);

// ── process management ────────────────────────────────────────────────────
if (!existsSync(exe)) {
  console.error(`executable not found: ${exe} (run cargo build -p orkestr-desktop first)`);
  process.exit(2);
}
let child = null;
let childLog = "";
let childExited = false;

function launch() {
  childLog = "";
  childExited = false;
  try {
    rmSync(join(home, "core.json"), { force: true });
  } catch {
    /* ignore */
  }
  console.log(`launching ${exe}`);
  child = spawn(exe, [], {
    env: {
      ...process.env,
      ORKESTR_HOME: home,
      ORKESTR_SECRET_BACKEND: "vault-file",
      RUST_LOG: process.env.RUST_LOG ?? "info",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout.on("data", (d) => (childLog += d));
  child.stderr.on("data", (d) => (childLog += d));
  child.on("exit", () => (childExited = true));
}

function killHard() {
  if (!child || childExited) return;
  if (isWin) {
    try {
      execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
    } catch {
      /* already gone */
    }
  } else child.kill("SIGKILL");
}

async function waitFor(fn, timeoutMs, label) {
  const end = Date.now() + timeoutMs;
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${label}`);
    if (childExited) throw new Error(`process exited while waiting for ${label}\n${childLog.slice(-4000)}`);
    await sleep(150);
  }
}

// ── tiny RPC client ───────────────────────────────────────────────────────
class Rpc {
  constructor(url) {
    this.url = url;
    this.seq = 0;
    this.pending = new Map();
    this.events = [];
    this.listeners = new Set();
  }
  async open(token) {
    this.ws = new WebSocket(this.url);
    await new Promise((res, rej) => {
      this.ws.onopen = res;
      this.ws.onerror = (e) => rej(new Error(`websocket error: ${e.message ?? e.type}`));
    });
    this.ws.onmessage = (m) => {
      const f = JSON.parse(typeof m.data === "string" ? m.data : m.data.toString());
      if (f.type === "res") {
        const p = this.pending.get(f.id);
        if (p) {
          this.pending.delete(f.id);
          f.ok
            ? p.resolve(f.result)
            : p.reject(Object.assign(new Error(`${f.error.code}: ${f.error.message}`), { rpc: f.error }));
        }
      } else if (f.type === "event") {
        this.events.push(f.event);
        for (const l of this.listeners) l(f.event);
      }
    };
    return this.frame({ type: "auth", token, id: "auth" }, "auth");
  }
  frame(obj, id) {
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify(obj));
      setTimeout(() => {
        if (this.pending.delete(id)) reject(new Error(`timeout waiting for response ${id} (${obj.method ?? obj.type})`));
      }, 60_000);
    });
  }
  call(method, params = {}) {
    const id = `r${++this.seq}`;
    return this.frame({ type: "req", id, method, params }, id);
  }
  sub(topics) {
    const id = `s${++this.seq}`;
    return this.frame({ type: "sub", id, topics }, id);
  }
  async expectError(method, params, code) {
    try {
      await this.call(method, params);
    } catch (e) {
      if (e.rpc?.code === code) return e.rpc;
      throw e;
    }
    throw new Error(`${method} succeeded but ${code} was expected`);
  }
  waitEvent(pred, timeoutMs = 30_000, label = "event") {
    const hit = this.events.find(pred);
    if (hit) return Promise.resolve(hit);
    return new Promise((resolve, reject) => {
      const l = (ev) => {
        if (pred(ev)) {
          this.listeners.delete(l);
          clearTimeout(timer);
          resolve(ev);
        }
      };
      this.listeners.add(l);
      const timer = setTimeout(() => {
        this.listeners.delete(l);
        reject(new Error(`timed out waiting for ${label}`));
      }, timeoutMs);
    });
  }
  close() {
    this.ws?.close();
  }
}

async function connect() {
  const info = await waitFor(
    () => {
      try {
        const v = JSON.parse(readFileSync(join(home, "core.json"), "utf8"));
        return v.pid === child.pid || headless || v.url ? v : null;
      } catch {
        return null;
      }
    },
    90_000,
    "core.json",
  );
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = new Rpc(info.url);
  const auth = await waitFor(
    async () => {
      try {
        return await rpc.open(token);
      } catch {
        return null;
      }
    },
    30_000,
    "websocket auth",
  );
  await rpc.sub(["*"]);
  return { rpc, info, auth };
}

// ── helpers over the Manager API ──────────────────────────────────────────
async function chatForPlan(rpc, projectId, text) {
  const sent = await rpc.call("manager.chat", { projectId, text });
  const replyId = sent.replyMessageId;
  const added = await rpc.waitEvent(
    (e) => e.kind === "ManagerMessageAdded" && e.payload.message.id === replyId,
    60_000,
    "Manager reply",
  );
  return { sent, reply: added.payload.message };
}

async function runOf(rpc, runId) {
  const st = await rpc.call("manager.status", { runId });
  return st.runs[0];
}

async function waitRun(rpc, runId, pred, timeoutMs, label) {
  return waitFor(
    async () => {
      const run = await runOf(rpc, runId);
      return pred(run) ? run : null;
    },
    timeoutMs,
    label,
  );
}

const transitions = (rpc, taskId) =>
  rpc.events.filter((e) => e.kind === "TaskStatusChanged" && e.payload.taskId === taskId).map((e) => e.payload);

// ── scenario ──────────────────────────────────────────────────────────────
let rpc;
try {
  launch();
  ({ rpc } = await connect());
  check("app started: discovery + auth", true, `pid ${child.pid}`);

  section("1. onboarding-equivalent setup");
  const st = await rpc.call("settings.update", {
    patch: { profileName: "Acceptance", defaultApprovalMode: "SemiAuto", onboardingCompleted: true },
  });
  check("settings: profile + approval mode + onboarding completed", st.settings.onboardingCompleted === true);
  const providers = (await rpc.call("providers.list")).providers.map((p) => p.provider ?? p);
  const mock = providers.find((p) => p.kind === "Mock");
  check("Mock (offline demo) provider present", !!mock, providers.map((p) => p.name).join(", "));
  const test = await rpc.call("providers.test", { providerId: mock.id });
  check("providers.test (mock)", test.ok === true);
  const smart = { providerId: mock.id, model: "mock-smart" };
  const routeTo = (model) =>
    rpc.call("providers.setRouting", {
      config: { mode: "Manual", rules: [], defaultFallback: [model], preferLocal: true, maxRateLimitWaitSecs: 0 },
    });
  await rpc.call("providers.setManager", {
    config: { model: smart, maxParallelAgents: 2, approvalPolicy: "SemiAuto" },
  });
  await routeTo(smart);
  check("Manager model = Mock, agent routing = Mock", true);
  const { project } = await rpc.call("projects.create", {
    name: "Acceptance",
    rootPath: repoDir,
    approvalMode: "SemiAuto",
  });
  const projectId = project.id;
  check("projects.create (git repo)", !!projectId, projectId);

  section("2. Manager: chat → plan → approve → run");
  const { reply } = await chatForPlan(rpc, projectId, "Create a hello module with a unit test");
  check("manager.chat → streamed reply with a plan", !!reply.planId && reply.role === "Manager");
  check(
    "ManagerMessage deltas streamed",
    rpc.events.some((e) => e.kind === "ManagerMessage"),
  );
  const hist = await rpc.call("manager.history", { projectId });
  const plan = hist.plans.find((p) => p.id === reply.planId);
  check(
    "plan card: 4 tasks with dependencies",
    plan?.tasks.length === 4 && plan.tasks.some((t) => t.dependsOn.length > 0),
    plan?.tasks.map((t) => `${t.key}:${t.role}`).join(" "),
  );
  const approved = await rpc.call("manager.approvePlan", { planId: plan.id });
  const runId = approved.run.id;
  check("manager.approvePlan → run started", approved.plan.status === "Running" && !!runId);
  const run1 = await waitRun(
    rpc,
    runId,
    (r) => ["Completed", "Failed", "Cancelled"].includes(r.status) || r.tasks.some((t) => t.awaitingReview),
    180_000,
    "run 1 to finish",
  );
  check("run finished Completed", run1.status === "Completed", `${run1.status}: ${run1.report?.summary ?? ""}`);
  for (const t of run1.tasks) {
    const tr = transitions(rpc, t.taskId);
    const seen = (from, to) => tr.some((x) => x.from === from && x.to === to);
    check(
      `task "${t.title}" Ready→Running→Completed`,
      seen("Ready", "Running") && tr.some((x) => x.from === "Running" || x.from === "Testing" || x.from === "Review") && t.status === "Completed",
      tr.map((x) => `${x.from}→${x.to}`).join(", "),
    );
  }
  // Parallelism: the two tasks that depend only on the design overlap in time.
  const window = (taskId) => {
    const evs = rpc.events.filter((e) => e.kind === "TaskStatusChanged" && e.payload.taskId === taskId);
    const start = evs.find((e) => e.payload.to === "Running");
    const end = evs.find((e) => e.payload.to === "Completed");
    return [Date.parse(start?.ts ?? 0), Date.parse(end?.ts ?? 0)];
  };
  const keyed = Object.fromEntries(run1.tasks.map((t) => [t.key, t]));
  const [a0, a1] = window(keyed.t2.taskId);
  const [b0, b1] = window(keyed.t3.taskId);
  check(
    "t2 ∥ t3 ran in parallel",
    run1.peakParallel >= 2 && a0 < b1 && b0 < a1,
    `peakParallel ${run1.peakParallel}, t2 [${a0}..${a1}] t3 [${b0}..${b1}]`,
  );
  const spawned = rpc.events.filter((e) => e.kind === "AgentSpawned" && e.projectId === projectId);
  check("AgentSpawned per task", spawned.length >= 4, `${spawned.length} agents`);
  const wtPaths = run1.tasks.map((t) => t.worktreePath).filter(Boolean);
  const listed = git(repoDir, "worktree", "list", "--porcelain")
    .split(/\r?\n/)
    .filter((l) => l.startsWith("worktree "))
    .map((l) => norm(l.slice(9)));
  check(
    "each agent in its own git worktree (git worktree list)",
    wtPaths.length === 4 && new Set(wtPaths).size === 4 && wtPaths.every((p) => listed.includes(norm(p))),
    run1.tasks.map((t) => t.branch).join(", "),
  );
  check(
    "worktrees are not the project checkout",
    wtPaths.every((p) => norm(p) !== norm(repoDir)),
  );
  const backendWt = keyed.t2.worktreePath;
  const qaWt = keyed.t4.worktreePath;
  check(
    "files written: design note, src/hello.mjs, docs, tests",
    existsSync(join(keyed.t1.worktreePath, "docs", "hello-design.md")) &&
      existsSync(join(backendWt, "src", "hello.mjs")) &&
      existsSync(join(keyed.t3.worktreePath, "docs", "hello.md")) &&
      existsSync(join(qaWt, "tests", "hello.test.mjs")),
  );
  check(
    "agent commits on agent/* branches",
    git(backendWt, "log", "-1", "--format=%s").includes("Implement the hello module") &&
      keyed.t2.branch.startsWith("agent/"),
    git(backendWt, "log", "-1", "--format=%s"),
  );
  check(
    "main checkout untouched by agents",
    !existsSync(join(repoDir, "src", "hello.mjs")) && git(repoDir, "status", "--porcelain") === "",
  );
  const report = run1.report;
  check(
    "final report",
    !!report && report.completed === 4 && report.tests.length > 0 && report.mergeProposals.length > 0,
    `${report?.completed} completed, tests: ${report?.tests.join("; ")}, ${report?.mergeProposals.length} merge proposal(s)`,
  );
  check("ManagerRunUpdated events", rpc.events.some((e) => e.kind === "ManagerRunUpdated"));

  section("3. dangerous command → approval → continue");
  const templates = (await rpc.call("templates.list", {})).templates;
  const backendTpl = templates.find((t) => t.role === "Backend");
  const spawnedAgent = await rpc.call("agents.spawn", {
    projectId,
    templateId: backendTpl.id,
    newTask: {
      title: "Clean the hello build output",
      description: "Remove untracked build leftovers from the worktree.\nRun: `git clean -fd`",
    },
  });
  const dangerAgentId = spawnedAgent.agent.id;
  const dangerTaskId = spawnedAgent.task.id;
  const asked = await rpc.waitEvent(
    (e) => e.kind === "ApprovalRequested" && e.payload.approval.agentId === dangerAgentId,
    60_000,
    "ApprovalRequested for git clean",
  );
  const approval = asked.payload.approval;
  check(
    "ApprovalRequested for the dangerous command",
    /git clean -fd/.test(approval.commandPreview ?? approval.actionSummary) && approval.status === "Pending",
    `${approval.actionSummary} (${approval.danger ?? approval.risk})`,
  );
  // The runtime moves the task to Waiting first, then the agent (audit r1):
  // wait until both are observed.
  const { waitingTask, waitingAgent } = await waitFor(
    async () => {
      const t = (await rpc.call("tasks.get", { taskId: dangerTaskId })).task;
      const a = (await rpc.call("agents.get", { agentId: dangerAgentId })).agent;
      return t.status === "Waiting" && a.state === "Waiting" ? { waitingTask: t, waitingAgent: a } : null;
    },
    15_000,
    "task + agent Waiting",
  );
  check("agent + task wait for the user", waitingAgent.state === "Waiting" && waitingTask.status === "Waiting");
  await rpc.call("approvals.resolve", { approvalId: approval.id, decision: "Approve", note: "acceptance" });
  await rpc.waitEvent((e) => e.kind === "ApprovalResolved" && e.payload.approvalId === approval.id && e.payload.status === "Approved", 10_000, "ApprovalResolved");
  const finishedAgent = await waitFor(
    async () => {
      const a = (await rpc.call("agents.get", { agentId: dangerAgentId })).agent;
      return ["Completed", "Error", "Stopped"].includes(a.state) ? a : null;
    },
    60_000,
    "agent to finish after approval",
  );
  const transcript = await rpc.call("agents.transcript", { agentId: dangerAgentId });
  const cleanCall = transcript.toolCalls.find((c) => c.toolCall.tool === "run_command");
  check(
    "approvals.resolve → command ran, agent continued to completion",
    finishedAgent.state === "Completed" && cleanCall?.toolCall.status === "Succeeded",
    `agent ${finishedAgent.state}, run_command ${cleanCall?.toolCall.status}`,
  );

  section("4. snapshots + task rollback (§63 J)");
  const snaps = (await rpc.call("snapshots.list", { projectId, taskId: keyed.t2.taskId })).snapshots;
  check(
    "before/after checkpoints exist for the backend task",
    snaps.some((s) => s.taskPhase === "Before") && snaps.some((s) => s.taskPhase === "After"),
    `${snaps.length} snapshot(s)`,
  );
  const allSnaps = (await rpc.call("snapshots.list", { projectId })).snapshots;
  check("SnapshotCreated events", rpc.events.some((e) => e.kind === "SnapshotCreated"), `${allSnaps.length} total`);
  // The user edits an unrelated file in the same worktree after the task.
  const userFile = join(backendWt, "notes", "todo.md");
  writeFileSync(userFile, "- original note\n- user edit made after the task\n");
  const gated = await rpc.expectError(
    "snapshots.rollbackTask",
    { projectId, taskId: keyed.t2.taskId, confirm: false },
    "approval_required",
  );
  check("rollbackTask requires confirmation", !!gated);
  const rb = await rpc.call("snapshots.rollbackTask", { projectId, taskId: keyed.t2.taskId, confirm: true });
  check(
    "rollbackTask restores the pre-task state",
    !existsSync(join(backendWt, "src", "hello.mjs")) && rb.conflicts.length === 0,
    `restored ${rb.restored.length}, deleted ${rb.deleted.join(", ")}`,
  );
  check(
    "unrelated user edit untouched",
    readFileSync(userFile, "utf8").includes("user edit made after the task"),
  );
  await rpc.waitEvent((e) => e.kind === "RollbackPerformed", 10_000, "RollbackPerformed");
  check("RollbackPerformed event", true);

  section("5. emergency.stopAll stops a live run");
  await routeTo({ providerId: mock.id, model: "mock-slow" }); // slow agents → time to stop them
  const { reply: r2 } = await chatForPlan(rpc, projectId, "Create a greeter module with tests");
  const ap2 = await rpc.call("manager.approvePlan", { planId: r2.planId });
  const run2Id = ap2.run.id;
  await waitRun(rpc, run2Id, (r) => r.tasks.some((t) => t.status === "Running" && t.agentId), 60_000, "run 2 agent running");
  const liveAgents = (await rpc.call("agents.list", { projectId, activeOnly: true })).agents;
  check("run 2 has a live agent", liveAgents.length > 0, liveAgents.map((a) => `${a.name}:${a.state}`).join(", "));
  const stopped = await rpc.call("emergency.stopAll");
  check("emergency.stopAll", stopped.flags.allStopped === true, `${stopped.agentsStopped ?? "?"} agent(s) stopped`);
  const run2 = await waitRun(rpc, run2Id, (r) => r.status !== "Running" && r.status !== "Paused", 30_000, "run 2 stopped");
  const afterStop = (await rpc.call("agents.list", { projectId, activeOnly: true })).agents;
  check(
    "run stopped, no live agents",
    ["Cancelled", "Failed", "Interrupted"].includes(run2.status) && afterStop.length === 0,
    `run ${run2.status}; tasks ${run2.tasks.map((t) => t.status).join(",")}`,
  );
  const settled = await waitFor(
    async () => {
      const ts = await Promise.all(run2.tasks.map((t) => rpc.call("tasks.get", { taskId: t.taskId })));
      const statuses = ts.map((r) => r.task.status);
      return statuses.some((s) => ["Running", "Waiting", "Testing", "Review"].includes(s)) ? null : statuses;
    },
    15_000,
    "run 2 tasks to leave Running",
  ).catch(async () => (await Promise.all(run2.tasks.map((t) => rpc.call("tasks.get", { taskId: t.taskId })))).map((r) => `${r.task.status}!`));
  check(
    "stopped run's in-flight task is Cancelled (no task left Running)",
    settled.every((s) => !s.endsWith("!")) && settled.includes("Cancelled"),
    settled.join(","),
  );
  await rpc.waitEvent((e) => e.kind === "EmergencyStop", 5_000, "EmergencyStop event");
  check("EmergencyStop event", true);
  await rpc.expectError("manager.approvePlan", { planId: r2.planId }, "conflict");
  await rpc.call("emergency.reset");
  check("emergency.reset", (await rpc.call("emergency.status")).flags.allStopped === false);

  section("6. crash recovery");
  // Phase 2 resumes interrupted runs automatically after a restart; this
  // check is about the Phase 1 recovery state itself, so auto-resume is off.
  const sched = (await rpc.call("scheduler.status", {})).settings;
  await rpc.call("scheduler.setLimits", { settings: { ...sched, autoResume: "off" } });
  const { reply: r3 } = await chatForPlan(rpc, projectId, "Create a counter module with tests");
  const ap3 = await rpc.call("manager.approvePlan", { planId: r3.planId });
  const run3Id = ap3.run.id;
  const live3 = await waitRun(rpc, run3Id, (r) => r.tasks.some((t) => t.status === "Running" && t.agentId), 60_000, "run 3 agent running");
  const inflight = live3.tasks.filter((t) => t.status === "Running").map((t) => t.taskId);
  rpc.close();
  killHard();
  await waitFor(() => childExited || null, 15_000, "process exit").catch(() => {});
  check("process killed mid-run", true, `${inflight.length} task(s) Running`);
  launch();
  ({ rpc } = await connect());
  check("restarted on the same data dir", true, `pid ${child.pid}`);
  const after = await Promise.all(inflight.map((taskId) => rpc.call("tasks.get", { taskId })));
  check(
    "in-flight tasks are Interrupted",
    after.every((r) => r.task.status === "Interrupted"),
    after.map((r) => `${r.task.title}: ${r.task.status}`).join("; "),
  );
  const run3 = await runOf(rpc, run3Id);
  check("run is Interrupted (resumable)", run3.status === "Interrupted", run3.status);
  const agents3 = (await rpc.call("agents.list", { projectId, activeOnly: true })).agents;
  check("no agent left active after restart", agents3.length === 0, agents3.map((a) => a.state).join(","));
  const resume = await rpc.call("manager.resumeSummary", { projectId });
  check("manager.resumeSummary after crash", !resume.isEmpty, resume.headline);
} catch (e) {
  failures++;
  console.error(`\nACCEPTANCE FAILED: ${e.stack ?? e}`);
  console.error(`--- app log tail ---\n${childLog.slice(-4000)}`);
} finally {
  rpc?.close();
  killHard();
  await sleep(1000);
  if (!keep) {
    try {
      rmSync(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
    } catch {
      console.warn(`could not remove ${base}`);
    }
  }
  const passed = results.filter((r) => r.ok).length;
  console.log(`\n${passed}/${results.length} checks passed${failures ? `, ${failures} failure(s)` : ""}`);
  process.exit(failures ? 1 : 0);
}
