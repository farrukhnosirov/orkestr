#!/usr/bin/env node
// Phase 3 voice-sound UI review: launches the desktop app built with the
// bundled frontend (`cargo build -p orkestr-desktop --features custom-protocol`
// after `npm run build`), seeds an isolated ORKESTR_HOME through the RPC API,
// drives the voice HUD (typed commands — no microphone is opened, so no OS
// permission prompt) and the Voice / Sounds settings through the WebView2
// DevTools protocol, and captures the webview only (never the screen).
//
// Usage (repo root, Windows): node scripts/smoke/phase3-voice-screenshots.mjs [--out docs/screenshots/phase3-voice]

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "phase3-voice"));
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(repo, "target", "debug", "orkestr-desktop.exe");
const cdpPort = 9341;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

const base = mkdtempSync(join(tmpdir(), "orkestr-voice-shots-"));
const home = join(base, "home");
const repoDir = join(base, "hello-app");
mkdirSync(home, { recursive: true });
mkdirSync(repoDir, { recursive: true });
const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();
git(repoDir, "init", "-q", "-b", "main");
git(repoDir, "config", "user.email", "demo@orkestr.local");
git(repoDir, "config", "user.name", "Orkestr Demo");
writeFileSync(join(repoDir, "README.md"), "# hello-app\n");
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
  ws.onmessage = (m) => {
    const f = JSON.parse(m.data);
    if (f.type === "res") {
      const p = pending.get(f.id);
      pending.delete(f.id);
      f.ok ? p?.resolve(f.result) : p?.reject(new Error(`${f.error.code}: ${f.error.message}`));
    }
  };
  const frame = (o, id) =>
    new Promise((res, rej) => {
      pending.set(id, { resolve: res, reject: rej });
      ws.send(JSON.stringify(o));
    });
  return {
    open: () =>
      new Promise((res, rej) => {
        ws.onopen = () => frame({ type: "auth", token, id: "a" }, "a").then(res, rej);
        ws.onerror = rej;
      }),
    call: (method, params = {}) => {
      const id = `r${++seq}`;
      return frame({ type: "req", id, method, params }, id);
    },
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
  async key(key, { ctrl = false } = {}) {
    const code = key.length === 1 ? `Key${key.toUpperCase()}` : key;
    const vk = key.length === 1 ? key.toUpperCase().charCodeAt(0) : { Enter: 13, Escape: 27 }[key];
    for (const type of ["rawKeyDown", "keyUp"]) {
      // Enter carries its text so forms see a real submit keystroke (implicit submission).
      const extra = key === "Enter" && type === "rawKeyDown" ? { type: "keyDown", text: "\r", unmodifiedText: "\r" } : {};
      await this.send("Input.dispatchKeyEvent", { type, modifiers: ctrl ? 2 : 0, key, code, windowsVirtualKeyCode: vk, ...extra });
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
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(file, Buffer.from(data, "base64"));
  const errors = cdp.console.splice(0);
  shots.push({ name, note, errors });
  console.log(`${name}${errors.length ? `  — ${errors.length} console message(s)` : ""}`);
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

async function hudCommand(cdp, text) {
  await cdp.eval(`document.querySelector('.vx-hud__input')?.focus(), true`);
  await cdp.type(text);
  await sleep(200);
  await cdp.key("Enter");
  await waitFor(() => cdp.eval(`!!document.querySelector('.vx-card')`), 15_000, "HUD reply");
}

const clickText = (cdp, selector, text) =>
  cdp.eval(`(() => {
    const el = [...document.querySelectorAll(${JSON.stringify(selector)})].find((e) => e.textContent.trim().includes(${JSON.stringify(text)}));
    if (!el) return false;
    el.click();
    return true;
  })()`);

const scrollTo = (cdp, selector, text) =>
  cdp.eval(`(() => {
    const el = [...document.querySelectorAll(${JSON.stringify(selector)})].find((e) => e.textContent.trim().includes(${JSON.stringify(text)}));
    el?.scrollIntoView({ block: "start" });
    return !!el;
  })()`);

let exitCode = 0;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, language: "en" } });
  const { project } = await rpc.call("projects.create", { name: "hello-app", rootPath: repoDir, approvalMode: "SemiAuto" });
  for (const [title, path] of [
    ["Design the greeting API", ["Ready", "Running"]],
    ["Write unit tests", ["Ready", "Running", "Failed"]],
    ["Document the module", []],
  ]) {
    const { task } = await rpc.call("tasks.create", { projectId: project.id, title, description: "" });
    for (const to of path) await rpc.call("tasks.transition", { taskId: task.id, to }).catch((e) => console.log(`transition: ${e.message}`));
  }
  rpc.close();
  console.log("seeded");

  const target = await waitFor(async () => {
    const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
    return list.find((t) => t.type === "page");
  }, 60_000, "webview DevTools target");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send("Page.reload", { ignoreCache: true });
  await waitFor(() => cdp.eval("!!document.querySelector('.app__body')"), 60_000, "app shell");
  await sleep(2500);
  await cdp.eval(`document.querySelector('[aria-label="Open project hello-app"]')?.click(), true`);
  await sleep(1200);
  await cdp.eval(`document.querySelector('.resume-card button[aria-label*="ismiss" i], .resume-card button[aria-label*="lose" i]')?.click(), true`);

  await palette(cdp, "Open voice commands");
  await capture(cdp, "01-hud-ready", "Voice HUD (typed mode: speech recognition off by default) with the title-bar mic button");
  await hudCommand(cdp, "Jarvis, what's happening with hello-app?");
  await capture(cdp, "02-hud-status", "G: status question → state lookup → concise summary");
  await hudCommand(cdp, "stop hello-app");
  await capture(cdp, "03-hud-confirm", "Destructive intent: confirmation before emergency.stopProject");
  await clickText(cdp, ".vx-card button", "Cancel");
  await cdp.key("Escape");
  await sleep(500);

  await palette(cdp, "Voice settings");
  await capture(cdp, "04-settings-voice", "Settings → Voice: commands, microphone, speech recognition engines");
  await clickText(cdp, ".ok-segmented__item", "OpenAI Whisper");
  await sleep(400);
  await scrollTo(cdp, ".vx-notice", "Send your voice");
  await capture(cdp, "05-settings-consent", "Explicit consent before audio may leave the device");
  await clickText(cdp, ".vx-notice button", "Cancel");
  await scrollTo(cdp, ".set-group", "Wake word");
  await capture(cdp, "06-settings-wake", "Wake word: local enrollment, level meter, sensitivity, CPU budget");
  await scrollTo(cdp, ".set-group", "Agent voices");
  await cdp.eval(`document.querySelector('.vx-agent__toggle')?.click(), true`);
  await capture(cdp, "07-settings-agent-voices", "Voice per agent");
  await clickText(cdp, "[role=tab]", "Sounds");
  await capture(cdp, "08-settings-sound", "Sounds and spoken notifications");
  await cdp.key("Escape");
  await sleep(400);

  const rpc2 = rpcClient(info.url, token);
  await rpc2.open();
  await rpc2.call("settings.update", { patch: { language: "ru" } });
  rpc2.close();
  await sleep(1500);
  await palette(cdp, "Открыть голосовые команды");
  await hudCommand(cdp, "Джарвис, что происходит с hello-app?");
  await capture(cdp, "09-hud-ru", "HUD in Russian");
  await cdp.key("Escape");
  await palette(cdp, "Настройки голоса");
  await capture(cdp, "10-settings-voice-ru", "Settings → Voice in Russian");

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
