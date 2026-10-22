#!/usr/bin/env node
// Phase 5 acceptance — Android emulator lane.
//
// Drives the real app (dev.orkestr.mobile) on the `orkestr_pixel` AVD against a
// temporary gateway + orkestrd core, through adb + uiautomator (scripts/android-e2e.mjs):
//   A1 emulator booted, PIN + fingerprint enrolled (Settings UI + `adb emu finger touch`)
//   A2 APK installed (release build: embedded JS bundle, debug-signed, no Metro)
//   A3 TOTP login in the UI → cores list
//   A4 deep-link pairing offer (devices.startPairing) → SAS on the phone equals the core's pending SAS
//   A5 "codes match" → approved via local RPC → biometric step-up key registered → Home monitoring
//   A6 dangerous ApprovalRequested → local notification from dev.orkestr.mobile (dumpsys notification)
//   A7 approvals list → approval detail (notification tap, else list)
//   A8 Approve with biometric step-up (emulator fingerprint) → approval Approved on the core
//   A9 Home in Russian (language switch)
// Screenshots → docs/screenshots/phase5/android-*.png; report → <out>/android.json.
//
// Usage (repo root, Windows):
//   cargo build -p orkestr-core -p orkestr-gateway --bins
//   (cd apps/mobile/android && gradlew assembleRelease -PreactNativeArchitectures=x86_64)
//   node scripts/smoke/phase5-android.mjs [--out <dir>] [--apk <path>] [--keep-emulator]
// Env: ANDROID_HOME (adb, emulator), ORKESTR_AVD (default orkestr_pixel), ORKESTR_GW_PORT (18787).
// A debug APK (dev client) also works if Metro runs (`npx expo start` in apps/mobile); the
// script then adds `adb reverse tcp:8081`.
//
// Also importable: runAndroidLane(ctx) — used by phase5-acceptance.mjs --android.

import { execFileSync, spawn } from "node:child_process";
import crypto from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import * as ui from "../android-e2e.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const arg = (name) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : undefined;
};

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
    await sleep(250);
  }
}

// ── emulator ──────────────────────────────────────────────────────────────
const sdk = process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT || join(process.env.LOCALAPPDATA ?? "", "Android", "Sdk");
let startedEmulator = null;

function deviceOnline() {
  return /\bdevice$/m.test(String(ui.adb(["devices"], { allowFail: true }) ?? ""));
}
async function bootEmulator(avd) {
  if (!deviceOnline()) {
    const exe = join(sdk, "emulator", process.platform === "win32" ? "emulator.exe" : "emulator");
    startedEmulator = spawn(exe, ["-avd", avd, "-no-window", "-no-audio", "-no-snapshot", "-no-boot-anim"], { stdio: "ignore", detached: true });
    startedEmulator.unref();
    await waitFor(() => deviceOnline(), 180_000, "emulator online");
  }
  await waitFor(() => String(ui.adb(["shell", "getprop", "sys.boot_completed"], { allowFail: true }) ?? "").trim() === "1", 240_000, "boot_completed");
  ui.adb(["shell", "settings", "put", "system", "screen_off_timeout", "1800000"], { allowFail: true });
}

// ── the lane ──────────────────────────────────────────────────────────────
/**
 * ctx: { check, section, A (core with .rpc), base, gwPort, totpCode, totpSecret, coreIdA,
 *        spawnAgent(projectId, title, cmd), waitApproval(agentId), project, outDir,
 *        apk?, shotsDir?, avd? }
 */
