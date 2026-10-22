#!/usr/bin/env node
// Phase 6 (Multi-node) acceptance run — ADR-0033 / ADR-0039, node-protocol.md.
//
// Setup: one self-hosted `orkestr-gateway` (temp data dir) and three headless
// cores (`orkestrd`, separate ORKESTR_HOME each, all tunneled to the gateway):
//   win  — coordinator, real profile (real GPU detection is informational);
//          its gateway traffic goes through a local recording TCP proxy so the
//          core-to-core relay can be checked for plaintext;
//   mac  — worker, ORKESTR_NODE_PROFILE = fixtures/node-mac.json (macOS / arm64,
//          xcodebuild → fixtures/fake-xcodebuild.mjs shim), trust "standard";
//   gpu  — worker, ORKESTR_NODE_PROFILE = fixtures/node-gpu.json (Linux / x86_64,
//          RTX 4090 24 GB, cuda), trust "trusted".
// Fast timings: ORKESTR_NODE_HEARTBEAT_MS=500, ORKESTR_NODE_LEASE_TTL_MS=3000.
// Waits are condition waits (poll RPC / events), never fixed sleeps.
//
//   C1 setup              3 cores + gateway, account (passkey + TOTP), tunnels
//   C2 registry           enroll via kind:"node" pairing, SAS equal both sides,
//                         wrong code rejected, nodes.list = 3 with profile
//                         resources, NodeRegistered, relay carries no plaintext
//   C3 scenario F         "build macOS app" (requirements.os = MacOs) → mac,
//                         explanation stored, JobAssigned, shim runs on the
//                         worker, progress streamed, artifact blob hash-verified
//                         on the coordinator, task Completed with owner = mac
//   C4 routing            GPU ≥ 16 GB → gpu; impossible → infeasible + task
//                         Blocked; draining node excluded; restricted node
//                         refuses AgentTask
//   C5 cross-node agent   AgentTask (mock provider) on gpu edits its worktree;
//                         the change lands in the coordinator's repository
//   C6 migration          planned gpu → mac (JobCheckpointed + JobMigrated, no
//                         repeated steps); unplanned (kill gpu → lease expiry →
//                         Interrupted → Recovering → Running elsewhere);
//                         restarted gpu's stale epoch rejected; side-effect job
//                         → failed(needs_user_decision) + ApprovalRequested
//   C7 models             import mock on the coordinator, distribute to gpu,
//                         digest verified on disk, tampered blob rejected,
//                         listed on gpu, job requiring the model routed there
//   C8 security           emergency stop cancels on all nodes; revoked node
//                         refused at next heartbeat, its jobs cancelled; audit
//                         offer / accept / complete / migrate with nodeId
//   C9 deferred           secrets-to-node, preemption, Service jobs answer
//                         unimplementedFeature (RR-G14) / infeasible
//
// Usage (repo root, Windows):
//   cargo build -p orkestr-core -p orkestr-gateway --bins
//   node scripts/smoke/phase6-acceptance.mjs [--out <dir>] [--only C2,C3]
// Env: ORKESTR_SMOKE_BIN_DIR (default target/debug), ORKESTR_GW_PORT (18887),
//      ORKESTR_GW_PROXY_PORT (18888), ORKESTR_P6_TIMEOUT_SCALE (1).
// Writes <out>/acceptance.json (default: %TEMP%/orkestr-phase6-acceptance).

import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import net from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const exeDir = process.env.ORKESTR_SMOKE_BIN_DIR ?? join(repo, "target", "debug");
const coreExe = join(exeDir, process.platform === "win32" ? "orkestrd.exe" : "orkestrd");
const gwExe = join(exeDir, process.platform === "win32" ? "orkestr-gateway.exe" : "orkestr-gateway");
// Avoid the Windows excluded TCP range 1349–1548.
const gwPort = Number(process.env.ORKESTR_GW_PORT ?? 18887);
const proxyPort = Number(process.env.ORKESTR_GW_PROXY_PORT ?? 18888);
const base = `http://localhost:${gwPort}`;
const proxyBase = `http://localhost:${proxyPort}`;
const SCALE = Number(process.env.ORKESTR_P6_TIMEOUT_SCALE ?? 1);
const T = (ms) => Math.round(ms * SCALE);
const HEARTBEAT_MS = 500;
const LEASE_TTL_MS = 3000;
const MiB = 1024 * 1024;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const tmp = mkdtempSync(join(tmpdir(), "orkestr-p6acc-"));
const argOf = (name) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : undefined;
};
const outDir = argOf("--out") ? resolve(repo, argOf("--out")) : join(tmpdir(), "orkestr-phase6-acceptance");
mkdirSync(outDir, { recursive: true });
const only = argOf("--only")?.split(",").map((s) => s.trim().toUpperCase());

// ── results ───────────────────────────────────────────────────────────────
const checks = [];
function check(area, name, ok, detail = "") {
  checks.push({ area, name, ok: Boolean(ok), detail: String(detail ?? "") });
  console.log(`${ok ? "PASS" : "FAIL"}  [${area}] ${name}${detail ? ` — ${String(detail).slice(0, 240)}` : ""}`);
}
const info = (area, name, detail) => {
  checks.push({ area, name, ok: true, informational: true, detail: String(detail ?? "") });
  console.log(`INFO  [${area}] ${name} — ${String(detail).slice(0, 240)}`);
};
const section = (s) => console.log(`\n── ${s}`);

/** Runs one acceptance section; a thrown error fails the section but not the run. */
async function runSection(id, title, fn) {
  if (only && !only.includes(id)) return;
  section(`${id}. ${title}`);
  try {
    await fn();
  } catch (e) {
    check(id, "section ran to the end", false, describe(e));
    if (process.env.ORKESTR_P6_VERBOSE) console.error(e.stack ?? e);
  }
}

const describe = (e) => (e ? `${e.code ? `${e.code}: ` : ""}${e.message ?? e}${e.data?.unimplementedFeature ? ` [unimplementedFeature ${e.data.unimplementedFeature}]` : ""}` : "");

async function waitFor(fn, ms, label) {
  const end = Date.now() + T(ms);
  let last;
  for (;;) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (e) {
      last = e;
      // an unimplemented feature never starts working by waiting
      if (e?.data?.unimplementedFeature) throw e;
    }
    if (Date.now() > end) throw new Error(`timed out: ${label}${last ? ` (${describe(last)})` : ""}`);
    await sleep(150);
  }
}
const soft = (p) => p.then((v) => v, () => null);
const errOf = async (p) => {
  try {
    await p;
    return null;
  } catch (e) {
    return e;
  }
};
const isUnimplemented = (e) => typeof e?.data?.unimplementedFeature === "string";
/** A deferred feature's own RR-G14 answer (not the whole method still being a stub). */
const isDeferred = (e, method) => isUnimplemented(e) && e.data.unimplementedFeature !== method;
/** Sections after C2 need both workers enrolled. */
function needNodes() {
  if (!ctx.win || !ctx.macId || !ctx.gpuId) throw new Error("prerequisite: C1 setup + C2 enrollment did not complete");
}

