#!/usr/bin/env node
// Phase 3 floating robots: launches the real desktop app (debug build with the
// bundled frontend: `cargo build -p orkestr-desktop --features custom-protocol`
// after `npm run build`), seeds an isolated ORKESTR_HOME through RPC (mock
// provider, a project, a QA agent that completes — Scenario H — and a Backend
// agent that waits for an approval), then captures each robot window and the
// main window through the WebView2 DevTools protocol (webview-only captures,
// never the screen).
//
// Usage (repo root, Windows):
//   npm run build && cargo build -p orkestr-desktop --features custom-protocol
//   node scripts/smoke/phase3-robots-screenshots.mjs [--out docs/screenshots/phase3-robots]

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "phase3-robots"));
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(repo, "target", "debug", "orkestr-desktop.exe");
const cdpPort = Number(process.env.ORKESTR_CDP_PORT ?? 9347);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

const base = mkdtempSync(join(tmpdir(), "orkestr-robots-"));
const home = join(base, "home");
const repoDir = join(base, "schoolify");
mkdirSync(home, { recursive: true });
mkdirSync(join(repoDir, "src"), { recursive: true });
const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();
git(repoDir, "init", "-q", "-b", "main");
git(repoDir, "config", "user.email", "demo@orkestr.local");
git(repoDir, "config", "user.name", "Orkestr Demo");
writeFileSync(join(repoDir, "README.md"), "# schoolify\n");
writeFileSync(join(repoDir, "src", "index.mjs"), "export const add = (a, b) => a + b;\n");
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
    await sleep(250);
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
      } else if (f.method === "Runtime.consoleAPICalled" && f.params.type === "error") {
        this.console.push(f.params.args.map((a) => a.value ?? a.description ?? "").join(" "));
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
  async key(key, { ctrl = false } = {}) {
    const code = key.length === 1 ? `Key${key.toUpperCase()}` : key;
    const vk = key.length === 1 ? key.toUpperCase().charCodeAt(0) : { Enter: 13, Escape: 27 }[key];
    for (const type of ["rawKeyDown", "keyUp"]) {
      // Enter carries its text so forms see a real submit keystroke (implicit submission).
      const extra = key === "Enter" && type === "rawKeyDown" ? { type: "keyDown", text: "\r", unmodifiedText: "\r" } : {};
      await this.send("Input.dispatchKeyEvent", { type, modifiers: ctrl ? 2 : 0, key, code, windowsVirtualKeyCode: vk, ...extra });
    }
  }
  close() {
    this.ws.close();
  }
}

const shots = [];
async function capture(cdp, name, note, { backdrop = false } = {}) {
  await sleep(1200);
  // Robot windows are transparent: paint a desktop-like backdrop behind them for the PNG only.
  const paint = (on) =>
    cdp.eval(`(() => { const s = document.documentElement.style; ${on ? "s.setProperty('background', 'radial-gradient(circle at 30% 20%, #2a3566, #11162b 70%)', 'important')" : "s.removeProperty('background')"}; return true; })()`);
  if (backdrop) await paint(true);
  await sleep(150);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  if (backdrop) await paint(false);
  const file = join(outDir, `${name}.png`);
  writeFileSync(file, Buffer.from(data, "base64"));
  const errors = cdp.console.splice(0);
  shots.push({ name: `${name}.png`, note, consoleErrors: errors });
  console.log(`${name}${errors.length ? `  — ${errors.length} console error(s)` : ""}`);
  for (const e of errors) console.log(`    ${e.slice(0, 300)}`);
}

const targets = async () => (await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json()).filter((t) => t.type === "page");
async function attach(match, label) {
  const t = await waitFor(async () => (await targets()).find(match), 60_000, label);
  const cdp = new Cdp(t.webSocketDebuggerUrl);
  await cdp.open();
  return cdp;
}

async function palette(cdp, text) {
  await cdp.eval("document.activeElement?.blur?.(), true");
  await cdp.key("k", { ctrl: true });
  await sleep(300);
  await cdp.send("Input.insertText", { text });
  await sleep(400);
  await cdp.key("Enter");
  await sleep(700);
}