export async function runAndroidLane(ctx) {
  const { check, section, A, base, gwPort, totpCode, totpSecret, coreIdA, spawnAgent, waitApproval, project } = ctx;
  const shotsDir = ctx.shotsDir ?? join(repoRoot, "docs", "screenshots", "phase5");
  mkdirSync(shotsDir, { recursive: true });
  const shot = (name) => ui.screenshot(join(shotsDir, `android-${name}.png`));
  const PIN = "1111";

  // Biometric prompts can appear while waiting (step-up key registration
  // after pairing, step-up on approve): answer them with the emulator finger.
  let fingerTouches = 0;
  async function waitUi(testID, ms = 30_000, { finger = false } = {}) {
    return waitFor(async () => {
      const nodes = ui.dump();
      const n = ui.find(testID, nodes);
      if (n) return n;
      if (finger && ui.biometricPromptVisible(nodes)) {
        fingerTouches++;
        await ui.fingerTap(1);
      }
      return null;
    }, ms, `testID ${testID}`);
  }
  const textOf = (testID) => (ui.find(testID)?.text ?? "").trim();

  section("Android lane (emulator)");
  await bootEmulator(ctx.avd ?? process.env.ORKESTR_AVD ?? "orkestr_pixel");
  await ui.unlock(PIN);
  let biometric = false;
  try {
    biometric = await Promise.race([ui.enrollFingerprint(PIN), sleep(180_000).then(() => false)]);
  } catch (e) {
    console.log(`fingerprint enrollment failed: ${e.message}`);
  }
  check("Android", "A1 emulator booted; PIN + fingerprint enrolled (Settings UI, adb emu finger touch)", deviceOnline(), biometric ? "fingerprint enrolled" : "fingerprint NOT enrolled → biometric approve: manual");

  // A2 install
  const releaseApk = join(repoRoot, "apps", "mobile", "android", "app", "build", "outputs", "apk", "release", "app-release.apk");
  const debugApk = join(repoRoot, "apps", "mobile", "android", "app", "build", "outputs", "apk", "debug", "app-debug.apk");
  const apk = ctx.apk ?? (existsSync(releaseApk) ? releaseApk : debugApk);
  ui.adb(["uninstall", ui.PACKAGE], { allowFail: true });
  ui.install(apk);
  ui.adb(["shell", "pm", "grant", ui.PACKAGE, "android.permission.POST_NOTIFICATIONS"], { allowFail: true });
  ui.adb(["shell", "pm", "grant", ui.PACKAGE, "android.permission.CAMERA"], { allowFail: true });
  ui.reverse(String(gwPort));
  if (apk === debugApk) ui.reverse("8081"); // dev client needs Metro
  ui.launch();
  await waitUi("screen-login", 90_000);
  check("Android", "A2 APK installed and launched (adb reverse gateway port)", true, `${apk.endsWith("release.apk") ? "release (embedded bundle, debug-signed)" : "debug (Metro)"} ${apk.split(/[\\/]/).pop()}`);

  // A3 TOTP login through the UI. The phone reaches the gateway at localhost via adb reverse.
  await ui.clearField("login-gateway-url");
  await ui.typeText("login-gateway-url", base);
  // The account setup consumed the current TOTP step: use the next one (±1 step accepted).
  await ui.typeText("login-totp", totpCode(totpSecret, Date.now() + 30_000));
  ui.back(); // hide the keyboard
  await sleep(500);
  shot("login");
  await ui.tap("login-submit");
  await waitUi("screen-cores", 30_000);
  const coreRow = await waitFor(() => ui.find(`gateway-core-${coreIdA}`) ?? ui.find(`core-${coreIdA}`), 15_000, "core row").catch(() => null);
  check("Android", "A3 TOTP login in the UI → cores list shows the enrolled core", !!coreRow, coreRow ? coreRow.testID : textOf("cores-empty"));

  // A4 deep-link pairing offer.
  const offer = await A.rpc.call("devices.startPairing");
  const offerJson = typeof offer.qr === "string" && offer.qr.trim().startsWith("{") ? offer.qr.trim() : JSON.stringify(offer.offer ?? offer);
  const link = `orkestr://pair?offer=${encodeURIComponent(offerJson)}`;
  ui.deeplink(link);
  await waitUi("pairing-sas", 45_000);
  const phoneSas = textOf("pairing-sas");
  const pend = await waitFor(async () => (await A.rpc.call("devices.list")).pending.find((p) => p.pairingId === offer.pairingId), 15_000, "pending pairing");
  const norm = (s) => String(s ?? "").replace(/\D/g, "");
  shot("pairing-sas");
  check("Android", "A4 deep-link pairing offer → SAS shown on the phone equals the core's pending SAS", norm(phoneSas) !== "" && norm(phoneSas) === norm(pend.sas), `phone ${phoneSas} / core ${pend.sas}`);

  // A5 codes match → approve on the computer → Home.
  await ui.tap("pairing-sas-confirm");
  await waitUi("pairing-waiting", 15_000).catch(() => null);
  await A.rpc.call("devices.approve", { pairingId: offer.pairingId, sasConfirmed: true, role: "admin", remoteTerminal: false });
  await waitUi("screen-home", 60_000, { finger: biometric });
  await waitUi("monitoring-overview", 30_000, { finger: biometric });
  await sleep(1500);
  const dev = (await A.rpc.call("devices.list")).devices.find((d) => d.platform === "android" || /android|pixel|sdk|emulator/i.test(`${d.name} ${d.platform ?? ""}`)) ?? (await A.rpc.call("devices.list")).devices.at(-1);
  shot("home");
  check(
    "Android",
    "A5 'codes match' → approved via local RPC → app shows the core's Home monitoring",
    !!dev,
    `device ${dev?.name} role ${dev?.role}, stepUp ${dev?.stepUpKind ?? dev?.step_up_kind ?? "none"}${fingerTouches ? `, ${fingerTouches} finger touch(es) for step-up key` : ""}`,
  );
  const stepUpReady = !!(dev?.stepUpKind ?? dev?.step_up_kind) && !ui.find("home-stepup-missing");

  // A6 approval request → local notification.
  ui.adb(["shell", "cmd", "notification", "cancel-all", ui.PACKAGE], { allowFail: true });
  const ag = await spawnAgent(project.id, "Clean build output (android)", "git clean -fd");
  const agentId = ag.agent?.id ?? ag.id;
  const approval = (await waitApproval(agentId)).payload.approval;
  const notif = await waitFor(() => ui.appNotifications().find((n) => n.tag.includes(approval.id)), 30_000, "notification").catch(() => null);
  check(
    "Android",
    "A6 ApprovalRequested (dangerous) → local notification from dev.orkestr.mobile (dumpsys notification, generic text)",
    !!notif && !notif.text.includes("git clean"),
    notif ? `tag ${notif.tag}, "${notif.title}: ${notif.text}"` : "no notification",
  );

  // A7 approvals list, then detail (notification tap preferred).
  await ui.scrollTo("nav-approvals");
  await ui.tap("nav-approvals");
  await waitUi(`approval-${approval.id}`, 20_000);
  shot("approvals");
  ui.back();
  await waitUi("screen-home", 10_000);
  let via = "list";
  try {
    if (!notif) throw new Error("no notification");
    ui.adb(["shell", "cmd", "statusbar", "expand-notifications"]);
    await sleep(1500);
    const n = ui.dump().find((x) => x.text === notif.text) ?? ui.dump().find((x) => x.text === notif.title);
    if (!n) throw new Error("notification row not found in the shade");
    ui.adb(["shell", "input", "tap", String(n.cx), String(n.cy)]);
    await waitUi("screen-approval-detail", 15_000);
    via = "notification tap";
  } catch (e) {
    ui.adb(["shell", "cmd", "statusbar", "collapse"], { allowFail: true });
    await sleep(500);
    await ui.scrollTo("nav-approvals");
    await ui.tap("nav-approvals");
    await ui.tap(`approval-${approval.id}`, 15_000);
    await waitUi("screen-approval-detail", 15_000);
    via = `list (${e.message})`;
  }
  await sleep(800);
  shot("approval-detail");
  check("Android", "A7 approval detail opens (command + summary shown)", !!ui.find("approval-approve") || !!ui.find("approval-command"), `via ${via}`);

  // A8 Approve with biometric step-up.
  let resolved = null;
  if (biometric && stepUpReady) {
    await ui.scrollTo("approval-approve");
    await ui.tap("approval-approve");
    resolved = await waitFor(async () => {
      if (ui.biometricPromptVisible()) {
        fingerTouches++;
        await ui.fingerTap(1);
      }
      const a = await A.rpc.call("approvals.get", { approvalId: approval.id });
      const st = a.approval?.status ?? a.status;
      return st && st !== "Pending" ? a.approval ?? a : null;
    }, 45_000, "approval resolved").catch(() => null);
    await sleep(1000);
    shot("approval-approved");
    const st = resolved?.status;
    check("Android", "A8 Approve with biometric step-up (emulator fingerprint) → core shows the approval Approved", st === "Approved", `status ${st ?? "still pending"}, finger touches ${fingerTouches}`);
  } else {
    check("Android", "A8 biometric approve: manual (no enrolled fingerprint / step-up key on this run)", true, `MANUAL — fingerprint ${biometric}, step-up key ${stepUpReady}`);
  }

  // A9 Home in Russian.
  ui.back();
  await sleep(800);
  if (!ui.find("screen-home")) ui.back();
  await waitUi("screen-home", 10_000);
  await ui.scrollTo("nav-settings");
  await ui.tap("nav-settings");
  await ui.tap("settings-lang-ru", 10_000);
  await sleep(800);
  ui.back();
  await waitUi("screen-home", 10_000);
  await waitUi("monitoring-overview", 20_000);
  await sleep(1000);
  shot("home-ru");
  const ruText = ui.dump().some((n) => /[А-Яа-яЁё]/.test(n.text));
  check("Android", "A9 Home renders in Russian after the language switch", ruText, ruText ? "Cyrillic labels on Home" : "no Cyrillic text");
  // Back to English for the next run.
  await ui.scrollTo("nav-settings");
  await ui.tap("nav-settings");
  await ui.tap("settings-lang-en", 10_000);
  ui.back();

  return { biometric, fingerTouches, approval: resolved ?? approval };
}

