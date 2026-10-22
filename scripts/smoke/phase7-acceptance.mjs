#!/usr/bin/env node
// Phase 7 (Ecosystem) acceptance run — ADR-0034 / ADR-0040 / ADR-0041 / ADR-0042.
//
// Setup: one self-hosted `orkestr-gateway` (temp data dir; serves the plugin
// registry at /registry and the account / tunnel for the remote viewer), the
// fake cloud API (scripts/smoke/fixtures/fake-cloud-api.mjs) and one headless
// `orkestrd` (temp ORKESTR_HOME, ORKESTR_PLUGINS_TRUSTED_KEYS = the dev
// publisher key of the plugin fixtures, ORKESTR_{VERCEL,NETLIFY,FLY,KUBERNETES}_API
// = the fake API, offline Mock provider routing). The project is a git copy of
// fixtures/phase7/greeter-service (tiny Node HTTP service + node:test suite).
// Waits are condition waits (poll RPC / events), never fixed sleeps.
//
//   E1 setup        gateway + account (passkey + TOTP), fake cloud API, orkestrd,
//                   plugin fixtures, greeter project (git) + Mock routing
//   E2 plugins      hello-tool install → pendingGrants → grant → plugins.invoke
//                   + agent tool call (Mock); tampered / unsigned / traversal
//                   refused; undeclared host denied + PluginCallDenied + audit;
//                   loop trapped < 1 s; memory capped; killAll → Killed + invoke
//                   refused; safe mode survives an orkestrd restart
//   E3 marketplace  publish template / widget / plugin to the gateway registry,
//                   search, install template → templates.list, widget install;
//                   unknown publisher needs trustKey; key change on update
//                   refused; revoked entry refused; tampered index → lastError
//   E4 widgets      widgets.list has the plugin widget; write / local_only
//                   bindings hidden + bundle forbidden + dashboard refused;
//                   dashboards save/get global + project; bundle carries CSP
//   E5 deployments  StaticSite → healthy + DeploymentFinished Succeeded;
//                   LocalProcess healthy; Production → approval → deployed;
//                   Vercel / Netlify / Fly / Kubernetes vs the fake API (no
//                   token in logs, NeedsCredential without a credential);
//                   Docker runs or is skipped with a reason; rollback
//   E6 monitor      overview CPU / RAM / disk; HTTP check down → up with
//                   HealthCheckChanged + AlertRaised / AlertResolved; ingest +
//                   redacted logs
//   E7 detective    broken greeter deployed → check alert → detective.open from
//                   the alert → evidence (stack trace, deployed version, commit)
//                   → hypothesis → reproduction fails → approveFix → tests pass
//                   on detective/<id>, review → AwaitingMergeApproval, main
//                   untouched
//   E8 security     emergency stop kills plugins, cancels a deploy and a
//                   detective run; a remote viewer is refused plugins.install
//                   and deploy.start; audit records
//   E9 deferred     AWS adapter, JS plugins, Panel kind → unimplementedFeature
//
// Usage (repo root, Windows):
//   cargo build -p orkestr-core -p orkestr-gateway --bins
//   cargo build -p orkestr-plugins --example make-fixtures
//   node scripts/smoke/phase7-acceptance.mjs [--out <dir>] [--only E2,E5]
// E1 (setup) always runs. Env: ORKESTR_SMOKE_BIN_DIR (default target/debug),
//   ORKESTR_P7_PLUGIN_FIXTURES (dir with prebuilt plugin fixtures; else the
//   make-fixtures example binary next to the bins, else `cargo run`),
//   ORKESTR_P7_PORT_BASE (default 18960; ports base..base+9 are used),
//   ORKESTR_P7_TIMEOUT_SCALE (1), ORKESTR_P7_DOCKER=1 (really deploy to Docker
//   when it is available), ORKESTR_P7_VERBOSE, ORKESTR_P7_LOGS=0.
// Writes <out>/acceptance.json (default: %TEMP%/orkestr-phase7-acceptance).

