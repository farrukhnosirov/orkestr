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
//           benchmarks have rows; a team; AI Product Manager draft → approve →
//           tasks
//   shots   (desktop only) app-window screenshots in English and Russian of
//           the Phase 2 surfaces, captured from the webview through WebView2
//           DevTools (`Page.captureScreenshot`) — never the screen
//
// Usage (repo root, after `cargo build -p orkestr-desktop`):
//   node scripts/smoke/phase2-acceptance.mjs              # target/debug/orkestr-desktop(.exe)
//   node scripts/smoke/phase2-acceptance.mjs --headless   # target/debug/orkestrd(.exe)
//   ORKESTR_SMOKE_EXE=path/to/exe node scripts/smoke/phase2-acceptance.mjs
//   --keep  keeps the temp dirs (printed at start) for inspection
//   --no-shots  skips the screenshots; --out <dir> (default docs/screenshots/phase2)
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
const shotsOn = !headless && !process.argv.includes("--no-shots");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "phase2"));
const cdpPort = Number(process.env.ORKESTR_CDP_PORT ?? 9372);
const exe =
  process.env.ORKESTR_SMOKE_EXE ??
  join(repo, "target", "debug", `${headless ? "orkestrd" : "orkestr-desktop"}${isWin ? ".exe" : ""}`);

const results = [];
let failures = 0;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const norm = (p) => resolve(p).replace(/\\/g, "/").toLowerCase();

