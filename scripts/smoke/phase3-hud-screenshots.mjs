#!/usr/bin/env node
// Phase 3 (hud-perf-aiui) visual review: launches the real desktop app with
// the bundled frontend (no Vite port needed), seeds an isolated ORKESTR_HOME
// through RPC, and captures (webview only, via DevTools — never the screen):
//   01  main window (Balanced)
//   02  Settings → Interface (Performance Mode, density, HUD, history)
//   03  AI-editable UI: the Manager's proposal card + live preview, from the
//       sentence «Сделай Mission Control компактнее, Claude справа, убери
//       график токенов, сделай фон темнее» (Russian UI)
//   04  after "Keep" (compact density, darker background, undo toast)
//   05  the HUD window while a run is in progress
//   06  Battery Saver mode
//
// Usage (repo root, Windows):
//   npm run build -w @orkestr/desktop
//   cargo build -p orkestr-desktop --features custom-protocol
//   node scripts/smoke/phase3-hud-screenshots.mjs [--out docs/screenshots/phase3-hud]

import { spawn, execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "phase3-hud"));
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(repo, "target", "debug", "orkestr-desktop.exe");
const cdpPort = Number(process.env.ORKESTR_CDP_PORT ?? 9447);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

const base = mkdtempSync(join(tmpdir(), "orkestr-p3hud-"));
const home = join(base, "home");
const repoDir = join(base, "hello-app");
mkdirSync(home, { recursive: true });
mkdirSync(join(repoDir, "src"), { recursive: true });
const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();
git(repoDir, "init", "-q", "-b", "main");
git(repoDir, "config", "user.email", "demo@orkestr.local");
git(repoDir, "config", "user.name", "Orkestr Demo");
writeFileSync(join(repoDir, "README.md"), "# hello-app\n");
writeFileSync(join(repoDir, "package.json"), '{\n  "name": "hello-app",\n  "private": true,\n  "type": "module"\n}\n');
writeFileSync(join(repoDir, "src", "index.mjs"), 'export function main() {\n  console.log("Hello");\n}\n');
git(repoDir, "add", "-A");
git(repoDir, "commit", "-q", "-m", "initial");