// ── processes ─────────────────────────────────────────────────────────────
const procs = [];
function launch(exe, args, env, label) {
  const child = spawn(exe, args, { env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"] });
  const rec = { child, label, log: "", exited: false };
  child.stdout.on("data", (d) => (rec.log = (rec.log + d).slice(-400000)));
  child.stderr.on("data", (d) => (rec.log = (rec.log + d).slice(-400000)));
  child.on("exit", () => (rec.exited = true));
  procs.push(rec);
  return rec;
}
function kill(rec) {
  if (!rec?.child?.pid) return;
  try {
    if (process.platform === "win32") execFileSync("taskkill", ["/PID", String(rec.child.pid), "/T", "/F"], { stdio: "ignore" });
    else rec.child.kill("SIGKILL");
  } catch {
    /* gone */
  }
}

/** A fixture profile with the tool shims made absolute (the worker runs jobs in its own worktree). */
function loadProfile(file) {
  const p = JSON.parse(readFileSync(join(here, "fixtures", file), "utf8"));
  for (const [tool, cmd] of Object.entries(p.toolShims ?? {})) {
    p.toolShims[tool] = cmd.replace(/scripts\/smoke\/fixtures\/(\S+)/g, (_m, f) => JSON.stringify(join(here, "fixtures", f)));
  }
  return p;
}

/**
 * Starts (or restarts, same ORKESTR_HOME) an `orkestrd`. `profile` becomes
 * ORKESTR_NODE_PROFILE (inline JSON). Subscribes to every event topic.
 */
async function startCore(name, { profile = null } = {}) {
  const home = join(tmp, name);
  mkdirSync(home, { recursive: true });
  rmSync(join(home, "core.json"), { force: true });
  const env = {
    ORKESTR_HOME: home,
    ORKESTR_SECRET_BACKEND: "vault-file",
    ORKESTR_NODE_HEARTBEAT_MS: String(HEARTBEAT_MS),
    ORKESTR_NODE_LEASE_TTL_MS: String(LEASE_TTL_MS),
    RUST_LOG: "info",
    ...(profile ? { ORKESTR_NODE_PROFILE: JSON.stringify(profile) } : {}),
  };
  const rec = launch(coreExe, ["--data-dir", home], env, name);
  const coreInfo = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 60_000, `${name} core.json`);
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(coreInfo.url, token);
  await waitFor(() => rpc.open().then(() => true), 30_000, `${name} RPC`);
  await rpc.sub(["*"]);
  return { name, home, rec, rpc, url: coreInfo.url, profile };
}
async function stopCore(core) {
  core.rpc.close();
  kill(core.rec);
  await waitFor(() => core.rec.exited, 15_000, `${core.name} exit`).catch(() => null);
}

// ── local RPC (the desktop side of each core) ─────────────────────────────
function rpcClient(url, token) {
  let ws;
  let seq = 0;
  const pending = new Map();
  const events = [];
  const frame = (o, id) =>
    new Promise((res, rej) => {
      pending.set(id, { resolve: res, reject: rej });
      ws.send(JSON.stringify(o));
    });
  return {
    events,
    open: () =>
      new Promise((res, rej) => {
        ws = new WebSocket(url);
        ws.onmessage = (m) => {
          const f = JSON.parse(m.data);
          if (f.type === "res") {
            const p = pending.get(f.id);
            pending.delete(f.id);
            if (f.ok) p?.resolve(f.result);
            else p?.reject(Object.assign(new Error(`${f.error.code}: ${f.error.message}`), { code: f.error.code, data: f.error.data }));
          } else if (f.type === "event") events.push(f.event);
        };
        ws.onopen = () => frame({ type: "auth", token, id: "a" }, "a").then(res, rej);
        ws.onerror = () => rej(new Error("ws error"));
      }),
    call: (method, params = {}) => {
      const id = `r${++seq}`;
      return frame({ type: "req", id, method, params }, id);
    },
    sub: (topics) => {
      const id = `s${++seq}`;
      return frame({ type: "sub", id, topics }, id);
    },
    waitEvent: (pred, ms, label) => waitFor(() => events.find(pred), ms, label),
    close: () => ws?.close(),
  };
}

// ── gateway HTTP + account bootstrap (as phase 5) ─────────────────────────
async function gwFetch(path, { token, method = "GET", body } = {}) {
  const res = await fetch(`${base}${path}`, {
    method,
    headers: { ...(token ? { authorization: `Bearer ${token}` } : {}), ...(body ? { "content-type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  let json = null;
  try {
    json = JSON.parse(text);
  } catch {
    /* not json */
  }
  return { status: res.status, json, text };
}

function totpCode(secretB32, t = Date.now()) {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = "";
  for (const c of secretB32.replace(/=+$/, "").toUpperCase()) bits += alphabet.indexOf(c).toString(2).padStart(5, "0");
  const key = Buffer.from(bits.match(/.{8}/g).map((b) => parseInt(b, 2)));
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(Math.floor(t / 1000 / 30)));
  const h = crypto.createHmac("sha1", key).update(counter).digest();
  const o = h[h.length - 1] & 15;
  return String((h.readUInt32BE(o) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}

const b64u = (b) => Buffer.from(b).toString("base64url");
function cbor(v) {
  const head = (major, n) => {
    if (n < 24) return Buffer.from([(major << 5) | n]);
    if (n < 256) return Buffer.from([(major << 5) | 24, n]);
    if (n < 65536) return Buffer.from([(major << 5) | 25, n >> 8, n & 255]);
    const b = Buffer.alloc(5);
    b[0] = (major << 5) | 26;
    b.writeUInt32BE(n, 1);
    return b;
  };
  if (typeof v === "number") return v >= 0 ? head(0, v) : head(1, -1 - v);
  if (typeof v === "string") {
    const b = Buffer.from(v, "utf8");
    return Buffer.concat([head(3, b.length), b]);
  }
  if (v instanceof Uint8Array) return Buffer.concat([head(2, v.length), Buffer.from(v)]);
  if (v instanceof Map) return Buffer.concat([head(5, v.size), ...[...v].flatMap(([k, x]) => [cbor(k), cbor(x)])]);
  throw new Error(`cbor: unsupported ${typeof v}`);
}
/** Software passkey → account bearer; then TOTP enabled (the gateway requires a second factor). */
async function bootstrapAccount() {
  const begin = await gwFetch("/v1/auth/register/begin", { method: "POST", body: { name: "Owner" } });
  const { ceremonyId, publicKey: o } = begin.json;
  const kp = crypto.generateKeyPairSync("ec", { namedCurve: "P-256" });
  const jwk = kp.publicKey.export({ format: "jwk" });
  const credId = crypto.randomBytes(16);
  const cose = cbor(new Map([[1, 2], [3, -7], [-1, 1], [-2, Buffer.from(jwk.x, "base64url")], [-3, Buffer.from(jwk.y, "base64url")]]));
  const u16 = Buffer.alloc(2);
  u16.writeUInt16BE(credId.length);
  const authData = Buffer.concat([crypto.createHash("sha256").update(o.rp.id).digest(), Buffer.from([0x45]), Buffer.alloc(4), Buffer.alloc(16), u16, credId, cose]);
  const clientData = Buffer.from(JSON.stringify({ type: "webauthn.create", challenge: o.challenge, origin: base, crossOrigin: false }));
  const att = cbor(new Map([["fmt", "none"], ["attStmt", new Map()], ["authData", authData]]));
  const fin = await gwFetch("/v1/auth/register/finish", {
    method: "POST",
    body: { ceremonyId, credential: { id: b64u(credId), rawId: b64u(credId), type: "public-key", response: { clientDataJSON: b64u(clientData), attestationObject: b64u(att) } } },
  });
  const token = fin.json?.token;
  const setup = (await gwFetch("/v1/auth/totp/setup", { token, method: "POST" })).json;
  const confirm = await gwFetch("/v1/auth/totp/confirm", { token, method: "POST", body: { code: totpCode(setup.secret) } });
  return { token, ok: fin.status === 200 && !!token && confirm.status === 200, detail: `register ${fin.status}, totp ${confirm.status}` };
}

/** Enrolls a core with the gateway (remote.enable) and waits for its tunnel. */
async function tunnel(core, ownerToken, gatewayUrl = base) {
  const t = (await gwFetch("/v1/cores/enrollment-token", { token: ownerToken, method: "POST" })).json.token;
  await core.rpc.call("remote.enable", { gatewayUrl, enrollmentToken: t });
  const s = await waitFor(async () => {
    const st = await core.rpc.call("remote.status");
    return st.connected ? st : null;
  }, 20_000, `${core.name} tunnel`);
  core.coreId = s.coreId;
  return s;
}

// ── recording TCP proxy (coordinator ⇄ gateway): relay plaintext check ─────
const captured = [];
const proxy = net.createServer((client) => {
  const upstream = net.connect(gwPort, "127.0.0.1");
  client.on("data", (d) => {
    captured.push(Buffer.from(d));
    upstream.write(d);
  });
  upstream.on("data", (d) => {
    captured.push(Buffer.from(d));
    client.write(d);
  });
  const close = () => {
    client.destroy();
    upstream.destroy();
  };
  client.on("error", close);
  upstream.on("error", close);
  client.on("close", close);
  upstream.on("close", close);
});
/** Captured bytes that contain any of the plaintext markers (after the HTTP upgrade). */
function plaintextLeaks(markers) {
  const all = Buffer.concat(captured).toString("latin1");
  return markers.filter((m) => all.includes(m));
}

// ── Phase 6 helpers ───────────────────────────────────────────────────────
const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();
function makeRepo(name) {
  const dir = join(tmp, name);
  mkdirSync(join(dir, "src"), { recursive: true });
  git(dir, "init", "-q", "-b", "main");
  git(dir, "config", "user.email", "p6@orkestr.local");
  git(dir, "config", "user.name", "Orkestr P6");
  writeFileSync(join(dir, "README.md"), "# Multi-node demo\n");
  writeFileSync(join(dir, "src", "app.txt"), "line 1\n");
  git(dir, "add", "-A");
  git(dir, "commit", "-q", "-m", "initial");
  return dir;
}

/**
 * Enrollment through a `kind: "node"` pairing:
 *   worker:      devices.startPairing { kind: "node", trustLevel }
 *   coordinator: nodes.enroll { coreId, pairingId, code }  → { enrollment, sas }
 *   worker:      devices.list → pending[pairingId].sas  (SAS compare)
 *   worker:      devices.approve { pairingId, sasConfirmed: true, role: "operator",
 *                                  remoteTerminal: false, kind: "node", trustLevel }
 * nodes.enroll may resolve before or after the worker approves; both work.
 */
async function enrollNode(coord, worker, trustLevel, { code } = {}) {
  const offer = await worker.rpc.call("devices.startPairing", { kind: "node", trustLevel });
  const enrolling = coord.rpc.call("nodes.enroll", { coreId: offer.coreId ?? worker.coreId, pairingId: offer.pairingId, code: code ?? offer.code, name: worker.name });
  enrolling.catch(() => null);
  if (code) return { offer, error: await errOf(enrolling) };
  const pend = await Promise.race([
    waitFor(async () => (await worker.rpc.call("devices.list")).pending.find((p) => p.pairingId === offer.pairingId), 20_000, `${worker.name} pending pairing`),
    enrolling.then(() => null, (e) => Promise.reject(e)),
  ]).then((p) => p ?? waitFor(async () => (await worker.rpc.call("devices.list")).pending.find((x) => x.pairingId === offer.pairingId), 10_000, "pending"));
  await worker.rpc.call("devices.approve", { pairingId: offer.pairingId, sasConfirmed: true, role: "operator", remoteTerminal: false, kind: "node", trustLevel });
  const res = await enrolling;
  return { offer, res, workerSas: pend.sas, nodeId: res.enrollment.nodeId };
}

const snapshots = async (core) => (await core.rpc.call("nodes.list")).snapshots ?? [];
const snapshotOf = async (core, nodeId) => (await snapshots(core)).find((s) => s.node.id === nodeId);
async function waitNodeStatus(core, nodeId, statuses, ms = 15_000) {
  const want = [].concat(statuses);
  return waitFor(async () => {
    const s = await snapshotOf(core, nodeId);
    return s && want.includes(s.status) ? s : null;
  }, ms, `node ${nodeId} → ${want.join("|")}`);
}

/** A `JobSubmitSpec` with defaults (node-protocol.md §5). */
function jobSpec({ kind, requirements = {}, estimate = {}, needs = {}, affinity = {} }) {
  return {
    kind,
    requirements: { tools: [], localModels: [], labels: {}, ...requirements },
    estimate: { cpuCores: 1, memMb: 256, vramMb: 0, diskMb: 50, durationS: 30, ...estimate },
    needs: { network: false, secrets: [], ...needs },
    affinity: { preferred: [], forbidden: [], stickyToCache: false, ...affinity },
  };
}
const command = (program, args = [], env = {}) => ({ type: "command", program, args, env, timeoutSecs: 300 });
const agentTask = (prompt, extra = {}) => ({ type: "agentTask", prompt, ...extra });
const fakeXcode = join(here, "fixtures", "fake-xcodebuild.mjs");
/** A long-running command job (node + the fake build tool) usable on any node. */
const longCommand = (steps = 60, stepMs = 250, extra = []) => command("node", [fakeXcode, "-scheme", "Long", "--steps", String(steps), "--step-ms", String(stepMs), ...extra]);

/** tasks.create + jobs.submit { taskId, spec } on the coordinator. */
async function submit(coord, projectId, title, spec, { priority } = {}) {
  const task = (await coord.rpc.call("tasks.create", { projectId, title, description: title, ...(priority ? { priority } : {}) })).task;
  const job = (await coord.rpc.call("jobs.submit", { taskId: task.id, spec })).job;
  return { task, job };
}
const getJob = async (coord, jobId) => (await coord.rpc.call("jobs.get", { jobId })).job;
const ownerNode = (job) => (job?.owner?.kind === "node" ? job.owner.nodeId : null);
async function waitJob(coord, jobId, pred, ms, label) {
  return waitFor(async () => {
    const j = await getJob(coord, jobId);
    return pred(j) ? j : null;
  }, ms, `job ${jobId.slice(0, 8)}: ${label}`);
}
const TERMINAL = new Set(["completed", "failed", "cancelled", "infeasible"]);
const progressOf = (core, jobId) => core.rpc.events.filter((e) => e.kind === "JobProgress" && e.payload.jobId === jobId);
const outputText = (core, jobId) =>
  progressOf(core, jobId)
    .flatMap((e) => e.payload.items)
    .filter((i) => i.type === "output")
    .map((i) => i.text)
    .join("");
const taskStatuses = (core, taskId) => core.rpc.events.filter((e) => e.kind === "TaskStatusChanged" && e.payload.taskId === taskId).map((e) => e.payload.to);
/** "Preferred" is a soft score term; jobs meant for a worker also exclude the coordinator itself. */
const offCoordinator = () => (ctx.winId ? [ctx.winId] : []);
const explanationOf = (job) => (job?.placement?.explanation ?? []).join(" | ");

/** Files under a store directory whose name is the sha256 hex (FsBlobStore layout: blobs/sha256/..). */
function findBlob(home, hex) {
  const root = join(home, "blobs");
  if (!existsSync(root)) return null;
  const stack = [root];
  while (stack.length) {
    const d = stack.pop();
    for (const e of readdirSync(d)) {
      const p = join(d, e);
      if (statSync(p).isDirectory()) stack.push(p);
      else if (e === hex || e.startsWith(hex)) return p;
    }
  }
  return null;
}
const sha256File = (p) => `sha256:${crypto.createHash("sha256").update(readFileSync(p)).digest("hex")}`;
const hexOf = (digest) => String(digest).replace(/^sha256:/, "");

async function auditActions(core) {
  const r = await soft(core.rpc.call("audit.list", { limit: 2000 }));
  return (r?.entries ?? []).map((e) => ({ action: e.event?.action ?? e.action ?? "", details: JSON.stringify(e.event?.details ?? {}), resource: e.event?.resource ?? "" }));
}

// ══════════════════════════════════════════════════════════════════════════
let exitCode = 0;
const ctx = {}; // shared state between sections (cores, node ids, project, …)

try {
  for (const [p, what] of [
    [coreExe, "orkestrd"],
    [gwExe, "orkestr-gateway"],
  ]) {
    if (!existsSync(p)) throw new Error(`missing ${what} (${p}) — build first or set ORKESTR_SMOKE_BIN_DIR`);
  }

  // ════════════════════════════════════════════════════════════════════════
  await runSection("C1", "setup: gateway + 3 cores (win coordinator, mac / gpu simulated workers)", async () => {
    const gw = launch(gwExe, ["--data-dir", join(tmp, "gateway"), "--bind", `127.0.0.1:${gwPort}`], { RUST_LOG: "info" }, "gateway");
    void gw;
    await waitFor(async () => (await fetch(`${base}/healthz`)).ok, 30_000, "gateway /healthz");
    await new Promise((r) => proxy.listen(proxyPort, "127.0.0.1", r));
    check("C1", "gateway up; recording proxy in front of it for the coordinator", true, `${base} via ${proxyBase}`);

    const acct = await bootstrapAccount();
    ctx.ownerToken = acct.token;
    check("C1", "account: passkey (software authenticator) + TOTP", acct.ok, acct.detail);

    const [win, mac, gpu] = await Promise.all([
      startCore("win"),
      startCore("mac", { profile: loadProfile("node-mac.json") }),
      startCore("gpu", { profile: loadProfile("node-gpu.json") }),
    ]);
    Object.assign(ctx, { win, mac, gpu });
    check("C1", "three orkestrd cores started (separate ORKESTR_HOME, 500 ms heartbeat, 3 s lease TTL)", true, [win, mac, gpu].map((c) => c.url).join(" "));

    await tunnel(win, ctx.ownerToken, proxyBase);
    await tunnel(mac, ctx.ownerToken);
    await tunnel(gpu, ctx.ownerToken);
    const cores = (await gwFetch("/v1/cores", { token: ctx.ownerToken })).json?.cores ?? [];
    check("C1", "all three cores tunneled to the gateway and online", [win, mac, gpu].every((c) => cores.find((x) => x.coreId === c.coreId)?.online), cores.map((c) => `${c.name ?? c.coreId}:${c.online}`).join(", "));

    const local = (await win.rpc.call("nodes.list")).nodes.find((n) => n.isLocal);
    info("C1", "real GPU detection on the coordinator (informational)", local ? JSON.stringify(local.resources.gpus) : "no local node");

    await win.rpc.call("settings.update", { patch: { profileName: "P6", defaultApprovalMode: "SemiAuto", onboardingCompleted: true } });
    const mock = (await win.rpc.call("providers.list")).providers.map((p) => p.provider ?? p).find((p) => p.kind === "Mock");
    ctx.smart = { providerId: mock.id, model: "mock-smart" };
    await win.rpc.call("providers.setRouting", { config: { mode: "Manual", rules: [], defaultFallback: [ctx.smart], preferLocal: true, maxRateLimitWaitSecs: 0 } });
    ctx.repo = makeRepo("repo-win");
    ctx.project = (await win.rpc.call("projects.create", { name: "Multi-node Demo", rootPath: ctx.repo, approvalMode: "SemiAuto" })).project;
    ctx.backendTpl = (await win.rpc.call("templates.list", {})).templates.find((t) => t.role === "Backend");
    check("C1", "coordinator project (git repo) + mock provider routing", !!ctx.project && !!ctx.backendTpl, ctx.project?.id);
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("C2", "registry: enrollment with SAS, trust levels, resources", async () => {
    const { win, mac, gpu } = ctx;
    const wrong = await enrollNode(win, mac, "standard", { code: "WRONG000" });
    check("C2", "nodes.enroll with a wrong pairing code is rejected", !!wrong.error && !isUnimplemented(wrong.error), describe(wrong.error));

    const eMac = await enrollNode(win, mac, "standard");
    check("C2", "mac enrolled via kind:\"node\" pairing; SAS equal on coordinator and worker", eMac.res.sas === eMac.workerSas && eMac.res.enrollment.trustLevel === "standard", `${eMac.res.sas} / ${eMac.workerSas}`);
    const eGpu = await enrollNode(win, gpu, "trusted");
    check("C2", "gpu enrolled (trusted); SAS equal", eGpu.res.sas === eGpu.workerSas && eGpu.res.enrollment.trustLevel === "trusted", `${eGpu.res.sas} / ${eGpu.workerSas}`);
    ctx.macId = eMac.nodeId;
    ctx.winId = (await snapshots(win)).find((s) => s.isCoordinator)?.node.id;
    ctx.gpuId = eGpu.nodeId;
    const pinned = eMac.res.enrollment.publicKey && eMac.res.enrollment.sig;
    check("C2", "enrollment record pins the worker CIK and is signed by the coordinator", !!pinned, `${String(eMac.res.enrollment.publicKey).slice(0, 16)}…`);

    const snaps = await waitFor(async () => {
      const s = await snapshots(win);
      return s.length >= 3 && s.filter((x) => !x.isCoordinator).every((x) => x.status === "Online" && x.lastHeartbeatAt) ? s : null;
    }, 20_000, "3 nodes online with heartbeats");
    const sMac = snaps.find((s) => s.node.id === ctx.macId);
    const sGpu = snaps.find((s) => s.node.id === ctx.gpuId);
    const vram = Math.max(0, ...(sGpu?.node.resources.gpus ?? []).map((g) => g.vramBytes ?? 0));
    check(
      "C2",
      "nodes.list: 3 nodes (coordinator first) with profile resources (mac: MacOs/Arm64 + xcodebuild; gpu: Linux/X64, RTX 4090 24 GB, cuda)",
      snaps[0]?.isCoordinator &&
        sMac?.node.os === "MacOs" &&
        sMac.node.arch === "Arm64" &&
        sMac.node.tools.includes("xcodebuild") &&
        sGpu?.node.os === "Linux" &&
        sGpu.node.arch === "X64" &&
        sGpu.node.tools.includes("cuda") &&
        vram >= 24 * 1024 * MiB &&
        /4090/.test(sGpu.node.resources.gpus[0]?.name ?? ""),
      snaps.map((s) => `${s.node.name}:${s.node.os}/${s.node.arch}:${s.status}:${s.trustLevel}`).join(", "),
    );
    check("C2", "trust levels as granted on the workers", sMac?.trustLevel === "standard" && sGpu?.trustLevel === "trusted");
    const reg = win.rpc.events.filter((e) => e.kind === "NodeRegistered").map((e) => e.payload.node.node.id);
    check("C2", "NodeRegistered events for both workers", [ctx.macId, ctx.gpuId].every((id) => reg.includes(id)), reg.join(","));
    const leaks = plaintextLeaks(["node.heartbeat", "node.hello", "\"renew\"", "xcodebuild", "RTX 4090"]);
    check("C2", "core-to-core relay (coordinator ⇄ gateway capture) carries no plaintext", captured.length > 10 && leaks.length === 0, `${captured.length} chunks; leaked: ${leaks.join(",")}`);
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("C3", "scenario F: build a macOS app on the mac node", async () => {
    needNodes();
    const { win, mac } = ctx;
    const { task, job } = await submit(
      win,
      ctx.project.id,
      "Build macOS app",
      jobSpec({ kind: command("xcodebuild", ["-scheme", "Demo", "-configuration", "Release", "build"]), requirements: { os: "MacOs", tools: ["xcodebuild"] } }),
    );
    ctx.fJob = job.spec.jobId;
    check("C3", "placement → mac (requirements.os = MacOs), explanation stored", job.placement?.decision === "place" && job.placement.nodeId === ctx.macId && job.placement.explanation.length > 0, explanationOf(job));
    const assigned = await win.rpc.waitEvent((e) => e.kind === "JobAssigned" && e.payload.jobId === ctx.fJob, 10_000, "JobAssigned");
    check("C3", "JobAssigned (nodeId = mac, epoch, explanation)", assigned.payload.nodeId === ctx.macId && assigned.payload.explanation.length > 0, `epoch ${assigned.payload.epoch}`);
    const done = await waitJob(win, ctx.fJob, (j) => TERMINAL.has(j.state), 60_000, "terminal");
    const out = outputText(win, ctx.fJob);
    check("C3", "the xcodebuild shim ran on the worker; progress streamed to the coordinator", /CompileSwift/.test(out) && /BUILD SUCCEEDED/.test(out) && progressOf(win, ctx.fJob).every((e) => e.payload.nodeId === ctx.macId), `${progressOf(win, ctx.fJob).length} JobProgress events`);
    const m = /ARTIFACT (.+?) (sha256:[0-9a-f]{64})/.exec(out);
    const art = done.artifacts?.[0];
    const blob = art ? findBlob(win.home, hexOf(art)) : null;
    check(
      "C3",
      "artifact (.app.zip) returned as a blob, hash-verified on the coordinator",
      done.state === "completed" && !!art && m?.[2] === art && !!blob && sha256File(blob) === art,
      `${done.state}${done.detail ? ` (${done.detail})` : ""}; artifact ${art}; worker printed ${m?.[2]}; blob ${blob ?? "missing"}`,
    );
    const t = (await win.rpc.call("tasks.get", { taskId: task.id })).task;
    check("C3", "task Completed on the coordinator with owner = mac", t.status === "Completed" && t.owner?.kind === "node" && t.owner.nodeId === ctx.macId, `${t.status} owner ${JSON.stringify(t.owner)}`);
    check("C3", "placement explanation stored on the task", JSON.stringify(t).includes(job.placement.explanation[0] ?? "\u0000"), explanationOf(job).slice(0, 120));
    void mac;
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("C4", "routing: GPU, infeasible, draining, restricted", async () => {
    needNodes();
    const { win } = ctx;
    const g = await submit(win, ctx.project.id, "Train on GPU", jobSpec({ kind: command("node", ["--version"]), requirements: { gpu: { minVramMb: 16384 } }, estimate: { vramMb: 16384 } }));
    check("C4", "GPU ≥ 16 GB VRAM → gpu", g.job.placement?.decision === "place" && g.job.placement.nodeId === ctx.gpuId, explanationOf(g.job));
    await waitJob(win, g.job.spec.jobId, (j) => TERMINAL.has(j.state), 30_000, "gpu job done").catch(() => null);

    // no requirements: the coordinator itself wins (locality) and runs it on its own worker host
    const l = await submit(win, ctx.project.id, "Local job", jobSpec({ kind: command("node", ["-e", "console.log('local ok')"]) }));
    const lDone = await waitJob(win, l.job.spec.jobId, (j) => TERMINAL.has(j.state), 30_000, "local job done").catch(() => null);
    check("C4", "job without requirements → coordinator node, runs on its own worker host", l.job.placement?.nodeId === ctx.winId && lDone?.state === "completed" && /local ok/.test(outputText(win, l.job.spec.jobId)), `${l.job.placement?.decision} ${l.job.placement?.nodeId === ctx.winId ? "coordinator" : l.job.placement?.nodeId}; ${lDone?.state} ${lDone?.detail ?? ""}`);

    const imp = await submit(win, ctx.project.id, "Impossible GPU job", jobSpec({ kind: command("node", ["--version"]), requirements: { gpu: { minVramMb: 100_000 } } }));
    const impTask = await waitFor(async () => {
      const t = (await win.rpc.call("tasks.get", { taskId: imp.task.id })).task;
      return t.status === "Blocked" ? t : null;
    }, 10_000, "task Blocked").catch(() => null);
    check("C4", "impossible requirement → infeasible (explanation per node), task Blocked", imp.job.placement?.decision === "infeasible" && imp.job.state === "infeasible" && imp.job.placement.explanation.length >= 2 && !!impTask, explanationOf(imp.job));

    await win.rpc.call("nodes.drain", { nodeId: ctx.gpuId, drain: true });
    await waitNodeStatus(win, ctx.gpuId, "Draining", 10_000);
    const d = await submit(win, ctx.project.id, "GPU while draining", jobSpec({ kind: command("node", ["--version"]), requirements: { gpu: { minVramMb: 16384 } } }));
    check("C4", "draining node excluded from placement", d.job.placement?.decision !== "place" || d.job.placement.nodeId !== ctx.gpuId, `${d.job.placement?.decision}: ${explanationOf(d.job)}`);
    await win.rpc.call("nodes.drain", { nodeId: ctx.gpuId, drain: false });
    await waitNodeStatus(win, ctx.gpuId, "Online", 10_000);
    await soft(win.rpc.call("jobs.cancel", { jobId: d.job.spec.jobId, immediate: true }));

    await win.rpc.call("nodes.setPolicy", { nodeId: ctx.macId, trustLevel: "restricted" });
    const r = await submit(win, ctx.project.id, "Agent on the mac", jobSpec({ kind: agentTask("Say hello", { templateId: ctx.backendTpl.id }), requirements: { os: "MacOs" } }));
    check("C4", "restricted node refuses AgentTask (infeasible, explanation names the trust level)", r.job.placement?.decision !== "place" && /trust|restricted/i.test(explanationOf(r.job)), explanationOf(r.job));
    await win.rpc.call("nodes.setPolicy", { nodeId: ctx.macId, trustLevel: "standard" });
    const back = await snapshotOf(win, ctx.macId);
    check("C4", "nodes.setPolicy restores trust (never above the worker's grant)", back?.trustLevel === "standard");
    const over = await errOf(win.rpc.call("nodes.setPolicy", { nodeId: ctx.macId, trustLevel: "trusted" }));
    check("C4", "raising trust above the worker's grant is refused", !!over || (await snapshotOf(win, ctx.macId))?.trustLevel === "standard", describe(over));
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("C5", "cross-node AgentTask (mock provider) on gpu", async () => {
    needNodes();
    const { win } = ctx;
    const before = git(ctx.repo, "log", "--all", "--oneline").split("\n").length;
    const a = await submit(
      win,
      ctx.project.id,
      "Add hello module",
      // the remote mock provider follows a deterministic plan: one prompt line = one tool step
      jobSpec({ kind: agentTask("Add a hello module.\nwrite src/hello.txt Hello from the gpu node\\n", { templateId: ctx.backendTpl.id, model: ctx.smart }), requirements: { gpu: { minVramMb: 16384 } } }),
    );
    check("C5", "AgentTask placed on gpu", a.job.placement?.decision === "place" && a.job.placement.nodeId === ctx.gpuId, explanationOf(a.job));
    const done = await waitJob(win, a.job.spec.jobId, (j) => TERMINAL.has(j.state), 120_000, "agent done");
    const log = git(ctx.repo, "log", "--all", "--oneline");
    const diff = await soft(win.rpc.call("vcs.diff", { projectId: ctx.project.id, target: { kind: "Head" } }));
    // the worker's result lands in the coordinator's project as uncommitted changes (new file → untracked)
    const hello = join(ctx.repo, "src", "hello.txt");
    const landed = existsSync(hello) && readFileSync(hello, "utf8").includes("Hello from the gpu node");
    const status = git(ctx.repo, "status", "--porcelain");
    check(
      "C5",
      "agent edited the worker's worktree; the change is applied in the coordinator's repository",
      done.state === "completed" && (log.split("\n").length > before || landed || JSON.stringify(diff ?? {}).length > 50),
      `${done.state}${done.detail ? ` (${done.detail})` : ""}; commits ${before} → ${log.split("\n").length}; src/hello.txt ${landed ? "applied" : "missing"}; status ${status.replace(/\n/g, " ")}`,
    );
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("C6", "migration: planned, unplanned, stale epoch, side effects", async () => {
    needNodes();
    const { win } = ctx;
    // planned gpu → mac
    const p = await submit(
      win,
      ctx.project.id,
      "Long agent job",
      jobSpec({
        // one plan line = one step; safe points sit between steps (a drained job resumes after the last completed one)
        kind: agentTask(["Work in steps.", ...Array.from({ length: 14 }, (_, i) => `run node ${fakeXcode} -scheme Mig${i + 1} --steps 2 --step-ms 250`)].join("\n"), { templateId: ctx.backendTpl.id, model: ctx.smart }),
        affinity: { preferred: [ctx.gpuId], forbidden: offCoordinator() },
      }),
    );
    const pid = p.job.spec.jobId;
    await waitJob(win, pid, (j) => j.state === "running" && ownerNode(j) === ctx.gpuId, 30_000, "running on gpu");
    await waitFor(() => progressOf(win, pid).length >= 2, 20_000, "some progress on gpu");
    const migrated = await win.rpc.call("jobs.migrate", { jobId: pid, targetNodeId: ctx.macId });
    const cp = await win.rpc.waitEvent((e) => e.kind === "JobCheckpointed" && e.payload.checkpoint.jobId === pid, 30_000, "JobCheckpointed");
    const mv = await win.rpc.waitEvent((e) => e.kind === "JobMigrated" && e.payload.jobId === pid, 30_000, "JobMigrated");
    const pDone = await waitJob(win, pid, (j) => TERMINAL.has(j.state), 120_000, "migrated job done");
    const doneSteps = cp.payload.checkpoint.agent?.completedSteps ?? [];
    const stepsAfter = progressOf(win, pid)
      .filter((e) => e.payload.nodeId === ctx.macId)
      .flatMap((e) => e.payload.items)
      .filter((i) => i.type === "step")
      .map((i) => i.name);
    check("C6", "planned migration gpu → mac: JobCheckpointed + JobMigrated (epoch + 1)", mv.payload.fromNodeId === ctx.gpuId && mv.payload.toNodeId === ctx.macId && mv.payload.epoch > p.job.epoch && !!migrated, `epoch ${p.job.epoch} → ${mv.payload.epoch}`);
    check(
      "C6",
      "resumed on mac from the handoff, completed steps not repeated",
      pDone.state === "completed" && ownerNode(pDone) === ctx.macId && !!cp.payload.checkpoint.handoff && stepsAfter.every((s) => !doneSteps.includes(s)),
      `${pDone.state}; done before: ${doneSteps.join(",")}; after: ${stepsAfter.join(",")}`,
    );

    // unplanned: kill gpu mid-job
    const u = await submit(win, ctx.project.id, "Long command (unplanned)", jobSpec({ kind: longCommand(80, 250), affinity: { preferred: [ctx.gpuId], forbidden: offCoordinator() } }));
    const uid = u.job.spec.jobId;
    const running = await waitJob(win, uid, (j) => j.state === "running" && ownerNode(j) === ctx.gpuId, 30_000, "running on gpu");
    const oldEpoch = running.epoch;
    await stopCore(ctx.gpu);
    const t0 = Date.now();
    await waitNodeStatus(win, ctx.gpuId, ["Suspect", "Offline"], 15_000);
    const moved = await waitJob(win, uid, (j) => j.state === "running" && ownerNode(j) && ownerNode(j) !== ctx.gpuId, 30_000, "running elsewhere");
    const seq = taskStatuses(win, u.task.id);
    const iI = seq.indexOf("Interrupted");
    const iR = seq.indexOf("Recovering", iI);
    const iRun = seq.indexOf("Running", iR);
    check(
      "C6",
      "unplanned: gpu killed → lease expiry → Interrupted → Recovering → Running on another node (epoch + 1)",
      iI >= 0 && iR > iI && iRun > iR && moved.epoch > oldEpoch,
      `${seq.join("→")}; ${ownerNode(moved)} epoch ${oldEpoch} → ${moved.epoch}; ${Date.now() - t0} ms`,
    );

    // restart gpu: its stale epoch must be rejected
    ctx.gpu = await startCore("gpu", { profile: loadProfile("node-gpu.json") });
    await tunnel(ctx.gpu, ctx.ownerToken);
    await waitNodeStatus(win, ctx.gpuId, "Online", 30_000);
    await sleep(LEASE_TTL_MS); // let a few heartbeats carry the old lease, if the worker kept it
    const after = await getJob(win, uid);
    const audits = [...(await auditActions(win)), ...(await auditActions(ctx.gpu))];
    const stale = audits.filter((a) => /stale|fenc/i.test(a.action) || /stale.?epoch/i.test(a.details));
    check(
      "C6",
      "restarted gpu reconnects; a stale-epoch call / report is rejected (job stays with the new owner)",
      ownerNode(after) === ownerNode(moved) && after.epoch >= moved.epoch && stale.length > 0,
      `owner ${ownerNode(after)} epoch ${after.epoch}; stale audits: ${stale.map((a) => a.action).join(",")}`,
    );
    await soft(win.rpc.call("jobs.cancel", { jobId: uid, immediate: true }));

    // side effects: a non-idempotent action, then an unplanned loss → needs_user_decision
    const s = await submit(win, ctx.project.id, "Deploy (side effect)", jobSpec({ kind: longCommand(80, 250, ["--side-effect", "deploy"]), affinity: { preferred: [ctx.gpuId], forbidden: offCoordinator() } }));
    const sid = s.job.spec.jobId;
    await waitJob(win, sid, (j) => j.state === "running" && ownerNode(j) === ctx.gpuId, 30_000, "side-effect job running on gpu");
    await waitFor(() => outputText(win, sid).includes("::orkestr-side-effect"), 20_000, "side effect reported");
    await stopCore(ctx.gpu);
    const failed = await waitJob(win, sid, (j) => j.state === "failed", 30_000, "failed");
    const appr = await win.rpc.waitEvent((e) => e.kind === "ApprovalRequested" && e.payload.approval.taskId === s.task.id, 15_000, "ApprovalRequested").catch(() => null);
    check("C6", "unknown / non-idempotent side effects → failed(needs_user_decision) + ApprovalRequested (no silent re-run)", failed.detail === "needs_user_decision" && !!appr, `${failed.state} ${failed.detail}`);
    ctx.gpu = await startCore("gpu", { profile: loadProfile("node-gpu.json") });
    await tunnel(ctx.gpu, ctx.ownerToken);
    await waitNodeStatus(win, ctx.gpuId, "Online", 30_000);
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("C7", "models: import, distribute, verify, route", async () => {
    needNodes();
    const { win } = ctx;
    const imported = (await win.rpc.call("models.import", { name: "mock-coder", tag: "7b", mockSizeBytes: 20 * MiB })).model;
    check("C7", "models.import (mock, 20 MiB) on the coordinator", imported.modelRef === "mock-coder:7b" && imported.sizeBytes >= 20 * MiB, `${imported.digest}`);
    const dist = await win.rpc.call("models.distribute", { modelRef: "mock-coder:7b", nodeIds: [ctx.gpuId] });
    const ev = await win.rpc.waitEvent((e) => e.kind === "ModelDistributed" && e.payload.nodeId === ctx.gpuId && e.payload.modelRef === "mock-coder:7b", 60_000, "ModelDistributed");
    check("C7", "models.distribute → gpu ok, ModelDistributed", dist.outcomes.find((o) => o.nodeId === ctx.gpuId)?.ok && ev.payload.ok && dist.digest === imported.digest, JSON.stringify(dist.outcomes));
    const onGpu = (await ctx.gpu.rpc.call("models.list")).models.find((m) => m.modelRef === "mock-coder:7b");
    const layersOk = (onGpu?.manifest.layers ?? []).every((l) => {
      const p = findBlob(ctx.gpu.home, hexOf(l.digest));
      return p && sha256File(p) === l.digest;
    });
    check("C7", "listed on gpu; every layer blob present and digest-verified on disk", onGpu?.digest === imported.digest && onGpu.manifest.layers.length > 0 && layersOk, onGpu?.digest);

    // tampered source blob: the receiver must reject it
    const bad = (await win.rpc.call("models.import", { name: "mock-tampered", tag: "1", mockSizeBytes: 2 * MiB })).model;
    const layer = bad.manifest.layers.find((l) => l.size > 1024) ?? bad.manifest.layers[0];
    const path = findBlob(win.home, hexOf(layer.digest));
    if (path) {
      const buf = readFileSync(path);
      buf[Math.floor(buf.length / 2)] ^= 0xff;
      writeFileSync(path, buf);
    }
    const tam = await errOf(win.rpc.call("models.distribute", { modelRef: "mock-tampered:1", nodeIds: [ctx.gpuId] }).then((r) => (r.outcomes.every((o) => !o.ok) ? Promise.reject(Object.assign(new Error(r.outcomes.map((o) => o.error).join("; ")), { code: "outcome" })) : r)));
    const tamOnGpu = (await ctx.gpu.rpc.call("models.list")).models.find((m) => m.modelRef === "mock-tampered:1");
    check("C7", "tampered chunk rejected (digest mismatch), model not installed on gpu", !!path && !!tam && !tamOnGpu, describe(tam));

    const j = await submit(win, ctx.project.id, "Use the local model", jobSpec({ kind: command("node", ["--version"]), requirements: { localModels: ["mock-coder:7b"] }, affinity: { forbidden: ctx.winId ? [ctx.winId] : [] } }));
    // (the coordinator holds the imported model too; it is excluded so the route is decided between the workers)
    check("C7", "job requiring mock-coder:7b routed to gpu (scheduler sees the installed model)", j.job.placement?.decision === "place" && j.job.placement.nodeId === ctx.gpuId, explanationOf(j.job));
    await waitJob(win, j.job.spec.jobId, (x) => TERMINAL.has(x.state), 30_000, "done").catch(() => null);
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("C8", "security: emergency stop, revoke, audit", async () => {
    needNodes();
    const { win, mac } = ctx;
    const a = await submit(win, ctx.project.id, "Long on mac", jobSpec({ kind: longCommand(200, 250), affinity: { preferred: [ctx.macId], forbidden: offCoordinator() } }));
    const b = await submit(win, ctx.project.id, "Long on gpu", jobSpec({ kind: longCommand(200, 250), affinity: { preferred: [ctx.gpuId], forbidden: offCoordinator() } }));
    await waitJob(win, a.job.spec.jobId, (j) => j.state === "running", 30_000, "a running");
    await waitJob(win, b.job.spec.jobId, (j) => j.state === "running", 30_000, "b running");
    await win.rpc.call("emergency.stopAll", {});
    const [ca, cb] = await Promise.all([
      waitJob(win, a.job.spec.jobId, (j) => j.state === "cancelled", 15_000, "a cancelled"),
      waitJob(win, b.job.spec.jobId, (j) => j.state === "cancelled", 15_000, "b cancelled"),
    ]);
    const leasesClear = await waitFor(async () => (await snapshots(win)).every((s) => s.leases.every((l) => l.phase !== "running" && l.phase !== "starting")), 15_000, "no running leases").catch(() => false);
    const workerStops = [mac, ctx.gpu].map((c) => c.rpc.events.some((e) => e.kind === "EmergencyStop"));
    check("C8", "emergency stop broadcast cancels jobs on all nodes", ca.state === "cancelled" && cb.state === "cancelled" && !!leasesClear && workerStops.every(Boolean), `worker EmergencyStop events: ${workerStops.join(",")}`);
    await soft(win.rpc.call("emergency.reset", {}));

    const r = await submit(win, ctx.project.id, "Long on mac (revoke)", jobSpec({ kind: longCommand(200, 250), affinity: { preferred: [ctx.macId], forbidden: offCoordinator() } }));
    await waitJob(win, r.job.spec.jobId, (j) => j.state === "running" && ownerNode(j) === ctx.macId, 30_000, "running on mac");
    await win.rpc.call("nodes.revoke", { nodeId: ctx.macId });
    const revoked = await waitNodeStatus(win, ctx.macId, "Revoked", 10_000);
    const rj = await waitJob(win, r.job.spec.jobId, (j) => j.state === "cancelled" || (j.state === "running" && ownerNode(j) !== ctx.macId), 15_000, "revoked node's job cancelled / moved");
    const after = await submit(win, ctx.project.id, "Mac after revoke", jobSpec({ kind: command("xcodebuild", ["-version"]), requirements: { os: "MacOs" } }));
    check("C8", "revoked node refused at next heartbeat; its job cancelled; never placed again", revoked.status === "Revoked" && rj.state === "cancelled" && after.job.placement?.decision !== "place", `${rj.state}; next: ${after.job.placement?.decision}`);
    const workerRefuses = await waitFor(async () => {
      const d = await mac.rpc.call("devices.list");
      return d.devices.some((x) => x.revoked) ? true : null;
    }, 10_000, "worker marks the coordinator revoked").catch(() => false);
    info("C8", "worker side lists the coordinator as revoked", String(workerRefuses));

    const acts = await auditActions(win);
    const wanted = { offer: /offer/i, accept: /accept/i, complete: /complet/i, migrate: /migrat/i };
    const missing = Object.entries(wanted)
      .filter(([, re]) => !acts.some((a) => re.test(a.action) && /nodeId|node_id/.test(a.details)))
      .map(([k]) => k);
    check("C8", "audit: job offer / accept / complete / migrate entries carry the nodeId", missing.length === 0, missing.length ? `missing ${missing.join(",")}; have ${[...new Set(acts.map((a) => a.action))].filter((x) => /job|node/.test(x)).join(",")}` : `${acts.length} entries`);
    const leaks = plaintextLeaks(["node.heartbeat", "jobs.offer", "CompileSwift", "xcodebuild", "Build macOS app", "mock-coder"]);
    check("C8", "relay capture (whole run) still carries no plaintext", leaks.length === 0, `${captured.length} chunks; leaked: ${leaks.join(",")}`);
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("C9", "deferred features answer unimplementedFeature", async () => {
    if (!ctx.win || !ctx.project) throw new Error("prerequisite: C1 setup did not complete");
    const { win } = ctx;
    const sec = await errOf(submit(win, ctx.project.id, "Needs a secret", jobSpec({ kind: command("node", ["--version"]), needs: { secrets: ["API_KEY"] }, affinity: { forbidden: ctx.winId ? [ctx.winId] : [] }, requirements: { os: "Linux" } })).then((r) => (r.job.placement?.decision === "infeasible" && /secret/i.test(explanationOf(r.job)) ? Promise.reject(Object.assign(new Error(explanationOf(r.job)), { code: "infeasible" })) : r)));
    check("C9", "secrets to a remote node (HPKE delivery deferred) → unimplementedFeature or infeasible naming secrets", isDeferred(sec, "jobs.submit") || sec?.code === "infeasible", describe(sec));
    const svc = await errOf(submit(win, ctx.project.id, "A service", jobSpec({ kind: { type: "service" } })));
    check("C9", "Service jobs → unimplementedFeature", isDeferred(svc, "jobs.submit"), describe(svc));
    // preemption: fill the gpu's slots, then a Critical job that only fits there
    const fill = [];
    for (let i = 0; i < 2; i++) fill.push(await submit(win, ctx.project.id, `Filler ${i}`, jobSpec({ kind: longCommand(200, 250), requirements: { os: "Linux" } })));
    const crit = await errOf(submit(win, ctx.project.id, "Critical GPU job", jobSpec({ kind: command("node", ["--version"]), requirements: { os: "Linux" } }), { priority: "Critical" }).then((r) => (r.job.placement?.decision === "wait" ? Promise.reject(Object.assign(new Error(explanationOf(r.job)), { code: "wait" })) : r)));
    check("C9", "critical-job preemption deferred → unimplementedFeature or waits (no running job preempted)", isDeferred(crit, "jobs.submit") || crit?.code === "wait", describe(crit));
    for (const f of fill) await soft(win.rpc.call("jobs.cancel", { jobId: f.job.spec.jobId, immediate: true }));
  });
} catch (e) {
  exitCode = 1;
  check("Run", "scenario ran to the end", false, e.message);
  console.error(e.stack ?? e);
} finally {
  const failed = checks.filter((c) => !c.ok);
  if (failed.length && process.env.ORKESTR_P6_LOGS !== "0") for (const p of procs) console.error(`--- ${p.label} log tail ---\n${p.log.slice(-1500)}`);
  procs.forEach((p, i) => writeFileSync(join(outDir, `${p.label}-${i}.log`), p.log));
  writeFileSync(join(outDir, "acceptance.json"), `${JSON.stringify({ phase: 6, startedFrom: repo, checks }, null, 2)}\n`);
  console.log(`\n${checks.length - failed.length}/${checks.length} checks passed  (report: ${join(outDir, "acceptance.json")})`);
  if (failed.length || checks.length === 0) exitCode = 1;
  for (const c of [ctx.win, ctx.mac, ctx.gpu]) c?.rpc.close();
  for (const p of procs) kill(p);
  proxy.close();
  await sleep(1500);
  try {
    rmSync(tmp, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  } catch {
    /* ignore */
  }
  process.exit(exitCode);
}
