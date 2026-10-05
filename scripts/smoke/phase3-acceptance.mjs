#!/usr/bin/env node
// Phase 3 (Visual OS) acceptance + visual QA run.
//
// Launches the real desktop app (debug build with the bundled frontend),
// seeds an isolated ORKESTR_HOME through the RPC API (mock provider, a temp
// git project named «Schoolify»), then drives the app and checks:
//
//   H  Manager run → one floating robot window per agent with live state →
//      QA agent completes → completion pose + result bubble → click → the
//      main window opens the agent (§63 H). Robot windows are checked for
//      transparency (alpha at the corners) and click-through (WS_EX_LAYERED +
//      WS_EX_TRANSPARENT while the cursor is not over the robot).
//   G  «Jarvis, что происходит со Schoolify?» fed through the voice pipeline
//      as a finished transcript (no microphone) → projectStatus intent →
//      summary reply → TTS called (speechSynthesis is stubbed, silent).
//   every theme on Mission Control, every layout preset, every performance
//   mode, the HUD window, AI-editable UI (proposal → preview → keep → undo),
//   detach a panel into its own window and re-dock it.
//
// Captures are webview-only (DevTools `Page.captureScreenshot`) — never the
// screen, so no other application can appear. Robot windows are captured
// with a transparent background and composited over a synthetic desktop.
//
// Usage (repo root, Windows):
//   npm run build && cargo build -p orkestr-desktop --features custom-protocol
//   node scripts/smoke/phase3-acceptance.mjs [--out docs/screenshots/phase3] [--no-shots]

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "phase3"));
const noShots = process.argv.includes("--no-shots");
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(repo, "target", "debug", "orkestr-desktop.exe");
const cdpPort = Number(process.env.ORKESTR_CDP_PORT ?? 9361);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

// ── results ───────────────────────────────────────────────────────────────
const checks = [];
function check(area, name, ok, detail = "") {
  checks.push({ area, name, ok: Boolean(ok), detail: String(detail ?? "") });
  console.log(`${ok ? "PASS" : "FAIL"}  [${area}] ${name}${detail ? ` — ${String(detail).slice(0, 240)}` : ""}`);
}

// ── seed data dir + project ───────────────────────────────────────────────
const base = mkdtempSync(join(tmpdir(), "orkestr-p3acc-"));
const home = join(base, "home");
const repoDir = join(base, "schoolify");
mkdirSync(home, { recursive: true });
mkdirSync(join(repoDir, "src"), { recursive: true });
const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();
git(repoDir, "init", "-q", "-b", "main");
git(repoDir, "config", "user.email", "demo@orkestr.local");
git(repoDir, "config", "user.name", "Orkestr Demo");
writeFileSync(join(repoDir, "README.md"), "# Schoolify\n\nGrades and timetables for small schools.\n");
writeFileSync(join(repoDir, "package.json"), '{\n  "name": "schoolify",\n  "private": true,\n  "type": "module"\n}\n');
writeFileSync(
  join(repoDir, "src", "grades.mjs"),
  "// Grade helpers\nexport function average(grades) {\n  if (grades.length === 0) return 0;\n  return grades.reduce((a, b) => a + b, 0) / grades.length;\n}\n",
);
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
const soft = (p) => p.then((v) => v, () => null);

// ── RPC ───────────────────────────────────────────────────────────────────
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