const child = spawn(exe, [], {
  env: {
    ...process.env,
    ORKESTR_HOME: home,
    ORKESTR_SECRET_BACKEND: "vault-file",
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${cdpPort}`,
  },
  stdio: ["ignore", "pipe", "pipe"],
});
let log = "";
child.stdout.on("data", (d) => (log += d));
child.stderr.on("data", (d) => (log += d));

async function waitFor(fn, ms, label) {
  const end = Date.now() + ms;
  for (;;) {
    try {
      const v = await fn();
      if (v) return v;
    } catch {
      /* retry */
    }
    if (Date.now() > end) throw new Error(`timed out: ${label}`);
    await sleep(200);
  }
}

function rpcClient(url, token) {
  const ws = new WebSocket(url);
  let seq = 0;
  const pending = new Map();
  const events = [];
  ws.onmessage = (m) => {
    const f = JSON.parse(m.data);
    if (f.type === "res") {
      const p = pending.get(f.id);
      pending.delete(f.id);
      f.ok ? p?.resolve(f.result) : p?.reject(new Error(`${f.error.code}: ${f.error.message}`));
    } else if (f.type === "event") events.push(f.event);
  };
  const frame = (o, id) =>
    new Promise((res, rej) => {
      pending.set(id, { resolve: res, reject: rej });
      ws.send(JSON.stringify(o));
    });
  return {
    events,
    open: () =>
      new Promise((res, rej) => {
        ws.onopen = () => frame({ type: "auth", token, id: "a" }, "a").then(res, rej);
        ws.onerror = rej;
      }),
    call: (method, params = {}) => {
      const id = `r${++seq}`;
      return frame({ type: "req", id, method, params }, id);
    },
    sub: (topics) => frame({ type: "sub", id: `s${++seq}`, topics }, `s${seq}`),
    close: () => ws.close(),
  };
}

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
      } else if (f.method === "Runtime.consoleAPICalled" && ["error", "warning"].includes(f.params.type)) {
        this.console.push(`${f.params.type}: ${f.params.args.map((a) => a.value ?? a.description ?? "").join(" ")}`);
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
      await this.send("Input.dispatchKeyEvent", { type, modifiers, key, code, windowsVirtualKeyCode: vk });
    }
  }
  async type(text) {
    await this.send("Input.insertText", { text });
  }
}

const shots = [];
async function capture(cdp, name, note) {
  await sleep(1000);
  const file = join(outDir, `${name}.png`);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(file, Buffer.from(data, "base64"));
  const errors = cdp.console.splice(0);
  shots.push({ name, note, errors });
  console.log(`${name}${errors.length ? `  — ${errors.length} console error(s)` : ""}`);
  for (const e of errors) console.log(`    ${e.slice(0, 300)}`);
}

async function palette(cdp, text) {
  await cdp.eval("document.activeElement && document.activeElement.blur && document.activeElement.blur(), true");
  await cdp.key("k", { ctrl: true });
  await sleep(300);
  await cdp.type(text);
  await sleep(400);
  await cdp.key("Enter");
  await sleep(700);
}

const pages = async () => (await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json()).filter((t) => t.type === "page");

let exitCode = 0;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.sub(["*"]);
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, language: "en" } });
  const providers = (await rpc.call("providers.list")).providers.map((p) => p.provider ?? p);
  const mock = providers.find((p) => p.kind === "Mock");
  const smart = { providerId: mock.id, model: "mock-smart" };
  await rpc.call("providers.setManager", { config: { model: smart, maxParallelAgents: 2, approvalPolicy: "SemiAuto" } });
  await rpc.call("providers.setRouting", {
    config: { mode: "Manual", rules: [], defaultFallback: [smart], preferLocal: true, maxRateLimitWaitSecs: 0 },
  });
  const { project } = await rpc.call("projects.create", { name: "hello-app", rootPath: repoDir, approvalMode: "SemiAuto" });
  const projectId = project.id;
  const chat = async (text) => {
    const sent = await rpc.call("manager.chat", { projectId, text });
    return waitFor(
      () => rpc.events.find((e) => e.kind === "ManagerMessageAdded" && e.payload.message.id === sent.replyMessageId)?.payload.message,
      60_000,
      "manager reply",
    );
  };
  console.log("seeded");

  const main = await waitFor(async () => (await pages()).find((t) => !t.url.includes("hud")), 60_000, "main webview");
  const cdp = new Cdp(main.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send("Page.reload", { ignoreCache: true });
  await waitFor(() => cdp.eval("!!document.querySelector('.app__body')"), 60_000, "app shell");
  await sleep(2500);
  await cdp.eval(`document.querySelector('[aria-label="Open project hello-app"]')?.click(), true`);
  await sleep(1200);
  await cdp.eval(`document.querySelector('.resume-card button[aria-label*="ismiss" i], .resume-card button[aria-label*="lose" i]')?.click(), true`);
  await palette(cdp, "Open Panel: Manager");
  await capture(cdp, "01-main-balanced", "Main window, Balanced mode: chrome, status bar performance chip, HUD button");

  await palette(cdp, "Open Settings");
  await sleep(400);
  await cdp.eval(`[...document.querySelectorAll('[role=tab], button')].find((e) => e.textContent.trim() === 'Interface')?.click(), true`);
  await capture(cdp, "02-settings-interface", "Settings → Interface: Performance Mode cards, policy, auto switching, density, HUD, history");
  await cdp.key("Escape");
  await sleep(400);

  // HUD while a run is in progress.
  const r1 = await chat("Create a hello module with a unit test");
  await rpc.call("manager.approvePlan", { planId: r1.planId });
  await sleep(2500);
  await cdp.eval("window.__TAURI_INTERNALS__.invoke('hud_show', { position: null }).then(() => true)");
  const hud = await waitFor(async () => (await pages()).find((t) => t.id !== main.id), 30_000, "HUD webview");
  const hcdp = new Cdp(hud.webSocketDebuggerUrl);
  await hcdp.open();
  await waitFor(() => hcdp.eval("!!document.querySelector('.hud')"), 30_000, "HUD rendered");
  await sleep(2500);
  await capture(hcdp, "05-hud", "HUD window: run status, agent strip, approvals, voice slot, mini Manager input");

  // AI-editable UI in Russian: the Manager's proposal + live preview.
  await rpc.call("settings.update", { patch: { language: "ru" } });
  await sleep(1500);
  await chat("Сделай Mission Control компактнее, Claude справа, убери график токенов, сделай фон темнее");
  await waitFor(() => cdp.eval("!!document.querySelector('.aiui-card .aiui-changes')"), 30_000, "proposal card");
  await capture(cdp, "03-aiui-proposal-ru", "Manager proposal card with the diff summary; the live preview is applied (compact, darker)");
  await cdp.eval(`[...document.querySelectorAll('.aiui-card button')].find((e) => e.textContent.trim() === 'Оставить')?.click(), true`);
  await waitFor(() => cdp.eval("!!document.querySelector('.aiui-card--applied')"), 15_000, "applied");
  await capture(cdp, "04-aiui-applied-ru", "After Keep: stored as a new UI config version, with Undo");

  await rpc.call("settings.update", { patch: { language: "en", performanceMode: "BatterySaver" } });
  await sleep(1500);
  await capture(cdp, "06-battery-saver", "Battery Saver: no blur/glow, no motion, static background");
  writeFileSync(join(outDir, "manifest.json"), `${JSON.stringify(shots, null, 2)}\n`);
  rpc.close();
} catch (e) {
  exitCode = 1;
  console.error(e.stack ?? e);
  console.error(log.slice(-3000));
} finally {
  try {
    execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
  } catch {
    /* gone */
  }
  await sleep(1000);
  try {
    rmSync(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  } catch {
    /* ignore */
  }
  process.exit(exitCode);
}
