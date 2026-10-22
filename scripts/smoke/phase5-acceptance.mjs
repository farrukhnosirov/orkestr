#!/usr/bin/env node
// Phase 5 (Mobile) acceptance run.
//
// Node lane (always): a self-hosted `orkestr-gateway` (temp data dir) with the
// native push senders pointed at local mocks — FCM HTTP v1 (service-account
// JSON with a fresh RSA key whose token_uri is the mock OAuth endpoint) and
// APNs (fresh P-256 .p8 key, HTTP/2 prior-knowledge h2c mock) — plus two
// headless cores (`orkestrd`, "laptop" A and "desktop" B). The phone is the
// real mobile core (apps/mobile/src/core, pure TS) bundled for Node with
// in-memory adapters (MemorySecureKV / MemoryBiometricGate / MemoryNotifier)
// and the noble crypto provider; `globalThis.crypto.subtle` is replaced by a
// throwing proxy to prove nothing falls back to WebCrypto (Hermes has none).
//
//   E1  TOTP login → bearer; /v1/cores online; QR offer parsed; SAS equal;
//       approve admin; native step-up key registered; projects.list over E2E
//   E2  push.registerNative (fcm, apns) deliverable; ApprovalRequested →
//       FCM mock (RS256 JWT verifies, OAuth bearer, HIGH, generic alert,
//       payload decrypts: approval + eventId) and APNs mock (ES256 JWT,
//       headers, mutable-content); local notification routes to the approval
//   E3  approvals.resolve → stepUpRequired; gate denied / stale counter /
//       other device origin rejected (still pending); biometric prompted once
//       → Approved; replay rejected
//   Roles, Manager/voice, Monitoring, Node management (core B, revoke with
//   step-up, remote.disable from the phone, no redial), audit, DND.
//
// Android emulator lane: `--android` runs runAndroidLane() from scripts/smoke/phase5-android.mjs
// after the Node lane (also runnable standalone).
//
// Usage (repo root, Windows):
//   cargo build -p orkestr-core -p orkestr-gateway --bins
//   node scripts/smoke/phase5-acceptance.mjs [--out <dir>] [--android]
// Writes <out>/acceptance.json (default: %TEMP%/orkestr-phase5-acceptance).

import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import http2 from "node:http2";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const exeDir = process.env.ORKESTR_SMOKE_BIN_DIR ?? join(repo, "target", "debug");
const coreExe = join(exeDir, "orkestrd.exe");
const gwExe = join(exeDir, "orkestr-gateway.exe");
// Avoid the Windows excluded TCP range 1349–1548.
const gwPort = Number(process.env.ORKESTR_GW_PORT ?? 18787);
const fcmPort = Number(process.env.ORKESTR_FCM_PORT ?? 18789);
const apnsPort = Number(process.env.ORKESTR_APNS_PORT ?? 18790);
const base = `http://localhost:${gwPort}`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const tmp = mkdtempSync(join(tmpdir(), "orkestr-p5acc-"));
const outArg = process.argv.indexOf("--out");
const outDir = outArg > 0 ? resolve(repo, process.argv[outArg + 1]) : join(tmpdir(), "orkestr-phase5-acceptance");
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
const withTimeout = (p, ms) => Promise.race([p, sleep(ms).then(() => Promise.reject(Object.assign(new Error("timeout"), { code: "timeout" })))]);

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

// ── local RPC (the desktop side) ──────────────────────────────────────────
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

// ── software passkey (account bootstrap without a browser) ────────────────
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
async function registerPasskeyAccount() {
  const begin = await gwFetch("/v1/auth/register/begin", { method: "POST", body: { name: "Owner" } });
  const { ceremonyId, publicKey: o } = begin.json;
  const kp = crypto.generateKeyPairSync("ec", { namedCurve: "P-256" });
  const jwk = kp.publicKey.export({ format: "jwk" });
  const credId = crypto.randomBytes(16);
  const cose = cbor(new Map([[1, 2], [3, -7], [-1, 1], [-2, Buffer.from(jwk.x, "base64url")], [-3, Buffer.from(jwk.y, "base64url")]]));
  const u16 = Buffer.alloc(2);
  u16.writeUInt16BE(credId.length);
  const authData = Buffer.concat([
    crypto.createHash("sha256").update(o.rp.id).digest(),
    Buffer.from([0x45]), // UP | UV | AT
    Buffer.alloc(4),
    Buffer.alloc(16),
    u16,
    credId,
    cose,
  ]);
  const clientData = Buffer.from(JSON.stringify({ type: "webauthn.create", challenge: o.challenge, origin: base, crossOrigin: false }));
  const att = cbor(new Map([["fmt", "none"], ["attStmt", new Map()], ["authData", authData]]));
  const fin = await gwFetch("/v1/auth/register/finish", {
    method: "POST",
    body: {
      ceremonyId,
      credential: { id: b64u(credId), rawId: b64u(credId), type: "public-key", response: { clientDataJSON: b64u(clientData), attestationObject: b64u(att) } },
    },
  });
  return fin;
}