/** Records a check; a failed check is counted but the run goes on. */
function check(name, ok, detail = "") {
  results.push({ name, ok: Boolean(ok), detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  — ${String(detail).slice(0, 600)}` : ""}`);
  return Boolean(ok);
}
/** A check later steps depend on: a failure aborts the run. */
function must(name, ok, detail = "") {
  if (!check(name, ok, detail)) throw new Error(`check failed: ${name} ${detail}`);
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
  '{\n  "name": "payroll-demo",\n  "private": true,\n  "type": "module",\n  "scripts": { "build": "node --check src/money.mjs", "test": "node --test" },\n  "dependencies": { "decimal.js": "^10.4.3" }\n}\n',
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
      ...(shotsOn ? { WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${cdpPort}` } : {}),
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

// ── CDP (screenshots of the app window's webview only) ──────────────────
class Cdp {
  constructor(wsUrl) {
    this.ws = new WebSocket(wsUrl);
    this.id = 0;
    this.pending = new Map();
    this.console = [];
  }
  async open() {
    await new Promise((r, j) => {
      this.ws.onopen = r;
      this.ws.onerror = j;
    });
    this.ws.onmessage = (m) => {
      const f = JSON.parse(m.data);
      if (f.id && this.pending.has(f.id)) {
        const p = this.pending.get(f.id);
        this.pending.delete(f.id);
        f.error ? p.reject(new Error(f.error.message)) : p.resolve(f.result);
      } else if (f.method === "Runtime.consoleAPICalled" && f.params.type === "error") {
        this.console.push(`error: ${f.params.args.map((a) => a.value ?? a.description ?? "").join(" ")}`);
      } else if (f.method === "Runtime.exceptionThrown") {
        const d = f.params.exceptionDetails;
        this.console.push(`exception: ${d.exception?.description ?? d.text}`);
      }
    };
    await this.send("Runtime.enable");
    await this.send("Page.enable");
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((res, rej) => {
      if (this.ws.readyState !== 1) return rej(new Error("target closed"));
      this.pending.set(id, { resolve: res, reject: rej });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  async eval(expr) {
    const r = await this.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  }
  async key(key, { ctrl = false, shift = false } = {}) {
    const modifiers = (ctrl ? 2 : 0) | (shift ? 8 : 0);
    const code = key.length === 1 ? `Key${key.toUpperCase()}` : key;
    const vk = key.length === 1 ? key.toUpperCase().charCodeAt(0) : { Enter: 13, Escape: 27 }[key];
    for (const type of ["rawKeyDown", "keyUp"]) {
      const extra = key === "Enter" && type === "rawKeyDown" ? { type: "keyDown", text: "\r", unmodifiedText: "\r" } : {};
      await this.send("Input.dispatchKeyEvent", { type, modifiers, key, code, windowsVirtualKeyCode: vk, ...extra });
    }
  }
  async type(text) {
    await this.send("Input.insertText", { text });
  }
  close() {
    this.ws.close();
  }
}

async function attachMain() {
  const t = await waitFor(
    async () => {
      try {
        const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
        return list.find(
          (x) => x.type === "page" && !x.url.includes("robot.html") && !x.url.includes("#/panel") && !x.url.includes("hud") && /tauri\.localhost|127\.0\.0\.1:1420/.test(x.url),
        );
      } catch {
        return null;
      }
    },
    60_000,
    "main webview DevTools target",
  );
  const cdp = new Cdp(t.webSocketDebuggerUrl);
  await cdp.open();
  return cdp;
}

// Visible text that is clipped, plus Latin-only phrases in the Russian UI.
const AUDIT = `(() => {
  const vis = (el) => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none' && r.bottom > 0 && r.right > 0 && r.top < innerHeight && r.left < innerWidth; };
  const clipped = [];
  for (const el of document.querySelectorAll('body *')) {
    if (!vis(el) || el.closest('.monaco-editor, .xterm, svg, .react-flow__edge, pre, code, textarea, input')) continue;
    const own = [...el.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent).join('').trim();
    if (!own) continue;
    const s = getComputedStyle(el);
    const hidesX = ['hidden', 'clip'].includes(s.overflowX) || s.textOverflow === 'ellipsis';
    if (hidesX && el.scrollWidth > el.clientWidth + 1) clipped.push(own.slice(0, 80));
  }
  const latin = new Set();
  if (document.documentElement.lang === 'ru') {
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let n; (n = walker.nextNode()); ) {
      const p = n.parentElement; if (!p || !vis(p) || p.closest('.monaco-editor, .xterm, pre, code, kbd, script, style, textarea, input, [data-user-content]')) continue;
      const t = n.textContent.trim();
      if (/[A-Za-z]{3,}\\s+[A-Za-z]{3,}/.test(t) && !/[А-Яа-яЁё]/.test(t)) latin.add(t.slice(0, 100));
    }
  }
  const hscroll = document.documentElement.scrollWidth > innerWidth + 1;
  // Scroll containers that overflow sideways (a stray horizontal scrollbar).
  const sideways = [];
  for (const el of document.querySelectorAll('body *')) {
    if (!vis(el) || el.closest('.monaco-editor, .xterm, pre, code, table, .react-flow, .mm-mono')) continue;
    const s = getComputedStyle(el);
    if (['auto', 'scroll'].includes(s.overflowX) && el.scrollWidth > el.clientWidth + 1 && el.clientWidth > 200) {
      const right = el.getBoundingClientRect().right;
      const wide = [...el.querySelectorAll('*')].filter((c) => c.getBoundingClientRect().right > right + 1).slice(0, 3).map((c) => String(c.className || c.tagName).slice(0, 60));
      sideways.push(String(el.className || el.tagName).slice(0, 60) + ' ' + el.scrollWidth + '>' + el.clientWidth + ' ' + wide.join(' | '));
    }
  }
  return { clipped: [...new Set(clipped)].slice(0, 30), latin: [...latin].slice(0, 30), hscroll, sideways: sideways.slice(0, 8) };
})()`;

const shots = [];
/** Closes the "since your last session" card so it does not cover panels. */
const dismissResume = (cdp) =>
  cdp.eval(`(() => { const b = document.querySelector('.resume-card header button, .resume-card button[aria-label]'); b?.click(); return !!b; })()`).catch(() => false);
/** Switches the UI language through the settings (live `SettingsChanged`). */
async function setLanguage(rpc, cdp, lang) {
  await rpc.call("settings.update", { patch: { language: lang } });
  await waitFor(() => cdp.eval(`document.documentElement.lang === '${lang}'`).catch(() => false), 20_000, `${lang} UI`);
  await sleep(1500);
}
async function capture(cdp, name, note) {
  await sleep(1100);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
  let audit = { clipped: [], latin: [], hscroll: false };
  try {
    audit = await cdp.eval(AUDIT);
  } catch (e) {
    audit.error = String(e.message ?? e);
  }
  const errors = cdp.console.splice(0);
  shots.push({ name: `${name}.png`, note, consoleErrors: errors, ...audit });
  console.log(`  shot ${name}${errors.length ? `  — ${errors.length} console error(s)` : ""}`);
  for (const e of errors) console.log(`      ! ${e.slice(0, 300)}`);
  for (const c of audit.clipped) console.log(`      clipped: ${c}`);
  for (const l of audit.latin) console.log(`      latin: ${l}`);
  if (audit.hscroll) console.log("      horizontal page scroll!");
  for (const w of audit.sideways ?? []) console.log(`      sideways: ${w}`);
}
async function palette(cdp, text) {
  await cdp.eval("document.activeElement?.blur?.(), true");
  await cdp.key("k", { ctrl: true });
  await sleep(300);
  await cdp.type(text);
  await sleep(450);
  await cdp.key("Enter");
  await sleep(800);
  const still = await cdp
    .eval(`(() => { if (!document.querySelector('.palette')) return false; const o = document.querySelector('.palette [aria-selected="true"]'); o?.click(); return true; })()`)
    .catch(() => false);
  if (still) await sleep(800);
}
const clickText = (cdp, selector, text) =>
  cdp.eval(`(() => {
    const els = [...document.querySelectorAll(${JSON.stringify(selector)})].filter((e) => e.getClientRects().length);
    const want = ${JSON.stringify(text)};
    const el = els.find((e) => e.textContent.trim() === want) ?? els.find((e) => e.textContent.trim().includes(want));
    if (!el) return false;
    el.scrollIntoView({ block: "center" });
    el.click();
    return true;
  })()`);
const scrollTo = (cdp, selector, text) =>
  cdp.eval(`(() => {
    const els = [...document.querySelectorAll(${JSON.stringify(selector)})].filter((e) => e.getClientRects().length);
    const el = els.find((e) => e.textContent.trim().includes(${JSON.stringify(text)}));
    if (!el) return false;
    el.scrollIntoView({ block: "start" });
    return true;
  })()`);
async function step(label, fn) {
  try {
    await fn();
  } catch (e) {
    console.log(`  step failed (${label}): ${String(e.message ?? e).slice(0, 300)}`);
    check(`screenshot step: ${label}`, false, String(e.message ?? e).slice(0, 200));
  }
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
const ACTIVE = ["Running", "Review", "Testing", "Waiting"];
const runBrief = (run) =>
  `${run.status}: ` +
  run.tasks
    .map((t) => `${t.key}/${t.role}=${t.status}${t.pipeline ? `[${t.pipeline.status} ${t.pipeline.stages.map((s) => `${s.kind.slice(0, 2)}:${s.status}`).join(",")} heal ${t.pipeline.healingAttempts}]` : ""}`)
    .join(" ");

/**
 * Follows a run to its end. `onApproval(approval)` decides pending approvals
 * of the project ("Approve" / "Reject" / null = leave pending).
 */
async function followRun(rpc, projectId, runId, { timeoutMs = 600_000, onApproval = () => null } = {}) {
  let last = "";
  let maxActive = 0;
  const handled = new Set();
  const decided = [];
  const run = await waitFor(
    async () => {
      const r = await runOf(rpc, runId);
      maxActive = Math.max(maxActive, r.tasks.filter((t) => ACTIVE.includes(t.status)).length);
      const b = runBrief(r);
      if (b !== last) console.log(`   … ${(last = b)}`);
      const pending = (await rpc.call("approvals.list", { projectId, status: "Pending" }).catch(() => ({ approvals: [] }))).approvals ?? [];
      for (const a of pending) {
        if (handled.has(a.id)) continue;
        const decision = onApproval(a);
        if (!decision) continue;
        handled.add(a.id);
        console.log(`   … ${decision}: ${a.actionSummary}`);
        decided.push({ approval: a, decision });
        await rpc.call("approvals.resolve", { approvalId: a.id, decision, note: "phase 2 acceptance" });
      }
      return ["Completed", "Failed", "Cancelled", "Interrupted"].includes(r.status) ? r : null;
    },
    timeoutMs,
    "run to finish",
  );
  return { run, maxActive, decided };
}

// ── scenario ──────────────────────────────────────────────────────────────
let rpc;
let fake;
let cdp;
try {
  launch();
  ({ rpc } = await connect());
  must("app started: discovery + auth", true, `pid ${child.pid}`);

  section("0. setup (Mock as Manager + agents)");
  await rpc.call("settings.update", {
    patch: { profileName: "Acceptance", defaultApprovalMode: "SemiAuto", onboardingCompleted: true, language: "en" },
  });
  const providers = (await rpc.call("providers.list")).providers.map((p) => p.provider ?? p);
  const mock = providers.find((p) => p.kind === "Mock");
  must("Mock (offline demo) provider present", !!mock, providers.map((p) => p.name).join(", "));
  const smart = { providerId: mock.id, model: "mock-smart" };
  const routeTo = (chain) =>
    rpc.call("providers.setRouting", {
      config: { mode: "Manual", rules: [], defaultFallback: chain, preferLocal: true, maxRateLimitWaitSecs: 0 },
    });
  await rpc.call("providers.setManager", { config: { model: smart, maxParallelAgents: 3, approvalPolicy: "SemiAuto" } });
  await routeTo([smart]);
  const { project } = await rpc.call("projects.create", { name: "Payroll", rootPath: repoDir, approvalMode: "SemiAuto" });
  const projectId = project.id;
  must("projects.create (temp npm repo)", !!projectId, projectId);
  const pcfg = await rpc.call("pipeline.getConfig", { projectId });
  check(
    "quality pipeline on by default: every stage, healing limits",
    pcfg.enabled && pcfg.stages.length === 6 && pcfg.healing.maxAttempts >= 1,
    `${pcfg.stages.join("→")}, ${pcfg.healing.maxAttempts} attempts, ${pcfg.healing.maxWallMins} min, merge ${pcfg.merge}`,
  );
  const detection = await rpc.call("quality.detect", { projectId });
  const detected = JSON.stringify(detection);
  check("quality runner detects the npm build + test scripts", /npm run build/.test(detected) && /npm run test/.test(detected));

  if (shotsOn) {
    mkdirSync(outDir, { recursive: true });
    cdp = await attachMain();
    await cdp.send("Emulation.setDeviceMetricsOverride", { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
    await cdp.eval("(() => { try { localStorage.removeItem('orkestr.layout.v3'); } catch {} return true; })()");
    await cdp.send("Page.reload", { ignoreCache: true });
    await waitFor(() => cdp.eval("!!document.querySelector('.app__body')").catch(() => false), 60_000, "app shell");
    await sleep(2000);
    await cdp.eval(`document.querySelector('[aria-label="Open project Payroll"]')?.click(), true`);
    await sleep(1200);
    await dismissResume(cdp);
  }

  // ── A ───────────────────────────────────────────────────────────────────
  section("A. multi-agent payroll (§63 A-lite)");
  const goal = "Create a payroll module: architecture review, backend, tests, security review";
  const { reply } = await chatForPlan(rpc, projectId, goal);
  must("manager.chat → plan", !!reply.planId);
  const plan = (await rpc.call("manager.history", { projectId })).plans.find((p) => p.id === reply.planId);
  const roles = plan.tasks.map((t) => t.role);
  check(
    "plan has Architect / Backend / QA / Security tasks with dependencies",
    ["Architect", "Backend", "Qa", "Security"].every((r) => roles.includes(r)) && plan.tasks.some((t) => t.dependsOn.length),
    plan.tasks.map((t) => `${t.key}:${t.role}${t.dependsOn.length ? `←${t.dependsOn.join("+")}` : ""}`).join(" "),
  );
  const backendPlanTask = plan.tasks.find((t) => t.role === "Backend");
  const backendTitle = `${backendPlanTask.title} [mock:heal-once]`;
  const approved = await rpc.call("manager.approvePlan", {
    planId: plan.id,
    edits: { removeTasks: [], titles: [{ key: backendPlanTask.key, title: backendTitle }] },
  });
  const runId = approved.run.id;
  must("plan approved (backend task carries an injected failing test) → run", !!runId, backendTitle);
  const { run: run1, maxActive } = await followRun(rpc, projectId, runId, { onApproval: () => "Approve" });
  check("run completed", run1.status === "Completed", runBrief(run1));
  // Tasks that changed files go through the quality pipeline; a review-only
  // task (the security review: no diff) completes with its own report.
  const changing = run1.tasks.filter((t) => t.filesChanged.length > 0);
  check(
    "every task completed; every changing task passed its quality pipeline",
    run1.tasks.every((t) => t.status === "Completed") &&
      changing.length >= 4 &&
      changing.every((t) => t.pipeline?.status === "Passed" && t.pipeline.stages.filter((s) => s.kind !== "Merge").every((s) => s.status === "Passed")),
    run1.tasks.map((t) => `${t.role}:${t.status}/${t.pipeline?.status ?? `no pipeline (${t.filesChanged.length} files)`}`).join(" "),
  );
  check("agents ran in parallel", maxActive >= 2, `max ${maxActive} tasks active at once`);
  const wts = run1.tasks.map((t) => t.worktreePath).filter(Boolean);
  check(
    "each task had its own agent branch + worktree (main checkout untouched during the run)",
    wts.length === run1.tasks.length && new Set(wts.map(norm)).size === wts.length && wts.every((w) => norm(w) !== norm(repoDir)),
    run1.tasks.map((t) => t.branch).join(", "),
  );
  const backend = run1.tasks.find((t) => t.role === "Backend");
  check(
    "Self-Healing fixed the injected failing test (1 attempt, pipeline passed)",
    backend.pipeline?.healingAttempts >= 1 && backend.pipeline?.status === "Passed",
    `${backend.pipeline?.healingAttempts} attempt(s)`,
  );
  const qruns = (await rpc.call("quality.runs", { projectId, limit: 200 })).runs;
  const qres = qruns.flatMap((r) => r.results.map((c) => ({ ...c, taskId: r.taskId })));
  const npmTestPassed = qres.filter((c) => /^npm run test/.test(c.command) && c.status === "Passed");
  const npmTestFailed = qres.filter((c) => /^npm run test/.test(c.command) && c.status === "Failed" && c.taskId === backend.taskId);
  const npmBuild = qres.filter((c) => /^npm run build/.test(c.command) && c.status === "Passed");
  check(
    "quality runner really ran npm (Windows cmd shim): build + test passed",
    npmTestPassed.length > 0 && npmTestPassed.every((c) => c.passed > 0) && npmBuild.length > 0,
    `${qruns.length} runs; npm test passed ×${npmTestPassed.length} (${npmTestPassed.map((c) => c.passed).join("/")} tests), npm build passed ×${npmBuild.length}`,
  );
  check(
    "the injected failure was a real failing npm test (parsed failure)",
    npmTestFailed.length > 0 && npmTestFailed.some((c) => c.failed > 0 || c.failures.length > 0),
    npmTestFailed.map((c) => `${c.summary} :: ${c.failures.map((f) => `${f.name} @ ${f.file}:${f.line} — ${f.message.split(String.fromCharCode(10))[0]}`).join("; ")}`).join(" | "),
  );
  const reviews = (await rpc.call("reviews.list", { projectId, limit: 200 })).reviews;
  const adversarial = reviews.filter((r) => r.kind === "Adversarial" && r.status === "Finished");
  check(
    "standard + adversarial reviews finished",
    reviews.some((r) => r.kind === "Standard" && r.status === "Finished") && adversarial.length > 0,
    `${reviews.length} review(s): ${reviews.map((r) => `${r.kind}:${r.verdict}`).join(", ")}`,
  );
  if (shotsOn) {
    // The finished run is the newest Manager message right now.
    for (const lang of ["en", "ru"]) {
      await setLanguage(rpc, cdp, lang);
      await dismissResume(cdp);
      await palette(cdp, lang === "en" ? "Open Panel: Manager" : "Открыть панель: Менеджер");
      await palette(cdp, lang === "en" ? "Toggle Maximize Panel" : "Развернуть или восстановить панель");
      for (let i = 0; i < 3; i++) {
        await cdp.eval(`(() => { const r = [...document.querySelectorAll('.mgr-run')].pop(); r?.scrollIntoView({ block: 'start' }); return !!r; })()`);
        await sleep(500);
      }
      await capture(cdp, `${lang}-01-manager-run-pipeline`, lang === "en" ? "Manager run with per-task pipeline stages" : "Запуск Менеджера со стадиями конвейера по задачам");
      await palette(cdp, lang === "en" ? "Toggle Maximize Panel" : "Развернуть или восстановить панель");
    }
    await setLanguage(rpc, cdp, "en");
  }
  const report = run1.report;
  check(
    "final report: pipelines, merge proposals, changed files",
    !!report && report.pipelines?.length === changing.length && report.mergeProposals.length > 0 && report.filesChanged.length > 0,
    `${report?.completed} completed, files ${report?.filesChanged.join(", ")}`,
  );

  // Merge Manager: one proposal for the whole run → approve → merged into main.
  const mainBefore = git(repoDir, "rev-parse", "main");
  const { proposal } = await rpc.call("merge.propose", { projectId, runId });
  check(
    "merge.propose collects the run's branches with a recommendation",
    proposal.branches.length >= 1,
    `${proposal.branches.length} branch(es), order ${proposal.order.join(" → ")}, ${proposal.recommendation}, conflicts ${proposal.conflicts.length}`,
  );
  const exec = await rpc.call("merge.execute", { proposalId: proposal.id });
  check("merge.execute asks for approval first", exec.proposal.status === "AwaitingApproval", exec.proposal.status);
  const mergeApproval = await waitFor(
    async () => ((await rpc.call("approvals.list", { projectId, status: "Pending" })).approvals ?? []).find((a) => /merge/i.test(a.actionSummary)),
    30_000,
    "merge approval",
  );
  check("merge approval requested", !!mergeApproval, mergeApproval.actionSummary);
  if (shotsOn) {
    for (const lang of ["en", "ru"]) {
      const L = (en, ru) => (lang === "en" ? en : ru);
      await setLanguage(rpc, cdp, lang);
      await palette(cdp, L("Open Panel: Git", "Открыть панель: Git"));
      await palette(cdp, L("Toggle Maximize Panel", "Развернуть или восстановить панель"));
      await clickText(cdp, '[role="tab"]', L("Merge", "Слияние"));
      await sleep(800);
      await cdp.eval(`(() => { const items = [...document.querySelectorAll('.mm-item__main')]; const p = items.find((e) => /5 branch|5 вет/.test(e.textContent)); p?.click(); return !!p; })()`);
      await capture(cdp, `${lang}-git-merge-awaiting-approval`, L("Git → Merge Manager: run proposal awaiting approval", "Git → Слияние: предложение запуска ждёт подтверждения"));
      await palette(cdp, L("Toggle Maximize Panel", "Развернуть или восстановить панель"));
    }
    await setLanguage(rpc, cdp, "en");
  }
  await rpc.call("approvals.resolve", { approvalId: mergeApproval.id, decision: "Approve", note: "phase 2 acceptance" });
  const merged = await waitFor(
    async () => {
      const p = (await rpc.call("merge.get", { proposalId: proposal.id })).proposal;
      return ["Completed", "Failed", "Conflicted", "Aborted", "Rejected"].includes(p.status) ? p : null;
    },
    300_000,
    "merge to finish",
  );
  check("merge completed", merged.status === "Completed", `${merged.status} ${merged.statusDetail ?? ""}`);
  const mainAfter = git(repoDir, "rev-parse", "main");
  const onDisk = ["src/payroll.mjs", "tests/payroll.test.mjs", "docs/payroll-design.md"].filter((f) => existsSync(join(repoDir, f)));
  check(
    "merged into main of the temp repo (on disk)",
    mainAfter !== mainBefore && onDisk.length === 3 && git(repoDir, "status", "--porcelain") === "",
    `main ${mainBefore.slice(0, 8)} → ${mainAfter.slice(0, 8)}; ${onDisk.join(", ")}; ${git(repoDir, "log", "--oneline", "-6").replace(/\n/g, " | ")}`,
  );
  let npmOut = "";
  let npmOk = false;
  try {
    npmOut = execFileSync(isWin ? "cmd.exe" : "npm", isWin ? ["/d", "/s", "/c", "npm test"] : ["test"], {
      cwd: repoDir,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    npmOk = true;
  } catch (e) {
    npmOut = `${e.stdout ?? ""}${e.stderr ?? ""}`;
  }
  check("npm test passes on the merged main", npmOk, (npmOut.match(/# (pass|fail) \d+/g) ?? []).join(", "));

  // ── B ───────────────────────────────────────────────────────────────────
  section("B. provider fallback on limit (§63 B)");
  const hits = [];
  fake = createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      hits.push(`${req.method} ${req.url}`);
      if (req.method === "GET" && req.url.endsWith("/models")) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ object: "list", data: [{ id: "gpt-primary", object: "model" }] }));
        return;
      }
      res.writeHead(429, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          error: { message: "You exceeded your current quota, please check your plan and billing details.", type: "insufficient_quota", code: "insufficient_quota" },
        }),
      );
    });
  });
  await new Promise((r) => fake.listen(0, "127.0.0.1", r));
  const fakeUrl = `http://127.0.0.1:${fake.address().port}/v1`;
  const created = await rpc.call("providers.create", {
    kind: "OpenAiCompatible",
    name: "Primary (fake quota)",
    baseUrl: fakeUrl,
    defaultModel: "gpt-primary",
    apiKey: "sk-acceptance-fake-key",
  });
  const pview = created.provider ?? created;
  const primary = { providerId: (pview.provider ?? pview).id, model: "gpt-primary" };
  check("fake OpenAI-compatible primary provider registered", !!primary.providerId, fakeUrl);
  await routeTo([primary, smart]);
  const eventsBeforeB = rpc.events.length;
  const { reply: replyB } = await chatForPlan(rpc, projectId, "Write usage docs for the invoice module");
  must("manager.chat → plan (B)", !!replyB.planId, replyB.text?.slice(0, 200));
  const planB = (await rpc.call("manager.history", { projectId })).plans.find((p) => p.id === replyB.planId);
  const keepB = planB.tasks.find((t) => t.role === "Architect") ?? planB.tasks[0];
  const apB = await rpc.call("manager.approvePlan", {
    planId: planB.id,
    edits: { removeTasks: planB.tasks.filter((t) => t.key !== keepB.key).map((t) => t.key), titles: [] },
  });
  let toastSeen = null;
  if (shotsOn) {
    // The toast over the Manager panel alone (not the cramped default layout).
    await palette(cdp, "Open Panel: Manager");
    await palette(cdp, "Toggle Maximize Panel");
  }
  const toastPoll = shotsOn
    ? (async () => {
        for (let i = 0; i < 600 && !toastSeen; i++) {
          toastSeen = await cdp
            .eval(`[...document.querySelectorAll('.ntf-toast')].map((e) => e.textContent).find((t) => t.includes('fallback model')) ?? null`)
            .catch(() => null);
          if (toastSeen) await capture(cdp, "en-provider-fallback-toast", "ProviderFallback notification toast");
          else await sleep(200);
        }
      })()
    : Promise.resolve();
  const { run: runB } = await followRun(rpc, projectId, apB.run.id, { timeoutMs: 300_000, onApproval: () => "Approve" });
  await toastPoll;
  const fb = rpc.events.slice(eventsBeforeB).find((e) => e.kind === "ProviderFallback");
  check("primary answered 429 insufficient_quota", hits.some((h) => h.startsWith("POST")), hits.slice(0, 4).join(", "));
  check(
    "ProviderFallback event: fake primary → Mock",
    !!fb && fb.payload.from.providerId === primary.providerId && fb.payload.to.providerId === mock.id,
    fb ? `${fb.payload.from.model} → ${fb.payload.to.model} (${fb.payload.reason})` : "none",
  );
  const taskB = runB.tasks[0];
  const handoffs = taskB?.agentId ? (await rpc.call("agents.handoff", { agentId: taskB.agentId })).handoffs : [];
  const agentRuns = await Promise.all(
    runB.tasks.map((t) => (t.agentId ? rpc.call("agents.handoff", { agentId: t.agentId }).then((r) => r.handoffs) : [])),
  );
  const fbHandoff = [...handoffs, ...agentRuns.flat()].find((h) => h.reason === "ProviderFallback");
  check("structured handoff saved for the fallback", !!fbHandoff, fbHandoff ? `${fbHandoff.handoffId}: ${fbHandoff.context.slice(0, 120)}` : "none");
  check("fallback model finished the task", runB.status === "Completed" && taskB.status === "Completed", runBrief(runB));
  if (shotsOn) {
    check("ProviderFallback shown as a UI notification", !!toastSeen, toastSeen ?? "no toast");
    await palette(cdp, "Toggle Maximize Panel");
  }
  await routeTo([smart]);

  // ── J ───────────────────────────────────────────────────────────────────
  section("J. failed autonomous refactor → Rollback Task (§63 J)");
  const { reply: replyJ } = await chatForPlan(rpc, projectId, "Refactor the money helpers");
  must("manager.chat → plan (J)", !!replyJ.planId, replyJ.text?.slice(0, 200));
  const planJ = (await rpc.call("manager.history", { projectId })).plans.find((p) => p.id === replyJ.planId);
  const keepJ = planJ.tasks.find((t) => t.role === "Backend") ?? planJ.tasks[0];
  const apJ = await rpc.call("manager.approvePlan", {
    planId: planJ.id,
    edits: {
      removeTasks: planJ.tasks.filter((t) => t.key !== keepJ.key).map((t) => t.key),
      titles: [{ key: keepJ.key, title: "Refactor the money helpers [mock:heal-never]" }],
    },
  });
  // The user keeps working in the main checkout meanwhile (uncommitted).
  writeFileSync(join(repoDir, "notes", "todo.md"), "- original note\n- user edit in main while the agent works\n");
  const { run: runJ, decided } = await followRun(rpc, projectId, apJ.run.id, {
    timeoutMs: 600_000,
    // Decline more attempts when Self-Healing gives up: the refactor fails.
    onApproval: (a) => (/self-healing/i.test(a.actionSummary) ? "Reject" : "Approve"),
  });
  const taskJ = runJ.tasks[0];
  check(
    "refactor failed: Self-Healing escalated, the user declined, task Failed",
    taskJ.status === "Failed" && decided.some((d) => d.decision === "Reject"),
    `${runBrief(runJ)}; ${taskJ.lastError ?? ""}`,
  );
  const wtJ = taskJ.worktreePath;
  const changedJ = taskJ.filesChanged.length ? taskJ.filesChanged : [];
  const agentFile = changedJ.find((f) => f.startsWith("src/")) ?? "src/money-helpers.mjs";
  check("the failed refactor left files in its worktree", !!wtJ && existsSync(join(wtJ, agentFile)), `${wtJ} ${changedJ.join(", ")}`);
  // An unrelated user change in the same worktree, after the task.
  const userFile = join(wtJ, "notes", "todo.md");
  writeFileSync(userFile, "- original note\n- unrelated user edit after the refactor\n");
  const gated = await rpc.expectError("snapshots.rollbackTask", { projectId, taskId: taskJ.taskId, confirm: false }, "approval_required");
  check("rollbackTask requires confirmation", !!gated);
  const dry = await rpc.call("snapshots.rollbackTask", { projectId, taskId: taskJ.taskId, dryRun: true, confirm: false });
  check("dry run lists the task's files only", !dry.restored.concat(dry.deleted).some((f) => f.includes("notes/todo.md")), `restore ${dry.restored.join(", ")}; delete ${dry.deleted.join(", ")}`);
  const rb = await rpc.call("snapshots.rollbackTask", { projectId, taskId: taskJ.taskId, confirm: true });
  check(
    "Rollback Task removed the refactor",
    !existsSync(join(wtJ, agentFile)) && rb.conflicts.length === 0,
    `restored ${rb.restored.join(", ") || "-"}; deleted ${rb.deleted.join(", ") || "-"}; safety snapshot ${rb.preRollbackSnapshotId}`,
  );
  check("unrelated user edit in the worktree survived", readFileSync(userFile, "utf8").includes("unrelated user edit"));
  check(
    "user edit in the main checkout survived; merged payroll intact",
    readFileSync(join(repoDir, "notes", "todo.md"), "utf8").includes("user edit in main") && existsSync(join(repoDir, "src", "payroll.mjs")),
  );
  await rpc.waitEvent((e) => e.kind === "RollbackPerformed", 10_000, "RollbackPerformed").then(
    () => check("RollbackPerformed event", true),
    () => check("RollbackPerformed event", false),
  );
  // Restore the main checkout so later merges stay clean.
  writeFileSync(join(repoDir, "notes", "todo.md"), "- original note\n");

  // ── consensus, architecture, replay, benchmarks, teams, PM ─────────────
  section("+ consensus → Decision Log, architecture, replay, benchmarks, teams, PM");
  const cons = (
    await rpc.call("consensus.run", {
      projectId,
      question: "How should the payroll module store pay periods: SQLite tables or JSON files?",
      context: "Small team, local-first desktop app, needs history and queries.",
      participants: 3,
      wait: true,
    })
  ).run;
  const consDone = cons.status === "Finished" ? cons : await waitFor(
    async () => {
      const r = (await rpc.call("consensus.get", { consensusId: cons.id })).run;
      return ["Finished", "Failed"].includes(r.status) ? r : null;
    },
    120_000,
    "consensus",
  );
  check("consensus run finished with proposals + a decision", consDone.status === "Finished" && consDone.proposals.length >= 2 && !!consDone.decision, `${consDone.proposals.length} proposals; ${consDone.warnings.join("; ")}`);
  const decision = consDone.decisionId ? (await rpc.call("decisions.get", { id: consDone.decisionId })).decision : null;
  check(
    "Decision Log entry with alternatives",
    !!decision && decision.alternatives.length >= 1,
    decision ? `#${decision.number} ${decision.title}: ${decision.alternatives.map((a) => a.title).join(" / ")}` : "none",
  );
  await rpc.call("architecture.refresh", { projectId }).catch(() => null);
  const arch = (await rpc.call("architecture.map", { projectId })).map;
  check(
    "architecture map is non-empty",
    arch.nodes.length >= 2 && arch.edges.length >= 1 && arch.stats.filesParsed > 0,
    `${arch.nodes.length} node(s) (${arch.nodes.map((n) => `${n.kind}:${n.name}`).join(", ")}), ${arch.edges.length} edge(s), ${arch.stats.filesParsed} file(s) parsed`,
  );
  const timeline = (await rpc.call("replay.timeline", { projectId, runId })).timeline;
  const phaseCount = Object.fromEntries(timeline.phases.map((p) => [p.phase, p.count ?? p.steps ?? 0]));
  const need = ["Plan", "Agents", "Tools", "Files", "Tests", "Reviews", "Approvals", "Merge"];
  check(
    "replay timeline: plan → agents → tools → files → tests → reviews → approvals → merge",
    need.every((p) => phaseCount[p] > 0),
    JSON.stringify(phaseCount),
  );
  const bm = await rpc.call("benchmarks.models", { projectId });
  const ba = await rpc.call("benchmarks.agents", { projectId });
  check(
    "benchmarks show stats from the real runs",
    bm.rows.length > 0 && bm.rows.some((r) => r.executions > 0) && (ba.rows ?? ba.agents ?? []).length > 0,
    bm.rows.map((r) => `${r.model}: ${r.executions} exec, first-pass ${r.firstPassSuccessRate}`).join("; "),
  );
  const templates = (await rpc.call("templates.list", {})).templates;
  const tpl = (role) => templates.find((t) => t.role === role)?.id;
  const team = await rpc.call("teams.create", {
    name: "Payroll squad",
    kind: "Erp",
    description: "Lead architect, two backend engineers, QA and security review.",
    members: [
      { templateId: tpl("Architect"), isLead: true, count: 1, model: null },
      { templateId: tpl("Backend"), isLead: false, count: 2, model: null },
      { templateId: tpl("Qa"), isLead: false, count: 1, model: null },
      { templateId: tpl("Security"), isLead: false, count: 1, model: null },
    ],
    projectId,
  });
  const teams = (await rpc.call("teams.list", { projectId })).teams;
  check("team created and listed", teams.some((t) => (t.team ?? t).name === "Payroll squad"), `${teams.length} team(s); ${(team.team ?? team).id}`);
  const draft = await rpc.call("pm.draft", { projectId, idea: "Employees should see their payslips and download a yearly tax summary." });
  const nFeatures = draft.epic.features.length;
  check("PM draft: epic → features → tasks", draft.status === "Draft" && nFeatures > 0, `${draft.epic.title}: ${nFeatures} feature(s)`);
  const pmOk = await rpc.call("pm.approve", { draftId: draft.id });
  const pmTasks = (await rpc.call("tasks.list", { projectId })).tasks.filter((t) => pmOk.plan.tasks.some((p) => p.title === t.title));
  check(
    "PM approve → executable tasks + Manager plan",
    pmOk.draft.status === "Approved" && !!pmOk.draft.epic.taskId && pmOk.plan.tasks.length > 0 && pmTasks.length > 0,
    `${pmOk.plan.tasks.length} plan task(s), ${pmTasks.length} in the task engine`,
  );

  // ── screenshots ─────────────────────────────────────────────────────────
  if (shotsOn) {
    section("screenshots (app window only, English + Russian)");
    const backendTaskTitle = backend.title;
    for (const lang of ["en", "ru"]) {
      await setLanguage(rpc, cdp, lang);
      await dismissResume(cdp);
      const L = (en, ru) => (lang === "en" ? en : ru);
      const open = (en, ru) => palette(cdp, L(`Open Panel: ${en}`, `Открыть панель: ${ru}`));
      const maximize = () => palette(cdp, L("Toggle Maximize Panel", "Развернуть или восстановить панель"));
      await step("task drawer", async () => {
        await open("Tasks", "Задачи");
        await maximize();
        await clickText(cdp, '[role="tab"]', L("List", "Список"));
        await sleep(600);
        const ok = await clickText(cdp, ".tasks-list [role='row'], .tasks-list li, .tasks-row, tr", backendTaskTitle);
        if (!ok) throw new Error("backend task row not found");
        await sleep(1000);
        await cdp.eval(`document.querySelector('.pl-section')?.scrollIntoView({ block: 'start' }), true`);
        await capture(cdp, `${lang}-02-task-drawer-pipeline`, L("Task drawer: quality pipeline", "Карточка задачи: конвейер качества"));
        await cdp.eval(`document.querySelector('.q-section')?.scrollIntoView({ block: 'start' }), true`);
        await capture(cdp, `${lang}-03-task-drawer-quality`, L("Task drawer: quality runs and gates", "Карточка задачи: проверки и критерии"));
        await cdp.eval(`document.querySelector('.rv-section')?.scrollIntoView({ block: 'start' }), true`);
        await capture(cdp, `${lang}-04-task-drawer-reviews`, L("Task drawer: reviews", "Карточка задачи: ревью"));
        await cdp.key("Escape");
        await sleep(300);
      });
      await step("product", async () => {
        await clickText(cdp, '[role="tab"]', L("Product", "Продукт"));
        await capture(cdp, `${lang}-05-product`, L("Tasks → Product view (AI PM draft)", "Задачи → Продукт (черновик AI PM)"));
        await clickText(cdp, '[role="tab"]', L("List", "Список"));
        await maximize();
      });
      await step("git merge", async () => {
        await open("Git", "Git");
        await maximize();
        await clickText(cdp, '[role="tab"]', L("Merge Manager", "Слияние"));
        await cdp.eval(`(() => { const items = [...document.querySelectorAll('.mm-item__main')]; const done = items.find((e) => /5 branch|5 вет/.test(e.textContent)); done?.click(); return !!done; })()`);
        await capture(cdp, `${lang}-06-git-merge-manager`, L("Git → Merge Manager", "Git → Менеджер слияний"));
        await maximize();
      });
      await step("architecture", async () => {
        await open("Architecture", "Архитектура");
        await maximize();
        await sleep(1500);
        await capture(cdp, `${lang}-07-architecture-map`, L("Architecture map", "Карта архитектуры"));
        await maximize();
      });
      await step("replay", async () => {
        await palette(cdp, L("Open Logs", "Открыть журнал"));
        await maximize();
        await clickText(cdp, '[role="tab"]', L("Replay", "Воспроизведение"));
        await sleep(1500);
        // Replay the payroll run (scenario A), not the latest one.
        const picked = await cdp.eval(`(() => {
          const sel = [...document.querySelectorAll('select')].find((s) => [...s.options].some((o) => o.textContent.includes('Create a payroll module')));
          if (!sel) return false;
          const opt = [...sel.options].find((o) => o.textContent.includes('Create a payroll module'));
          const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set;
          set.call(sel, opt.value);
          sel.dispatchEvent(new Event('change', { bubbles: true }));
          return true;
        })()`);
        if (!picked) throw new Error("payroll run not in the replay selector");
        await sleep(2000);
        await capture(cdp, `${lang}-08-replay`, L("Logs → Execution Replay", "Журнал → Воспроизведение"));
        await maximize();
      });
      await step("teams", async () => {
        await open("Agents", "Агенты");
        await maximize();
        await clickText(cdp, '[role="tab"]', L("Teams", "Команды"));
        await capture(cdp, `${lang}-09-teams`, L("Agents → Teams", "Агенты → Команды"));
        await clickText(cdp, '[role="tab"]', L("Agents", "Агенты"));
        await maximize();
      });
      await step("benchmarks", async () => {
        await open("Costs", "Расходы");
        await maximize();
        await clickText(cdp, '[role="tab"]', L("Model Benchmarks", "Сравнение моделей"));
        await capture(cdp, `${lang}-10-benchmarks`, L("Costs → model benchmarks", "Расходы → сравнение моделей"));
        await maximize();
      });
    }
    writeFileSync(join(outDir, "shots.json"), JSON.stringify(shots, null, 2));
  }
} catch (e) {
  failures++;
  console.error(`\nACCEPTANCE FAILED: ${e.stack ?? e}`);
  console.error(`--- app log tail ---\n${childLog.slice(-4000)}`);
} finally {
  rpc?.close();
  cdp?.close();
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
  const failed = results.filter((r) => !r.ok);
  console.log(`\n${passed}/${results.length} checks passed${failures ? `, ${failures} fatal error(s)` : ""}`);
  for (const f of failed) console.log(`  FAILED: ${f.name} — ${String(f.detail).slice(0, 300)}`);
  process.exit(failures || failed.length ? 1 : 0);
}