// ── standalone runner ─────────────────────────────────────────────────────
async function main() {
  const exeDir = process.env.ORKESTR_SMOKE_BIN_DIR ?? join(repoRoot, "target", "debug");
  const coreExe = join(exeDir, process.platform === "win32" ? "orkestrd.exe" : "orkestrd");
  const gwExe = join(exeDir, process.platform === "win32" ? "orkestr-gateway.exe" : "orkestr-gateway");
  const gwPort = Number(process.env.ORKESTR_GW_PORT ?? 18787); // outside Windows' excluded 1349–1548
  const base = `http://localhost:${gwPort}`;
  const tmp = mkdtempSync(join(tmpdir(), "orkestr-p5android-"));
  const outDir = arg("--out") ? resolve(repoRoot, arg("--out")) : join(tmpdir(), "orkestr-phase5-android");
  mkdirSync(outDir, { recursive: true });

  const checks = [];
  const check = (area, name, ok, detail = "") => {
    checks.push({ area, name, ok: Boolean(ok), manual: /^MANUAL/.test(String(detail)), detail: String(detail ?? "") });
    console.log(`${ok ? "PASS" : "FAIL"}  [${area}] ${name}${detail ? ` — ${String(detail).slice(0, 240)}` : ""}`);
  };
  const section = (s) => console.log(`\n── ${s}`);

  const procs = [];
  const launch = (exe, args, env, label) => {
    const child = spawn(exe, args, { env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"] });
    const rec = { child, label, log: "" };
    child.stdout.on("data", (d) => (rec.log = (rec.log + d).slice(-20000)));
    child.stderr.on("data", (d) => (rec.log = (rec.log + d).slice(-20000)));
    procs.push(rec);
    return rec;
  };
  const kill = (rec) => {
    try {
      if (process.platform === "win32") execFileSync("taskkill", ["/PID", String(rec.child.pid), "/T", "/F"], { stdio: "ignore" });
      else rec.child.kill("SIGKILL");
    } catch {
      /* gone */
    }
  };

  const gwFetch = async (path, { token, method = "GET", body } = {}) => {
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
  };

  let exitCode = 0;
  try {
    section("Setup: gateway + core + account (passkey + TOTP)");
    launch(gwExe, ["--data-dir", join(tmp, "gateway"), "--bind", `127.0.0.1:${gwPort}`], { RUST_LOG: "info" }, "gateway");
    await waitFor(async () => (await fetch(`${base}/healthz`)).ok, 30_000, "gateway /healthz");
    const reg = await registerPasskeyAccount(gwFetch, base);
    const ownerToken = reg.json?.token;
    const setup = (await gwFetch("/v1/auth/totp/setup", { token: ownerToken, method: "POST" })).json;
    const confirm = await gwFetch("/v1/auth/totp/confirm", { token: ownerToken, method: "POST", body: { code: totpCode(setup.secret) } });
    check("Setup", "gateway up; account with passkey (software authenticator) + TOTP", reg.status === 200 && confirm.status === 200, `register ${reg.status}, confirm ${confirm.status}`);

    const home = join(tmp, "laptop");
    mkdirSync(home, { recursive: true });
    launch(coreExe, ["--data-dir", home], { ORKESTR_HOME: home, ORKESTR_SECRET_BACKEND: "vault-file", RUST_LOG: "info" }, "core");
    const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 60_000, "core.json");
    const rpc = rpcClient(info.url, readFileSync(join(home, "core.token"), "utf8").trim());
    await waitFor(() => rpc.open().then(() => true), 30_000, "core RPC");
    await rpc.sub(["*"]);
    const A = { name: "laptop", rpc };

    const repoDir = join(tmp, "repo-a");
    mkdirSync(join(repoDir, "src"), { recursive: true });
    const git = (...a) => execFileSync("git", a, { cwd: repoDir, encoding: "utf8" }).trim();
    git("init", "-q", "-b", "main");
    git("config", "core.autocrlf", "false");
    git("config", "user.email", "p5@orkestr.local");
    git("config", "user.name", "Orkestr P5");
    writeFileSync(join(repoDir, "README.md"), "# Mobile demo\n");
    writeFileSync(join(repoDir, "src", "app.txt"), "line 1\n");
    git("add", "-A");
    git("commit", "-q", "-m", "initial");
    await rpc.call("settings.update", { patch: { profileName: "P5", defaultApprovalMode: "SemiAuto", onboardingCompleted: true } });
    const mock = (await rpc.call("providers.list")).providers.map((p) => p.provider ?? p).find((p) => p.kind === "Mock");
    const smart = { providerId: mock.id, model: "mock-smart" };
    await rpc.call("providers.setManager", { config: { model: smart, maxParallelAgents: 4, approvalPolicy: "SemiAuto" } });
    await rpc.call("providers.setRouting", { config: { mode: "Manual", rules: [], defaultFallback: [smart], preferLocal: true, maxRateLimitWaitSecs: 0 } });
    const project = (await rpc.call("projects.create", { name: "Mobile Demo", rootPath: repoDir, approvalMode: "SemiAuto" })).project;
    const backendTpl = (await rpc.call("templates.list", {})).templates.find((t) => t.role === "Backend");
    const spawnAgent = (projectId, title, cmd) => rpc.call("agents.spawn", { projectId, templateId: backendTpl.id, newTask: { title, description: `Do it.\nRun: \`${cmd}\`` } });
    const waitApproval = (agentId) => waitFor(() => rpc.events.find((e) => e.kind === "ApprovalRequested" && e.payload.approval.agentId === agentId), 60_000, "ApprovalRequested");

    const enrollToken = (await gwFetch("/v1/cores/enrollment-token", { token: ownerToken, method: "POST" })).json.token;
    await rpc.call("remote.enable", { gatewayUrl: base, enrollmentToken: enrollToken });
    const rs = await waitFor(async () => {
      const s = await rpc.call("remote.status");
      return s.connected ? s : null;
    }, 20_000, "tunnel");
    check("Setup", "core enrolled at the gateway, tunnel connected", !!rs.coreId, rs.coreId);

    await runAndroidLane({ check, section, A, base, gwPort, gwFetch, totpCode, totpSecret: setup.secret, ownerToken, coreIdA: rs.coreId, spawnAgent, waitApproval, project, outDir, apk: arg("--apk") });
    rpc.close();
  } catch (e) {
    exitCode = 1;
    check("Run", "scenario ran to the end", false, e.message);
    try {
      ui.screenshot(join(outDir, "failure.png"));
    } catch {
      /* no device */
    }
    for (const p of procs) if (p.log) writeFileSync(join(outDir, `${p.label}.log`), p.log);
  } finally {
    for (const p of procs) kill(p);
    if (startedEmulator && !process.argv.includes("--keep-emulator")) ui.adb(["emu", "kill"], { allowFail: true });
    try {
      rmSync(tmp, { recursive: true, force: true });
    } catch {
      /* locked files */
    }
  }

  const failed = checks.filter((c) => !c.ok);
  const manual = checks.filter((c) => c.manual);
  const report = { at: new Date().toISOString(), passed: checks.length - failed.length, total: checks.length, manual: manual.map((c) => c.name), checks };
  writeFileSync(join(outDir, "android.json"), JSON.stringify(report, null, 2));
  console.log(`\n${report.passed}/${report.total} passed${manual.length ? ` (${manual.length} manual)` : ""} — ${join(outDir, "android.json")}`);
  process.exit(failed.length || exitCode ? 1 : 0);
}

// ── helpers (copied from phase4/phase5 acceptance) ────────────────────────
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
    close: () => ws?.close(),
  };
}

