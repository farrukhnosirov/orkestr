#!/usr/bin/env node
// Phase 4 (Remote & Sync) acceptance run.
//
// Starts a self-hosted `orkestr-gateway` (temp data dir, serving apps/web/dist),
// two headless cores (`orkestrd`, separate ORKESTR_HOME: "laptop" A and
// "desktop" B), headless Microsoft Edge driven over CDP with a virtual WebAuthn
// authenticator (ctap2, internal, user verification), and a local mock Web
// Push endpoint. Devices are driven from the real web app in Edge and from
// Node through `@orkestr/client`'s remote module (WebCrypto E2E, bundled on
// the fly with rolldown).
//
//   D  passkey register → logout → login in the web UI; wrong TOTP rejected;
//      remote.enable → tunnel connected (VAPID key in welcome); web UI pairing
//      (SAS shown on both sides, approve) lists projects over E2E; Node device
//      pairing + SAS + approve operator; projects.list / tasks.list over E2E;
//      relay traffic has no plaintext; local-only + viewer mutation forbidden;
//      dangerous approval → stepUpRequired → ok with a WebAuthn assertion,
//      replay rejected; terminal forbidden until remoteTerminal, then echo;
//      editor save visible via files.read; burst → rate_limited;
//      ApprovalRequested → mock push decrypts (RFC 8291) category approval;
//      revoke closes + refuses; remote.disable drops the tunnel, no redial;
//      audit entries (gateway + core); orkestrd listens on loopback only.
//   C  same remote key on A and B; A edits a tracked file (uncommitted) + new
//      file + uiState + task, continueOn; B sync.now + resume: identical
//      content, git status shows the uncommitted change, rev-list unchanged,
//      uiState restored, summary mentions the task; concurrent edit → one
//      conflict, both versions on disk, resolve ok; tampered chunk →
//      integrity error; offline outbox flushed after a gateway restart.
//
// Usage (repo root, Windows):
//   cargo build -p orkestr-core -p orkestr-gateway --bins && npm run build
//   node scripts/smoke/phase4-acceptance.mjs [--out <dir>]
// Writes <out>/acceptance.json (default: %TEMP%/orkestr-phase4-acceptance).

import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const exeDir = process.env.ORKESTR_SMOKE_BIN_DIR ?? join(repo, "target", "debug");
const coreExe = join(exeDir, "orkestrd.exe");
const gwExe = join(exeDir, "orkestr-gateway.exe");
const webDist = join(repo, "apps", "web", "dist");
// Avoid the Windows excluded TCP range 1349–1548 (and Vite's 1420).
const gwPort = Number(process.env.ORKESTR_GW_PORT ?? 18787);
const pushPort = Number(process.env.ORKESTR_PUSH_PORT ?? 18788);
const cdpPort = Number(process.env.ORKESTR_CDP_PORT ?? 9372);
const base = `http://localhost:${gwPort}`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const edgeExe =
  process.env.ORKESTR_EDGE ??
  ["C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe", "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe"].find(
    (p) => existsSync(p),
  );

const tmp = mkdtempSync(join(tmpdir(), "orkestr-p4acc-"));
const outArg = process.argv.indexOf("--out");
const outDir = outArg > 0 ? resolve(repo, process.argv[outArg + 1]) : join(tmpdir(), "orkestr-phase4-acceptance");
mkdirSync(outDir, { recursive: true });

// ── results ───────────────────────────────────────────────────────────────
const checks = [];
function check(area, name, ok, detail = "") {
  checks.push({ area, name, ok: Boolean(ok), detail: String(detail ?? "") });
  console.log(`${ok ? "PASS" : "FAIL"}  [${area}] ${name}${detail ? ` — ${String(detail).slice(0, 240)}` : ""}`);
}
const section = (s) => console.log(`\n── ${s}`);

