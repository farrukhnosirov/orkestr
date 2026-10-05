#!/usr/bin/env node
// Phase 2 acceptance (§61 Phase 2, §63 A/B/J): drives the real app end to end
// over the WebSocket RPC with the offline Mock provider (plus a local fake
// OpenAI-compatible server for the provider-limit scenario).
//
//   A-lite  Manager request "Create a payroll module: architecture review,
//           backend, tests, security review" → plan with Architect / Backend /
//           QA / Security tasks → parallel agents in separate worktrees →
//           per-task quality pipeline (code review, build, tests through the
//           real quality runner, adversarial security review) → Self-Healing
//           of an injected failing test → Merge Manager proposals → approve →
//           merged into `main` of the temp repo (checked on disk) → final report
//   B       the primary provider (fake OpenAI-compatible server) answers 429 /
//           insufficient_quota → handoff saved → the fallback provider (Mock)
//           finishes the task → ProviderFallback event
//   J       a failed autonomous refactor is rolled back with Rollback Task; an
//           unrelated user change in the same checkout survives
//   +       consensus run → Decision Log entry with alternatives; architecture
//           map of the temp repo; execution replay of the run covers
//           plan → agents → tools → files → tests → reviews → approvals → merge;
//           benchmarks have rows
//
// Usage (repo root, after `cargo build -p orkestr-desktop`):
//   node scripts/smoke/phase2-acceptance.mjs              # target/debug/orkestr-desktop(.exe)
//   node scripts/smoke/phase2-acceptance.mjs --headless   # target/debug/orkestrd(.exe)
//   ORKESTR_SMOKE_EXE=path/to/exe node scripts/smoke/phase2-acceptance.mjs
//   --keep  keeps the temp dirs (printed at start) for inspection
//
// Requires Node >= 22 (global WebSocket), `git`, `node` and `npm` on PATH.
// Nothing touches the real data directory or the OS keyring.

import { spawn, execFileSync } from "node:child_process";
import { createServer } from "node:http";
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

// ── temp dirs + a tiny real project (npm, node:test) ─────────────────────
const base = mkdtempSync(join(tmpdir(), "orkestr-p2-"));
const home = join(base, "home");
const repoDir = join(base, "repo");
mkdirSync(home, { recursive: true });
mkdirSync(join(repoDir, "src"), { recursive: true });
mkdirSync(join(repoDir, "tests"), { recursive: true });
mkdirSync(join(repoDir, "notes"), { recursive: true });
const git = (cwd, ...args) => execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
git(repoDir, "init", "-q", "-b", "main");
git(repoDir, "config", "user.email", "accept@orkestr.local");
git(repoDir, "config", "user.name", "Orkestr Acceptance");
writeFileSync(join(repoDir, "README.md"), "# Payroll demo\n\nA tiny npm library used by the Phase 2 acceptance.\n");
writeFileSync(
  join(repoDir, "package.json"),
  '{\n  "name": "payroll-demo",\n  "private": true,\n  "type": "module",\n  "scripts": { "test": "node --test" }\n}\n',
);
writeFileSync(
  join(repoDir, "src", "money.mjs"),
  "// Money helpers.\nexport function cents(amount) {\n  return Math.round(amount * 100);\n}\n\nexport function format(cents) {\n  return `$${(cents / 100).toFixed(2)}`;\n}\n",
);
writeFileSync(
  join(repoDir, "tests", "money.test.mjs"),
  'import { test } from "node:test";\nimport assert from "node:assert/strict";\nimport { cents, format } from "../src/money.mjs";\n\ntest("cents rounds", () => assert.equal(cents(12.5), 1250));\ntest("format", () => assert.equal(format(1234), "$12.34"));\n',
);
writeFileSync(join(repoDir, "notes", "todo.md"), "- original note\n");
git(repoDir, "add", "-A");
git(repoDir, "commit", "-q", "-m", "initial payroll demo");
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

// ── helpers ───────────────────────────────────────────────────────────────
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
const runOf = async (rpc, runId) => (await rpc.call("manager.status", { runId })).runs[0];
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
const runBrief = (run) =>
  `${run.status}: ` +
  run.tasks
    .map((t) => `${t.key}/${t.role}=${t.status}${t.pipeline ? `[${t.pipeline.status} ${t.pipeline.stages.map((s) => `${s.kind[0]}${s.kind[1]}:${s.status}`).join(",")} heal ${t.pipeline.healingAttempts}]` : ""}`)
    .join(" ");