import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { createInterface } from "node:readline";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { buildPackage, devKey, randomKey, sign, WIDGET_HTML, widgetManifest } from "./fixtures/phase7/orkp.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const fx = join(here, "fixtures", "phase7");
const exe = (n) => (process.platform === "win32" ? `${n}.exe` : n);
const exeDir = process.env.ORKESTR_SMOKE_BIN_DIR ?? join(repo, "target", "debug");
const coreExe = join(exeDir, exe("orkestrd"));
const gwExe = join(exeDir, exe("orkestr-gateway"));
// Avoid the Windows excluded TCP range 1349–1548.
const PORT_BASE = Number(process.env.ORKESTR_P7_PORT_BASE ?? 18960);
const PORTS = {
  gw: PORT_BASE,
  site: PORT_BASE + 1, // StaticSite (Local)
  prod: PORT_BASE + 2, // StaticSite (Production)
  svc: PORT_BASE + 3, // greeter LocalProcess
  flappy: PORT_BASE + 4, // E6 check target
  hang: PORT_BASE + 5, // E8 never-healthy deploy (nothing listens)
  docker: PORT_BASE + 6,
};
const base = `http://localhost:${PORTS.gw}`;
const SCALE = Number(process.env.ORKESTR_P7_TIMEOUT_SCALE ?? 1);
const T = (ms) => Math.round(ms * SCALE);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const tmp = mkdtempSync(join(tmpdir(), "orkestr-p7acc-"));
const argOf = (name) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : undefined;
};
const outDir = argOf("--out") ? resolve(repo, argOf("--out")) : join(tmpdir(), "orkestr-phase7-acceptance");
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
async function runSection(id, title, fn, { always = false } = {}) {
  if (!always && only && !only.includes(id)) return;
  section(`${id}. ${title}`);
  try {
    await fn();
  } catch (e) {
    check(id, "section ran to the end", false, describe(e));
    if (process.env.ORKESTR_P7_VERBOSE) console.error(e.stack ?? e);
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
/** A real refusal (an error that is not "this method is still a stub"). */
const refused = (e) => !!e && !isUnimplemented(e);
const text = (v) => JSON.stringify(v ?? null);

// ── processes ─────────────────────────────────────────────────────────────
const procs = [];
function launch(cmd, args, env, label, opts = {}) {
  const child = spawn(cmd, args, { env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"], ...opts });
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

// ── local RPC (the desktop side of the core) ──────────────────────────────
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
        ws.onclose = () => {
          for (const [, p] of pending) p.reject(Object.assign(new Error("connection closed"), { code: "closed" }));
          pending.clear();
        };
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

/** Starts (or restarts, same ORKESTR_HOME) the orkestrd under test. */
async function startCore() {
  const home = join(tmp, "core");
  mkdirSync(home, { recursive: true });
  rmSync(join(home, "core.json"), { force: true });
  const env = {
    ORKESTR_HOME: home,
    ORKESTR_SECRET_BACKEND: "vault-file",
    ORKESTR_PLUGINS_TRUSTED_KEYS: ctx.devPub,
    ORKESTR_PLUGINS_DEV: "",
    ORKESTR_VERCEL_API: ctx.cloudBase,
    ORKESTR_NETLIFY_API: ctx.cloudBase,
    ORKESTR_FLY_API: ctx.cloudBase,
    ORKESTR_KUBERNETES_API: ctx.cloudBase,
    RUST_LOG: "info",
  };
  const rec = launch(coreExe, ["--data-dir", home], env, "orkestrd");
  const coreInfo = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 60_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(coreInfo.url, token);
  await waitFor(() => rpc.open().then(() => true), 30_000, "core RPC");
  await rpc.sub(["*"]);
  ctx.core = { home, rec, rpc, url: coreInfo.url };
  return ctx.core;
}
async function stopCore() {
  const c = ctx.core;
  c.rpc.close();
  kill(c.rec);
  await waitFor(() => c.rec.exited, 15_000, "orkestrd exit").catch(() => null);
}
const R = () => ctx.core.rpc;
const call = (m, p) => R().call(m, p);

// ── gateway HTTP + account bootstrap (as phase 5/6) ───────────────────────
async function gwFetch(path, { token, method = "GET", body } = {}) {
  const res = await fetch(`${base}${path}`, {
    method,
    headers: { ...(token ? { authorization: `Bearer ${token}` } : {}), ...(body ? { "content-type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  const t = await res.text();
  let json = null;
  try {
    json = JSON.parse(t);
  } catch {
    /* not json */
  }
  return { status: res.status, json, text: t };
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

/** Enrolls the core with the gateway (remote.enable) and waits for its tunnel. */
async function tunnel() {
  const t = (await gwFetch("/v1/cores/enrollment-token", { token: ctx.ownerToken, method: "POST" })).json.token;
  await call("remote.enable", { gatewayUrl: base, enrollmentToken: t });
  return waitFor(async () => {
    const st = await call("remote.status");
    return st.connected ? st : null;
  }, 20_000, "tunnel connected");
}

/** `@orkestr/client` (remote E2E module) bundled for Node with rolldown, as phase 4. */
async function bundleClient() {
  const roots = [repo];
  try {
    const common = execFileSync("git", ["rev-parse", "--path-format=absolute", "--git-common-dir"], { cwd: repo, encoding: "utf8" }).trim();
    roots.push(dirname(common));
  } catch {
    /* not a worktree */
  }
  const root = roots.find((r) => existsSync(join(r, "node_modules", "rolldown", "bin", "cli.mjs")) && existsSync(join(r, "packages", "client", "src", "index.ts")));
  if (!root) throw new Error("rolldown / packages/client not found (run npm install in the repo)");
  const out = join(tmp, "client-bundle.mjs");
  execFileSync(process.execPath, [join(root, "node_modules", "rolldown", "bin", "cli.mjs"), join(root, "packages", "client", "src", "index.ts"), "--format", "esm", "--platform", "node", "-o", out], { cwd: root, stdio: "ignore" });
  return import(pathToFileURL(out).href);
}

// ── Phase 7 helpers ───────────────────────────────────────────────────────
const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();

/** The greeter service fixture as a git repository (one commit on main). */
function makeGreeterRepo() {
  const dir = join(tmp, "greeter");
  cpSync(join(fx, "greeter-service"), dir, { recursive: true });
  git(dir, "init", "-q", "-b", "main");
  git(dir, "config", "core.autocrlf", "false");
  git(dir, "config", "user.email", "p7@orkestr.local");
  git(dir, "config", "user.name", "Orkestr P7");
  git(dir, "add", "-A");
  git(dir, "commit", "-q", "-m", "greeter service v1");
  writeFileSync(join(dir, ".git", "info", "exclude"), ".p7-deploy/\n");
  return dir;
}

/** Plugin fixtures from the Rust generator (prebuilt dir → example binary → cargo run). */
function pluginFixtures() {
  const pre = process.env.ORKESTR_P7_PLUGIN_FIXTURES;
  if (pre && existsSync(join(pre, "fixtures.json"))) return pre;
  const dir = join(tmp, "plugin-fixtures");
  const bin = join(exeDir, "examples", exe("make-fixtures"));
  if (existsSync(bin)) execFileSync(bin, [dir], { stdio: "ignore" });
  else execFileSync("cargo", ["run", "-q", "-p", "orkestr-plugins", "--example", "make-fixtures", "--", dir], { cwd: repo, stdio: "ignore" });
  return dir;
}
const fixture = (f) => join(ctx.fixtures, f);

const pluginsList = async () => (await call("plugins.list", {})).plugins ?? [];
const pluginByPackage = async (pkg) => (await pluginsList()).find((p) => p.plugin.packageId === pkg);
/** Installs (or finds) a plugin package, grants its requested permissions and enables it. */
async function installAndEnable(params, pkg) {
  let info = pkg ? await pluginByPackage(pkg) : null;
  let pending = [];
  if (!info) {
    const r = await call("plugins.install", params);
    info = r.plugin;
    pending = r.pendingGrants ?? [];
  }
  const id = info.plugin.id;
  if (info.plugin.state !== "Enabled") {
    const decisions = (pending.length ? pending : info.pendingGrants ?? []).map((g) => ({ capability: g.capability, scope: g.scope, allow: true }));
    info = (await call("plugins.grant", { pluginId: id, decisions, enable: true })).plugin;
    if (info.plugin.state !== "Enabled") info = (await call("plugins.enable", { pluginId: id })).plugin;
  }
  return info;
}
const HELLO = "orkestr-dev.hello-tool";
const HELLO_WIDGET = "orkestr-dev.hello-widget";
const HELLO_TOOL_NAME = "plugin_orkestr_dev_hello_tool__hello";
async function ensureHello() {
  const p = await installAndEnable({ path: fixture("hello-tool.orkp") }, HELLO);
  ctx.helloId = p.plugin.id;
  return p;
}

async function auditActions() {
  const r = await soft(call("audit.list", { limit: 5000 }));
  return (r?.entries ?? []).map((e) => ({ action: e.event?.action ?? e.action ?? "", details: JSON.stringify(e.event?.details ?? e.details ?? {}), resource: String(e.event?.resource ?? e.resource ?? "") }));
}

/** Approves pending approvals for the project that belong to agents (never a detective / deploy gate). */
async function approveAgentApprovals(skip = new Set()) {
  const pend = (await soft(call("approvals.list", { status: "Pending", projectId: ctx.project.id })))?.approvals ?? [];
  for (const a of pend) {
    if (skip.has(a.id) || !a.agentId || /^Bug Detective/.test(a.actionSummary ?? "")) continue;
    await soft(call("approvals.resolve", { approvalId: a.id, decision: "Approve", note: "phase 7 acceptance" }));
  }
}

// deployments
const FINAL = new Set(["Succeeded", "Failed", "RolledBack", "Cancelled"]);
async function createEnv(name, kind, target, adapterSettings, extra = {}) {
  return (await call("environments.create", { projectId: ctx.project.id, name, kind, target, adapterSettings, ...extra })).environment;
}
const deployGet = async (id) => (await call("deploy.get", { deploymentId: id })).deployment;
async function waitDeploy(id, pred = (d) => FINAL.has(d.status), ms = 90_000, label = "final") {
  return waitFor(async () => {
    const d = await deployGet(id);
    return pred(d) ? d : null;
  }, ms, `deployment ${String(id).slice(0, 8)} ${label}`);
}
async function deployLogs(id) {
  const r = await soft(call("deploy.logs", { deploymentId: id }));
  return (r?.lines ?? []).join("\n");
}
const deployStart = (environmentId, version, sourceDir) => call("deploy.start", { environmentId, version, ...(sourceDir ? { sourceDir } : {}) });
async function httpText(url) {
  try {
    const r = await fetch(url);
    return { status: r.status, body: await r.text() };
  } catch (e) {
    return { status: 0, body: String(e.message ?? e) };
  }
}
/** Deploy sources must live under the project root: <repo>/.p7-deploy/<name> (excluded from git). */
function deployDir(name) {
  const d = join(ctx.repo, ".p7-deploy", name);
  mkdirSync(d, { recursive: true });
  return d;
}
function siteDir(name, content) {
  const d = deployDir(name);
  mkdirSync(join(d, "dist"), { recursive: true });
  writeFileSync(join(d, "dist", "index.html"), content);
  return d;
}

/** Staging LocalProcess environment of the greeter service (health URL "/"). */
async function ensureServiceEnv() {
  if (ctx.svcEnv) return ctx.svcEnv;
  ctx.svcEnv = await createEnv("greeter-staging", "Staging", "LocalProcess", {
    command: "node src/server.mjs",
    workdir: ".",
    port: String(PORTS.svc),
    healthUrl: `http://127.0.0.1:${PORTS.svc}/`,
    healthTimeoutSecs: "30",
  });
  return ctx.svcEnv;
}
/** Deploys the greeter repo's working tree as `version`. */
async function deployService(version) {
  const env = await ensureServiceEnv();
  const r = await deployStart(env.environment.id, version, ctx.repo);
  return waitDeploy(r.deployment.id);
}

// monitor
async function waitCheckState(checkId, state, ms = 20_000) {
  return waitFor(async () => {
    const r = (await call("monitor.checkRun", { checkId })).check;
    return r.state === state ? r : null;
  }, ms, `check ${String(checkId).slice(0, 8)} → ${state}`);
}

// detective
const incidentGet = async (id) => (await call("detective.get", { incidentId: id })).incident;
async function waitIncident(id, stages, ms, { approveAgents = false } = {}) {
  const want = [].concat(stages);
  return waitFor(async () => {
    if (approveAgents) await approveAgentApprovals();
    const i = await incidentGet(id);
    if (want.includes(i.stage)) return i;
    if (["Resolved", "Failed", "Cancelled"].includes(i.stage)) throw Object.assign(new Error(`incident ended ${i.stage}: ${i.error ?? ""}`), { code: "ended", incident: i });
    return null;
  }, ms, `incident → ${want.join("|")}`);
}

// ══════════════════════════════════════════════════════════════════════════
let exitCode = 0;
const ctx = {}; // shared state between sections

try {
  for (const [p, what] of [
    [coreExe, "orkestrd"],
    [gwExe, "orkestr-gateway"],
  ]) {
    if (!existsSync(p)) throw new Error(`missing ${what} (${p}) — build first or set ORKESTR_SMOKE_BIN_DIR`);
  }

  // ════════════════════════════════════════════════════════════════════════
  await runSection(
    "E1",
    "setup: gateway, fake cloud API, orkestrd, plugin fixtures, greeter project",
    async () => {
      launch(gwExe, ["--data-dir", join(tmp, "gateway"), "--bind", `127.0.0.1:${PORTS.gw}`], { RUST_LOG: "info" }, "gateway");
      await waitFor(async () => (await fetch(`${base}/healthz`)).ok, 30_000, "gateway /healthz");
      const idx = await gwFetch("/registry/index.json");
      check("E1", "gateway up; registry index served at /registry/index.json", idx.status === 200 && Array.isArray(idx.json?.entries), `${base} → ${idx.status}`);
      const acct = await bootstrapAccount();
      ctx.ownerToken = acct.token;
      check("E1", "gateway account: passkey (software authenticator) + TOTP", acct.ok, acct.detail);

      ctx.cloudToken = `fake-cloud-token-${crypto.randomBytes(12).toString("hex")}`;
      const cloud = launch(process.execPath, [join(here, "fixtures", "fake-cloud-api.mjs"), "--port", "0", "--token", ctx.cloudToken], {}, "fake-cloud-api");
      const first = await new Promise((res, rej) => {
        const rl = createInterface({ input: cloud.child.stdout });
        rl.once("line", (l) => res(l));
        cloud.child.once("exit", () => rej(new Error("fake cloud API exited")));
      });
      ctx.cloudBase = JSON.parse(first).base;
      check("E1", "fake cloud API up (Vercel / Netlify / Fly / Kubernetes shapes)", !!ctx.cloudBase, ctx.cloudBase);

      ctx.fixtures = pluginFixtures();
      const fxIndex = JSON.parse(readFileSync(fixture("fixtures.json"), "utf8"));
      ctx.devPub = readFileSync(fixture("dev-publisher.pub"), "utf8").trim();
      check("E1", "plugin fixtures generated (hello-tool, hello-widget, evil-*) + dev publisher key", fxIndex.fixtures.length >= 8 && ctx.devPub === devKey().publicB64, `${fxIndex.fixtures.map((f) => f.file).join(", ")}; key ${fxIndex.keyId}`);

      await startCore();
      check("E1", "orkestrd started (temp ORKESTR_HOME, trusted dev key, cloud APIs → fake)", true, ctx.core.url);

      await call("settings.update", { patch: { profileName: "P7", defaultApprovalMode: "SemiAuto", onboardingCompleted: true } });
      const mock = (await call("providers.list")).providers.map((p) => p.provider ?? p).find((p) => p.kind === "Mock");
      ctx.mockId = mock.id;
      ctx.smart = { providerId: mock.id, model: "mock-smart" };
      await soft(call("providers.setManager", { config: { model: ctx.smart, maxParallelAgents: 2, approvalPolicy: "SemiAuto" } }));
      await call("providers.setRouting", { config: { mode: "Manual", rules: [], defaultFallback: [ctx.smart], preferLocal: true, maxRateLimitWaitSecs: 0 } });
      ctx.repo = makeGreeterRepo();
      ctx.project = (await call("projects.create", { name: "Greeter Service", rootPath: ctx.repo, approvalMode: "SemiAuto" })).project;
      const t = execFileSync(process.execPath, ["--test"], { cwd: ctx.repo, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
      check("E1", "greeter project (git, Node HTTP service + node:test suite passing) + Mock routing", !!ctx.project && /pass 2/.test(t), `${ctx.project?.id}; ${git(ctx.repo, "log", "--oneline")}`);
    },
    { always: true },
  );
  if (!ctx.core || !ctx.project) throw new Error("setup (E1) did not complete");

  // ════════════════════════════════════════════════════════════════════════
  await runSection("E2", "Plugin SDK: signed packages, grants, sandbox, kill switch, safe mode", async () => {
    const inst = await call("plugins.install", { path: fixture("hello-tool.orkp") });
    const pend = inst.pendingGrants ?? [];
    ctx.helloId = inst.plugin.plugin.id;
    check("E2", "hello-tool installs (signature trusted) with a pending Network grant", inst.plugin.plugin.packageId === HELLO && pend.some((g) => g.capability === "Network" && g.scope === "example.com") && inst.plugin.plugin.state !== "Enabled", `${inst.plugin.plugin.state}; pending ${text(pend)}; signer ${inst.plugin.signerFingerprint ?? "?"}`);
    const installedEv = await R().waitEvent((e) => e.kind === "PluginInstalled" && e.payload.plugin?.plugin?.id === ctx.helloId, 5000, "PluginInstalled").catch(() => null);
    check("E2", "PluginInstalled event", !!installedEv);
    const preInvoke = await errOf(call("plugins.invoke", { pluginId: ctx.helloId, tool: "hello", input: { name: "Ada" } }));
    check("E2", "invoke before the grant is refused", refused(preInvoke), describe(preInvoke));

    const granted = (await call("plugins.grant", { pluginId: ctx.helloId, decisions: pend.map((g) => ({ capability: g.capability, scope: g.scope, allow: true })), enable: true })).plugin;
    check("E2", "plugins.grant (Network example.com) + enable → Enabled, grants recorded", granted.plugin.state === "Enabled" && granted.grants.some((g) => g.capability === "Network" && g.allowed) && granted.pendingGrants.length === 0, `${granted.plugin.state}; ${text(granted.grants)}`);

    const inv = await call("plugins.invoke", { pluginId: ctx.helloId, tool: "hello", input: { name: "Ada" }, projectId: ctx.project.id });
    const logs = (await call("plugins.logs", { pluginId: ctx.helloId, limit: 50 })).lines ?? [];
    check("E2", "plugins.invoke hello → echo of the input; host Log call lands in plugins.logs", text(inv.output).includes("Ada") && logs.some((l) => /hello called/.test(l.message)), `${text(inv.output)}; fuel ${inv.fuelUsed}, ${inv.durationMs} ms; logs ${logs.length}`);

    // agent tool call through the offline Mock (`mock-tool-roundtrip` calls the first offered tool)
    const backend = (await call("templates.list", {})).templates.find((t) => t.role === "Backend");
    const exported = await call("templates.export", { templateId: backend.id });
    const imported = (await call("templates.import", { json: exported.json })).template;
    const draft = { ...imported };
    for (const k of ["id", "source", "createdAt", "updatedAt"]) delete draft[k];
    Object.assign(draft, { name: "Plugin tool caller", tools: [], qualityGates: [], worktreePolicy: "SharedWorktree" });
    await call("templates.update", { templateId: imported.id, template: draft });
    const spawned = await call("agents.spawn", { projectId: ctx.project.id, templateId: imported.id, newTask: { title: "Greet Ada with the hello plugin tool" }, model: { providerId: ctx.mockId, model: "mock-tool-roundtrip" } });
    const agentId = spawned.agent.id;
    let lastTr = null;
    const toolCall = await waitFor(async () => {
      await approveAgentApprovals();
      const tr = await call("agents.transcript", { agentId });
      lastTr = tr;
      return tr.toolCalls.find((c) => c.toolCall.tool === HELLO_TOOL_NAME && ["Succeeded", "Failed", "Denied"].includes(c.toolCall.status));
    }, 60_000, "agent plugin tool call").catch((e) => ({ error: e }));
    await soft(call("agents.stop", { agentId }));
    check("E2", `agent (Mock) calls ${HELLO_TOOL_NAME}; the plugin runs`, toolCall?.toolCall?.status === "Succeeded", toolCall?.error ? `${describe(toolCall.error)}; agent ${(await soft(call("agents.get", { agentId })))?.agent?.state}; calls ${(lastTr?.toolCalls ?? []).map((c) => `${c.toolCall.tool}:${c.toolCall.status}`).join(",")}; last msg ${String(lastTr?.messages?.at(-1)?.content ?? "").slice(0, 120)}` : `${toolCall.toolCall.status}: ${toolCall.toolCall.summary ?? ""}`);

    // refused packages
    for (const [file, why] of [
      ["evil-tampered.orkp", "tampered (content changed after signing)"],
      ["evil-unsigned.orkp", "unsigned (ORKESTR_PLUGINS_DEV not set)"],
      ["evil-traversal.orkp", "path traversal entry"],
    ]) {
      const e = await errOf(call("plugins.install", { path: fixture(file) }));
      check("E2", `${why} package refused`, refused(e), describe(e));
    }
    const notInstalled = (await pluginsList()).filter((p) => /tampered|unsigned|traversal/.test(p.plugin.packageId));
    check("E2", "refused packages are not installed", notInstalled.length === 0, notInstalled.map((p) => p.plugin.packageId).join(","));

    // undeclared network host
    const net = await installAndEnable({ path: fixture("evil-network.orkp") }, "orkestr-dev.evil-network");
    const netRes = await call("plugins.invoke", { pluginId: net.plugin.id, tool: "steal", input: {} }).then((r) => ({ r }), (e) => ({ e }));
    const netMsg = netRes.e ? describe(netRes.e) : text(netRes.r.output);
    const denied = await R().waitEvent((e) => e.kind === "PluginCallDenied" && e.payload.pluginId === net.plugin.id, 5000, "PluginCallDenied").catch(() => null);
    const netAudit = (await auditActions()).filter((a) => /plugin/i.test(a.action + a.resource) && /den(y|ied)|not granted|evil\.example\.com/i.test(a.action + a.details));
    check("E2", "undeclared host (evil.example.com) denied, PluginCallDenied event, audited", /not granted/i.test(netMsg) && !!denied && netAudit.length > 0, `${netMsg.slice(0, 120)}; event ${denied ? denied.payload.op + " " + (denied.payload.target ?? "") : "missing"}; audit ${netAudit.map((a) => a.action).join(",")}`);

    // runaway code
    const loop = await installAndEnable({ path: fixture("evil-loop.orkp") }, "orkestr-dev.evil-loop");
    const t0 = Date.now();
    const loopErr = await errOf(call("plugins.invoke", { pluginId: loop.plugin.id, tool: "spin", input: {} }));
    const loopMs = Date.now() - t0;
    check("E2", "infinite loop trapped by the 500 ms deadline in < 1 s", /timed out/i.test(describe(loopErr)) && loopMs < T(1500), `${loopMs} ms: ${describe(loopErr)}`);
    const mem = await installAndEnable({ path: fixture("evil-memory.orkp") }, "orkestr-dev.evil-memory");
    const memErr = await errOf(call("plugins.invoke", { pluginId: mem.plugin.id, tool: "bomb", input: {} }));
    check("E2", "memory bomb capped at 16 MiB", /memory/i.test(describe(memErr)), describe(memErr));

    // kill switch
    const killed = await call("plugins.killAll", { reason: "phase 7 acceptance" });
    const killEv = await R().waitEvent((e) => e.kind === "PluginStateChanged" && e.payload.pluginId === ctx.helloId && e.payload.state === "Killed", 5000, "PluginStateChanged Killed").catch(() => null);
    const afterKill = await errOf(call("plugins.invoke", { pluginId: ctx.helloId, tool: "hello", input: { name: "Ada" } }));
    check("E2", "plugins.killAll → PluginStateChanged(Killed); invoke refused", killed.killed >= 1 && !!killEv && refused(afterKill), `killed ${killed.killed}, interrupted ${killed.callsInterrupted}; ${describe(afterKill)}`);

    // safe mode survives a restart
    await call("plugins.setSafeMode", { enabled: true });
    await stopCore();
    await startCore();
    const listed = await call("plugins.list", {});
    const smErr = await errOf(call("plugins.invoke", { pluginId: ctx.helloId, tool: "hello", input: { name: "Ada" } }));
    const enableInSafe = await errOf(call("plugins.enable", { pluginId: ctx.helloId }));
    check("E2", "safe mode survives an orkestrd restart; plugin calls refused in safe mode", listed.safeMode === true && refused(smErr), `safeMode ${listed.safeMode}; invoke: ${describe(smErr)}; enable: ${describe(enableInSafe) || "ok"}`);
    await call("plugins.setSafeMode", { enabled: false });
    const re = await ensureHello();
    const ok2 = await soft(call("plugins.invoke", { pluginId: ctx.helloId, tool: "hello", input: { name: "again" } }));
    check("E2", "safe mode off + re-enable → hello-tool works again", re.plugin.state === "Enabled" && text(ok2?.output).includes("again"), `${re.plugin.state}; ${text(ok2?.output)}`);
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("E3", "Marketplace: gateway registry publish / search / install, trust, revocation, tampering", async () => {
    const src = (await call("marketplace.addSource", { kind: "Http", location: `${base}/registry`, name: "Team registry", token: ctx.ownerToken })).source;
    ctx.srcId = src.id;
    check("E3", "gateway registry added as an Http source (index verified, registry key pinned)", src.kind === "Http" && !src.lastError, `${src.id} ${src.lastError ?? ""}`);

    // publish template, widget, plugin (vault-held local publisher key)
    const tplData = JSON.parse(readFileSync(join(repo, "crates", "agent-runtime", "templates", "reviewer.json"), "utf8"));
    tplData.origin = null;
    tplData.template.name = "P7 Release Reviewer";
    const tplFile = join(tmp, "p7-release-reviewer.agent.json");
    writeFileSync(tplFile, JSON.stringify(tplData, null, 2));
    const pubT = (await call("marketplace.publish", { path: tplFile, kind: "AgentTemplate", sourceId: ctx.srcId, publisher: "p7" })).entry;
    const pubW = (await call("marketplace.publish", { path: fixture("hello-widget.orkp"), kind: "Widget", sourceId: ctx.srcId, publisher: "Orkestr Dev" })).entry;
    const pubP = (await call("marketplace.publish", { path: fixture("hello-tool.orkp"), kind: "Plugin", sourceId: ctx.srcId, publisher: "Orkestr Dev" })).entry;
    const gwIndex = (await gwFetch("/registry/index.json")).json;
    check("E3", "template, widget and plugin published to the gateway registry (POST /registry/publish)", [pubT, pubW, pubP].every((e) => gwIndex.entries.some((x) => x.id === e.id && x.sha256 === e.sha256)), [pubT, pubW, pubP].map((e) => `${e.kind}:${e.id}@${e.version}`).join(", "));
    await call("marketplace.refresh", { sourceId: ctx.srcId });
    const all = (await call("marketplace.search", { sourceId: ctx.srcId })).entries;
    const q = (await call("marketplace.search", { query: "release reviewer" })).entries;
    check("E3", "marketplace.search finds the three entries (and by text query)", [pubT, pubW, pubP].every((e) => all.some((x) => x.id === e.id)) && q.some((x) => x.id === pubT.id), `${all.length} in source; query → ${q.map((x) => x.id).join(",")}`);

    const insT = await call("marketplace.install", { id: pubT.id, sourceId: ctx.srcId });
    const tpls = (await call("templates.list", {})).templates;
    check("E3", "install template → appears in templates.list", insT.kind === "AgentTemplate" && insT.templateIds.length > 0 && tpls.some((t) => insT.templateIds.includes(t.id) && t.name === "P7 Release Reviewer"), insT.templateIds.join(","));
    const insW = await call("marketplace.install", { id: pubW.id, sourceId: ctx.srcId });
    const wInfo = await installAndEnable({}, HELLO_WIDGET);
    check("E3", "install widget package → plugin installed (no grants) and enabled", insW.plugin?.plugin?.packageId === HELLO_WIDGET && wInfo.plugin.state === "Enabled", `${insW.kind}; ${wInfo.plugin.state}`);

    // unknown publisher: POST straight to the gateway with a Node ed25519 key
    const tpl3 = JSON.parse(JSON.stringify(tplData));
    tpl3.template.name = "Third Party Reviewer";
    const pkg3 = Buffer.from(JSON.stringify(tpl3, null, 2));
    const k1 = randomKey();
    const postEntry = async (key, version, bytes) =>
      gwFetch("/registry/publish", {
        token: ctx.ownerToken,
        method: "POST",
        body: {
          entry: { id: "thirdparty.p7-reviewer", kind: "AgentTemplate", name: "Third Party Reviewer", version, publisher: "thirdparty", description: "Reviewer by an unknown publisher", tags: ["review"], permissions: [] },
          publicKey: key.publicB64,
          signature: sign(key, crypto.createHash("sha256").update(bytes).digest()),
          packageBase64: bytes.toString("base64"),
        },
      });
    const p1 = await postEntry(k1, "1.0.0", pkg3);
    await call("marketplace.refresh", { sourceId: ctx.srcId });
    const untrusted = await errOf(call("marketplace.install", { id: "thirdparty.p7-reviewer", sourceId: ctx.srcId }));
    const det = await call("marketplace.get", { id: "thirdparty.p7-reviewer", sourceId: ctx.srcId });
    check("E3", "unknown publisher key: install refused; marketplace.get shows key Untrusted + fingerprint", p1.status === 200 && untrusted?.code === "forbidden" && det.key.trust === "Untrusted" && det.key.fingerprint.replace(/[^0-9a-f]/gi, "").toLowerCase() === k1.fingerprint, `publish ${p1.status}; install ${describe(untrusted)}; key ${det.key.trust} ${det.key.fingerprint.slice(0, 16)}…`);
    await call("marketplace.trustKey", { keyId: det.key.keyId, fingerprint: det.key.fingerprint });
    const ins3 = await call("marketplace.install", { id: "thirdparty.p7-reviewer", sourceId: ctx.srcId });
    check("E3", "trustKey (fingerprint confirmed) → install ok → templates.list", (await call("templates.list", {})).templates.some((t) => ins3.templateIds.includes(t.id)), ins3.templateIds.join(","));

    // key change on update
    const k2 = randomKey();
    tpl3.template.description = "v1.1";
    const p2 = await postEntry(k2, "1.1.0", Buffer.from(JSON.stringify(tpl3, null, 2)));
    await call("marketplace.refresh", { sourceId: ctx.srcId });
    const k2info = (await call("marketplace.keys", {})).keys.find((k) => k.keyId === k2.keyId);
    if (k2info) await soft(call("marketplace.trustKey", { keyId: k2info.keyId, fingerprint: k2info.fingerprint }));
    const keyChange = await errOf(call("marketplace.install", { id: "thirdparty.p7-reviewer", version: "1.1.0", sourceId: ctx.srcId }));
    const stillV1 = (await call("marketplace.installed", {})).packages.find((p) => p.id === "thirdparty.p7-reviewer");
    check("E3", "update signed by a different key refused (per-package key pin), even with the new key trusted", p2.status === 200 && refused(keyChange) && stillV1?.version === "1.0.0", `${describe(keyChange)}; installed ${stillV1?.version}`);

    // revocation
    const rv = await gwFetch("/registry/revoke", { token: ctx.ownerToken, method: "POST", body: { id: "thirdparty.p7-reviewer" } });
    await call("marketplace.refresh", { sourceId: ctx.srcId });
    const revokedErr = await errOf(call("marketplace.install", { id: "thirdparty.p7-reviewer", version: "1.0.0", sourceId: ctx.srcId }));
    const instRev = (await call("marketplace.installed", {})).packages.find((p) => p.id === "thirdparty.p7-reviewer");
    check("E3", "revoked entry: install refused; installed package flagged revoked", rv.status === 200 && refused(revokedErr) && instRev?.revoked === true, `revoke ${rv.status}; ${describe(revokedErr)}; installed.revoked ${instRev?.revoked}`);

    // tampered index
    const idxPath = join(tmp, "gateway", "registry", "index.json");
    const raw = readFileSync(idxPath, "utf8");
    writeFileSync(idxPath, raw.replace("P7 Release Reviewer", "P7 Release Reviewer (tampered)"));
    const refreshed = (await call("marketplace.refresh", { sourceId: ctx.srcId })).sources.find((s) => s.id === ctx.srcId);
    const cached = (await call("marketplace.search", { sourceId: ctx.srcId })).entries;
    check("E3", "tampered index.json → source lastError, last verified entries kept", !!refreshed?.lastError && cached.some((e) => e.id === pubT.id && e.name === "P7 Release Reviewer"), `${refreshed?.lastError}; ${cached.length} cached`);
    writeFileSync(idxPath, raw);
    await soft(call("marketplace.refresh", { sourceId: ctx.srcId }));
    const updEv = R().events.some((e) => e.kind === "MarketplaceUpdated");
    check("E3", "MarketplaceUpdated events", updEv);
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("E4", "Widget SDK + Dashboard Builder", async () => {
    await ensureHello();
    await installAndEnable({ path: fixture("hello-widget.orkp") }, HELLO_WIDGET);
    const bad = [
      ["orkestr-dev.bad-write-widget", "Bad write widget", ["tasks.create"]],
      ["orkestr-dev.bad-local-widget", "Bad local-only widget", ["secrets.list"]],
    ];
    for (const [id, title, methods] of bad) {
      if (await pluginByPackage(id)) continue;
      const bytes = buildPackage(widgetManifest(id, title, methods), { "widgets/w.html": WIDGET_HTML });
      await installAndEnable({ packageBase64: bytes.toString("base64"), source: "phase7-acceptance" }, id);
    }
    const widgets = (await call("widgets.list", {})).widgets;
    const plugW = widgets.find((w) => w.id === `${HELLO_WIDGET}/hello-widget`);
    const builtins = widgets.filter((w) => w.source.type === "BuiltIn");
    check("E4", "widgets.list: built-ins + the enabled plugin widget (source Plugin)", !!plugW && plugW.source.type === "Plugin" && builtins.length > 0, `${builtins.length} built-in; plugin: ${widgets.filter((w) => w.source.type === "Plugin").map((w) => w.id).join(", ")}`);
    const hidden = bad.filter(([id]) => !widgets.some((w) => w.id.startsWith(`${id}/`)));
    check("E4", "widgets bound to a write method (tasks.create) / local_only method (secrets.list) are hidden", hidden.length === 2, `hidden ${hidden.map((b) => b[0]).join(", ")}`);
    const bWrite = await errOf(call("widgets.bundle", { widgetId: "orkestr-dev.bad-write-widget/w" }));
    const bLocal = await errOf(call("widgets.bundle", { widgetId: "orkestr-dev.bad-local-widget/w" }));
    check("E4", "widgets.bundle refuses the write / local_only bindings (forbidden)", bWrite?.code === "forbidden" && bLocal?.code === "forbidden", `${describe(bWrite)} | ${describe(bLocal)}`);

    const b = builtins[0];
    const sz = b.sizes?.[0] ?? { w: 4, h: 3 };
    const inst = (instanceId, widgetId, x, w, h, config = {}, y = 0) => ({ instanceId, widgetId, x, y, w, h, config });
    const g = (await call("dashboards.save", { name: "P7 global", widgets: [inst("g1", b.id, 0, sz.w, sz.h), inst("g2", plugW.id, 0, 3, 2, { name: "P7" }, sz.h)] })).dashboard;
    const gGet = (await call("dashboards.get", { dashboardId: g.id })).dashboard;
    const p = (await call("dashboards.save", { projectId: ctx.project.id, name: "Greeter ops", widgets: [inst("p1", plugW.id, 0, 3, 2, { name: "Greeter" })] })).dashboard;
    const pGet = (await call("dashboards.get", { dashboardId: p.id })).dashboard;
    const listed = (await call("dashboards.list", { projectId: ctx.project.id, includeGlobal: true })).dashboards;
    check("E4", "dashboards.save / get: global + project dashboards round-trip (layout + config)", !gGet.projectId && gGet.widgets.length === 2 && gGet.widgets[1].config.name === "P7" && pGet.projectId === ctx.project.id && listed.some((d) => d.id === g.id) && listed.some((d) => d.id === p.id), `global v${gGet.version} ${gGet.widgets.map((w) => w.widgetId).join(",")}; project v${pGet.version}`);
    const upd = (await call("dashboards.save", { dashboardId: p.id, projectId: ctx.project.id, name: "Greeter ops", widgets: [{ ...pGet.widgets[0], x: 3, w: 4 }], expectedVersion: pGet.version })).dashboard;
    const stale = await errOf(call("dashboards.save", { dashboardId: p.id, projectId: ctx.project.id, name: "Greeter ops", widgets: pGet.widgets, expectedVersion: pGet.version }));
    check("E4", "drag/resize persists (version bump); a stale expectedVersion conflicts", upd.version > pGet.version && upd.widgets[0].x === 3 && refused(stale), `v${pGet.version} → v${upd.version}; stale: ${describe(stale)}`);
    const dashEv = R().events.some((e) => e.kind === "DashboardChanged" && e.payload.dashboardId === p.id);
    check("E4", "DashboardChanged events", dashEv);
    const badDash = await errOf(call("dashboards.save", { name: "bad", widgets: [inst("x1", "orkestr-dev.bad-write-widget/w", 0, 3, 2)] }));
    check("E4", "a dashboard with the write-bound widget is refused", refused(badDash), describe(badDash));

    const bundle = await call("widgets.bundle", { widgetId: plugW.id });
    const sha = crypto.createHash("sha256").update(bundle.html).digest("hex");
    check("E4", "widgets.bundle: plugin HTML + CSP (no network, scripts only) + sha256", /Hello widget/.test(bundle.html) && /default-src 'none'/.test(bundle.csp) && !/connect-src [^;]*(https?:|\*)/.test(bundle.csp) && (bundle.sha256 === sha || bundle.sha256 === `sha256:${sha}`), bundle.csp);
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("E5", "Deployments: StaticSite, LocalProcess, production approval, cloud adapters, Docker, rollback", async () => {
    const adapters = (await call("deploy.adapters", {})).adapters;
    const byId = Object.fromEntries(adapters.map((a) => [a.id, a]));
    info("E5", "deploy.adapters", adapters.map((a) => `${a.id}:${a.availability.state}`).join(", "));

    // StaticSite → Local
    const siteEnv = await createEnv("site-local", "Local", "StaticSite", { port: String(PORTS.site), outputDir: "dist", healthPath: "/index.html" });
    ctx.siteEnv = siteEnv;
    const s1Dir = siteDir("site-v1", "<h1>version one</h1>");
    const plan = (await call("deploy.plan", { environmentId: siteEnv.environment.id, version: "v1", sourceDir: s1Dir })).plan;
    const s1 = await deployStart(siteEnv.environment.id, "v1", s1Dir);
    const d1 = await waitDeploy(s1.deployment.id);
    const fin = await R().waitEvent((e) => e.kind === "DeploymentFinished" && e.payload.deployment.id === d1.id, 5000, "DeploymentFinished").catch(() => null);
    const siteUrl = d1.url ?? `http://127.0.0.1:${PORTS.site}/`;
    const page = await httpText(siteUrl);
    const started = R().events.some((e) => e.kind === "DeploymentStarted" && e.payload.deployment.id === d1.id);
    check("E5", "StaticSite → Local: healthy, served, DeploymentStarted + DeploymentFinished(Succeeded)", !plan.requiresApproval && d1.status === "Succeeded" && fin?.payload.deployment.status === "Succeeded" && started && /version one/.test(page.body), `${d1.status} ${siteUrl} → ${page.status}; plan steps ${plan.steps.length}`);

    // LocalProcess (greeter service)
    const ds = await deployService("v1");
    const health = await httpText(`http://127.0.0.1:${PORTS.svc}/health`);
    check("E5", "LocalProcess (greeter service) deployed healthy; GET /health 200", ds.status === "Succeeded" && health.status === 200 && /Hello, world!/.test(health.body), `${ds.status}; /health ${health.status} ${health.body.slice(0, 80)}${ds.status !== "Succeeded" ? ` | ${(await deployLogs(ds.id)).slice(-300)}` : ""}`);

    // Production → approval
    const prodEnv = await createEnv("site-production", "Production", "StaticSite", { port: String(PORTS.prod), outputDir: "dist", healthPath: "/index.html" });
    const pDir = siteDir("site-prod", "<h1>production</h1>");
    const ps = await deployStart(prodEnv.environment.id, "p1", pDir);
    const asked = await R().waitEvent((e) => e.kind === "ApprovalRequested" && (e.payload.approval.id === ps.approvalId || JSON.stringify(e.payload.approval).includes(prodEnv.environment.name)), 10_000, "ApprovalRequested (production deploy)").catch(() => null);
    const waiting = await deployGet(ps.deployment.id);
    check("E5", "Production deploy → AwaitingApproval + ApprovalRequested (Dangerous Action Guard)", ps.deployment.status === "AwaitingApproval" && !!ps.approvalId && waiting.status === "AwaitingApproval" && !!asked, `${ps.deployment.status}; approval ${ps.approvalId}`);
    if (ps.approvalId) await call("approvals.resolve", { approvalId: ps.approvalId, decision: "Approve", note: "phase 7 acceptance" });
    const pd = await waitDeploy(ps.deployment.id);
    const prodPage = await httpText(pd.url ?? `http://127.0.0.1:${PORTS.prod}/`);
    check("E5", "approved → production deployed and served", pd.status === "Succeeded" && /production/.test(prodPage.body), `${pd.status} → ${prodPage.status}`);

    // cloud adapters vs the fake API
    const cloudDir = siteDir("site-cloud", "<h1>cloud</h1>");
    const cloud = [
      ["vercel", "Vercel", "VERCEL_TOKEN", { project: "site", dir: "dist", pollIntervalMs: "100", timeoutSecs: "60" }],
      ["netlify", "Netlify", "NETLIFY_AUTH_TOKEN", { siteId: "site1", dir: "dist", pollIntervalMs: "100", timeoutSecs: "60" }],
      ["fly", "Fly", "FLY_API_TOKEN", { app: "app1", image: "img:{version}", waitTimeoutSecs: "30" }],
      ["kubernetes", "Kubernetes", "KUBE_TOKEN", { deployment: "web", image: "img:{version}", pollIntervalMs: "100", timeoutSecs: "60" }],
    ];
    const needCred = cloud.filter(([id, , secret]) => byId[id]?.availability.state === "NeedsCredential" && byId[id].availability.secret === secret);
    check("E5", "cloud adapters report NeedsCredential{secret} while no vault credential exists", needCred.length === 4, cloud.map(([id]) => `${id}:${text(byId[id]?.availability)}`).join(" "));
    const noCredEnv = await createEnv("vercel-nocred", "Dev", "Vercel", cloud[0][3]);
    const noCred = await errOf(deployStart(noCredEnv.environment.id, "c0", cloudDir));
    check("E5", "deploy without the credential refused, naming the missing secret", refused(noCred) && /VERCEL_TOKEN/.test(describe(noCred)), describe(noCred));
    await fetch(`${ctx.cloudBase}/__reset`, { method: "POST" });
    const results = [];
    for (const [id, target, secret, settings] of cloud) {
      const sref = (await call("secrets.set", { name: secret, scope: { scope: "Project", projectId: ctx.project.id }, value: ctx.cloudToken })).secret;
      const env = await createEnv(`${id}-dev`, "Dev", target, settings, { secretRefs: [sref.id] });
      const st = await errOf(
        deployStart(env.environment.id, "c1", cloudDir).then(async (r) => {
          const d = await waitDeploy(r.deployment.id, undefined, 90_000);
          results.push({ id, d, logs: await deployLogs(d.id) });
        }),
      );
      if (st) results.push({ id, error: st });
    }
    const reqs = (await (await fetch(`${ctx.cloudBase}/__requests`)).json()) ?? [];
    const allOk = results.every((r) => r.d?.status === "Succeeded");
    const shapesOk = reqs.length > 0 && reqs.every((r) => r.authOk && r.shapeOk);
    const leaked = results.filter((r) => (r.logs ?? "").includes(ctx.cloudToken)).map((r) => r.id);
    check("E5", "Vercel / Netlify / Fly / Kubernetes deploy against the fake API (request-shape verified)", allOk && shapesOk, `${results.map((r) => `${r.id}:${r.d?.status ?? describe(r.error)}`).join(" ")}; ${reqs.length} requests, bad: ${reqs.filter((r) => !r.authOk || !r.shapeOk).map((r) => `${r.method} ${r.path} ${r.problems ?? ""}`).join("; ")}`);
    check("E5", "the token never appears in deployment logs or the fake API log", leaked.length === 0 && !JSON.stringify(reqs).includes(ctx.cloudToken), leaked.join(","));
    const after = Object.fromEntries((await call("deploy.adapters", {})).adapters.map((a) => [a.id, a]));
    check("E5", "with the vault secrets present the cloud adapters are Ready", cloud.every(([id]) => after[id]?.availability.state === "Ready"), cloud.map(([id]) => `${id}:${after[id]?.availability.state}`).join(" "));

    // Docker
    const docker = byId.docker;
    if (docker?.availability.state === "Unavailable") {
      check("E5", "Docker unavailable → skipped with a reason", !!docker.availability.reason, docker.availability.reason);
    } else if (docker?.availability.state === "Ready" && process.env.ORKESTR_P7_DOCKER === "1") {
      const dDir = deployDir("docker-src");
      writeFileSync(join(dDir, "index.html"), "docker ok\n");
      writeFileSync(join(dDir, "Dockerfile"), 'FROM busybox:1.36\nCOPY index.html /www/index.html\nEXPOSE 8080\nCMD ["httpd", "-f", "-p", "8080", "-h", "/www"]\n');
      const env = await createEnv("docker-local", "Local", "Docker", { hostPort: String(PORTS.docker), containerPort: "8080", healthPath: "/", healthTimeoutSecs: "120" });
      const r = await deployStart(env.environment.id, "d1", dDir);
      const d = await waitDeploy(r.deployment.id, undefined, 600_000);
      check("E5", "Docker available → container deployed and healthy", d.status === "Succeeded", `${d.status} ${(await deployLogs(d.id)).slice(-200)}`);
      // The deployed container outlives the test core by design: remove it (and its image).
      try {
        const names = execFileSync("docker", ["ps", "-a", "--filter", `name=orkestr-`, "--format", "{{.Names}} {{.Image}}"], { encoding: "utf8" })
          .split("\n").map((l) => l.trim().split(" ")).filter(([n, img]) => n && img?.startsWith(`${n}:`));
        for (const [n, img] of names) {
          execFileSync("docker", ["rm", "-f", n], { stdio: "ignore" });
          execFileSync("docker", ["rmi", "-f", img], { stdio: "ignore" });
        }
      } catch {
        /* best effort */
      }
    } else {
      info("E5", "Docker run skipped", docker ? `availability ${docker.availability.state}; set ORKESTR_P7_DOCKER=1 to deploy a busybox container` : "no docker adapter listed");
    }

    // rollback (StaticSite v1 → v2 → rollback)
    const s2Dir = siteDir("site-v2", "<h1>version two</h1>");
    const d2 = await waitDeploy((await deployStart(siteEnv.environment.id, "v2", s2Dir)).deployment.id);
    const v2page = await httpText(siteUrl);
    const rb = await call("deploy.rollback", { environmentId: siteEnv.environment.id });
    if (rb.approvalId) await call("approvals.resolve", { approvalId: rb.approvalId, decision: "Approve", note: "phase 7 acceptance" });
    const rbd = await waitDeploy(rb.deployment.id);
    const v1page = await httpText(siteUrl);
    check("E5", "rollback restores the previous version (v2 → v1)", d2.status === "Succeeded" && /version two/.test(v2page.body) && rbd.status === "Succeeded" && rbd.version === "v1" && /version one/.test(v1page.body), `v2 ${d2.status}; rollback ${rbd.status} → ${rbd.version}; page ${v1page.body.trim()}`);
    const hist = (await call("deploy.list", { projectId: ctx.project.id, environmentId: siteEnv.environment.id })).deployments;
    check("E5", "deploy.list history (newest first)", hist.length >= 3 && hist[0].id === rbd.id, hist.map((d) => `${d.version}:${d.status}`).join(", "));
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("E6", "Infra monitor: metrics, checks, alerts, logs", async () => {
    const ov = await waitFor(async () => {
      const o = await call("monitor.overview", {});
      return o.nodes?.length && o.nodes[0].memTotalBytes > 0 ? o : null;
    }, 20_000, "local node sampled");
    const n = ov.nodes.find((x) => x.local) ?? ov.nodes[0];
    check("E6", "monitor.overview: local node CPU / RAM / disk", n.local && typeof n.cpuPercent === "number" && n.memTotalBytes > 0 && n.diskTotalBytes > 0, `cpu ${n.cpuPercent?.toFixed?.(1)}%, mem ${(n.memUsedBytes / 2 ** 30).toFixed(1)}/${(n.memTotalBytes / 2 ** 30).toFixed(1)} GiB, disk ${(n.diskUsedBytes / 2 ** 30).toFixed(0)}/${(n.diskTotalBytes / 2 ** 30).toFixed(0)} GiB`);
    const series = await waitFor(async () => {
      const s = (await call("monitor.metrics", { metrics: ["cpu", "mem", "disk"] })).series ?? [];
      return ["cpu", "mem", "disk"].every((m) => s.some((x) => x.metric === m && x.points.length > 0)) ? s : null;
    }, 15_000, "metric series").catch(() => []);
    check("E6", "monitor.metrics series (cpu, mem, disk)", ["cpu", "mem", "disk"].every((m) => series.some((s) => s.metric === m)), series.map((s) => `${s.metric}:${s.points.length}`).join(" "));

    // HTTP check against a local server that fails, then recovers
    let healthy = false;
    const flappy = http.createServer((q, s) => {
      s.writeHead(healthy ? 200 : 503);
      s.end(healthy ? "ok" : "down");
    });
    await new Promise((r) => flappy.listen(PORTS.flappy, "127.0.0.1", r));
    ctx.flappy = flappy;
    const c = (await call("monitor.checkUpsert", { projectId: ctx.project.id, name: "p7 flappy", probe: { type: "Http", url: `http://127.0.0.1:${PORTS.flappy}/` }, failureThreshold: 1, timeoutMs: 3000, enabled: false })).check;
    const checkId = c.check.id;
    await waitCheckState(checkId, "Down");
    const down = await R().waitEvent((e) => e.kind === "HealthCheckChanged" && e.payload.checkId === checkId && e.payload.to === "Down", 10_000, "HealthCheckChanged → Down").catch(() => null);
    const raised = await R().waitEvent((e) => e.kind === "AlertRaised" && e.payload.alert.source?.type === "Check" && e.payload.alert.source.checkId === checkId, 10_000, "AlertRaised").catch(() => null);
    check("E6", "check down → HealthCheckChanged(Down) + AlertRaised (check-down rule)", !!down && !!raised, raised ? `${raised.payload.alert.rule}: ${raised.payload.alert.title}` : "no AlertRaised");
    healthy = true;
    await waitCheckState(checkId, "Up");
    const up = await R().waitEvent((e) => e.kind === "HealthCheckChanged" && e.payload.checkId === checkId && e.payload.to === "Up", 10_000, "HealthCheckChanged → Up").catch(() => null);
    const resolved = await waitFor(async () => {
      if (raised && R().events.some((e) => e.kind === "AlertResolved" && e.payload.alert.id === raised.payload.alert.id)) return true;
      await soft(call("monitor.checkRun", { checkId }));
      return false;
    }, 20_000, "AlertResolved").catch(() => false);
    check("E6", "check up again → HealthCheckChanged(Up) + AlertResolved", !!up && !!resolved, `up ${!!up}; resolved ${!!resolved}`);
    const status = (await call("monitor.checksList", { projectId: ctx.project.id })).checks.find((x) => x.check.id === checkId);
    check("E6", "checksList shows state Up with run history", status?.state === "Up" && (status.history?.length ?? 0) >= 2, `${status?.state}; ${status?.history?.length} samples`);

    // ingest + redaction
    const ing = await call("monitor.ingest", {
      stream: "p7-web",
      projectId: ctx.project.id,
      lines: [
        { level: "error", message: "db connect failed password=hunter2hunter2 host=db.internal" },
        { level: "info", message: "user login", attrs: { token: "abc123abc123abc123", user: "bob" } },
      ],
    });
    const logs = await call("monitor.logs", { sourceId: "ingest:p7-web" });
    const t = text(logs.lines);
    check("E6", "monitor.ingest → monitor.logs, secrets redacted (password=, token attr)", ing.accepted === 2 && logs.lines.length >= 2 && !t.includes("hunter2hunter2") && !t.includes("abc123abc123abc123") && t.includes("user=bob"), t.slice(0, 200));
    const srcs = (await call("monitor.logSources", {})).sources;
    check("E6", "log sources include the ingest stream and deployment logs", srcs.some((s) => s.id === "ingest:p7-web") && srcs.some((s) => s.kind?.type === "Deployment"), srcs.map((s) => `${s.id}(${s.kind?.type})`).join(", "));
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("E7", "Bug Detective: alert → evidence → hypothesis → fix branch → merge gate", async () => {
    // the broken version goes out (deploy health URL is "/", so the deploy itself succeeds)
    if (!ctx.svcEnv) await deployService("v1");
    cpSync(join(fx, "broken", "greeter.mjs"), join(ctx.repo, "src", "greeter.mjs"));
    git(ctx.repo, "add", "-A");
    git(ctx.repo, "commit", "-q", "-m", "greeter: personalise the greeting with a display name");
    const badSha = git(ctx.repo, "rev-parse", "HEAD");
    const dep = await deployService("v2");
    const h500 = await httpText(`http://127.0.0.1:${PORTS.svc}/health`);
    check("E7", "broken version v2 deployed; GET /health → 500 with a stack trace", dep.status === "Succeeded" && h500.status === 500 && /greeter\.mjs:\d+/.test(h500.body), `${dep.status}; ${h500.status} ${h500.body.split("\n")[0]}`);

    // health check → alert
    const chk = (await call("monitor.checkUpsert", { projectId: ctx.project.id, environmentId: ctx.svcEnv.environment.id, name: "greeter /health", probe: { type: "Http", url: `http://127.0.0.1:${PORTS.svc}/health` }, failureThreshold: 1, timeoutMs: 3000, enabled: false })).check;
    await waitCheckState(chk.check.id, "Down");
    const alertEv = await R().waitEvent((e) => e.kind === "AlertRaised" && e.payload.alert.source?.type === "Check" && e.payload.alert.source.checkId === chk.check.id, 10_000, "AlertRaised (greeter)");
    const alert = alertEv.payload.alert;
    // the service's error log reaches the monitor (as a log shipper would send it)
    await call("monitor.ingest", { stream: "greeter-svc", projectId: ctx.project.id, lines: [{ level: "error", message: h500.body }] });
    check("E7", "health check Down → AlertRaised; error log with the stack trace ingested", alert.state !== "Resolved", `${alert.rule}: ${alert.title}`);

    const mainBefore = git(ctx.repo, "rev-parse", "main");
    const opened = (await call("detective.open", { projectId: ctx.project.id, trigger: { type: "Alert", alertId: alert.id }, title: "Greeter module crashes the /health endpoint", budgetUsd: 5 })).incident;
    ctx.incidentId = opened.id;
    const openedEv = await R().waitEvent((e) => e.kind === "IncidentOpened" && e.payload.incident.id === opened.id, 5000, "IncidentOpened").catch(() => null);
    check("E7", "detective.open from the alert → incident Opened + IncidentOpened", opened.trigger.type === "Alert" && !!openedEv, `${opened.id} ${opened.stage}`);

    const gate1 = await waitIncident(opened.id, "AwaitingFixApproval", 120_000);
    const ev = gate1.evidence;
    const kinds = [...new Set(ev.map((e) => e.kind))];
    const trace = ev.find((e) => e.kind === "StackTrace");
    const snippet = ev.find((e) => e.kind === "CodeSnippet" && /greeter\.mjs/.test(e.reference ?? e.title));
    const deployEv = ev.find((e) => e.kind === "Deployments" && /v2/.test(e.content));
    const commitEv = ev.find((e) => e.kind === "Commits" && (e.content.includes(badSha.slice(0, 10)) || /display name/.test(e.content)));
    check("E7", "evidence: stack trace + code snippet (src/greeter.mjs), deployed version v2, the offending commit", !!trace && !!snippet && !!deployEv && !!commitEv, `kinds ${kinds.join(",")}; snippet ${snippet?.reference ?? "-"}`);
    const hyp = gate1.hypothesis;
    const ids = new Set(ev.map((e) => e.id));
    check("E7", "structured hypothesis (confidence, evidence refs that exist, suspected files)", !!hyp && hyp.confidence > 0 && hyp.evidenceRefs.length > 0 && hyp.evidenceRefs.every((r) => ids.has(r)), hyp ? `${(hyp.confidence * 100).toFixed(0)}% ${hyp.summary}; refs ${hyp.evidenceRefs.join(",")}; files ${hyp.suspectedFiles.join(",")}` : "none");
    check("E7", "reproduction fails as expected (tests fail on the current code)", gate1.reproduction && gate1.reproduction.passed === false, gate1.reproduction ? `${gate1.reproduction.command} → exit ${gate1.reproduction.exitCode}` : "no reproduction run");
    const stages = R().events.filter((e) => e.kind === "IncidentStageChanged" && e.payload.incidentId === opened.id).map((e) => e.payload.to);
    check("E7", "stage machine Opened → Collecting → Hypothesizing → Reproducing → AwaitingFixApproval (gate approval requested)", ["Collecting", "Hypothesizing", "Reproducing", "AwaitingFixApproval"].every((s) => stages.includes(s)) && !!gate1.approvalId, stages.join(" → "));

    await call("detective.approveFix", { incidentId: opened.id, note: "phase 7 acceptance" });
    const gate2 = await waitIncident(opened.id, "AwaitingMergeApproval", 300_000, { approveAgents: true });
    const branch = gate2.fixBranch;
    const fixed = branch ? soft(Promise.resolve().then(() => git(ctx.repo, "show", `${branch}:src/greeter.mjs`))) : null;
    const fixedSrc = (await fixed) ?? "";
    check("E7", "approveFix → agent fix on detective/<id>; tests pass on the fix branch", branch === `detective/${opened.id}` && gate2.fixTests?.passed === true && !/opts\.displayName/.test(fixedSrc) && /Hello, \$\{/.test(fixedSrc), `${branch}; tests ${gate2.fixTests?.command} → ${gate2.fixTests?.passed}`);
    const stages2 = R().events.filter((e) => e.kind === "IncidentStageChanged" && e.payload.incidentId === opened.id).map((e) => e.payload.to);
    check("E7", "Fixing → Testing → Reviewing (review ran) → AwaitingMergeApproval", ["Fixing", "Testing", "Reviewing", "AwaitingMergeApproval"].every((s) => stages2.includes(s)), `${stages2.join(" → ")}; review ${gate2.reviewId ?? "n/a"}`);
    const mainAfter = git(ctx.repo, "rev-parse", "main");
    const mainSrc = readFileSync(join(ctx.repo, "src", "greeter.mjs"), "utf8");
    check("E7", "main untouched while waiting for the merge approval (no auto-merge)", mainAfter === mainBefore && /opts\.displayName/.test(mainSrc) && gate2.stage === "AwaitingMergeApproval", `main ${mainBefore.slice(0, 10)} → ${mainAfter.slice(0, 10)}; spent $${gate2.spentUsd} of $${gate2.budgetUsd}`);
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("E8", "Security: emergency stop, remote viewer, audit", async () => {
    await ensureHello();
    // a deploy that never becomes healthy (nothing listens on the health port)
    const hangDir = deployDir("hang");
    writeFileSync(join(hangDir, "hang.mjs"), "setInterval(() => {}, 1000);\n");
    const hangEnv = await createEnv("hang", "Dev", "LocalProcess", { command: "node hang.mjs", workdir: ".", healthUrl: `http://127.0.0.1:${PORTS.hang}/`, healthTimeoutSecs: "300" });
    const hs = await deployStart(hangEnv.environment.id, "h1", hangDir);
    await waitDeploy(hs.deployment.id, (d) => d.status === "Running", 30_000, "Running");
    // a detective run in flight
    const inc = (await call("detective.open", { projectId: ctx.project.id, trigger: { type: "Manual", description: "TypeError: Cannot read properties of undefined (reading 'displayName')\n    at greeter (src/greeter.mjs:10:23)" }, budgetUsd: 1 })).incident;
    await call("emergency.stopAll", {});
    const plug = await waitFor(async () => {
      const p = await pluginByPackage(HELLO);
      return p && p.plugin.state === "Killed" ? p : null;
    }, 10_000, "hello-tool Killed").catch(() => null);
    const killEv = R().events.some((e) => e.kind === "PluginStateChanged" && e.payload.pluginId === ctx.helloId && e.payload.state === "Killed" && /emergency/i.test(e.payload.reason ?? ""));
    check("E8", "emergency stop kills plugins (Killed, reason emergency stop)", !!plug && killEv, plug?.plugin.state);
    const hd = await waitDeploy(hs.deployment.id, undefined, 15_000).catch(() => null);
    check("E8", "emergency stop cancels the running deployment", hd?.status === "Cancelled", hd?.status);
    const ci = await waitFor(async () => {
      const i = await incidentGet(inc.id);
      return i.stage === "Cancelled" ? i : null;
    }, 15_000, "incident Cancelled").catch(() => null);
    const e7 = ctx.incidentId ? await soft(incidentGet(ctx.incidentId)) : null;
    check("E8", "emergency stop cancels the detective run", !!ci, `${(await soft(incidentGet(inc.id)))?.stage}; E7 incident ${e7?.stage ?? "-"}`);
    await soft(call("emergency.reset", {}));

    // remote viewer (paired over the gateway like phase 4) is refused mutations
    const C = await bundleClient();
    await tunnel();
    const authFetch = (url, init = {}) => fetch(url, { ...init, headers: { ...(init.headers ?? {}), authorization: `Bearer ${ctx.ownerToken}` } });
    const offer = await call("devices.startPairing");
    const parsed = C.parsePairingQr(offer.qr);
    const session = await C.pairDevice({ ...parsed, deviceName: "P7 viewer", platform: "node", fetch: authFetch, store: new C.MemoryKeyStore() });
    await waitFor(async () => (await call("devices.list")).pending.find((p) => p.pairingId === offer.pairingId), 10_000, "pending viewer");
    await call("devices.approve", { pairingId: offer.pairingId, sasConfirmed: true, role: "viewer", remoteTerminal: false });
    const rec = await session.waitForApproval({ intervalMs: 200 });
    const opts = { gatewayUrl: rec.gatewayUrl, coreId: rec.coreId, deviceId: rec.deviceId, deviceKey: rec.keyPair, corePubKey: rec.corePubKey, sessionToken: ctx.ownerToken };
    const viewer = new C.OrkestrClient(C.remoteClientUrl(opts), "remote", { webSocket: C.createRemoteSocketFactory(opts), clientInfo: { kind: "web", version: "0.1.0" } });
    await viewer.connect();
    try {
      const vRead = await soft(viewer.call("plugins.list", {}));
      const vInstall = await errOf(viewer.call("plugins.install", { path: fixture("hello-tool.orkp") }));
      const vDeploy = await errOf(viewer.call("deploy.start", { environmentId: (ctx.siteEnv ?? hangEnv).environment.id, version: "viewer" }));
      check("E8", "remote viewer: plugins.install forbidden", vInstall?.code === "forbidden", describe(vInstall));
      check("E8", "remote viewer: deploy.start forbidden", vDeploy?.code === "forbidden", describe(vDeploy));
      info("E8", "remote viewer read (plugins.list)", vRead ? `${vRead.plugins?.length} plugins` : "refused");
    } finally {
      await soft(Promise.resolve(viewer.close?.()));
    }

    const acts = await auditActions();
    const wanted = ["plugins.install", "plugins.grant", "plugins.killAll", ...(ctx.srcId ? ["marketplace.install", "marketplace.trustKey"] : []), "deploy.start", "detective.open", "emergency.stopAll"];
    const missing = wanted.filter((w) => !acts.some((a) => a.action === w || a.action.startsWith(w) || a.details.includes(`"${w}"`)));
    check("E8", "audit records plugin, marketplace, deploy, detective and emergency actions", missing.length === 0, missing.length ? `missing ${missing.join(",")}; have ${[...new Set(acts.map((a) => a.action))].filter((x) => /plugin|market|deploy|detect|emerg|incident/i.test(x)).join(",")}` : `${acts.length} entries`);
  });

  // ════════════════════════════════════════════════════════════════════════
  await runSection("E9", "deferred features answer unimplementedFeature", async () => {
    const aws = await errOf(
      createEnv("aws", "Dev", "Aws", {}).then((env) => call("deploy.plan", { environmentId: env.environment.id, version: "a1" })),
    );
    check("E9", "AWS adapter → unimplementedFeature", isDeferred(aws, "deploy.plan") || isDeferred(aws, "environments.create"), describe(aws));
    const js = buildPackage(
      { id: "orkestr-dev.js-tool", name: "JS Tool", version: "1.0.0", publisher: "Orkestr Dev", description: "QuickJS plugin", abi: 1, kinds: ["Tool"], entry: "plugin.js", permissions: [], tools: [{ name: "hi", description: "hi", inputSchema: { type: "object" } }], widgets: [], deployProviders: [], agentTemplates: [], subscribes: [], limits: {} },
      { "plugin.js": "export function call(input) { return input; }\n" },
    );
    const jsErr = await errOf(call("plugins.install", { packageBase64: js.toString("base64") }));
    check("E9", "JS (QuickJS) plugin → unimplementedFeature", isDeferred(jsErr, "plugins.install"), describe(jsErr));
    const panel = buildPackage({ id: "orkestr-dev.panel", name: "Panel", version: "1.0.0", publisher: "Orkestr Dev", description: "A panel", abi: 1, kinds: ["Panel"], permissions: [], tools: [], widgets: [], deployProviders: [], agentTemplates: [], subscribes: [], limits: {} });
    const panelErr = await errOf(call("plugins.install", { packageBase64: panel.toString("base64") }));
    check("E9", "Panel plugin kind → unimplementedFeature", isDeferred(panelErr, "plugins.install"), describe(panelErr));
  });
} catch (e) {
  exitCode = 1;
  check("Run", "scenario ran to the end", false, e.message);
  console.error(e.stack ?? e);
} finally {
  const failed = checks.filter((c) => !c.ok);
  if (failed.length && process.env.ORKESTR_P7_LOGS !== "0") for (const p of procs) console.error(`--- ${p.label} log tail ---\n${p.log.slice(-1500)}`);
  procs.forEach((p, i) => writeFileSync(join(outDir, `${p.label}-${i}.log`), p.log));
  writeFileSync(join(outDir, "acceptance.json"), `${JSON.stringify({ phase: 7, startedFrom: repo, checks }, null, 2)}\n`);
  const real = checks.filter((c) => !c.informational);
  const infos = checks.length - real.length;
  console.log(`\n${real.length - failed.length}/${real.length} checks passed${infos ? ` (+${infos} informational)` : ""}  (report: ${join(outDir, "acceptance.json")})`);
  if (failed.length || real.length === 0) exitCode = 1;
  if (ctx.core) {
    // stop supervised deploy processes before the core goes away
    await soft(call("emergency.stopAll", {}));
    ctx.core.rpc.close();
  }
  ctx.flappy?.close();
  for (const p of procs) kill(p);
  await sleep(1500);
  // LocalProcess / static-site servers are children of orkestrd (killed with /T above)
  try {
    rmSync(tmp, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  } catch {
    /* ignore */
  }
  process.exit(exitCode);
}