async function waitFor(fn, ms, label) {
  const end = Date.now() + ms;
  let last;
  for (;;) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (e) {
      last = e;
    }
    if (Date.now() > end) throw new Error(`timed out: ${label}${last ? ` (${last.message ?? last})` : ""}`);
    await sleep(200);
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

// ── processes ─────────────────────────────────────────────────────────────
const procs = [];
function launch(exe, args, env, label) {
  const child = spawn(exe, args, { env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"] });
  const rec = { child, label, log: "" };
  child.stdout.on("data", (d) => (rec.log = (rec.log + d).slice(-20000)));
  child.stderr.on("data", (d) => (rec.log = (rec.log + d).slice(-20000)));
  procs.push(rec);
  return rec;
}
function kill(rec) {
  try {
    execFileSync("taskkill", ["/PID", String(rec.child.pid), "/T", "/F"], { stdio: "ignore" });
  } catch {
    /* gone */
  }
}

const gwDir = join(tmp, "gateway");
let gw = null;
async function startGateway() {
  gw = launch(gwExe, ["--data-dir", gwDir, "--bind", `127.0.0.1:${gwPort}`, "--web-dist", webDist], { RUST_LOG: "info" }, "gateway");
  await waitFor(async () => (await fetch(`${base}/healthz`)).ok, 30_000, "gateway /healthz");
}

async function startCore(name) {
  const home = join(tmp, name);
  mkdirSync(home, { recursive: true });
  const rec = launch(coreExe, ["--data-dir", home], { ORKESTR_HOME: home, ORKESTR_SECRET_BACKEND: "vault-file", RUST_LOG: "info" }, name);
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 60_000, `${name} core.json`);
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await waitFor(() => rpc.open().then(() => true), 30_000, `${name} RPC`);
  await rpc.sub(["*"]);
  return { name, home, rec, rpc, url: info.url };
}

// ── local RPC ─────────────────────────────────────────────────────────────
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

// ── gateway HTTP ──────────────────────────────────────────────────────────
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

// ── mock Web Push endpoint + RFC 8291 decryption ──────────────────────────
const pushes = [];
const pushServer = http.createServer((req, res) => {
  const parts = [];
  req.on("data", (d) => parts.push(d));
  req.on("end", () => {
    pushes.push({ path: req.url, headers: req.headers, body: Buffer.concat(parts) });
    res.writeHead(201).end();
  });
});
function decryptPush(body, ua, authSecret) {
  const salt = body.subarray(0, 16);
  const idlen = body[20];
  const asPublic = body.subarray(21, 21 + idlen);
  const ct = body.subarray(21 + idlen);
  const ecdh = ua.computeSecret(asPublic);
  const keyInfo = Buffer.concat([Buffer.from("WebPush: info\0"), ua.getPublicKey(), asPublic]);
  const ikm = Buffer.from(crypto.hkdfSync("sha256", ecdh, authSecret, keyInfo, 32));
  const cek = Buffer.from(crypto.hkdfSync("sha256", ikm, salt, Buffer.from("Content-Encoding: aes128gcm\0"), 16));
  const nonce = Buffer.from(crypto.hkdfSync("sha256", ikm, salt, Buffer.from("Content-Encoding: nonce\0"), 12));
  const d = crypto.createDecipheriv("aes-128-gcm", cek, nonce);
  d.setAuthTag(ct.subarray(ct.length - 16));
  const plain = Buffer.concat([d.update(ct.subarray(0, ct.length - 16)), d.final()]);
  let end = plain.length - 1;
  while (end > 0 && plain[end] === 0) end--;
  if (plain[end] !== 2) throw new Error("bad padding delimiter");
  return JSON.parse(plain.subarray(0, end).toString("utf8"));
}

// ── Edge over CDP ─────────────────────────────────────────────────────────
class Cdp {
  constructor(wsUrl) {
    this.ws = new WebSocket(wsUrl);
    this.id = 0;
    this.pending = new Map();
    this.console = [];
    this.ws.onmessage = (m) => {
      const f = JSON.parse(m.data);
      if (f.method === "Runtime.consoleAPICalled") this.console.push(f.params.args.map((a) => a.value ?? a.description).join(" "));
      if (f.method === "Runtime.exceptionThrown") this.console.push(f.params.exceptionDetails.exception?.description ?? f.params.exceptionDetails.text);
      if (f.id && this.pending.has(f.id)) {
        const p = this.pending.get(f.id);
        this.pending.delete(f.id);
        f.error ? p.reject(new Error(`${f.error.message} ${f.error.data ?? ""}`)) : p.resolve(f.result);
      }
    };
  }
  ready() {
    return new Promise((res, rej) => {
      this.ws.onopen = res;
      this.ws.onerror = rej;
    });
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((res, rej) => {
      this.pending.set(id, { resolve: res, reject: rej });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  async eval(expression) {
    const r = await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  }
  text() {
    return this.eval("document.body ? document.body.textContent : ''");
  }
  async waitText(t, ms = 20_000) {
    try {
      return await waitFor(async () => (await this.text()).includes(t), ms, `page text «${t}»`);
    } catch (e) {
      const now = await soft(this.eval("location.href + ' ' + document.readyState + ' html=' + document.documentElement.outerHTML.length + ' :: ' + (document.body ? document.body.textContent : '')"));
      throw new Error(`${e.message}; page: ${String(now).slice(0, 400)}; console: ${this.console.slice(-5).join(" | ").slice(0, 600)}`);
    }
  }
  clickButton(label) {
    return this.eval(`(() => { const b = [...document.querySelectorAll('button')].find((x) => x.textContent.trim() === ${JSON.stringify(label)} && !x.disabled); if (!b) throw new Error('no button ${label}'); b.click(); return true; })()`);
  }
  setField(label, value) {
    return this.eval(`(() => {
      const lab = [...document.querySelectorAll('label')].find((l) => l.textContent.trim().startsWith(${JSON.stringify(label)}));
      if (!lab) throw new Error('no field ${label}');
      const el = lab.htmlFor ? document.getElementById(lab.htmlFor) : lab.querySelector('input,textarea');
      const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, ${JSON.stringify(value)});
      el.dispatchEvent(new Event('input', { bubbles: true }));
      return true;
    })()`);
  }
}

// In-page WebAuthn helpers (the virtual authenticator answers).
const PAGE_HELPERS = `
  window.__b64u = (b) => btoa(String.fromCharCode(...new Uint8Array(b))).replace(/\\+/g, '-').replace(/\\//g, '_').replace(/=+$/, '');
  window.__unb64u = (s) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/') + '==='.slice((s.length + 3) % 4)), (c) => c.charCodeAt(0));
  window.__stepUpRegister = async () => {
    const c = await navigator.credentials.create({ publicKey: {
      rp: { id: location.hostname, name: 'Orkestr' },
      user: { id: crypto.getRandomValues(new Uint8Array(16)), name: 'step-up', displayName: 'step-up' },
      challenge: crypto.getRandomValues(new Uint8Array(32)),
      pubKeyCredParams: [{ type: 'public-key', alg: -7 }],
      authenticatorSelection: { userVerification: 'required', residentKey: 'discouraged' },
      attestation: 'none' } });
    window.__stepUpId = c.id;
    return { id: c.id, rawId: __b64u(c.rawId), type: c.type,
      response: { clientDataJSON: __b64u(c.response.clientDataJSON), attestationObject: __b64u(c.response.attestationObject) } };
  };
  window.__stepUpAssert = async (challenge) => {
    const a = await navigator.credentials.get({ publicKey: { challenge: __unb64u(challenge), rpId: location.hostname,
      allowCredentials: [{ type: 'public-key', id: __unb64u(window.__stepUpId) }], userVerification: 'required' } });
    return { credentialId: a.id, authenticatorData: __b64u(a.response.authenticatorData),
      clientDataJSON: __b64u(a.response.clientDataJSON), signature: __b64u(a.response.signature) };
  };
  true`;

// ── @orkestr/client remote module (bundled for Node) ──────────────────────
function bundleClient() {
  const out = join(tmp, "client-bundle.mjs");
  execFileSync(
    process.execPath,
    [join(repo, "node_modules", "rolldown", "bin", "cli.mjs"), join(repo, "packages", "client", "src", "index.ts"), "--format", "esm", "--platform", "node", "-o", out],
    { cwd: repo, stdio: "ignore" },
  );
  return import(pathToFileURL(out).href);
}

let edge = null;
let exitCode = 0;
const relayFrames = [];

try {
  for (const [p, what] of [
    [coreExe, "orkestrd"],
    [gwExe, "orkestr-gateway"],
    [join(webDist, "index.html"), "apps/web/dist"],
    [edgeExe ?? "", "Microsoft Edge"],
  ]) {
    if (!p || !existsSync(p)) throw new Error(`missing ${what} (${p}) — build first`);
  }
  const C = await bundleClient();
  await new Promise((r) => pushServer.listen(pushPort, "127.0.0.1", r));
  await startGateway();
  check("Setup", "gateway up (self-hosted, loopback, serves the web app)", (await fetch(`${base}/`)).ok, base);
  const A = await startCore("laptop");
  const B = await startCore("desktop");
  check("Setup", "two orkestrd cores up (separate ORKESTR_HOME)", true, `${A.url} | ${B.url}`);

  // project repo on A (mock provider: dangerous command → approval)
  const repoA = join(tmp, "repo-a");
  mkdirSync(join(repoA, "src"), { recursive: true });
  const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();
  git(repoA, "init", "-q", "-b", "main");
  // Byte-identical checkouts on both "machines" (no CRLF conversion on Windows).
  git(repoA, "config", "core.autocrlf", "false");
  git(repoA, "config", "user.email", "p4@orkestr.local");
  git(repoA, "config", "user.name", "Orkestr P4");
  writeFileSync(join(repoA, "README.md"), "# Remote demo\n");
  writeFileSync(join(repoA, "src", "app.txt"), "line 1\n");
  writeFileSync(join(repoA, "doc.txt"), "shared doc\n");
  git(repoA, "add", "-A");
  git(repoA, "commit", "-q", "-m", "initial");
  const repoB = join(tmp, "repo-b");
  git(tmp, "clone", "-q", "-c", "core.autocrlf=false", repoA, repoB);
  git(repoB, "config", "user.email", "p4@orkestr.local");
  git(repoB, "config", "user.name", "Orkestr P4");

  await A.rpc.call("settings.update", { patch: { profileName: "P4", defaultApprovalMode: "SemiAuto", onboardingCompleted: true } });
  const mock = (await A.rpc.call("providers.list")).providers.map((p) => p.provider ?? p).find((p) => p.kind === "Mock");
  const smart = { providerId: mock.id, model: "mock-smart" };
  await A.rpc.call("providers.setManager", { config: { model: smart, maxParallelAgents: 2, approvalPolicy: "SemiAuto" } });
  await A.rpc.call("providers.setRouting", {
    config: { mode: "Manual", rules: [], defaultFallback: [smart], preferLocal: true, maxRateLimitWaitSecs: 0 },
  });
  const projectA = (await A.rpc.call("projects.create", { name: "Remote Demo", rootPath: repoA, approvalMode: "SemiAuto" })).project;
  const projectB = (await B.rpc.call("projects.create", { name: "Remote Demo", rootPath: repoB, approvalMode: "SemiAuto" })).project;

  // ════════════════════════════════════════════════════════════════════════
  section("D1. passkey account in the web app (Edge + virtual authenticator)");
  const edgeProfile = join(tmp, "edge-profile");
  launch(edgeExe, [`--remote-debugging-port=${cdpPort}`, "--headless=new", `--user-data-dir=${edgeProfile}`, "--no-first-run", "--disable-gpu", "--lang=en-US", "--accept-lang=en-US", "about:blank"], {}, "edge");
  const target = await waitFor(
    async () => (await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json()).find((t) => t.type === "page"),
    30_000,
    "Edge CDP",
  );
  edge = new Cdp(target.webSocketDebuggerUrl);
  await edge.ready();
  await edge.send("WebAuthn.enable", { enableUI: false });
  await edge.send("WebAuthn.addVirtualAuthenticator", {
    options: { protocol: "ctap2", transport: "internal", hasResidentKey: true, hasUserVerification: true, isUserVerified: true },
  });
  await edge.send("Network.enable");
  await edge.send("Page.enable");
  await edge.send("Runtime.enable");
  await edge.send("Emulation.setLocaleOverride", { locale: "en-US" });
  await edge.send("Network.setUserAgentOverride", { userAgent: (await edge.eval("navigator.userAgent")).replace("HeadlessEdg", "Edg"), acceptLanguage: "en-US" });
  await edge.send("Page.navigate", { url: `${base}/` });
  await edge.waitText("Create account with a passkey");
  await edge.setField("Your name", "Owner");
  await edge.clickButton("Create account with a passkey");
  await edge.waitText("Pair this browser");
  const st1 = await edge.eval("fetch('/v1/auth/state', { credentials: 'include' }).then((r) => r.json())");
  check("D", "passkey registration in the web UI", st1.hasAccount && st1.authenticated, JSON.stringify(st1));
  await edge.eval("fetch('/v1/auth/logout', { method: 'POST', credentials: 'include' }).then((r) => r.status)");
  await edge.send("Page.reload");
  await edge.waitText("Sign in with a passkey");
  const st2 = await edge.eval("fetch('/v1/auth/state', { credentials: 'include' }).then((r) => r.json())");
  await edge.clickButton("Sign in with a passkey");
  await edge.waitText("Pair this browser");
  const st3 = await edge.eval("fetch('/v1/auth/state', { credentials: 'include' }).then((r) => r.json())");
  check("D", "logout → passkey login in the web UI", !st2.authenticated && st3.authenticated);
  const cookies = (await edge.send("Network.getCookies", { urls: [base] })).cookies;
  const token = cookies.find((c) => c.name === "orkestr_session")?.value;
  const st4 = (await gwFetch("/v1/auth/state", { token })).json;
  check("D", "session cookie is HttpOnly and works as a bearer token", !!token && cookies.find((c) => c.name === "orkestr_session").httpOnly && st4?.authenticated);
  const setup = (await gwFetch("/v1/auth/totp/setup", { token, method: "POST" })).json;
  const confirm = await gwFetch("/v1/auth/totp/confirm", { token, method: "POST", body: { code: totpCode(setup.secret) } });
  const good = totpCode(setup.secret);
  const wrong = String((Number(good) + 1) % 1_000_000).padStart(6, "0");
  const bad = await gwFetch("/v1/auth/totp/login", { method: "POST", body: { code: wrong } });
  check("D", "TOTP fallback: wrong code rejected", confirm.status === 200 && bad.status === 401, `confirm ${confirm.status}, wrong ${bad.status}`);

  // ════════════════════════════════════════════════════════════════════════
  section("D2. remote.enable → tunnel");
  // The one-time enrollment token comes from the web UI (Pairing screen).
  await edge.clickButton("Create enrollment token");
  const enrollToken = await waitFor(
    () => edge.eval(`document.querySelector('output[aria-label="Enrollment token"]')?.textContent ?? ''`),
    10_000,
    "enrollment token in the web UI",
  );
  await A.rpc.call("remote.enable", { gatewayUrl: base, enrollmentToken: enrollToken });
  const rs = await waitFor(async () => {
    const s = await A.rpc.call("remote.status");
    return s.connected ? s : null;
  }, 20_000, "tunnel connected");
  const coreId = rs.coreId;
  const cores = (await gwFetch("/v1/cores", { token })).json.cores;
  check("D", "enrollment token from the web UI → remote.enable → tunnel connected, core enrolled", cores.some((c) => c.coreId === coreId && c.online), coreId);
  const vapid = (await gwFetch("/v1/push/vapid")).json?.publicKey;
  const coreVapid = (await soft(A.rpc.call("push.vapidKey")))?.publicKey;
  check("D", "gateway VAPID key delivered to the core in the tunnel welcome", !!vapid && coreVapid === vapid);

  // ════════════════════════════════════════════════════════════════════════
  section("D3. web UI pairing over E2E");
  const offerWeb = await A.rpc.call("devices.startPairing");
  await edge.setField("Pairing text", offerWeb.qr);
  await edge.setField("Device name", "Edge browser");
  await edge.clickButton("Pair");
  const webSas = (
    await waitFor(() => edge.eval("document.querySelector('output.web-sas')?.textContent ?? ''"), 20_000, "web SAS")
  ).replace(/\s/g, "");
  const pendWeb = await waitFor(
    async () => (await A.rpc.call("devices.list")).pending.find((p) => p.pairingId === offerWeb.pairingId),
    10_000,
    "pending web device",
  );
  check("D", "web UI and desktop show the same SAS", webSas.length === 6 && webSas === pendWeb.sas, `${webSas} / ${pendWeb.sas}`);
  await A.rpc.call("devices.approve", { pairingId: offerWeb.pairingId, sasConfirmed: true, role: "viewer", remoteTerminal: false });
  await edge.waitText("Remote Demo", 30_000);
  check("D", "web UI paired (viewer) and lists projects over E2E", true);
  await edge.eval(PAGE_HELPERS);

  // ════════════════════════════════════════════════════════════════════════
  section("D4. Node device pairing + E2E RPC");
  const authFetch = (url, init = {}) => fetch(url, { ...init, headers: { ...(init.headers ?? {}), authorization: `Bearer ${token}` } });
  async function pairNode(name, role, remoteTerminal) {
    const offer = await A.rpc.call("devices.startPairing");
    const parsed = C.parsePairingQr(offer.qr);
    const session = await C.pairDevice({ ...parsed, deviceName: name, platform: "node", fetch: authFetch, store: new C.MemoryKeyStore() });
    const pend = await waitFor(
      async () => (await A.rpc.call("devices.list")).pending.find((p) => p.pairingId === offer.pairingId),
      10_000,
      "pending",
    );
    await A.rpc.call("devices.approve", { pairingId: offer.pairingId, sasConfirmed: true, role, remoteTerminal });
    const rec = await session.waitForApproval({ intervalMs: 200 });
    return { rec, sasMatch: session.sas === pend.sas, sas: session.sas };
  }
  const rawCapture = (url) => {
    const ws = new WebSocket(url);
    ws.binaryType = "arraybuffer";
    const origSend = ws.send.bind(ws);
    ws.send = (d) => {
      relayFrames.push(typeof d === "string" ? Buffer.from(d) : Buffer.from(d.buffer ?? d));
      origSend(d);
    };
    ws.addEventListener("message", (m) => relayFrames.push(typeof m.data === "string" ? Buffer.from(m.data) : Buffer.from(m.data)));
    return ws;
  };
  function socketOpts(rec, extra = {}) {
    return { gatewayUrl: rec.gatewayUrl, coreId: rec.coreId, deviceId: rec.deviceId, deviceKey: rec.keyPair, corePubKey: rec.corePubKey, sessionToken: token, ...extra };
  }
  async function connectDevice(rec, extra = {}) {
    const opts = socketOpts(rec, extra);
    const client = new C.OrkestrClient(C.remoteClientUrl(opts), "remote", {
      webSocket: C.createRemoteSocketFactory(opts),
      clientInfo: { kind: "web", version: "0.1.0" },
    });
    await client.connect();
    return client;
  }
  const op = await pairNode("node operator", "operator", false);
  check("D", "pairing: codeMac claim, SAS equal on both sides, approve operator", op.sasMatch && op.rec.role === "operator", op.sas);
  const dev = await connectDevice(op.rec, { rawSocket: rawCapture });
  const plist = await dev.call("projects.list", {});
  const tlist = await dev.call("tasks.list", { projectId: projectA.id });
  check("D", "projects.list + tasks.list over E2E", plist.projects.some((p) => p.id === projectA.id) && Array.isArray(tlist.tasks), `${plist.projects.length} project(s)`);
  const leaked = relayFrames.filter((b) => {
    const s = b.toString("latin1");
    return s.includes("projects.list") || s.includes("Remote Demo") || s.includes(projectA.id);
  });
  check("D", "relay traffic carries no plaintext (handshake JSON + ciphertext only)", relayFrames.length > 4 && leaked.length === 0, `${relayFrames.length} frames`);

  section("D5. policy");
  const e1 = await errOf(dev.call("remote.enable", { gatewayUrl: base }));
  const e2 = await errOf(dev.call("devices.startPairing", {}));
  check("D", "local-only methods forbidden remotely", e1?.code === "forbidden" && e2?.code === "forbidden", `${e1?.code}, ${e2?.code}`);
  const viewer = await pairNode("node viewer", "viewer", false);
  const vdev = await connectDevice(viewer.rec);
  const e3 = await errOf(vdev.call("tasks.create", { projectId: projectA.id, title: "viewer cannot" }));
  const vok = await soft(vdev.call("projects.list", {}));
  check("D", "viewer: read ok, mutation forbidden", !!vok && e3?.code === "forbidden", e3?.code);

  section("D6. push subscription + dangerous approval with step-up");
  const ua = crypto.createECDH("prime256v1");
  ua.generateKeys();
  const authSecret = crypto.randomBytes(16);
  await dev.call("push.subscribe", {
    endpoint: `http://127.0.0.1:${pushPort}/push/operator`,
    p256dh: ua.getPublicKey().toString("base64url"),
    auth: authSecret.toString("base64url"),
  });
  const stepCred = await edge.eval("window.__stepUpRegister()");
  await dev.call("devices.registerStepUp", { credential: stepCred });
  const templates = (await A.rpc.call("templates.list", {})).templates;
  const backendTpl = templates.find((t) => t.role === "Backend");
  const spawned = await A.rpc.call("agents.spawn", {
    projectId: projectA.id,
    templateId: backendTpl.id,
    newTask: { title: "Clean build output", description: "Remove untracked build leftovers.\nRun: `git clean -fd`" },
  });
  const asked = await A.rpc.waitEvent(
    (e) => e.kind === "ApprovalRequested" && e.payload.approval.agentId === spawned.agent.id,
    60_000,
    "ApprovalRequested",
  );
  const approval = asked.payload.approval;
  // Several triggers may fire (e.g. the task is also blocked): find the approval one.
  const tryDecrypt = (p) => {
    try {
      return decryptPush(p.body, ua, authSecret);
    } catch (e) {
      return { error: e.message };
    }
  };
  const push = await waitFor(
    () => pushes.find((p) => p.path === "/push/operator" && tryDecrypt(p).category === "approval"),
    20_000,
    "approval push delivery",
  ).catch(() => pushes.find((p) => p.path === "/push/operator") ?? null);
  const pushed = push && tryDecrypt(push);
  check(
    "D",
    "ApprovalRequested → Web Push via gateway (VAPID), RFC 8291 payload decrypts, category approval",
    pushed?.category === "approval" && push.headers["content-encoding"] === "aes128gcm" && /^vapid t=/.test(push.headers.authorization ?? ""),
    JSON.stringify(pushed),
  );
  const params = { approvalId: approval.id, decision: "Approve", note: "remote" };
  const su = await errOf(dev.call("approvals.resolve", params));
  check("D", "dangerous approval → forbidden{stepUpRequired, challenge}", su?.code === "forbidden" && su?.data?.stepUpRequired === true && !!su?.data?.challenge, `${approval.risk}: ${su?.message}`);
  const assertion = await edge.eval(`window.__stepUpAssert(${JSON.stringify(su?.data?.challenge ?? "")})`);
  const resolved = await soft(dev.call("approvals.resolve", { ...params, stepUp: assertion }));
  check("D", "retry with a WebAuthn assertion → approval resolved", resolved?.approval?.status === "Approved" || !!resolved, JSON.stringify(resolved?.approval?.status ?? resolved));
  const replay = await errOf(dev.call("approvals.resolve", { ...params, stepUp: assertion }));
  check("D", "replayed assertion rejected", replay?.code === "forbidden", replay?.message);

  section("D7. terminal + editor");
  const e4 = await errOf(dev.call("terminal.open", { projectId: projectA.id, cols: 80, rows: 24 }));
  check("D", "terminal forbidden without the remoteTerminal grant", e4?.code === "forbidden", e4?.message);
  const termDev = await pairNode("node terminal", "operator", true);
  const tdev = await connectDevice(termDev.rec);
  const opened = await tdev.call("terminal.open", { projectId: projectA.id, cols: 100, rows: 30 });
  const termId = opened.terminal.terminalId;
  let termOut = "";
  tdev.subscribe([`terminal:${termId}`], (ev) => {
    if (ev.kind === "TerminalOutput") termOut += ev.payload.data;
  });
  await sleep(1500);
  // The command text itself does not contain the marker: only its output does.
  const plainOut = () => termOut.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "");
  await tdev.call("terminal.write", { terminalId: termId, data: "echo ('orkestr-' + 'p4')\r" });
  await waitFor(() => /orkestr-p4/.test(plainOut()), 15_000, "terminal echo").catch(() => null);
  check("D", "remote terminal (granted): echo orkestr-p4 over E2E", /orkestr-p4/.test(plainOut()), plainOut().slice(-120));
  await soft(tdev.call("terminal.close", { terminalId: termId }));
  const content = `# Edited remotely\n${new Date().toISOString()}\n`;
  const opened2 = await dev.call("files.read", { projectId: projectA.id, path: "README.md" });
  await dev.call("files.write", { projectId: projectA.id, path: "README.md", content, expectedHash: opened2.hash });
  const readBack = await A.rpc.call("files.read", { projectId: projectA.id, path: "README.md" });
  check("D", "editor save over E2E visible via files.read", readBack.content === content && readFileSync(join(repoA, "README.md"), "utf8") === content);
  git(repoA, "checkout", "--", "README.md");

  section("D8. rate limit, revoke, kill switch, audit, loopback");
  let limited = null;
  for (let i = 0; i < 120 && !limited; i++) {
    const e = await errOf(vdev.call("remote.status", {}));
    if (e?.code === "rate_limited") limited = e;
  }
  check("D", "burst → rate_limited{retryAfterMs}", limited && limited.data?.retryAfterMs > 0, limited?.data?.retryAfterMs);
  let devClosed = false;
  dev.onStateChange?.((s) => {
    if (s !== "open" && s !== "connected") devClosed = true;
  });
  await A.rpc.call("devices.revoke", { deviceId: op.rec.deviceId });
  const afterRevoke = await errOf(Promise.race([dev.call("projects.list", {}), sleep(5000).then(() => "timeout")]).then((v) => {
    if (v === "timeout") throw Object.assign(new Error("no answer"), { code: "closed" });
    return v;
  }));
  dev.close();
  let refused = null;
  {
    const f = C.createRemoteSocketFactory(socketOpts(op.rec, { onHandshakeError: (c) => (refused = c) }));
    const s = f(C.remoteClientUrl(socketOpts(op.rec)));
    await waitFor(() => refused, 10_000, "handshake refused").catch(() => null);
    soft(Promise.resolve(s.close()));
  }
  check("D", "revoke closes the session and refuses the next handshake", !!afterRevoke && refused === "revoked", `${afterRevoke?.code ?? devClosed}, ${refused}`);
  const dropped = await waitFor(
    () => !JSON.parse(readFileSync(join(A.home, "push", "subscriptions.json"), "utf8")).some((s) => s.endpoint.endsWith("/push/operator")),
    10_000,
    "push subscriptions dropped",
  ).catch(() => false);
  check("D", "revoked device's push subscriptions dropped", dropped);
  await A.rpc.call("remote.disable");
  const offline = await waitFor(async () => !(await gwFetch("/v1/cores", { token })).json.cores.find((c) => c.coreId === coreId)?.online, 10_000, "core offline").catch(() => false);
  await sleep(3000);
  const stillOff = !(await gwFetch("/v1/cores", { token })).json.cores.find((c) => c.coreId === coreId)?.online;
  check("D", "remote.disable (kill switch) drops the tunnel, no redial", offline && stillOff);
  const gwAudit = (await gwFetch("/v1/audit", { token })).json.entries.map((e) => e.action ?? e.kind ?? "");
  const coreAudit = (await A.rpc.call("audit.list", { limit: 1000 })).entries.map((e) => e.event?.action ?? e.action);
  const wantGw = ["auth.register", "auth.login", "auth.login.failed", "cores.enrolled", "tunnel.connected"];
  const wantCore = ["remote.handshake", "devices.approve", "devices.revoke", "remote.disable"];
  const missing = [...wantGw.filter((a) => !gwAudit.includes(a)), ...wantCore.filter((a) => !coreAudit.includes(a))];
  check("D", "audit: gateway + core entries", missing.length === 0, missing.length ? `missing ${missing.join(", ")}` : `${gwAudit.length} gateway, ${coreAudit.length} core`);
  const listening = execFileSync("netstat", ["-ano", "-p", "TCP"], { encoding: "utf8" })
    .split(/\r?\n/)
    .filter((l) => /LISTENING/.test(l) && [A.rec.child.pid, B.rec.child.pid].includes(Number(l.trim().split(/\s+/).pop())))
    .map((l) => l.trim().split(/\s+/)[1]);
  check("D", "orkestrd listens on loopback only", listening.length > 0 && listening.every((a) => a.startsWith("127.0.0.1:")), listening.join(" "));
  vdev.close();
  tdev.close();

  // ════════════════════════════════════════════════════════════════════════
  section("C1. encrypted sync, Seamless Continue");
  const cfgA = await A.rpc.call("sync.configure", { projectId: projectA.id, serverUrl: base, accessToken: token, remoteProject: "remote-demo" });
  const remoteKey = cfgA.generatedRemoteKey;
  const cfgB = await B.rpc.call("sync.configure", { projectId: projectB.id, serverUrl: base, accessToken: token, remoteKey, remoteProject: "remote-demo" });
  check("C", "same remote key on A and B", !!remoteKey && cfgA.configured && cfgB.configured);
  const revBefore = git(repoB, "rev-list", "--count", "HEAD");
  writeFileSync(join(repoA, "src", "app.txt"), "line 1\nline 2 (uncommitted on A)\n");
  writeFileSync(join(repoA, "src", "new-file.txt"), "brand new, never committed\n");
  const task = (await A.rpc.call("tasks.create", { projectId: projectA.id, title: "Wire the payroll export" })).task;
  const uiState = { openEditors: [{ path: "src/app.txt" }, { path: "src/new-file.txt" }], activePath: "src/new-file.txt" };
  const cont = await A.rpc.call("sync.continueOn", { projectId: projectA.id, uiState });
  check("C", "A: continueOn published", !!cont.publishedAt, `${cont.report?.pushed} pushed, summary: ${cont.summary ?? ""}`);
  await B.rpc.call("sync.now", { projectId: projectB.id });
  const resumed = await B.rpc.call("sync.resume", { projectId: projectB.id });
  const sha = (p) => crypto.createHash("sha256").update(readFileSync(p)).digest("hex");
  const same = ["src/app.txt", "src/new-file.txt"].every((f) => existsSync(join(repoB, f)) && sha(join(repoA, f)) === sha(join(repoB, f)));
  check("C", "B: identical content incl. uncommitted + new file", same);
  const statusB = git(repoB, "status", "--porcelain");
  check("C", "B: git sees the change as uncommitted, history untouched", /M src\/app\.txt/.test(statusB) && /\?\? src\/new-file\.txt/.test(statusB) && git(repoB, "rev-list", "--count", "HEAD") === revBefore, statusB.replace(/\n/g, "; "));
  const canon = (v) => JSON.stringify(v, (_k, x) => (x && typeof x === "object" && !Array.isArray(x) ? Object.fromEntries(Object.entries(x).sort()) : x));
  check("C", "B: uiState restored", canon(resumed.uiState) === canon(uiState), JSON.stringify(resumed.uiState));
  check("C", "B: resume summary mentions the task", (resumed.summary ?? "").includes(task.title), resumed.summary);

  section("C2. concurrent edit → conflict copy");
  await A.rpc.call("sync.now", { projectId: projectA.id });
  writeFileSync(join(repoA, "doc.txt"), "shared doc\nedited on A\n");
  writeFileSync(join(repoB, "doc.txt"), "shared doc\nedited on B\n");
  await A.rpc.call("sync.now", { projectId: projectA.id });
  const nowB = await B.rpc.call("sync.now", { projectId: projectB.id });
  const conflicts = (await B.rpc.call("sync.listConflicts", { projectId: projectB.id })).conflicts;
  const cf = conflicts[0];
  const bothOnDisk = cf && existsSync(join(repoB, cf.path)) && cf.conflictCopy && existsSync(join(repoB, cf.conflictCopy));
  const texts = bothOnDisk ? [readFileSync(join(repoB, cf.path), "utf8"), readFileSync(join(repoB, cf.conflictCopy), "utf8")] : [];
  check(
    "C",
    "one conflict, both versions on disk (nothing lost)",
    conflicts.length === 1 && nowB.conflicts >= 1 && texts.some((t) => t.includes("edited on A")) && texts.some((t) => t.includes("edited on B")),
    JSON.stringify(cf),
  );
  const rsv = await errOf(B.rpc.call("sync.resolveConflict", { projectId: projectB.id, conflictId: cf?.id, resolution: "keepBoth" }));
  const left = (await B.rpc.call("sync.listConflicts", { projectId: projectB.id })).conflicts.length;
  check("C", "resolveConflict ok", !rsv && left === 0, rsv?.message);

  section("C3. tamper + offline outbox");
  await B.rpc.call("sync.now", { projectId: projectB.id });
  await A.rpc.call("sync.now", { projectId: projectA.id });
  const chunkFiles = () => {
    const out = [];
    const walk = (d) => {
      for (const e of readdirSync(d, { withFileTypes: true })) {
        const p = join(d, e.name);
        if (e.isDirectory()) walk(p);
        else if (p.includes(`${"\\"}chunks${"\\"}`) || p.includes("/chunks/")) out.push(p);
      }
    };
    walk(join(gwDir, "sync"));
    return new Set(out);
  };
  const beforeChunks = chunkFiles();
  writeFileSync(join(repoA, "src", "tamper.txt"), `tamper target ${crypto.randomBytes(24).toString("hex")}\n`);
  await A.rpc.call("sync.now", { projectId: projectA.id });
  const fresh = [...chunkFiles()].filter((p) => !beforeChunks.has(p)).sort((a, b) => statSync(a).size - statSync(b).size);
  const victim = fresh[0];
  const orig = victim ? readFileSync(victim) : null;
  if (victim) {
    const bad2 = Buffer.from(orig);
    bad2[bad2.length - 5] ^= 0xff;
    writeFileSync(victim, bad2);
  }
  const integ = await errOf(B.rpc.call("sync.now", { projectId: projectB.id }));
  check("C", "tampered chunk → integrity error, file not written", integ?.data?.syncError === "integrity" && !existsSync(join(repoB, "src", "tamper.txt")), `${fresh.length} new chunk(s): ${integ?.message}`);
  if (victim) writeFileSync(victim, orig);
  const healed = await soft(B.rpc.call("sync.now", { projectId: projectB.id }));
  check("C", "after restoring the chunk B syncs again", !!healed && existsSync(join(repoB, "src", "tamper.txt")));

  kill(gw);
  await sleep(1000);
  writeFileSync(join(repoA, "src", "offline.txt"), "written while the gateway was down\n");
  const offErr = await errOf(A.rpc.call("sync.now", { projectId: projectA.id }));
  const stOff = await A.rpc.call("sync.status", { projectId: projectA.id });
  await startGateway();
  const flushed = await waitFor(async () => {
    await soft(A.rpc.call("sync.now", { projectId: projectA.id }));
    const s = await A.rpc.call("sync.status", { projectId: projectA.id });
    return s.pending === 0 ? s : null;
  }, 30_000, "outbox flush").catch(() => null);
  await soft(B.rpc.call("sync.now", { projectId: projectB.id }));
  check(
    "C",
    "offline: outbox keeps changes, flushes after gateway restart",
    !!offErr && stOff.pending > 0 && !!flushed && existsSync(join(repoB, "src", "offline.txt")),
    `offline: ${offErr?.data?.syncError ?? offErr?.message}, pending ${stOff.pending} → ${flushed?.pending}`,
  );
  check("C", "git history untouched by sync on both sides", git(repoB, "rev-list", "--count", "HEAD") === revBefore && git(repoA, "rev-list", "--count", "HEAD") === revBefore);

  A.rpc.close();
  B.rpc.close();
} catch (e) {
  exitCode = 1;
  check("Run", "scenario ran to the end", false, e.message);
  console.error(e.stack ?? e);
  for (const p of procs) console.error(`--- ${p.label} log tail ---\n${p.log.slice(-1500)}`);
} finally {
  const failed = checks.filter((c) => !c.ok);
  writeFileSync(join(outDir, "acceptance.json"), `${JSON.stringify({ phase: 4, checks }, null, 2)}\n`);
  console.log(`\n${checks.length - failed.length}/${checks.length} checks passed  (report: ${join(outDir, "acceptance.json")})`);
  if (failed.length || checks.length === 0) exitCode = 1;
  await soft(edge?.send("Browser.close") ?? Promise.resolve());
  for (const p of procs) kill(p);
  // Edge's launcher detaches its browser processes: kill whatever still uses our temp profile.
  try {
    execFileSync(
      "powershell",
      ["-NoProfile", "-Command", `Get-CimInstance Win32_Process -Filter "Name='msedge.exe'" | Where-Object { $_.CommandLine -like '*${tmp.replace(/'/g, "''")}*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`],
      { stdio: "ignore" },
    );
  } catch {
    /* none */
  }
  pushServer.close();
  await sleep(1500);
  try {
    rmSync(tmp, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  } catch {
    /* ignore */
  }
  process.exit(exitCode);
}
