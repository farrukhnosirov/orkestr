#!/usr/bin/env node
// Phase 1 end-to-end smoke test (wave A).
//
// Launches the debug desktop app (or `orkestrd`) with an isolated
// ORKESTR_HOME, discovers the embedded core through `core.json` +
// `core.token`, and drives every Phase 1 area over the real WebSocket RPC:
// projects, files, vcs/worktrees, snapshots + task rollback, tasks DAG,
// terminal (PTY + gated `run` with an approval round-trip), providers,
// memory, cost, settings, nodes and the emergency stop.
//
// Usage (repo root, after `cargo build -p orkestr-desktop`):
//   node scripts/smoke/phase1-smoke.mjs                 # launches target/debug/orkestr-desktop(.exe)
//   node scripts/smoke/phase1-smoke.mjs --headless      # launches target/debug/orkestrd(.exe) instead
//   ORKESTR_SMOKE_EXE=path/to/exe node scripts/smoke/phase1-smoke.mjs
//
// Requires Node >= 22 (global WebSocket) and `git` on PATH. Exits non-zero on
// the first failed check. Nothing touches the real data directory or the OS
// keyring (secret backend forced to `vault-file`).

import { spawn, execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const isWin = process.platform === "win32";
const headless = process.argv.includes("--headless");
const exe =
  process.env.ORKESTR_SMOKE_EXE ??
  join(repo, "target", "debug", `${headless ? "orkestrd" : "orkestr-desktop"}${isWin ? ".exe" : ""}`);

const results = [];
let failures = 0;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function check(name, ok, detail = "") {
  results.push({ name, ok, detail });
  if (!ok) failures++;
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  — ${detail}` : ""}`);
  if (!ok) throw new Error(`check failed: ${name} ${detail}`);
}

// ── temp dirs ─────────────────────────────────────────────────────────────
const base = mkdtempSync(join(tmpdir(), "orkestr-smoke-"));
const home = join(base, "home");
const repoDir = join(base, "repo");
mkdirSync(home, { recursive: true });
mkdirSync(join(repoDir, "src"), { recursive: true });
const git = (...args) => execFileSync("git", args, { cwd: repoDir, encoding: "utf8" }).trim();
git("init", "-q", "-b", "main");
git("config", "user.email", "smoke@orkestr.local");
git("config", "user.name", "Orkestr Smoke");
writeFileSync(join(repoDir, "README.md"), "# Smoke repo\n");
writeFileSync(join(repoDir, "src", "main.ts"), "export const answer = 42;\n");
git("add", "-A");
git("commit", "-q", "-m", "initial");

// ── launch ────────────────────────────────────────────────────────────────
if (!existsSync(exe)) {
  console.error(`executable not found: ${exe} (run cargo build -p orkestr-desktop first)`);
  process.exit(2);
}
console.log(`launching ${exe}\nORKESTR_HOME=${home}`);
const child = spawn(exe, [], {
  env: { ...process.env, ORKESTR_HOME: home, ORKESTR_SECRET_BACKEND: "vault-file", RUST_LOG: process.env.RUST_LOG ?? "info" },
  stdio: ["ignore", "pipe", "pipe"],
  windowsHide: false,
});
let childLog = "";
child.stdout.on("data", (d) => (childLog += d));
child.stderr.on("data", (d) => (childLog += d));
let childExited = false;
child.on("exit", () => (childExited = true));

