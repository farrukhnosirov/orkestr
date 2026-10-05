#!/usr/bin/env node
// Phase 3 layout-multimonitor visual check: launches the desktop app against
// an isolated ORKESTR_HOME, drives presets, the switcher, the Layout Editor
// (en + ru), Save-as and a detached panel window through the WebView2
// DevTools protocol, captures each view (webview-only capture — never the
// screen) and verifies the layouts were persisted per device in the core.
//
// Usage (repo root, Windows):
//   npx vite --port 1437 --strictPort --host 127.0.0.1      (in apps/desktop)
//   TAURI_CONFIG='{"build":{"devUrl":"http://127.0.0.1:1437"}}' cargo build -p orkestr-desktop
//   node scripts/smoke/phase3-layout-screenshots.mjs [--out docs/screenshots/phase3-layout]
// Env: ORKESTR_SMOKE_EXE, ORKESTR_SMOKE_DEV_PORT (default 1437), ORKESTR_SMOKE_CDP_PORT (default 9347).

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "phase3-layout"));
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(repo, "target", "debug", "orkestr-desktop.exe");
const devPort = Number(process.env.ORKESTR_SMOKE_DEV_PORT ?? 1437);
const cdpPort = Number(process.env.ORKESTR_SMOKE_CDP_PORT ?? 9347);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

const base = mkdtempSync(join(tmpdir(), "orkestr-layout-shots-"));
const home = join(base, "home");
const repoDir = join(base, "hello-app");
mkdirSync(home, { recursive: true });
mkdirSync(repoDir, { recursive: true });
const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();
git(repoDir, "init", "-q", "-b", "main");
writeFileSync(join(repoDir, "README.md"), "# hello-app\n");
git(repoDir, "-c", "user.email=demo@orkestr.local", "-c", "user.name=Demo", "add", "-A");
git(repoDir, "-c", "user.email=demo@orkestr.local", "-c", "user.name=Demo", "commit", "-q", "-m", "initial");

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
    if (f.type !== "res") return;
    const p = pending.get(f.id);
    pending.delete(f.id);
    f.ok ? p?.resolve(f.result) : p?.reject(new Error(`${f.error.code}: ${f.error.message}`));
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
    const isDigit = /^[0-9]$/.test(key);
    const code = key.length === 1 ? (isDigit ? `Digit${key}` : `Key${key.toUpperCase()}`) : key;
    const vk = key.length === 1 ? key.toUpperCase().charCodeAt(0) : { Enter: 13, Escape: 27 }[key];
    for (const type of ["rawKeyDown", "keyUp"]) {
      await this.send("Input.dispatchKeyEvent", { type, modifiers: ctrl ? 2 : 0, key, code, windowsVirtualKeyCode: vk });
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
  console.log(`${name}${errors.length ? `  — ${errors.length} console error(s)` : ""}`);
  for (const e of errors) console.log(`    ${e.slice(0, 300)}`);
}

async function palette(cdp, text) {
  await cdp.eval("document.activeElement?.blur?.(), true");
  await cdp.key("k", { ctrl: true });
  await sleep(300);
  await cdp.type(text);
  await sleep(400);
  await cdp.key("Enter");
  await sleep(700);
}

