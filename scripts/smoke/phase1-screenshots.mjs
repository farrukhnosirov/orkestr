#!/usr/bin/env node
// Phase 1 UI review: launches the real desktop app (debug build, frontend from
// the Vite dev server), seeds an isolated ORKESTR_HOME through the RPC API
// (project, a finished Manager run, a pending plan, a terminal), drives the
// UI through the WebView2 DevTools protocol (command palette, settings,
// onboarding) and captures each view (DevTools capture of the webview; with
// `--window`, PowerShell CopyFromScreen of the native window). Console errors / exceptions are
// collected per view and reported.
//
// Usage (repo root, Windows):
//   npm run dev -w @orkestr/desktop          # Vite on 127.0.0.1:1420, in another terminal
//   cargo build -p orkestr-desktop
//   node scripts/smoke/phase1-screenshots.mjs [--out docs/screenshots/phase1]

import { spawn, execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "phase1"));
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(repo, "target", "debug", "orkestr-desktop.exe");
const cdpPort = 9333;
// `--window` captures the native window with PowerShell CopyFromScreen
// (includes the frameless chrome, but also anything on top of it, e.g. OS
// notifications from other apps). The default captures only the webview
// through DevTools, so no other application's content can leak into the PNGs.
const windowCapture = process.argv.includes("--window");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

// ── seed data dir + project ───────────────────────────────────────────────
const base = mkdtempSync(join(tmpdir(), "orkestr-shots-"));
const home = join(base, "home");
const repoDir = join(base, "hello-app");
mkdirSync(home, { recursive: true });
mkdirSync(join(repoDir, "src"), { recursive: true });
const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();
git(repoDir, "init", "-q", "-b", "main");
git(repoDir, "config", "user.email", "demo@orkestr.local");
git(repoDir, "config", "user.name", "Orkestr Demo");
writeFileSync(join(repoDir, "README.md"), "# hello-app\n\nA tiny demo project for Orkestr.\n");
writeFileSync(join(repoDir, "package.json"), '{\n  "name": "hello-app",\n  "private": true,\n  "type": "module"\n}\n');
writeFileSync(
  join(repoDir, "src", "index.mjs"),
  '// Entry point\nexport function main(argv = process.argv.slice(2)) {\n  const name = argv[0] ?? "world";\n  console.log(`Hello, ${name}!`);\n}\n',
);
git(repoDir, "add", "-A");
git(repoDir, "commit", "-q", "-m", "initial");
writeFileSync(join(repoDir, "README.md"), "# hello-app\n\nA tiny demo project for Orkestr.\n\nWork in progress.\n");

// ── launch ────────────────────────────────────────────────────────────────
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

// ── RPC (seed) ────────────────────────────────────────────────────────────
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

// ── CDP (drive the webview) ───────────────────────────────────────────────
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
        const text = f.params.args.map((a) => a.value ?? a.description ?? "").join(" ");
        this.console.push(`${f.params.type}: ${text}`);
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
      await this.send("Input.dispatchKeyEvent", {
        type,
        modifiers,
        key,
        code,
        windowsVirtualKeyCode: vk,
      });
    }
  }
  async type(text) {
    await this.send("Input.insertText", { text });
  }
}

const shots = [];
async function capture(cdp, name, note) {
  await sleep(900);
  const file = join(outDir, `${name}.png`);
  let how = "devtools";
  if (windowCapture) {
    try {
      execFileSync(
        "powershell",
        ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", join(here, "capture-window.ps1"), "-ProcessId", String(child.pid), "-Out", file],
        { stdio: "pipe" },
      );
      how = "window";
    } catch (e) {
      how = `devtools (window capture failed: ${String(e.stderr ?? e.message).trim().slice(0, 120)})`;
    }
  }
  if (how !== "window") {
    const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
    writeFileSync(file, Buffer.from(data, "base64"));
  }
  const errors = cdp.console.splice(0);
  shots.push({ name, file, how, note, errors });
  console.log(`${name}: ${how}${errors.length ? `  — ${errors.length} console error(s)` : ""}`);
  for (const e of errors) console.log(`    ${e.slice(0, 300)}`);
}

async function palette(cdp, text) {
  await cdp.eval("document.activeElement && document.activeElement.blur && document.activeElement.blur(), true");
  await cdp.key("k", { ctrl: true });
  await sleep(300);
  await cdp.type(text);
  await sleep(400);
  await cdp.key("Enter");
  await sleep(600);
}