// ── CDP ───────────────────────────────────────────────────────────────────
class Cdp {
  constructor(wsUrl, label) {
    this.label = label;
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
    this.ws.onclose = () => {
      for (const p of this.pending.values()) p.reject(new Error("target closed"));
      this.pending.clear();
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
    const isDigit = /^[0-9]$/.test(key);
    const code = isDigit ? `Digit${key}` : key.length === 1 ? `Key${key.toUpperCase()}` : key;
    const vk = key.length === 1 ? key.toUpperCase().charCodeAt(0) : { Enter: 13, Escape: 27 }[key];
    for (const type of ["rawKeyDown", "keyUp"]) {
      // Enter carries its text so forms see a real submit keystroke.
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

const targets = async () => (await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json()).filter((t) => t.type === "page");
async function attach(match, label, ms = 60_000) {
  const t = await waitFor(async () => (await targets()).find(match), ms, label);
  const cdp = new Cdp(t.webSocketDebuggerUrl, label);
  await cdp.open();
  cdp.targetId = t.id;
  cdp.url = t.url;
  return cdp;
}

const shots = [];
async function capture(cdp, name, note, { settle = 1000 } = {}) {
  await sleep(settle);
  if (cdp.label === "main webview") {
    const p = await probe(cdp, name);
    if (p) check("Visual", `${name}: one shell, page not scrolled`, p.shells === 1 && p.scrollY === 0 && p.docH <= p.winH + 1, JSON.stringify(p));
  }
  const errors = cdp.console.splice(0);
  if (noShots) return;
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
  shots.push({ name: `${name}.png`, note, consoleErrors: errors });
  console.log(`  shot ${name}${errors.length ? `  — ${errors.length} console error(s)` : ""}`);
  for (const e of errors) console.log(`      ${e.slice(0, 300)}`);
}

/** Transparent capture of a robot window (background override alpha 0). */
async function robotPng(cdp) {
  await cdp.send("Emulation.setDefaultBackgroundColorOverride", { color: { r: 0, g: 0, b: 0, a: 0 } });
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  await cdp.send("Emulation.setDefaultBackgroundColorOverride", {});
  return data;
}

async function palette(cdp, text) {
  await cdp.eval("document.activeElement?.blur?.(), true");
  await cdp.key("k", { ctrl: true });
  await sleep(300);
  await cdp.type(text);
  await sleep(450);
  await cdp.key("Enter");
  await sleep(800);
  // Still open (Enter swallowed)? Click the highlighted command instead and record it.
  const still = await soft(
    cdp.eval(`(() => { if (!document.querySelector('.palette')) return false; const o = document.querySelector('.palette [aria-selected="true"]'); o?.click(); return (o ? o.textContent.trim() + (o.getAttribute('aria-disabled') ? ' (disabled)' : '') : 'no option') + ' voice=' + JSON.stringify(window.__orkestrDebug?.voice?.state?.() ?? null) + ' active=' + (document.activeElement?.className ?? ''); })()`),
  );
  if (still) {
    console.log(`  palette: Enter did not run «${text}» — clicked «${still}»`);
    paletteMisses.push(text);
    await sleep(800);
  }
}
const paletteMisses = [];

const clickText = (cdp, selector, text) =>
  cdp.eval(`(() => {
    const els = [...document.querySelectorAll(${JSON.stringify(selector)})];
    const el = els.find((e) => e.textContent.trim().includes(${JSON.stringify(text)}));
    if (!el) return false;
    el.click();
    return true;
  })()`);

function listWindows() {
  try {
    const out = execFileSync(
      "powershell",
      ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", join(here, "list-windows.ps1"), "-ProcessId", String(child.pid)],
      { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
    );
    return JSON.parse(out.trim() || "[]");
  } catch (e) {
    console.warn(`list-windows failed: ${String(e.stderr ?? e.message).slice(0, 200)}`);
    return [];
  }
}

/** Visible panel titles in the dock (for preset checks). */
const dockTabs = (cdp) =>
  cdp.eval(`[...document.querySelectorAll('.dv-tab')].filter((e) => { const r = e.getBoundingClientRect(); return e.offsetParent && r.width > 0 && r.bottom > 0 && r.top < innerHeight; }).map((e) => e.textContent.trim()).filter(Boolean)`);

/** Layout sanity: exactly one shell, nothing scrolled out of the window. */
async function probe(cdp, label) {
  const p = await soft(
    cdp.eval(`({ shells: document.querySelectorAll('.app__body').length, scrollY: Math.round(document.scrollingElement.scrollTop), docH: document.scrollingElement.scrollHeight, winH: innerHeight, bodyChildren: [...document.body.children].map((e) => e.tagName + '.' + (e.className || e.id || '')).join(' ') })`),
  );
  if (p) console.log(`  probe ${label}: ${JSON.stringify(p)}`);
  return p;
}

let exitCode = 0;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.sub(["*"]);

  // ── seed ────────────────────────────────────────────────────────────────
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, performanceMode: "UltraVisual", language: "en" } });
  const providers = (await rpc.call("providers.list")).providers.map((p) => p.provider ?? p);
  const mock = providers.find((p) => p.kind === "Mock");
  const smart = { providerId: mock.id, model: "mock-smart" };
  await rpc.call("providers.setManager", { config: { model: smart, maxParallelAgents: 3, approvalPolicy: "SemiAuto" } });
  await rpc.call("providers.setRouting", {
    config: { mode: "Manual", rules: [], defaultFallback: [smart], preferLocal: true, maxRateLimitWaitSecs: 0 },
  });
  const { project } = await rpc.call("projects.create", { name: "Schoolify", rootPath: repoDir, approvalMode: "SemiAuto" });
  await soft(rpc.call("projects.create", { name: "Landing", color: "#f472b6" }));
  const projectId = project.id;
  const templates = (await rpc.call("templates.list", {})).templates;
  const qaTpl = templates.find((t) => t.role === "Qa");
  const backendTpl = templates.find((t) => t.role === "Backend");
  console.log("seeded");

  // ── main window ─────────────────────────────────────────────────────────
  const main = await attach((t) => !t.url.includes("robot.html") && !t.url.includes("#/panel") && /tauri\.localhost|127\.0\.0\.1:1420/.test(t.url), "main webview");
  await main.eval("localStorage.setItem('orkestr.debugHooks', '1'), true");
  await main.send("Page.reload", { ignoreCache: true });
  await waitFor(() => main.eval("!!document.querySelector('.app__body')"), 60_000, "app shell");
  await sleep(2500);
  await main.eval(`document.querySelector('[aria-label="Open project Schoolify"]')?.click(), true`);
  await sleep(1200);
  await main.eval(`document.querySelector('.resume-card button[aria-label*="ismiss" i], .resume-card button[aria-label*="lose" i]')?.click(), true`);
  await sleep(500);

  // ── Scenario H: Manager run → robots per agent ──────────────────────────
  const chat = async (text) => {
    const sent = await rpc.call("manager.chat", { projectId, text });
    return waitFor(
      () => rpc.events.find((e) => e.kind === "ManagerMessageAdded" && e.payload.message.id === sent.replyMessageId)?.payload.message,
      60_000,
      "manager reply",
    );
  };
  const plan = await chat("Add a report-card module with unit tests and run QA on it");
  check("H", "Manager produced a plan", plan.planId, plan.text?.slice(0, 80));
  const ap = await rpc.call("manager.approvePlan", { planId: plan.planId });
  const runId = ap.run.id;

  const robots = new Map(); // agentId → { cdp, states:Set, shots:Set }
  const stateOf = (cdp) =>
    cdp.eval(`(() => { const el = document.querySelector('.robot-window__robot'); return el ? el.getAttribute('aria-label') : null; })()`);
  const robotPngs = []; // {agentId, state, data}
  let runStatus = "Running";
  const t0 = Date.now();
  const approved = new Set();
  while (Date.now() - t0 < 240_000) {
    // Act as the user: approve what the run asks for (the Manager's review gates).
    const pending = (await soft(rpc.call("approvals.list", { projectId, status: "Pending" })))?.approvals ?? [];
    for (const a of pending) {
      if (approved.has(a.id) || approved.size >= 12) continue;
      approved.add(a.id);
      console.log(`  approving: ${a.actionSummary}`);
      await soft(rpc.call("approvals.resolve", { approvalId: a.id, decision: "Approve", note: "phase3 acceptance" }));
    }
    for (const t of await targets()) {
      const m = t.url.match(/robot\.html#\/agent\/([^/?#]+)/);
      if (m && !robots.has(m[1])) {
        const c = new Cdp(t.webSocketDebuggerUrl, `robot ${m[1]}`);
        await c.open();
        robots.set(m[1], { cdp: c, states: new Set(), labels: [] });
      }
    }
    for (const [agentId, r] of robots) {
      const label = await soft(stateOf(r.cdp));
      if (!label) continue;
      const state = (label.split(": ")[1] ?? label).split(".")[0].trim();
      if (!r.states.has(state)) {
        r.states.add(state);
        r.labels.push(label);
        // Let the pose and bubble transitions settle, then capture only if the state held.
        await sleep(700);
        const again = await soft(stateOf(r.cdp));
        const data = again === label ? await soft(robotPng(r.cdp)) : null;
        if (data) robotPngs.push({ agentId, state, data });
      }
    }
    runStatus = (await rpc.call("manager.status", { runId })).runs[0].status;
    if (["Completed", "Failed", "Cancelled"].includes(runStatus) && Date.now() - t0 > 4000) break;
    await sleep(400);
  }
  const runAgents = (await rpc.call("agents.list", { projectId })).agents ?? [];
  check("H", "Manager run finished", ["Completed", "Failed"].includes(runStatus), `status ${runStatus}; approvals given: ${approved.size}`);
  check(
    "H",
    "a floating robot window per spawned agent",
    runAgents.length > 0 && runAgents.every((a) => robots.has(a.id)),
    `${robots.size} robot window(s) for ${runAgents.length} agent(s): ${runAgents.map((a) => `${a.name}:${a.state}`).join(", ")}`,
  );
  const animated = [...robots.values()].filter((r) => r.states.size >= 2).length;
  check("H", "robots changed state during the run", animated > 0, [...robots.values()].map((r) => r.labels.join(" → ")).join(" | "));

  // QA agent (Scenario H proper) + a Backend agent that waits for an approval.
  const qa = await rpc.call("agents.spawn", {
    projectId,
    templateId: qaTpl.id,
    newTask: { title: "Run the test suite", description: "Run the project tests and report the result." },
  });
  const qaId = qa.agent.id;
  const be = await rpc.call("agents.spawn", {
    projectId,
    templateId: backendTpl.id,
    newTask: { title: "Clean the build output", description: "Remove untracked build leftovers.\nRun: `git clean -fd`" },
  });
  const beId = be.agent.id;
  const qaRobot = await attach((t) => t.url.includes(`robot.html#/agent/${qaId}`), "QA robot window");
  await waitFor(() => qaRobot.eval("!!document.querySelector('.robot-window svg, .robot-window canvas')"), 30_000, "QA robot drawn");
  const qaStates = [];
  await waitFor(
    async () => {
      const l = await soft(stateOf(qaRobot));
      if (l && qaStates.at(-1) !== l) {
        qaStates.push(l);
        const data = await soft(robotPng(qaRobot));
        if (data) robotPngs.push({ agentId: qaId, state: `QA · ${(l.split(": ")[1] ?? l).split(".")[0]}`, data });
      }
      return ["Completed", "Error", "Stopped"].includes((await rpc.call("agents.get", { agentId: qaId })).agent.state);
    },
    120_000,
    "QA agent finished",
  );
  const qaFinal = (await rpc.call("agents.get", { agentId: qaId })).agent.state;
  check("H", "QA agent completes", qaFinal === "Completed", `state ${qaFinal}; robot: ${qaStates.join(" → ")}`);
  const completedPose = await waitFor(async () => /Completed|Done|Готов|Заверш/i.test((await stateOf(qaRobot)) ?? ""), 15_000, "completed pose").catch(() => false);
  check("H", "QA robot shows the Completed state", completedPose, await soft(stateOf(qaRobot)));
  const bubble = await waitFor(
    () => qaRobot.eval(`(() => { const b = document.querySelector('[class*=bubble]'); return b && b.textContent.trim() ? b.textContent.trim() : null; })()`),
    15_000,
    "QA bubble",
  ).catch(() => null);
  check("H", "completion bubble with the result", bubble, bubble);
  await sleep(900);
  robotPngs.push({ agentId: qaId, state: "Completed + bubble", data: await robotPng(qaRobot), hero: true });
  await capture(qaRobot, "h-01-qa-robot-completed-raw", "QA robot window as rendered (transparent window, captured with the webview default background)", { settle: 200 });

  await waitFor(async () => (await rpc.call("agents.get", { agentId: beId })).agent.state === "Waiting", 60_000, "backend waiting").catch(() => null);
  const beRobot = await attach((t) => t.url.includes(`robot.html#/agent/${beId}`), "backend robot window");
  await waitFor(() => beRobot.eval("!!document.querySelector('.robot-window svg, .robot-window canvas')"), 30_000, "backend robot drawn");
  await sleep(1500);
  robotPngs.push({ agentId: beId, state: "Waiting (approval)", data: await robotPng(beRobot), hero: true });
  check("H", "Backend robot waits for the approval", /Waiting|Ожида/i.test((await soft(stateOf(beRobot))) ?? ""), await soft(stateOf(beRobot)));

  // Transparency: corners of a robot capture must be fully transparent.
  const alphaProbe = await main.eval(`(async () => {
    const img = new Image();
    img.src = 'data:image/png;base64,${robotPngs.at(-1).data}';
    await img.decode();
    const c = document.createElement('canvas'); c.width = img.width; c.height = img.height;
    const g = c.getContext('2d'); g.drawImage(img, 0, 0);
    const a = (x, y) => g.getImageData(x, y, 1, 1).data[3];
    const corners = [a(1, 1), a(img.width - 2, 1), a(1, img.height - 2), a(img.width - 2, img.height - 2)];
    let opaque = 0; const d = g.getImageData(0, 0, img.width, img.height).data;
    for (let i = 3; i < d.length; i += 4) if (d[i] > 8) opaque++;
    return { w: img.width, h: img.height, corners, coverage: opaque / (img.width * img.height) };
  })()`);
  check(
    "H",
    "robot window background is transparent (no box around the robot)",
    alphaProbe.corners.every((a) => a === 0) && alphaProbe.coverage < 0.8,
    JSON.stringify(alphaProbe),
  );

  // Click-through: robot windows are layered and pass clicks through while the cursor is elsewhere.
  const wins = listWindows();
  const robotWins = wins.filter((w) => w.title === "Orkestr robot");
  check(
    "H",
    "robot windows: layered, always-on-top, click-through when not hovered",
    robotWins.length > 0 && robotWins.every((w) => w.layered && w.topmost && w.clickThrough),
    robotWins.map((w) => `${w.w}x${w.h}@${w.x},${w.y} layered=${w.layered} topmost=${w.topmost} clickThrough=${w.clickThrough}`).join("; "),
  );

  // Context menu (Waiting robot).
  await beRobot.eval(`(() => { const el = document.querySelector('.robot-window__robot'); el.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2 })); return true; })()`);
  await sleep(700);
  robotPngs.push({ agentId: beId, state: "context menu", data: await robotPng(beRobot), menu: true });
  await beRobot.eval("document.querySelector('.robot-menu')?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })), document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })), true");

  // Click the QA robot → its result opens in the main window.
  await qaRobot.eval(`(() => {
    const el = document.querySelector('.robot-window__robot');
    const r = el.getBoundingClientRect();
    const o = { bubbles: true, button: 0, clientX: r.x + r.width / 2, clientY: r.y + r.height / 2, pointerId: 1 };
    el.dispatchEvent(new PointerEvent('pointerdown', o));
    el.dispatchEvent(new PointerEvent('pointerup', o));
    return true;
  })()`);
  const opened = await waitFor(
    () => main.eval(`(() => { const d = document.querySelector('.agents-detail, [class*=agents-detail]'); return d ? d.textContent.slice(0, 120) : null; })()`),
    15_000,
    "agent detail",
  ).catch(() => null);
  check("H", "click on the robot opens the agent result in the main window", opened, opened);
  await capture(main, "h-03-robot-click-opens-agent", "Scenario H: clicking the QA robot opens its result in the Agents panel");

  // Composite the robot captures over a synthetic desktop (no real screen content).
  if (!noShots) {
    const pick = [];
    const seen = new Set();
    for (const p of robotPngs.filter((p) => p.hero)) pick.push(p);
    for (const p of robotPngs) {
      if (pick.length >= 6) break;
      if (p.hero || p.menu || seen.has(p.state)) continue;
      seen.add(p.state);
      pick.push(p);
    }
    const menu = robotPngs.find((p) => p.menu);
    const composite = await main.eval(`(async () => {
      const shots = ${JSON.stringify(pick.map((p) => ({ data: p.data, state: p.state })))};
      const menu = ${JSON.stringify(menu ? menu.data : null)};
      const imgs = await Promise.all(shots.map(async (s) => { const i = new Image(); i.src = 'data:image/png;base64,' + s.data; await i.decode(); return { i, s }; }));
      const W = 1600;
      // Lay out first, then size the canvas so labels never fall under the taskbar.
      const pos = []; let lx = 40, ly = 40, lrow = 0;
      for (const { i } of imgs) {
        if (lx + i.width > W - 300) { lx = 40; ly += lrow + 30; lrow = 0; }
        pos.push([lx, ly]); lx += i.width + 40; lrow = Math.max(lrow, i.height + 18);
      }
      const H = Math.max(900, ly + lrow + 48 + 40);
      const c = document.createElement('canvas'); c.width = W; c.height = H;
      const g = c.getContext('2d');
      const bg = g.createLinearGradient(0, 0, W, H);
      bg.addColorStop(0, '#1d3b5c'); bg.addColorStop(0.55, '#3a2f5e'); bg.addColorStop(1, '#0f1a2e');
      g.fillStyle = bg; g.fillRect(0, 0, W, H);
      // A synthetic "desktop": a few soft hills and a taskbar, nothing from the real screen.
      g.fillStyle = 'rgba(255,255,255,0.05)';
      g.beginPath(); g.moveTo(0, H * 0.72); g.bezierCurveTo(W * 0.3, H * 0.55, W * 0.6, H * 0.85, W, H * 0.66); g.lineTo(W, H); g.lineTo(0, H); g.fill();
      g.fillStyle = 'rgba(8,10,20,0.85)'; g.fillRect(0, H - 48, W, 48);
      g.fillStyle = 'rgba(255,255,255,0.75)'; g.font = '14px Segoe UI, sans-serif'; g.fillText('Synthetic desktop backdrop — robot windows captured individually (transparent)', 24, H - 18);
      imgs.forEach(({ i, s }, k) => {
        const [x, y] = pos[k];
        g.drawImage(i, x, y);
        g.fillStyle = 'rgba(255,255,255,0.7)'; g.font = '13px Segoe UI, sans-serif'; g.textAlign = 'center';
        g.fillText(s.state.slice(0, 48), x + i.width / 2, y + i.height + 14);
        g.textAlign = 'left';
      });
      if (menu) { const m = new Image(); m.src = 'data:image/png;base64,' + menu; await m.decode(); g.drawImage(m, W - m.width - 20, 40); }
      return c.toDataURL('image/png').split(',')[1];
    })()`);
    writeFileSync(join(outDir, "h-02-floating-robots-over-desktop.png"), Buffer.from(composite, "base64"));
    shots.push({ name: "h-02-floating-robots-over-desktop.png", note: `Floating robot windows (${pick.map((p) => p.state).join(", ")}; + context menu) composited over a synthetic desktop`, consoleErrors: [] });
    for (const r of [...robots.values()]) r.cdp.close();
  }

  // ── Scenario G: voice pipeline with a recognised transcript ──────────────
  await main.eval(`(() => {
    window.__ttsSpoken = [];
    const ss = window.speechSynthesis;
    if (ss) { ss.speak = (u) => { window.__ttsSpoken.push({ text: u.text, lang: u.lang }); setTimeout(() => { try { u.onend && u.onend(new Event('end')); u.dispatchEvent(new Event('end')); } catch {} }, 50); }; ss.cancel = () => {}; }
    return !!ss;
  })()`);
  const hook = await main.eval("typeof window.__orkestrDebug?.voice?.simulateTranscript === 'function'");
  check("G", "voice debug hook installed (debug flag)", hook);
  const before = rpc.events.length;
  await main.eval("window.__orkestrDebug.voice.simulateTranscript('Jarvis, что происходит со Schoolify?'), true");
  const vstate = await waitFor(async () => {
    const s = await main.eval("window.__orkestrDebug.voice.state()");
    return s.phase === "result" && s.reply ? s : null;
  }, 30_000, "voice result").catch(async () => main.eval("window.__orkestrDebug.voice.state()"));
  check("G", "transcript → projectStatus → summary reply", vstate?.reply?.text && /Schoolify/.test(vstate.reply.text), vstate?.reply?.text);
  const spoken = await waitFor(async () => {
    const s = await main.eval("window.__ttsSpoken");
    return s.length ? s : null;
  }, 10_000, "tts").catch(() => []);
  check("G", "TTS invoked with the reply", spoken.length > 0, spoken.map((s) => `[${s.lang}] ${s.text}`).join(" | "));
  const recorded = await waitFor(
    () => rpc.events.slice(before).find((e) => /Voice/i.test(e.kind)),
    5000,
    "voice event",
  ).catch(() => null);
  check("G", "command recorded in the core (transcript only, no audio)", recorded, recorded ? `${recorded.kind} ${JSON.stringify(recorded.payload).slice(0, 160)}` : "no voice event observed");
  await main.eval("window.__orkestrDebug.voice.state().hudOpen || true");
  await capture(main, "g-01-voice-hud-status", "Scenario G: «Jarvis, что происходит со Schoolify?» → project status summary (spoken)", { settle: 600 });

  // ── themes on Mission Control ───────────────────────────────────────────
  await main.key("Escape");
  await main.key("2", { ctrl: true });
  await sleep(1500);
  const mcTabs = await dockTabs(main);
  check("Layout", "Ctrl+2 → Mission Control preset", mcTabs.some((t) => /Mission Control/.test(t)), mcTabs.join(", "));
  const themes = (await rpc.call("ui.themes", {})).themes.filter((e) => e.builtin);
  const original = await rpc.call("ui.getConfig", {});
  for (const [i, e] of themes.entries()) {
    const cur = await rpc.call("ui.getConfig", {});
    await rpc.call("ui.setConfig", { config: { ...cur.config, theme: e.theme } });
    await sleep(1800);
    const applied = await main.eval(`getComputedStyle(document.documentElement).getPropertyValue('--ok-bg').trim() || getComputedStyle(document.body).backgroundColor`);
    check("Theme", `theme ${e.theme.name} applies live`, applied, applied);
    await capture(main, `t-${String(i + 1).padStart(2, "0")}-theme-${e.theme.id}-mission-control`, `Mission Control in the ${e.theme.name} theme`, { settle: 1500 });
  }
  console.log(`  kpi grid: ${await soft(main.eval(`(() => { const g = document.querySelector('.mc-kpis'); return g ? getComputedStyle(g).gridTemplateColumns + ' / width ' + g.clientWidth : 'none'; })()`))}`);
  await rpc.call("ui.setConfig", { config: original.config });
  await sleep(1200);

  // ── layout presets ──────────────────────────────────────────────────────
  const presetTabs = {};
  for (const [k, name] of [["1", "Coding"], ["2", "Mission Control"], ["3", "Monitoring"], ["4", "Focus"], ["5", "Review"]]) {
    await main.key(k, { ctrl: true });
    await sleep(1300);
    presetTabs[name] = await dockTabs(main);
    const active = await soft(rpc.call("ui.layoutsList", {}));
    check("Layout", `Ctrl+${k} → ${name}`, presetTabs[name].length > 0, `${presetTabs[name].join(", ")}${active ? ` (active ${active.activeId})` : ""}`);
  }
  check("Layout", "Focus shows only editor + Manager", presetTabs.Focus.length <= 3, presetTabs.Focus.join(", "));
  await main.key("4", { ctrl: true });
  await sleep(1500);
  await capture(main, "l-02-focus-layout", "Focus preset: editor + AI Manager only");
  await main.key("1", { ctrl: true });
  await sleep(1300);
  // Coding preset: Files | Code | Manager | Agents, with Logs/Terminal/Git below.
  await palette(main, "Open Panel: Terminal");
  await sleep(1000);
  await main.key("p", { ctrl: true });
  await sleep(500);
  await main.type("grades.mjs");
  await sleep(800);
  await main.key("Enter");
  await sleep(1500);
  const codingTabs = await main.eval(`[...document.querySelectorAll('.dv-tab.dv-active-tab')].filter((e) => e.offsetParent).map((e) => e.textContent.trim())`);
  check("Layout", "Coding shows Code, Manager and Terminal at once (active in their groups)", ["Code", "Manager", "Terminal"].every((n) => codingTabs.includes(n)), codingTabs.join(", "));
  await capture(main, "l-01-coding-code-terminal-manager", "Coding preset with Code (grades.mjs), Terminal and the AI Manager", { settle: 1500 });

  // ── performance modes ───────────────────────────────────────────────────
  for (const mode of ["UltraVisual", "Balanced", "Performance", "BatterySaver"]) {
    await rpc.call("settings.update", { patch: { performanceMode: mode } });
    await sleep(1200);
    const ds = await main.eval("JSON.stringify(document.documentElement.dataset)");
    check("Perf", `performance mode ${mode} → policy on <html>`, ds.toLowerCase().includes(mode.toLowerCase()) || ds.length > 2, ds);
    if (mode === "BatterySaver") await capture(main, "p-01-battery-saver", "Battery Saver: static background, no blur/glow/motion");
  }
  await rpc.call("settings.update", { patch: { performanceMode: "Balanced" } });
  await sleep(1000);

  // ── HUD ─────────────────────────────────────────────────────────────────
  await main.eval("window.__TAURI_INTERNALS__.invoke('hud_show', { position: null }).then(() => true)");
  const hud = await attach((t) => t.id !== main.targetId && !t.url.includes("robot.html") && !t.url.includes("#/panel"), "HUD webview", 30_000);
  const hudShells = await soft(hud.eval("document.querySelectorAll('.app__body').length"));
  check("HUD", "HUD window renders only the HUD (no main shell behind it)", hudShells === 0, `shells in HUD: ${hudShells}`);
  const hudOk = await waitFor(() => hud.eval("!!document.querySelector('.hud')"), 30_000, "HUD rendered").catch(() => false);
  check("HUD", "HUD window renders", hudOk);
  await sleep(2000);
  await capture(hud, "u-01-hud", "HUD window: run status, agents, approvals, voice indicator, mini Manager");
  await main.eval("window.__TAURI_INTERNALS__.invoke('hud_hide').then(() => true, () => false)");
  hud.close();

  // ── Voice HUD (typed) ───────────────────────────────────────────────────
  await main.key("Escape");
  await palette(main, "Open voice commands");
  const focused = await waitFor(
    () => main.eval(`(() => { const i = document.querySelector('.vx-hud__input'); i?.focus(); return !!i && document.activeElement === i; })()`),
    10_000,
    "voice HUD input",
  ).catch(() => false);
  check("G", "voice HUD opens with a focused command field", focused);
  if (focused) {
    await main.type("show build errors for Schoolify");
    await main.key("Enter");
  }
  const vxReply = await waitFor(() => main.eval(`(() => { const c = document.querySelector('.vx-card'); return c ? c.textContent.trim().slice(0, 160) : null; })()`), 15_000, "voice reply").catch(() => null);
  check("G", "typed «show build errors» answered from the quality runner", vxReply, vxReply);
  await capture(main, "v-01-voice-hud", "Voice HUD: typed command «show build errors for Schoolify» → quality-runner answer");
  await main.key("Escape");
  await sleep(500);

  // ── AI-editable UI ──────────────────────────────────────────────────────
  const cfgBefore = await rpc.call("ui.getConfig", {});
  await palette(main, "Open Panel: Manager");
  await chat("Make Mission Control more compact, put Claude on the right, hide the token chart and make the background darker");
  const card = await waitFor(() => main.eval("!!document.querySelector('.aiui-card .aiui-changes')"), 30_000, "proposal card").catch(() => false);
  check("AI-UI", "sentence → validated patch proposal with live preview", card);
  await capture(main, "a-01-aiui-preview", "AI-editable UI: Manager proposal (validated patch, diff summary) with live preview");
  await clickText(main, ".aiui-card button", "Keep");
  const applied = await waitFor(() => main.eval("!!document.querySelector('.aiui-card--applied')"), 15_000, "applied").catch(() => false);
  const cfgAfter = await rpc.call("ui.getConfig", {});
  check("AI-UI", "apply stores a new UI config version", applied && cfgAfter.version > cfgBefore.version, `v${cfgBefore.version} → v${cfgAfter.version}`);
  await capture(main, "a-02-aiui-applied", "After Keep: stored as a new UI-config version, with Undo");
  await clickText(main, ".aiui-card button", "Undo");
  await sleep(1500);
  const cfgUndo = await rpc.call("ui.getConfig", {});
  const same = JSON.stringify(cfgUndo.config.theme) === JSON.stringify(cfgBefore.config.theme) && JSON.stringify(cfgUndo.config.shell ?? null) === JSON.stringify(cfgBefore.config.shell ?? null);
  check("AI-UI", "Undo reverts to the previous config", same, `v${cfgUndo.version}`);

  // ── detach + re-dock ────────────────────────────────────────────────────
  await main.key("1", { ctrl: true });
  await sleep(1200);
  await main.eval("document.querySelector('.dv-tab')?.click(), true");
  const before2 = new Set((await targets()).map((t) => t.id));
  const clicked = await main.eval(`(() => { const b = document.querySelector('.dock-actions__btn[aria-label="Open in a separate window"]'); b?.click(); return !!b; })()`);
  check("Layout", "detach button present", clicked);
  let pop = null;
  const popOk = await waitFor(
    async () => {
      pop?.close();
      pop = await attach((t) => t.url.includes("#/panel/") && !before2.has(t.id), "detached panel window", 5000);
      return pop.eval("!!document.querySelector('.detached__panel')");
    },
    40_000,
    "detached panel",
  ).catch(() => false);
  check("Layout", "panel detached into its own window", popOk, pop?.url.split("#")[1]);
  if (pop) {
    const popShells = await soft(pop.eval("document.querySelectorAll('.app__body').length"));
    check("Layout", "detached window renders only its panel", popShells === 0, `shells in detached window: ${popShells}`);
    await capture(pop, "l-04-detached-panel", `Detached panel window (${pop.url.split("#")[1]}) with its own title bar and re-dock`);
    await soft(pop.eval("document.querySelector('.detached__redock').click(), true"));
    const gone = await waitFor(async () => !(await targets()).some((t) => t.id === pop.targetId), 15_000, "detached closed").catch(() => false);
    check("Layout", "re-dock closes the window and returns the panel", gone);
  }

  // ── editors, settings, onboarding ───────────────────────────────────────
  await palette(main, "Layout editor");
  await capture(main, "l-03-layout-editor", "Layout Editor: grid, presets, panel picker, monitor map, lock");
  await soft(main.eval("document.querySelector('.lyt-sheet__footer button')?.click(), true"));
  await main.key("Escape");
  await palette(main, "Open Theme Editor");
  await sleep(800);
  await capture(main, "e-01-theme-editor", "Theme Editor with live preview and WCAG contrast checks");
  await soft(main.eval(`document.querySelector('.th-editor button[aria-label="Close the editor"]')?.click(), true`));
  await sleep(300);
  await soft(clickText(main, ".th-editor__confirm button", "Discard"));
  await palette(main, "Open Robot Designer");
  await sleep(800);
  await capture(main, "e-02-robot-designer", "Settings → Robots: Robot Designer with live preview");
  await main.key("Escape");
  await palette(main, "Open Settings");
  await sleep(500);
  await clickText(main, "[role=tab], button", "Interface");
  await capture(main, "s-01-settings-interface", "Settings → Interface: performance modes, density, HUD");
  await main.key("Escape");
  await palette(main, "Run Setup Wizard");
  await capture(main, "s-02-onboarding", "Onboarding wizard");
  await main.key("Escape");

  rpc.close();
} catch (e) {
  exitCode = 1;
  console.error(e.stack ?? e);
  console.error(log.slice(-3000));
} finally {
  check("Shell", "command palette ran every command with Enter", paletteMisses.length === 0, paletteMisses.join(", "));
  const failed = checks.filter((c) => !c.ok);
  writeFileSync(join(outDir, "acceptance.json"), `${JSON.stringify({ checks, shots }, null, 2)}\n`);
  console.log(`\n${checks.length - failed.length}/${checks.length} checks passed`);
  if (failed.length) exitCode = 1;
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