// ── mock FCM (OAuth token + messages:send) ────────────────────────────────
const fcm = { tokens: [], sends: [] };
const fcmKeys = crypto.generateKeyPairSync("rsa", { modulusLength: 2048 });
const fcmServer = http.createServer((req, res) => {
  const parts = [];
  req.on("data", (d) => parts.push(d));
  req.on("end", () => {
    const body = Buffer.concat(parts).toString("utf8");
    if (req.url === "/token") {
      fcm.tokens.push({ headers: req.headers, body });
      res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ access_token: "mock-access-token", expires_in: 3600, token_type: "Bearer" }));
      return;
    }
    const m = /^\/v1\/projects\/([^/]+)\/messages:send$/.exec(req.url ?? "");
    if (m) {
      let json = null;
      try {
        json = JSON.parse(body);
      } catch {
        /* keep null */
      }
      fcm.sends.push({ project: m[1], headers: req.headers, json, at: Date.now() });
      res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ name: `projects/${m[1]}/messages/${fcm.sends.length}` }));
      return;
    }
    res.writeHead(404).end();
  });
});
function verifyJwt(jwt, verify) {
  const [h, c, s] = String(jwt).split(".");
  if (!h || !c || !s) return null;
  if (!verify(Buffer.from(`${h}.${c}`), Buffer.from(s, "base64url"))) return null;
  return { header: JSON.parse(Buffer.from(h, "base64url")), claims: JSON.parse(Buffer.from(c, "base64url")) };
}

// ── mock APNs (HTTP/2 prior knowledge, h2c) ───────────────────────────────
const apns = { sends: [] };
const apnsKeys = crypto.generateKeyPairSync("ec", { namedCurve: "P-256" });
const apnsServer = http2.createServer();
apnsServer.on("stream", (stream, headers) => {
  const parts = [];
  stream.on("data", (d) => parts.push(d));
  stream.on("end", () => {
    let json = null;
    try {
      json = JSON.parse(Buffer.concat(parts).toString("utf8"));
    } catch {
      /* keep null */
    }
    apns.sends.push({ path: headers[":path"], method: headers[":method"], headers: { ...headers }, json, at: Date.now() });
    stream.respond({ ":status": 200, "apns-id": crypto.randomUUID() });
    stream.end();
  });
});

// ── the mobile core, bundled for Node ─────────────────────────────────────
async function bundleMobileCore() {
  const { rolldown } = await import(pathToFileURL(join(repo, "node_modules", "rolldown", "dist", "index.mjs")).href);
  const out = join(tmp, "mobile-core.mjs");
  const b = await rolldown({
    input: join(repo, "apps", "mobile", "src", "core", "index.ts"),
    platform: "node",
    resolve: {
      alias: {
        "@orkestr/client": join(repo, "packages", "client", "src", "index.ts"),
        "@orkestr/protocol": join(repo, "packages", "protocol", "src", "index.ts"),
      },
    },
    logLevel: "silent",
  });
  await b.write({ file: out, format: "esm" });
  await b.close();
  return import(pathToFileURL(out).href);
}

/** Replaces WebCrypto's `subtle` with a proxy that throws on any use. */
function poisonSubtle() {
  const real = globalThis.crypto;
  const used = [];
  const subtle = new Proxy(
    {},
    {
      get(_t, prop) {
        used.push(String(prop));
        throw new Error(`crypto.subtle.${String(prop)} used (must be the noble provider)`);
      },
    },
  );
  Object.defineProperty(globalThis, "crypto", {
    configurable: true,
    value: { getRandomValues: real.getRandomValues.bind(real), randomUUID: real.randomUUID.bind(real), subtle },
  });
  return used;
}

let exitCode = 0;
const started = { gateway: null };