const clickText = (cdp, selector, text) =>
  cdp.eval(`(() => {
    const els = [...document.querySelectorAll(${JSON.stringify(selector)})];
    const el = els.find((e) => e.textContent.trim().includes(${JSON.stringify(text)}));
    if (!el) return false;
    el.click();
    return true;
  })()`);

let exitCode = 0;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.sub(["*"]);

  // Seed: setup as onboarding would, a project, one finished run, one pending plan.
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true } });
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
  const r1 = await chat("Create a hello module with a unit test");
  const ap = await rpc.call("manager.approvePlan", { planId: r1.planId });
  await waitFor(
    async () => ["Completed", "Failed", "Cancelled"].includes((await rpc.call("manager.status", { runId: ap.run.id })).runs[0].status),
    120_000,
    "run",
  );
  await chat("Create a greeter module with tests"); // pending plan → plan card
  await rpc.call("memory.create", {
    scope: { scope: "Project", projectId },
    kind: "Convention",
    title: "ES modules only",
    content: "All source files are .mjs ES modules; tests use node:test.",
    tags: ["style"],
  });
  rpc.close();
  console.log("seeded");

  // Attach to the webview.
  const target = await waitFor(async () => {
    const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
    return list.find((t) => t.type === "page" && t.url.includes("127.0.0.1:1420"));
  }, 60_000, "webview DevTools target (is Vite running on :1420?)");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send("Page.reload", { ignoreCache: true });
  await waitFor(() => cdp.eval("!!document.querySelector('.app__body')"), 60_000, "app shell");
  await sleep(2500);
  // Select the seeded project.
  await cdp.eval(`document.querySelector('[aria-label="Open project hello-app"]')?.click(), true`);
  await sleep(1500);

  await palette(cdp, "Open Panel: Manager");
  await capture(cdp, "01-manager-plan-card", "Manager chat with the finished run and a pending plan card");
  // The session-resume card (shown in the first capture) is dismissed so it
  // does not cover the panels below.
  await cdp.eval(`document.querySelector('.resume-card button[aria-label*="ismiss" i], .resume-card button[aria-label*="lose" i]')?.click(), true`);
  await palette(cdp, "Open Panel: Agents");
  await capture(cdp, "02-agents", "Agents panel after the run");
  await palette(cdp, "Open Panel: Tasks");
  await palette(cdp, "Toggle Maximize Panel");
  await clickText(cdp, "[role=tab], button", "Graph");
  await sleep(1500);
  await capture(cdp, "03-tasks-graph", "Tasks graph (maximized): the Manager's DAG, design → implement ∥ document → test");
  await palette(cdp, "Toggle Maximize Panel");
  await cdp.key("p", { ctrl: true });
  await sleep(500);
  await cdp.type("index.mjs");
  await sleep(800);
  await cdp.key("Enter");
  await sleep(1500);
  await capture(cdp, "04-code", "Code panel (Monaco) with src/index.mjs");
  await palette(cdp, "Open Panel: Terminal");
  await palette(cdp, "Toggle Maximize Panel");
  await sleep(2500);
  await capture(cdp, "05-terminal", "Terminal panel (maximized): agent command output");
  await palette(cdp, "Toggle Maximize Panel");
  await palette(cdp, "Open Panel: Git");
  await palette(cdp, "Toggle Maximize Panel");
  await clickText(cdp, "[role=tab], button", "Worktrees");
  await sleep(1200);
  await capture(cdp, "06-git", "Git panel (maximized): agent worktrees");
  await palette(cdp, "Toggle Maximize Panel");
  await palette(cdp, "Open Settings");
  await sleep(500);
  await clickText(cdp, "[role=tab]", "Provider");
  await capture(cdp, "07-settings-providers", "Settings → AI Providers");
  await cdp.key("Escape");
  await sleep(400);
  await palette(cdp, "Run Setup Wizard");
  await capture(cdp, "08-onboarding", "Onboarding wizard");
  await cdp.key("Escape");
  writeFileSync(
    join(outDir, "manifest.json"),
    `${JSON.stringify(
      shots.map(({ name, how, note, errors }) => ({ name: `${name}.png`, note, capture: how, consoleErrors: errors })),
      null,
      2,
    )}\n`,
  );
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