let exitCode = 0;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.sub(["*"]);
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, performanceMode: "UltraVisual", language: "en" } });
  const providers = (await rpc.call("providers.list")).providers.map((p) => p.provider ?? p);
  const mock = providers.find((p) => p.kind === "Mock");
  const smart = { providerId: mock.id, model: "mock-smart" };
  await rpc.call("providers.setManager", { config: { model: smart, maxParallelAgents: 2, approvalPolicy: "SemiAuto" } });
  await rpc.call("providers.setRouting", {
    config: { mode: "Manual", rules: [], defaultFallback: [smart], preferLocal: true, maxRateLimitWaitSecs: 0 },
  });
  const { project } = await rpc.call("projects.create", { name: "Schoolify", rootPath: repoDir, approvalMode: "SemiAuto" });
  await rpc.call("projects.create", { name: "Landing", color: "#f472b6" }).catch((e) => console.warn("second project", e.message));
  const projectId = project.id;
  const templates = (await rpc.call("templates.list", {})).templates;
  const qaTpl = templates.find((t) => t.role === "Qa");
  const backendTpl = templates.find((t) => t.role === "Backend");
  // A distinct look for the QA robot (template design).
  await rpc.call("robots.setConfig", {
    design: {
      templateId: qaTpl.id,
      design: {
        mode: "TwoD",
        skin: "mint",
        material: "Glossy",
        accent: null,
        scale: 1,
        accessories: ["antenna", "headphones"],
        animationIntensity: 0.9,
        voice: { enabled: false, voiceId: null },
        sounds: "chime",
        behaviour: { snap: true, autoHideIdle: false, clickThroughIdle: true, bubble: "Normal" },
      },
    },
  });

  const cdpMain = await attach((t) => !t.url.includes("robot.html") && /tauri\.localhost|127\.0\.0\.1:1420/.test(t.url), "main webview");
  // Reload so the shell starts with the seeded settings (no onboarding wizard, English).
  await cdpMain.send("Page.reload", { ignoreCache: true });
  await waitFor(() => cdpMain.eval("!!document.querySelector('.app__body')"), 60_000, "app shell");
  await sleep(2500);
  await cdpMain.eval(`document.querySelector('[aria-label="Open project Schoolify"]')?.click(), true`);
  await sleep(800);

  // Scenario H: the QA agent runs and completes.
  const qa = await rpc.call("agents.spawn", {
    projectId,
    templateId: qaTpl.id,
    newTask: { title: "Run the test suite", description: "Run the project tests and report the result." },
  });
  const qaId = qa.agent.id;
  // A Backend agent that will wait for an approval (dangerous command).
  const be = await rpc.call("agents.spawn", {
    projectId,
    templateId: backendTpl.id,
    newTask: { title: "Clean the build output", description: "Remove untracked build leftovers.\nRun: `git clean -fd`" },
  });
  const beId = be.agent.id;
  await waitFor(
    async () => ["Completed", "Error", "Stopped"].includes((await rpc.call("agents.get", { agentId: qaId })).agent.state),
    120_000,
    "QA agent finished",
  );
  await waitFor(async () => (await rpc.call("agents.get", { agentId: beId })).agent.state === "Waiting", 60_000, "backend waiting");

  const qaRobot = await attach((t) => t.url.includes(`robot.html#/agent/${qaId}`), "QA robot window");
  await waitFor(() => qaRobot.eval("!!document.querySelector('.robot-window svg')"), 30_000, "QA robot drawn");
  await capture(qaRobot, "01-qa-robot-completed", "Scenario H: QA robot completed — celebration + result bubble", { backdrop: true });

  const beRobot = await attach((t) => t.url.includes(`robot.html#/agent/${beId}`), "backend robot window");
  await waitFor(() => beRobot.eval("!!document.querySelector('.robot-window svg')"), 30_000, "backend robot drawn");
  await capture(beRobot, "02-backend-robot-waiting", "Backend robot waiting for an approval (hourglass + bubble)", { backdrop: true });

  await beRobot.eval(`(() => { const el = document.querySelector('.robot-window__robot'); el.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2 })); return true; })()`);
  await capture(beRobot, "03-robot-context-menu", "Robot context menu (Open, Assign…, Voice, Hide, Stop agent)", { backdrop: true });
  await beRobot.eval("document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })), document.querySelector('.robot-menu')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })), true");

  // Click the QA robot → main window opens the Agents panel at the QA agent.
  await qaRobot.eval(`(() => {
    const el = document.querySelector('.robot-window__robot');
    const o = { bubbles: true, button: 0, clientX: 100, clientY: 150, pointerId: 1 };
    el.dispatchEvent(new PointerEvent('pointerdown', o));
    el.dispatchEvent(new PointerEvent('pointerup', o));
    return true;
  })()`);
  await waitFor(() => cdpMain.eval("!!document.querySelector('.agents-detail, [class*=agents-detail]')"), 15_000, "agent detail in main").catch(
    (e) => console.warn(String(e)),
  );
  await capture(cdpMain, "04-main-agent-opened", "Click on the QA robot opened its result in the Agents panel");

  await palette(cdpMain, "Open Robot Designer");
  await sleep(800);
  await capture(cdpMain, "05-settings-robot-designer", "Settings → Robots with the Robot Designer (live preview)");
  await cdpMain.eval(`(() => { const s = document.querySelector('select.set-select'); const o = [...s.options].find((x) => x.textContent.includes('QA') || x.textContent.includes('Тест')); if (!o) return false; s.value = o.value; s.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`);
  await capture(cdpMain, "06-designer-qa-template", "Robot Designer on the QA template's own design (Ultra Visual)");

  const rpc2 = rpcClient(info.url, token);
  await rpc2.open();
  await rpc2.call("settings.update", { patch: { language: "ru", performanceMode: "Balanced" } });
  await sleep(1500);
  await capture(cdpMain, "07-designer-ru", "Дизайнер роботов (русский интерфейс, режим «Баланс»)");
  await capture(qaRobot, "08-qa-robot-ru", "QA-робот на русском", { backdrop: true });

  // Overflow cluster: one visible robot (Settings → Robots), the rest gather in the cluster.
  await cdpMain.eval(`(() => { const r = [...document.querySelectorAll('.ok-segmented button')].find((b) => b.textContent.trim() === '1'); r?.click(); return !!r; })()`);
  const cluster = await attach((t) => t.url.includes("robot.html#/cluster"), "cluster window");
  await waitFor(() => cluster.eval("!!document.querySelector('.robot-cluster svg')"), 30_000, "cluster drawn");
  await capture(cluster, "09-cluster", "Overflow cluster (+N robots)", { backdrop: true });
  rpc.close();
  rpc2.close();
  writeFileSync(join(outDir, "manifest.json"), `${JSON.stringify(shots, null, 2)}\n`);
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