async function waitFor(fn, timeoutMs, label) {
  const end = Date.now() + timeoutMs;
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${label}`);
    if (childExited) throw new Error(`process exited while waiting for ${label}\n${childLog.slice(-4000)}`);
    await sleep(200);
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
          f.ok ? p.resolve(f.result) : p.reject(Object.assign(new Error(`${f.error.code}: ${f.error.message}`), { rpc: f.error }));
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
  waitEvent(pred, timeoutMs = 10_000) {
    const hit = this.events.find(pred);
    if (hit) return Promise.resolve(hit);
    return new Promise((resolve, reject) => {
      const l = (ev) => {
        if (pred(ev)) {
          this.listeners.delete(l);
          resolve(ev);
        }
      };
      this.listeners.add(l);
      setTimeout(() => {
        this.listeners.delete(l);
        reject(new Error("timed out waiting for event"));
      }, timeoutMs);
    });
  }
  close() {
    this.ws?.close();
  }
}

let rpc;
try {
  const discovery = join(home, "core.json");
  const info = await waitFor(
    () => {
      try {
        return JSON.parse(readFileSync(discovery, "utf8"));
      } catch {
        return null;
      }
    },
    60_000,
    "core.json",
  );
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  check("discovery: core.json + core.token", !!info.url && token.length > 20, info.url);

  rpc = new Rpc(info.url);
  const auth = await rpc.open(token);
  check("auth", !!auth.sessionId, `protocol v${auth.protocolVersion}`);
  await rpc.sub(["*"]);

  // ── settings / nodes / system ──
  const settings = await rpc.call("settings.get");
  check("settings.get", !!settings.settings, `approval default ${settings.settings.defaultApprovalMode}`);
  const node = await rpc.call("nodes.info", {});
  check("nodes.info", !!node.cpu || !!node.id || !!node.node, JSON.stringify(Object.keys(node)).slice(0, 120));

  // ── projects ──
  const { project } = await rpc.call("projects.create", { name: "Smoke Project", rootPath: repoDir });
  check("projects.create", !!project.id, project.id);
  const projectId = project.id;
  await rpc.sub([`project:${projectId}`]);

  // ── files ──
  const listing = await rpc.call("files.list", { projectId });
  const names = listing.entries.map((e) => e.name);
  check("files.list", names.includes("README.md") && names.includes("src"), names.join(","));
  const readme = await rpc.call("files.read", { projectId, path: "README.md" });
  check("files.read", readme.content.includes("Smoke repo"));
  const written = await rpc.call("files.write", { projectId, path: "notes.md", content: "hello from smoke\n", create: true });
  check("files.write", !!written, JSON.stringify(written).slice(0, 100));
  await rpc.waitEvent((e) => e.kind === "FileChanged" && e.payload.path === "notes.md");
  check("FileChanged event", true);
  await rpc.expectError("files.read", { projectId, path: "../outside.txt" }, "bad_request").catch(async (e) => {
    // Some builds map traversal to `forbidden`.
    if (!/forbidden/.test(String(e.message))) throw e;
  });
  check("files confinement rejects ..", true);

  // ── vcs ──
  const status = await rpc.call("vcs.status", { projectId });
  check("vcs.status", Array.isArray(status.entries ?? status.files ?? status.changes ?? []), `branch ${status.branch ?? status.head?.branch ?? "?"}`);
  const agentId = randomUUID();
  const wt = await rpc.call("vcs.addWorktree", { projectId, agentId, role: "smoke" });
  check("vcs.addWorktree", !!wt.worktree?.path && existsSync(wt.worktree.path), wt.worktree?.path);
  const wts = await rpc.call("vcs.worktrees", { projectId });
  check("vcs.worktrees lists the agent worktree", wts.worktrees.some((w) => w.agentId === agentId));

  // ── tasks ──
  const t1 = (await rpc.call("tasks.create", { projectId, title: "Design API" })).task;
  const t2 = (await rpc.call("tasks.create", { projectId, title: "Implement API", dependsOn: [t1.id] })).task;
  const t3 = (await rpc.call("tasks.create", { projectId, title: "Write tests" })).task;
  await rpc.call("tasks.addDependency", { taskId: t3.id, dependsOn: t2.id });
  const graph = await rpc.call("tasks.graph", { projectId });
  check("tasks.create x3 + graph", graph.nodes.length === 3 && graph.edges.length === 2, `topo ${graph.topoOrder.length}, critical ${graph.criticalPath.length}`);
  await rpc.expectError("tasks.addDependency", { taskId: t1.id, dependsOn: t3.id }, "conflict");
  check("tasks cycle rejected", true);
  const s1 = (await rpc.call("tasks.get", { taskId: t1.id })).task.status;
  if (s1 === "Planned") await rpc.call("tasks.transition", { taskId: t1.id, to: "Ready" });
  await rpc.call("tasks.transition", { taskId: t1.id, to: "Running" });
  await rpc.call("tasks.transition", { taskId: t1.id, to: "Completed" });
  const t2After = (await rpc.call("tasks.get", { taskId: t2.id })).task;
  check("tasks transitions + dependency promotion", t2After.status === "Ready", `t2=${t2After.status}`);
  await rpc.expectError("tasks.transition", { taskId: t1.id, to: "Running" }, "conflict");
  check("tasks invalid transition rejected", true);

  // ── snapshots + task rollback ──
  const taskId = t2.id;
  const before = await rpc.call("snapshots.create", { projectId, reason: "TaskCheckpoint", taskId, taskPhase: "Before" });
  await rpc.call("files.write", { projectId, path: "src/main.ts", content: "export const answer = 0; // broken by agent\n" });
  await rpc.call("files.write", { projectId, path: "src/extra.ts", content: "export {};\n", create: true });
  const after = await rpc.call("snapshots.create", { projectId, reason: "TaskCheckpoint", taskId, taskPhase: "After" });
  check("snapshots.create (before/after)", !!before.snapshot.id && !!after.snapshot.id);
  const gated = await rpc.expectError("snapshots.rollbackTask", { projectId, taskId, confirm: false }, "approval_required");
  check("rollbackTask requires confirm", !!gated);
  const rb = await rpc.call("snapshots.rollbackTask", { projectId, taskId, confirm: true });
  const mainNow = readFileSync(join(repoDir, "src", "main.ts"), "utf8");
  check(
    "snapshots.rollbackTask restores files",
    mainNow.includes("answer = 42") && !existsSync(join(repoDir, "src", "extra.ts")),
    `restored ${rb.restored.length}, deleted ${rb.deleted.length}, conflicts ${rb.conflicts.length}`,
  );
  await rpc.waitEvent((e) => e.kind === "RollbackPerformed");
  check("RollbackPerformed event", true);

  // ── terminal: PTY ──
  const term = (await rpc.call("terminal.open", { projectId, cols: 100, rows: 30, title: "smoke" })).terminal;
  check("terminal.open", !!term.terminalId, `${term.shellId} pid ${term.pid}`);
  await rpc.sub([`terminal:${term.terminalId}`]);
  await sleep(1500);
  await rpc.call("terminal.write", { terminalId: term.terminalId, data: "echo orkestr-smoke-ok\r" });
  const hist = await waitFor(
    async () => {
      const h = await rpc.call("terminal.history", { terminalId: term.terminalId });
      return h.data.split("orkestr-smoke-ok").length > 2 ? h : null; // the echoed command + its output
    },
    20_000,
    "terminal output",
  );
  check("terminal.write + history", true, `${hist.endOffset} bytes`);
  await rpc.call("terminal.close", { terminalId: term.terminalId });
  check("terminal.close", true);

  // ── terminal.run: safe + dangerous with approval round-trip ──
  const safe = await rpc.call("terminal.run", { projectId, command: "git status --short", cwd: repoDir, timeoutMs: 30_000 });
  check("terminal.run (safe)", safe.exitCode === 0, `${safe.durationMs} ms`);
  const dangerous = "git reset --hard HEAD";
  const ask = await rpc.expectError("terminal.run", { projectId, command: dangerous, cwd: repoDir, timeoutMs: 30_000 }, "approval_required");
  const approvalId = ask.data?.approvalId;
  check("terminal.run (dangerous) → approval_required", !!approvalId, ask.message);
  await rpc.waitEvent((e) => e.kind === "ApprovalRequested" && e.payload.approval.id === approvalId);
  check("ApprovalRequested event", true);
  const pending = await rpc.call("approvals.list", {});
  check("approvals.list shows it", pending.approvals.some((a) => a.id === approvalId));
  await rpc.call("approvals.resolve", { approvalId, decision: "Approve", note: "smoke" });
  await rpc.waitEvent((e) => e.kind === "ApprovalResolved");
  const approvedRun = await rpc.call("terminal.run", { projectId, command: dangerous, cwd: repoDir, timeoutMs: 30_000 });
  check("approved command runs once", approvedRun.exitCode === 0, approvedRun.stdout.trim().slice(0, 80));
  await rpc.expectError("terminal.run", { projectId, command: dangerous, cwd: repoDir }, "approval_required");
  check("approval is single-use", true);
  await rpc.expectError("terminal.run", { projectId, command: "format c: /q", cwd: repoDir }, "forbidden").catch(async (e) => {
    if (!/approval_required/.test(String(e.message))) throw e;
  });
  check("disk format refused or gated", true);

  // ── providers ──
  const providers = await rpc.call("providers.list");
  const cfg = (v) => v.provider ?? v; // ProviderView wraps the stored config
  const mock = providers.providers.map(cfg).find((p) => p.kind === "Mock");
  check("providers.list has the Mock provider", !!mock, providers.providers.map((p) => cfg(p).name).join(", "));
  const test = await rpc.call("providers.test", { providerId: mock.id });
  check("providers.test (mock)", test.ok, test.sample ?? "");

  // ── memory / cost ──
  await rpc.call("memory.create", {
    scope: { scope: "Project", projectId },
    kind: "Convention",
    title: "Use pnpm-free npm workspaces",
    content: "The repo uses npm workspaces; never run pnpm.",
    tags: ["tooling"],
  });
  const found = await rpc.call("memory.search", { query: "workspaces", projectId });
  check("memory.create + search", (found.results ?? found.hits ?? found.items ?? []).length > 0);
  const cost = await rpc.call("cost.summary", {});
  check("cost.summary (mock usage recorded)", (cost.month?.inputTokens ?? 0) > 0, JSON.stringify(cost).slice(0, 160));

  // ── emergency ──
  const stop = await rpc.call("emergency.stopAll");
  check("emergency.stopAll", stop.flags.allStopped === true);
  await rpc.expectError("terminal.run", { projectId, agentId, command: "git status", cwd: repoDir }, "forbidden");
  check("agent commands refused while stopped", true);
  const reset = await rpc.call("emergency.reset");
  check("emergency.reset", reset.flags.allStopped === false);
} catch (e) {
  failures++;
  console.error(`\nSMOKE FAILED: ${e.stack ?? e}`);
  console.error(`--- app log tail ---\n${childLog.slice(-3000)}`);
} finally {
  rpc?.close();
  if (!childExited) {
    if (isWin) {
      try {
        execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
      } catch {
        /* already gone */
      }
    } else child.kill("SIGTERM");
  }
  await sleep(1000);
  try {
    rmSync(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  } catch {
    console.warn(`could not remove ${base}`);
  }
  const passed = results.filter((r) => r.ok).length;
  console.log(`\n${passed}/${results.length} checks passed${failures ? `, ${failures} failure(s)` : ""}`);
  process.exit(failures ? 1 : 0);
}