try {
  for (const [p, what] of [
    [coreExe, "orkestrd"],
    [gwExe, "orkestr-gateway"],
  ]) {
    if (!existsSync(p)) throw new Error(`missing ${what} (${p}) — build first`);
  }
  const M = await bundleMobileCore();
  M.setRemoteCrypto(M.nobleCrypto());
  const subtleUses = poisonSubtle();
  check("Setup", "mobile core bundled for Node; noble provider installed; crypto.subtle poisoned", M.remoteCrypto().name === "noble");

  // push credentials + mocks
  const saPath = join(tmp, "service-account.json");
  writeFileSync(
    saPath,
    JSON.stringify({
      type: "service_account",
      project_id: "orkestr-acceptance",
      private_key_id: "k1",
      private_key: fcmKeys.privateKey.export({ type: "pkcs8", format: "pem" }),
      client_email: "push@orkestr-acceptance.iam.gserviceaccount.com",
      token_uri: `http://127.0.0.1:${fcmPort}/token`,
    }),
  );
  const p8 = join(tmp, "AuthKey_P5KEY12345.p8");
  writeFileSync(p8, apnsKeys.privateKey.export({ type: "pkcs8", format: "pem" }));
  await new Promise((r) => fcmServer.listen(fcmPort, "127.0.0.1", r));
  await new Promise((r) => apnsServer.listen(apnsPort, "127.0.0.1", r));

  const gwDir = join(tmp, "gateway");
  const gw = launch(
    gwExe,
    [
      "--data-dir", gwDir,
      "--bind", `127.0.0.1:${gwPort}`,
      "--fcm-service-account", saPath,
      "--fcm-endpoint", `http://127.0.0.1:${fcmPort}`,
      "--apns-key", p8,
      "--apns-key-id", "P5KEY12345",
      "--apns-team-id", "TEAM123456",
      "--apns-topic", "dev.orkestr.mobile",
      "--apns-endpoint", `http://127.0.0.1:${apnsPort}`,
    ],
    { RUST_LOG: "info" },
    "gateway",
  );
  started.gateway = gw;
  await waitFor(async () => (await fetch(`${base}/healthz`)).ok, 30_000, "gateway /healthz");
  check("Setup", "gateway up with FCM (service account → mock) + APNs (.p8 → h2c mock)", true, base);

  const reg = await registerPasskeyAccount();
  const ownerToken = reg.json?.token;
  const setup = (await gwFetch("/v1/auth/totp/setup", { token: ownerToken, method: "POST" })).json;
  const confirm = await gwFetch("/v1/auth/totp/confirm", { token: ownerToken, method: "POST", body: { code: totpCode(setup.secret) } });
  check("Setup", "account: passkey (software authenticator) + TOTP enabled", reg.status === 200 && !!ownerToken && confirm.status === 200, `register ${reg.status}, confirm ${confirm.status}`);

  const A = await startCore("laptop");
  const B = await startCore("desktop");
  const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();
  function makeRepo(name) {
    const dir = join(tmp, name);
    mkdirSync(join(dir, "src"), { recursive: true });
    git(dir, "init", "-q", "-b", "main");
    git(dir, "config", "user.email", "p5@orkestr.local");
    git(dir, "config", "user.name", "Orkestr P5");
    writeFileSync(join(dir, "README.md"), "# Mobile demo\n");
    writeFileSync(join(dir, "src", "app.txt"), "line 1\n");
    git(dir, "add", "-A");
    git(dir, "commit", "-q", "-m", "initial");
    return dir;
  }
  await A.rpc.call("settings.update", { patch: { profileName: "P5", defaultApprovalMode: "SemiAuto", onboardingCompleted: true } });
  const mock = (await A.rpc.call("providers.list")).providers.map((p) => p.provider ?? p).find((p) => p.kind === "Mock");
  const smart = { providerId: mock.id, model: "mock-smart" };
  await A.rpc.call("providers.setManager", { config: { model: smart, maxParallelAgents: 4, approvalPolicy: "SemiAuto" } });
  await A.rpc.call("providers.setRouting", { config: { mode: "Manual", rules: [], defaultFallback: [smart], preferLocal: true, maxRateLimitWaitSecs: 0 } });
  const project = (await A.rpc.call("projects.create", { name: "Mobile Demo", rootPath: makeRepo("repo-a"), approvalMode: "SemiAuto" })).project;
  const askProject = (await A.rpc.call("projects.create", { name: "Ask Demo", rootPath: makeRepo("repo-ask"), approvalMode: "Ask" })).project;
  await B.rpc.call("projects.create", { name: "Desktop Demo", rootPath: makeRepo("repo-b"), approvalMode: "SemiAuto" });
  const backendTpl = (await A.rpc.call("templates.list", {})).templates.find((t) => t.role === "Backend");
  const spawnAgent = (projectId, title, cmd) =>
    A.rpc.call("agents.spawn", { projectId, templateId: backendTpl.id, newTask: { title, description: `Do it.\nRun: \`${cmd}\`` } });
  const waitApproval = (agentId, pred = () => true) =>
    A.rpc.waitEvent((e) => e.kind === "ApprovalRequested" && e.payload.approval.agentId === agentId && pred(e.payload.approval), 60_000, "ApprovalRequested");

  async function enroll(core) {
    const t = (await gwFetch("/v1/cores/enrollment-token", { token: ownerToken, method: "POST" })).json.token;
    await core.rpc.call("remote.enable", { gatewayUrl: base, enrollmentToken: t });
    return waitFor(async () => {
      const s = await core.rpc.call("remote.status");
      return s.connected ? s : null;
    }, 20_000, `${core.name} tunnel`);
  }
  const rsA = await enroll(A);

  // ════════════════════════════════════════════════════════════════════════
  section("E1. login, pairing, E2E");
  const phone = M.createMemoryCore({ platform: "android", deviceName: "Pixel (acceptance)" });
  // Next TOTP step (the confirm call consumed the current one; ±1 step is accepted).
  const wrongErr = await errOf(phone.login(base, String((Number(totpCode(setup.secret, Date.now() + 30_000)) + 1) % 1_000_000).padStart(6, "0")));
  const sess = await phone.login(base, totpCode(setup.secret, Date.now() + 30_000));
  const restored = await M.createMemoryCore({ kv: phone.o.kv }).restore();
  check("E1", "TOTP login → bearer token in SecureKV (wrong code rejected; session restorable)", !!sess.token && wrongErr?.code === "unauthenticated" && restored?.token === sess.token, `wrong → ${wrongErr?.code}`);
  const cores1 = await sess.cores();
  check("E1", "/v1/cores lists the enrolled core as online", cores1.some((c) => c.coreId === rsA.coreId && c.online), JSON.stringify(cores1.map((c) => [c.name, c.online])));

  async function pairPhone(core, mc, name, role) {
    const offer = await core.rpc.call("devices.startPairing");
    const parsed = M.parsePairingInput(offer.qr);
    const viaLink = M.parsePairingInput(M.pairingDeepLink(parsed));
    const session = await mc.pair(parsed);
    const pend = await waitFor(async () => (await core.rpc.call("devices.list")).pending.find((p) => p.pairingId === offer.pairingId), 10_000, "pending");
    await core.rpc.call("devices.approve", { pairingId: offer.pairingId, sasConfirmed: true, role, remoteTerminal: false });
    const done = await mc.completePairing(session, { intervalMs: 200 });
    return { offer, parsed, viaLink, sas: session.sas, pendSas: pend.sas, done, name };
  }
  const p1 = await pairPhone(A, phone, "phone", "admin");
  check("E1", "QR offer from devices.startPairing parsed (QR text and orkestr://pair deep link)", !!p1.parsed && JSON.stringify(p1.viaLink) === JSON.stringify(p1.parsed), p1.parsed?.pairingId);
  check("E1", "SAS equal on phone and desktop", M.sasMatches(p1.sas, p1.pendSas), `${p1.sas} / ${p1.pendSas}`);
  const devInfo = (await A.rpc.call("devices.list")).devices.find((d) => d.deviceId === p1.done.record.deviceId);
  check(
    "E1",
    "approved as admin; native biometric step-up key registered (stepUpKind native, separate from the device key)",
    p1.done.stepUpRegistered && devInfo?.role === "admin" && devInfo?.stepUpKind === "native" && phone.o.gate.data.size === 1,
    `${devInfo?.role}/${devInfo?.stepUpKind} ${p1.done.stepUpError ?? ""}`,
  );
  const cA = await phone.connections.get(rsA.coreId);
  const plist = await cA.call("projects.list", {});
  check("E1", "projects.list over E2E (noble crypto, subtle never touched)", plist.projects.some((p) => p.id === project.id) && subtleUses.length === 0, `${plist.projects.length} projects; subtle uses: ${subtleUses.join(",")}`);

  // operator + viewer phones (same account bearer)
  const op = M.createMemoryCore({ platform: "ios", deviceName: "iPhone (acceptance)", keyStorage: "ios-keychain-wrapped" });
  op.session = new M.GatewaySession(base, { token: sess.token });
  const p2 = await pairPhone(A, op, "operator", "operator");
  const viewer = M.createMemoryCore({ platform: "android", deviceName: "Tablet (viewer)" });
  viewer.session = new M.GatewaySession(base, { token: sess.token });
  const p3 = await pairPhone(A, viewer, "viewer", "viewer");
  const cOp = await op.connections.get(rsA.coreId);
  const cView = await viewer.connections.get(rsA.coreId);
  check("E1", "operator (iOS) + viewer phones paired", p2.done.stepUpRegistered && !!p3.done.record, `viewer step-up: ${p3.done.stepUpRegistered ? "yes" : p3.done.stepUpError}`);

  // ════════════════════════════════════════════════════════════════════════
  section("E2. native push (FCM + APNs mocks)");
  phone.o.notifier.token = { transport: "fcm", token: "fcm-device-token-phone" };
  op.o.notifier.token = { transport: "apns", token: crypto.randomBytes(32).toString("hex") };
  const r1 = await phone.push.registerNative(cA, rsA.coreId);
  const r2 = await op.push.registerNative(cOp, rsA.coreId);
  check("E2", "push.registerNative fcm → deliverable", r1.status === "registered" && r1.deliverable === true, JSON.stringify(r1));
  check("E2", "push.registerNative apns → deliverable", r2.status === "registered" && r2.deliverable === true, JSON.stringify(r2));
  const noTok = await M.createMemoryCore().push.registerNative(cA, "x");
  check("E2", "no native token in the build → unavailable (nothing registered)", noTok.status === "unavailable" && noTok.reason === "no_token");
  const detach = phone.push.attach(cA, rsA.coreId);

  const ag1 = await spawnAgent(project.id, "Clean build output", "git clean -fd");
  const asked1 = await waitApproval(ag1.agent.id);
  const appr1 = asked1.payload.approval;
  const fcmSend = await waitFor(() => fcm.sends.find((s) => s.json?.message?.data?.k === "approval"), 20_000, "FCM approval send").catch(() => null);
  const tokReq = fcm.tokens[0];
  const assertion = tokReq ? new URLSearchParams(tokReq.body).get("assertion") : null;
  const fcmJwt = assertion && verifyJwt(assertion, (d, s) => crypto.verify("RSA-SHA256", d, fcmKeys.publicKey, s));
  check(
    "E2",
    "FCM OAuth: RS256 JWT verifies with the service-account key (iss, scope, aud = token_uri)",
    fcmJwt?.header.alg === "RS256" &&
      fcmJwt.claims.iss === "push@orkestr-acceptance.iam.gserviceaccount.com" &&
      fcmJwt.claims.aud === `http://127.0.0.1:${fcmPort}/token` &&
      /firebase\.messaging/.test(fcmJwt.claims.scope ?? "") &&
      new URLSearchParams(tokReq.body).get("grant_type") === "urn:ietf:params:oauth:grant-type:jwt-bearer",
    JSON.stringify(fcmJwt?.claims ?? null),
  );
  const fm = fcmSend?.json?.message;
  check(
    "E2",
    "FCM send: OAuth bearer, token, android priority HIGH + collapse key, generic notification",
    fcmSend?.headers.authorization === "Bearer mock-access-token" &&
      fcmSend.project === "orkestr-acceptance" &&
      fm.token === "fcm-device-token-phone" &&
      fm.android?.priority === "HIGH" &&
      !!fm.android?.collapse_key &&
      fm.notification?.title === "Orkestr" &&
      fm.notification?.body === "Action needed" &&
      !JSON.stringify(fm).includes("git clean"),
    JSON.stringify({ android: fm?.android, notification: fm?.notification }),
  );
  const dec1 = fm ? await soft(phone.push.decrypt(rsA.coreId, fm.data)) : null;
  check("E2", "FCM payload decrypts on the phone: category approval + eventId", dec1?.category === "approval" && !!dec1.eventId && (!asked1.id || dec1.eventId === asked1.id), JSON.stringify(dec1));
  const apnsSend = await waitFor(() => apns.sends.find((s) => s.json?.k === "approval"), 20_000, "APNs approval send").catch(() => null);
  const apnsJwt = apnsSend && verifyJwt(String(apnsSend.headers.authorization ?? "").replace(/^bearer /i, ""), (d, s) => crypto.verify("sha256", d, { key: apnsKeys.publicKey, dsaEncoding: "ieee-p1363" }, s));
  check(
    "E2",
    "APNs (HTTP/2 h2c): ES256 provider JWT verifies (kid, iss), push-type alert, priority 10, topic, collapse id",
    apnsJwt?.header.alg === "ES256" &&
      apnsJwt.header.kid === "P5KEY12345" &&
      apnsJwt.claims.iss === "TEAM123456" &&
      apnsSend.path === `/3/device/${op.o.notifier.token.token}` &&
      apnsSend.headers["apns-push-type"] === "alert" &&
      apnsSend.headers["apns-priority"] === "10" &&
      apnsSend.headers["apns-topic"] === "dev.orkestr.mobile" &&
      !!apnsSend.headers["apns-collapse-id"],
    `${apnsSend?.path} ${JSON.stringify(apnsJwt?.header ?? null)}`,
  );
  const dec2 = apnsSend ? await soft(op.push.decrypt(rsA.coreId, apnsSend.json)) : null;
  check(
    "E2",
    "APNs payload: generic alert, mutable-content, decrypts on the phone (approval)",
    apnsSend?.json?.aps?.alert?.body === "Action needed" && apnsSend.json.aps["mutable-content"] === 1 && dec2?.category === "approval",
    JSON.stringify(dec2),
  );
  const local = await waitFor(() => phone.o.notifier.shown.find((n) => n.data.approvalId === appr1.id), 10_000, "local notification").catch(() => null);
  const route = local && M.routeForNotification(local.data);
  check(
    "E2",
    "connected: ApprovalRequested over E2E → local notification (generic text) → tap routes to the approval",
    local?.body === "Action needed" && route?.screen === "ApprovalDetail" && route.approvalId === appr1.id,
    JSON.stringify(route),
  );

  // ════════════════════════════════════════════════════════════════════════
  section("E3. native biometric step-up");
  const params = { approvalId: appr1.id, decision: "Approve", note: "from the phone" };
  const su = await errOf(cA.call("approvals.resolve", params));
  check("E3", "dangerous approval → forbidden{stepUpRequired, challenge}", su?.code === "forbidden" && su?.data?.stepUpRequired === true && !!su?.data?.challenge, `${appr1.risk}: ${su?.message}`);
  const stillPending = async () => (await A.rpc.call("approvals.get", { approvalId: appr1.id })).approval.status === "Pending";
  // gate denied
  phone.o.gate.allow = false;
  const before = phone.o.gate.prompts;
  const denied = await errOf(M.resolveWithStepUp(cA, params, phone.stepUp(rsA.coreId)));
  phone.o.gate.allow = true;
  check("E3", "biometric gate denied → BiometricDeniedError, no retry, approval still pending", denied?.code === "biometric_denied" && phone.o.gate.prompts === before + 1 && (await stillPending()), denied?.message);
  const metaKey = `stepup.${rsA.coreId}.meta`.replace(/[^A-Za-z0-9._-]/g, "_");

  // other device origin: right key, assertion built for another device id
  const stepKey = JSON.parse(phone.o.gate.data.get(`stepup.${rsA.coreId}.key`.replace(/[^A-Za-z0-9._-]/g, "_")));
  const otherOrigin = await errOf(
    M.callWithStepUp(cA, "approvals.resolve", params, async (challenge) => {
      const m = JSON.parse(await phone.o.kv.get(metaKey));
      await phone.o.kv.set(metaKey, JSON.stringify({ ...m, counter: m.counter + 1 }));
      return M.nativeStepUpAssertion({ challenge, deviceId: p2.done.record.deviceId, credentialId: m.credentialId, counter: m.counter + 1, signer: M.remoteCrypto().signerFromRaw(stepKey) });
    }),
  );
  check("E3", "assertion for another device's origin rejected (approval still pending)", otherOrigin?.code === "forbidden" && (await stillPending()), otherOrigin?.message);
  // happy path
  let captured = null;
  const prov = phone.stepUp(rsA.coreId);
  const p0 = phone.o.gate.prompts;
  const resolved = await soft(M.resolveWithStepUp(cA, params, async (c) => (captured = await prov(c))));
  check("E3", "biometric prompted exactly once → native assertion → Approved", resolved?.status === "Approved" && phone.o.gate.prompts === p0 + 1, resolved?.status);
  const replay = await errOf(cA.call("approvals.resolve", { ...params, stepUp: captured }));
  check("E3", "replayed assertion rejected", replay?.code === "forbidden", replay?.message);
  // stale counter: a second dangerous approval, signed with the last used counter again
  const ag1b = await spawnAgent(project.id, "Clean ignored files", "git clean -fdX");
  const appr1b = (await waitApproval(ag1b.agent.id)).payload.approval;
  const metaUsed = JSON.parse(await phone.o.kv.get(metaKey));
  await phone.o.kv.set(metaKey, JSON.stringify({ ...metaUsed, counter: metaUsed.counter - 1 }));
  const stale = await errOf(M.resolveWithStepUp(cA, { approvalId: appr1b.id, decision: "Approve" }, phone.stepUp(rsA.coreId)));
  await phone.o.kv.set(metaKey, JSON.stringify({ ...metaUsed, counter: metaUsed.counter + 1 }));
  const pending1b = (await A.rpc.call("approvals.get", { approvalId: appr1b.id })).approval.status === "Pending";
  check("E3", "signature counter not increased (stale) rejected, approval still pending", stale?.code === "forbidden" && /counter/i.test(stale.message) && pending1b, stale?.message);
  await soft(cA.call("approvals.resolve", { approvalId: appr1b.id, decision: "Reject", note: "stale probe" }));

  // ════════════════════════════════════════════════════════════════════════
  section("Roles");
  const ag2 = await spawnAgent(askProject.id, "Say hello", "echo ok");
  const asked2 = await waitApproval(ag2.agent.id, (a) => !(a.danger || a.risk === "High" || a.risk === "Critical")).catch(() => null);
  const appr2 = asked2?.payload.approval;
  const vErr = appr2 ? await errOf(cView.call("approvals.resolve", { approvalId: appr2.id, decision: "Approve" })) : null;
  check("Roles", "viewer: approvals.resolve forbidden", vErr?.code === "forbidden", vErr?.message);
  const opRes = appr2 ? await soft(cOp.call("approvals.resolve", { approvalId: appr2.id, decision: "Approve" })) : null;
  check("Roles", "operator: non-dangerous approval resolved without step-up", opRes?.approval?.status === "Approved" && op.o.gate.prompts === 0, `${appr2?.risk} ${appr2?.capability} → ${opRes?.approval?.status}`);
  const vStop = await soft(M.emergencyStop(cView, { kind: "all" }));
  check("Roles", "viewer: emergency.stopAll allowed (stopping is always safe)", !!vStop && typeof vStop.agentsStopped === "number", JSON.stringify(vStop));
  await A.rpc.call("emergency.reset", {}).catch(() => null);

  // ════════════════════════════════════════════════════════════════════════
  section("Monitoring");
  const mon = await M.loadMonitoring(cA);
  const bad = Object.entries(mon).filter(([, s]) => !s.ok);
  check("Monitoring", "dashboard.snapshot, nodes.list, nodes.resources, agents, approvals, emergency.status all load", bad.length === 0, bad.length ? JSON.stringify(bad) : `nodes ${mon.nodes.value.length}, projects ${mon.dashboard.value.projects?.length}`);
  const tasks = await M.loadTasks(cA, project.id);
  const diff = await soft(M.loadDiff(cA, project.id));
  const transcript = await soft(M.loadTranscript(cA, ag1.agent.id));
  check("Monitoring", "quick review: tasks.list, vcs.diff, agent transcript", tasks.length > 0 && !!diff && !!transcript, `${tasks.length} tasks`);
  const f0 = await M.readFile(cA, project.id, "README.md");
  const saved = await M.saveFile(cA, project.id, "README.md", "# Edited on the phone\n", f0.hash);
  const conflict = await M.saveFile(cA, project.id, "README.md", "# stale\n", f0.hash);
  const back = await A.rpc.call("files.read", { projectId: project.id, path: "README.md" });
  check("Monitoring", "light editing: save with hash guard, stale hash → conflict (no overwrite)", saved.status === "saved" && conflict.status === "conflict" && back.content === "# Edited on the phone\n");

  // ════════════════════════════════════════════════════════════════════════
  section("Manager / voice");
  const sent = await cA.call("manager.chat", { projectId: project.id, text: "What is the status of this project?" });
  const reply = await A.rpc.waitEvent((e) => e.kind === "ManagerMessageAdded" && e.payload.message.id === sent.replyMessageId, 60_000, "Manager reply").catch(() => null);
  const hist = await cA.call("manager.history", { projectId: project.id });
  check("Manager", "manager.chat (mock provider) → reply visible in manager.history over E2E", !!reply && hist.messages.some((m) => m.id === sent.replyMessageId), reply?.payload.message.content?.slice?.(0, 80));
  const ctx = await M.loadIntentContext(cA, project.id);
  const ag3 = await spawnAgent(project.id, "Long task", "echo still-running");
  await sleep(500);
  const ask = await M.executeTranscript(cA, "stop all agents", ctx);
  const done = await M.executeTranscript(cA, "stop all agents", ctx, { confirm: async () => true });
  const emStatus = await A.rpc.call("emergency.status", {});
  check(
    "Manager",
    "voice \"stop all agents\" → intent stopAll → confirmation → emergency.stopAll",
    ask.status === "needsConfirmation" && done.status === "ok" && done.intent.kind === "stopAll" && typeof done.data?.agentsStopped === "number",
    `${ask.status} → ${done.status}: ${done.reply ?? done.error}; ${JSON.stringify(emStatus).slice(0, 80)}`,
  );
  await A.rpc.call("emergency.reset", {}).catch(() => null);
  void ag3;
  const wav = Buffer.alloc(44 + 3200);
  wav.write("RIFF", 0);
  wav.writeUInt32LE(36 + 3200, 4);
  wav.write("WAVEfmt ", 8);
  wav.writeUInt32LE(16, 16);
  wav.writeUInt16LE(1, 20);
  wav.writeUInt16LE(1, 22);
  wav.writeUInt32LE(16000, 24);
  wav.writeUInt32LE(32000, 28);
  wav.writeUInt16LE(2, 32);
  wav.writeUInt16LE(16, 34);
  wav.write("data", 36);
  wav.writeUInt32LE(3200, 40);
  const tr = await M.transcribe(cA, { base64: wav.toString("base64"), mimeType: "audio/wav" }).then(
    (v) => ({ ok: true, v }),
    (e) => ({ ok: false, e }),
  );
  check(
    "Manager",
    "voice.transcribe: text, or a clearly named configuration error (never a fake success)",
    tr.ok ? typeof (tr.v.text ?? tr.v) === "string" : ["bad_request", "forbidden", "not_found", "unimplemented"].includes(tr.e.code) && /provider|config|consent|speech/i.test(tr.e.message),
    tr.ok ? JSON.stringify(tr.v) : `${tr.e.code}: ${tr.e.message}`,
  );

  // ════════════════════════════════════════════════════════════════════════
  section("DND");
  const nonApproval = () => fcm.sends.filter((s) => s.json?.message?.data?.k && s.json.message.data.k !== "approval");
  const controlCount = nonApproval().length;
  await A.rpc.call("settings.update", { patch: { doNotDisturb: true } });
  await sleep(1200);
  const mark = Date.now();
  const nBefore = nonApproval().length;
  const ag4 = await spawnAgent(project.id, "Clean again", "git clean -fdx");
  const asked4 = await waitApproval(ag4.agent.id);
  const dndPush = await waitFor(() => fcm.sends.find((s) => s.at >= mark && s.json?.message?.data?.k === "approval"), 20_000, "approval push under DND").catch(() => null);
  await A.rpc.call("tasks.transition", { taskId: ag4.task?.id ?? asked4.payload.approval.taskId, to: "Blocked", reason: "dnd probe" }).catch(() => null);
  await sleep(2500);
  const mutedOk = nonApproval().length === nBefore;
  check("DND", "do-not-disturb: approval push still delivered, low-priority pushes suppressed", controlCount > 0 && !!dndPush && mutedOk, `before DND ${controlCount} low-priority push(es); during DND +${nonApproval().length - nBefore}`);
  await A.rpc.call("settings.update", { patch: { doNotDisturb: false } });
  await A.rpc.call("approvals.resolve", { approvalId: asked4.payload.approval.id, decision: "Reject", note: "cleanup" }).catch(() => null);

  // ════════════════════════════════════════════════════════════════════════
  section("Node management");
  const rsB = await enroll(B);
  const p4 = await pairPhone(B, phone, "phone@B", "admin");
  const records = await phone.store.list();
  const cores2 = await sess.cores();
  check("Nodes", "second core paired: two device records on the phone, /v1/cores lists both online", records.length === 2 && [rsA.coreId, rsB.coreId].every((id) => cores2.find((c) => c.coreId === id)?.online), `${records.length} records, ${cores2.length} cores`);
  const cB = await phone.connections.get(rsB.coreId);
  const pB = await cB.call("projects.list", {});
  check("Nodes", "each core reachable through its own E2E session", pB.projects.some((p) => p.name === "Desktop Demo") && !pB.projects.some((p) => p.id === project.id) && p4.done.stepUpRegistered);
  // revoke the viewer from the admin phone (step-up)
  const pr0 = phone.o.gate.prompts;
  const rv = await errOf(M.callWithStepUp(cA, "devices.revoke", { deviceId: p3.done.record.deviceId }, phone.stepUp(rsA.coreId)));
  const vAfter = await errOf(withTimeout(cView.call("projects.list", {}), 5000));
  await viewer.connections.disconnect(rsA.coreId);
  const vReconnect = await errOf(withTimeout(viewer.connections.get(rsA.coreId).then((c) => c.call("projects.list", {})), 8000));
  const revokedInfo = (await A.rpc.call("devices.list")).devices.find((d) => d.deviceId === p3.done.record.deviceId);
  check("Nodes", "devices.revoke from the phone (admin, biometric step-up) → viewer cut off", !rv && phone.o.gate.prompts === pr0 + 1 && revokedInfo?.revoked && !!vAfter && !!vReconnect, `${rv?.message ?? "ok"}; after: ${vAfter?.code ?? vAfter?.message}; reconnect: ${vReconnect?.code ?? vReconnect?.message}`);
  // kill switch from the phone on core B
  await soft(withTimeout(M.disableRemote(cB, phone.stepUp(rsB.coreId)), 5000));
  const offline = await waitFor(async () => !(await sess.cores()).find((c) => c.coreId === rsB.coreId)?.online, 10_000, "core B offline").catch(() => false);
  await sleep(3000);
  const stillOff = !(await sess.cores()).find((c) => c.coreId === rsB.coreId)?.online && !(await B.rpc.call("remote.status")).connected;
  const aStill = (await sess.cores()).find((c) => c.coreId === rsA.coreId)?.online;
  check("Nodes", "remote.disable from the phone drops core B's tunnel, no redial (core A unaffected)", offline && stillOff && aStill);
  // unpair B locally
  await phone.unpair(rsB.coreId);
  check("Nodes", "unpair forgets the core's keys on the phone", (await phone.store.list()).length === 1 && !(await phone.stepUpKeys.meta(rsB.coreId)));

  section("Audit");
  const gwAudit = (await gwFetch("/v1/audit", { token: sess.token })).json.entries.map((e) => e.action ?? e.kind ?? "");
  const coreAudit = (await cA.call("audit.list", { limit: 1000 })).entries.map((e) => e.event?.action ?? e.action);
  const bAudit = (await B.rpc.call("audit.list", { limit: 1000 })).entries.map((e) => e.event?.action ?? e.action);
  const wantGw = ["auth.register", "cores.enrolled", "tunnel.connected"];
  const wantCore = ["remote.handshake", "devices.approve", "devices.revoke"];
  const missing = [
    ...wantGw.filter((a) => !gwAudit.includes(a)),
    ...(gwAudit.some((a) => /login/.test(a) && /totp/.test(a)) || gwAudit.includes("auth.login") ? [] : ["(totp login)"]),
    ...wantCore.filter((a) => !coreAudit.includes(a)),
    ...(bAudit.includes("remote.disable") ? [] : ["B:remote.disable"]),
  ];
  check("Audit", "gateway + core audit entries (login, enrollment, handshake, approve, revoke, kill switch)", missing.length === 0, missing.length ? `missing ${missing.join(", ")} | gw: ${[...new Set(gwAudit)].join(",")}` : `${gwAudit.length} gateway, ${coreAudit.length} core A`);
  check("Setup", "crypto.subtle never used by the mobile core", subtleUses.length === 0, subtleUses.join(","));

  section("Unpair");
  const opUnpair = await op.unpair(rsA.coreId, { revoke: true });
  const opInfo = (await A.rpc.call("devices.list")).devices.find((d) => d.deviceId === p2.done.record.deviceId);
  check("Nodes", "unpair on an operator phone: keys forgotten, core not revoked → UI tells the user to revoke on the computer", opUnpair.revoked === false && opUnpair.reason === "not_admin" && opInfo && !opInfo.revoked && !(await op.store.get(rsA.coreId)));
  const pr1 = phone.o.gate.prompts;
  const adminUnpair = await phone.unpair(rsA.coreId, { revoke: true });
  const meInfo = (await A.rpc.call("devices.list")).devices.find((d) => d.deviceId === p1.done.record.deviceId);
  check("Nodes", "unpair on an admin phone revokes itself on the core (devices.revoke + step-up), keys forgotten", adminUnpair.revoked === true && meInfo?.revoked === true && phone.o.gate.prompts === pr1 + 1 && (await phone.store.list()).length === 0, JSON.stringify(adminUnpair));

  detach();
  await phone.connections.closeAll();
  await op.connections.closeAll();
  await viewer.connections.closeAll();

  if (process.argv.includes("--android")) {
    const { runAndroidLane } = await import(pathToFileURL(join(here, "phase5-android.mjs")).href);
    await runAndroidLane({ check, section, A, base, gwPort, totpCode, totpSecret: setup.secret, coreIdA: rsA.coreId, spawnAgent, waitApproval, project, outDir });
  }  A.rpc.close();
  B.rpc.close();
} catch (e) {
  exitCode = 1;
  check("Run", "scenario ran to the end", false, e.message);
  console.error(e.stack ?? e);
  for (const p of procs) console.error(`--- ${p.label} log tail ---\n${p.log.slice(-1500)}`);
} finally {
  const failed = checks.filter((c) => !c.ok);
  writeFileSync(join(outDir, "acceptance.json"), `${JSON.stringify({ phase: 5, lane: "node", checks }, null, 2)}\n`);
  console.log(`\n${checks.length - failed.length}/${checks.length} checks passed  (report: ${join(outDir, "acceptance.json")})`);
  if (failed.length || checks.length === 0) exitCode = 1;
  for (const p of procs) kill(p);
  fcmServer.close();
  apnsServer.close();
  await sleep(1500);
  try {
    rmSync(tmp, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  } catch {
    /* ignore */
  }
  process.exit(exitCode);
}