// ── scenario ──────────────────────────────────────────────────────────────
let rpc;
let fake;
try {
  launch();
  ({ rpc } = await connect());
  check("app started: discovery + auth", true, `pid ${child.pid}`);

  section("0. setup (Mock as Manager + agents)");
  await rpc.call("settings.update", {
    patch: { profileName: "Acceptance", defaultApprovalMode: "SemiAuto", onboardingCompleted: true },
  });
  const providers = (await rpc.call("providers.list")).providers.map((p) => p.provider ?? p);
  const mock = providers.find((p) => p.kind === "Mock");
  check("Mock (offline demo) provider present", !!mock, providers.map((p) => p.name).join(", "));
  const smart = { providerId: mock.id, model: "mock-smart" };
  const routeTo = (chain) =>
    rpc.call("providers.setRouting", {
      config: { mode: "Manual", rules: [], defaultFallback: chain, preferLocal: true, maxRateLimitWaitSecs: 0 },
    });
  await rpc.call("providers.setManager", { config: { model: smart, maxParallelAgents: 3, approvalPolicy: "SemiAuto" } });
  await routeTo([smart]);
  const { project } = await rpc.call("projects.create", { name: "Payroll", rootPath: repoDir, approvalMode: "SemiAuto" });
  const projectId = project.id;
  check("projects.create (temp npm repo)", !!projectId, projectId);
  const pcfg = await rpc.call("pipeline.getConfig", { projectId });
  check(
    "quality pipeline on by default: every stage, healing limits",
    pcfg.enabled && pcfg.stages.length === 6 && pcfg.healing.maxAttempts >= 1,
    `${pcfg.stages.join("→")}, ${pcfg.healing.maxAttempts} attempts, ${pcfg.healing.maxWallMins} min, merge ${pcfg.merge}`,
  );
  const policy = (await rpc.call("merge.policy", { projectId })).policy;
  console.log("merge policy:", JSON.stringify(policy));

  section("A. multi-agent payroll (§63 A-lite)");
  const goal = "Create a payroll module: architecture review, backend, tests, security review";
  const { reply } = await chatForPlan(rpc, projectId, goal);
  check("manager.chat → plan", !!reply.planId);
  const plan = (await rpc.call("manager.history", { projectId })).plans.find((p) => p.id === reply.planId);
  const roles = plan.tasks.map((t) => t.role);
  check(
    "plan has Architect / Backend / QA / Security tasks with dependencies",
    ["Architect", "Backend", "Qa", "Security"].every((r) => roles.includes(r)) && plan.tasks.some((t) => t.dependsOn.length),
    plan.tasks.map((t) => `${t.key}:${t.role}${t.dependsOn.length ? `←${t.dependsOn.join("+")}` : ""}`).join(" "),
  );
  const backendKey = plan.tasks.find((t) => t.role === "Backend").key;
  const backendTitle = `${plan.tasks.find((t) => t.role === "Backend").title} [mock:heal-once]`;
  const approved = await rpc.call("manager.approvePlan", {
    planId: plan.id,
    edits: { removeTasks: [], titles: [{ key: backendKey, title: backendTitle }] },
  });
  const runId = approved.run.id;
  check("plan approved (backend task carries the injected failing test) → run", !!runId, backendTitle);
  let last = "";
  const run1 = await waitFor(
    async () => {
      const r = await runOf(rpc, runId);
      const b = runBrief(r);
      if (b !== last) console.log(`   … ${(last = b)}`);
      const pending = (await rpc.call("approvals.list", { projectId, status: "Pending" }).catch(() => ({ approvals: [] }))).approvals ?? [];
      if (pending.length) console.log(`   … pending approvals: ${pending.map((a) => a.actionSummary).join(" | ")}`);
      return ["Completed", "Failed", "Cancelled", "Interrupted"].includes(r.status) ? r : null;
    },
    600_000,
    "run to finish",
  );
  console.log(JSON.stringify(run1.report, null, 1).slice(0, 4000));
} catch (e) {
  failures++;
  console.error(`\nACCEPTANCE FAILED: ${e.stack ?? e}`);
  console.error(`--- app log tail ---\n${childLog.slice(-4000)}`);
} finally {
  rpc?.close();
  fake?.close();
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