const targets = async () => (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();

let exitCode = 0;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, language: "en" } });
  await rpc.call("projects.create", { name: "hello-app", rootPath: repoDir, approvalMode: "SemiAuto" });

  const target = await waitFor(
    async () => (await targets()).find((t) => t.type === "page" && (t.url.includes(`127.0.0.1:${devPort}`) || t.url.includes("tauri.localhost")) && !t.url.includes("#/panel")),
    60_000,
    "main webview target",
  );
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send("Page.reload", { ignoreCache: true });
  await waitFor(() => cdp.eval("!!document.querySelector('.app__body .dv-groupview')"), 60_000, "workspace");
  await sleep(2500);
  await cdp.eval(`document.querySelector('[aria-label="Open project hello-app"]')?.click(), true`);
  await sleep(1200);
  await cdp.eval(`document.querySelector('.resume-card button[aria-label*="ismiss" i], .resume-card button[aria-label*="lose" i]')?.click(), true`);

  await capture(cdp, "01-coding", "Default Coding preset with dock group actions (float / detach / maximize)");
  await cdp.eval("document.querySelector('.lyt-switcher__btn').click(), true");
  await capture(cdp, "02-switcher", "Title-bar layout switcher: presets with Ctrl+1..5, my layouts, save / lock / editor");
  await cdp.key("Escape");
  await cdp.key("4", { ctrl: true });
  await sleep(800);
  await capture(cdp, "03-focus", "Ctrl+4: Focus preset");
  await cdp.key("3", { ctrl: true });
  await sleep(800);
  await capture(cdp, "04-monitoring", "Ctrl+3: Monitoring preset");
  await cdp.key("1", { ctrl: true });
  await sleep(800);
  await palette(cdp, "Layout editor");
  await capture(cdp, "05-editor", "Layout Editor: grid overlay, presets, panel picker, monitor map, lock");

  await rpc.call("settings.update", { patch: { language: "ru" } });
  await sleep(1500);
  await capture(cdp, "06-editor-ru", "Layout Editor in Russian");
  await cdp.eval("document.querySelector('.lyt-switch').click(), true");
  await sleep(500);
  await capture(cdp, "07-locked-ru", "Layout locked: no close/float/detach controls");
  await cdp.eval("document.querySelector('.lyt-switch').click(), true");
  await rpc.call("settings.update", { patch: { language: "en" } });
  await sleep(1200);
  await cdp.eval("document.querySelector('.lyt-sheet__footer button').click(), true");
  await sleep(500);

  await palette(cdp, "Save layout as");
  await cdp.type("Deep work");
  await capture(cdp, "08-save-dialog", "Save layout as…");
  await cdp.eval("document.querySelector('.dialog__footer button[type=submit]').click(), true");
  await sleep(1500);
  await cdp.eval("document.querySelector('.dv-tab')?.click(), true");
  await palette(cdp, "Float panel");
  await capture(cdp, "09-floating", "Saved as «Deep work»; a floating in-app panel");

  // Detach the active panel into its own frameless window.
  const before = new Set((await targets()).map((t) => t.id));
  const clicked = await cdp.eval(
    `(() => { const b = document.querySelector('.dv-resize-container .dock-actions__btn[aria-label="Open in a separate window"]') ?? document.querySelector('.dock-actions__btn[aria-label="Open in a separate window"]'); b?.click(); return !!b; })()`,
  );
  if (!clicked) throw new Error("no detach button");
  await sleep(2500);
  await capture(cdp, "10a-after-detach-click", "Main window right after the detach click");
  const popout = await waitFor(
    async () => (await targets()).find((t) => t.type === "page" && t.url.includes("#/panel/") && !before.has(t.id)),
    30_000,
    "detached panel window",
  );
  const pcdp = new Cdp(popout.webSocketDebuggerUrl);
  await pcdp.open();
  await waitFor(() => pcdp.eval("!!document.querySelector('.detached__panel')"), 30_000, "detached panel");
  await sleep(1500);
  await capture(pcdp, "10-detached-window", `Detached panel window (${popout.url.split("#")[1]}): custom title bar with re-dock`);
  await sleep(1500);
  await capture(cdp, "11-main-after-detach", "Main window after detaching");

  const state = await waitFor(async () => {
    const s = await rpc.call("ui.layoutsList", {});
    const deep = s.layouts.find((l) => l.name === "Deep work");
    return deep && deep.windows.popouts.length > 0 ? s : null;
  }, 15_000, "persisted layout with a detached window");
  const deep = state.layouts.find((l) => l.name === "Deep work");
  console.log(`persisted: device ${state.deviceId}, active ${state.activeId}, ${state.layouts.length} layout(s); «Deep work» popouts: ${deep.windows.popouts.map((p) => `${p.panelId}@${p.monitor}`).join(", ")}`);

  // Re-dock from the detached window.
  await pcdp.eval("document.querySelector('.detached__redock').click(), true").catch(() => null);
  await waitFor(async () => !(await targets()).some((t) => t.id === popout.id), 15_000, "detached window closed");
  await sleep(1200);
  await capture(cdp, "12-redocked", "Panel re-docked into the main window");
  rpc.close();
} catch (e) {
  exitCode = 1;
  console.error(e);
  console.error(log.slice(-3000));
} finally {
  child.kill();
}
writeFileSync(join(outDir, "shots.json"), JSON.stringify(shots, null, 2));
process.exit(exitCode);