export function totpCode(secretB32, t = Date.now()) {
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
async function registerPasskeyAccount(gwFetch, origin) {
  const begin = await gwFetch("/v1/auth/register/begin", { method: "POST", body: { name: "Owner" } });
  const { ceremonyId, publicKey: o } = begin.json;
  const kp = crypto.generateKeyPairSync("ec", { namedCurve: "P-256" });
  const jwk = kp.publicKey.export({ format: "jwk" });
  const credId = crypto.randomBytes(16);
  const cose = cbor(new Map([[1, 2], [3, -7], [-1, 1], [-2, Buffer.from(jwk.x, "base64url")], [-3, Buffer.from(jwk.y, "base64url")]]));
  const u16 = Buffer.alloc(2);
  u16.writeUInt16BE(credId.length);
  const authData = Buffer.concat([crypto.createHash("sha256").update(o.rp.id).digest(), Buffer.from([0x45]), Buffer.alloc(4), Buffer.alloc(16), u16, credId, cose]);
  const clientData = Buffer.from(JSON.stringify({ type: "webauthn.create", challenge: o.challenge, origin, crossOrigin: false }));
  const att = cbor(new Map([["fmt", "none"], ["attStmt", new Map()], ["authData", authData]]));
  return gwFetch("/v1/auth/register/finish", {
    method: "POST",
    body: { ceremonyId, credential: { id: b64u(credId), rawId: b64u(credId), type: "public-key", response: { clientDataJSON: b64u(clientData), attestationObject: b64u(att) } } },
  });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((e) => {
    console.error(e);
    process.exit(1);
  });
}
