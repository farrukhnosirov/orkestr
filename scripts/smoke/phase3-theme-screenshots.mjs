#!/usr/bin/env node
// Phase 3 theme-engine review (infrastructure from phase1-screenshots.mjs):
// launches the desktop app with an isolated ORKESTR_HOME, seeds a project over
// RPC, drives the Appearance settings and the Theme Editor through the WebView2
// DevTools protocol and captures each view (webview-only DevTools capture by
// default, so no other application can leak into the PNGs).
//
// Usage (repo root, Windows) — no Vite needed, the frontend is bundled:
//   npm run build
//   cargo build -p orkestr-desktop --features custom-protocol
//   node scripts/smoke/phase3-theme-screenshots.mjs [--out docs/screenshots/phase3-theme]
// ORKESTR_SMOKE_EXE overrides the executable, ORKESTR_SMOKE_CDP_PORT the DevTools port.

import { spawn, execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "phase3-theme"));
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(repo, "target", "debug", "orkestr-desktop.exe");
const cdpPort = Number(process.env.ORKESTR_SMOKE_CDP_PORT ?? 9347);
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

const clickSel = (cdp, selector) => cdp.eval(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); if (!el) return false; el.click(); return true; })()`);

let exitCode = 0;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.sub(["*"]);
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, performanceMode: "UltraVisual", language: "en" } });
  await rpc.call("projects.create", { name: "hello-app", rootPath: repoDir, approvalMode: "SemiAuto" });
  console.log("seeded");

  const target = await waitFor(async () => {
    const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
    return list.find((t) => t.type === "page" && !t.url.startsWith("devtools"));
  }, 60_000, "webview DevTools target");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send("Page.reload", { ignoreCache: true });
  await waitFor(() => cdp.eval("!!document.querySelector('.app__body')"), 60_000, "app shell");
  await sleep(2500);
  await cdp.eval(`document.querySelector('[aria-label="Open project hello-app"]')?.click(), true`);
  await sleep(1500);
  await cdp.eval(`document.querySelector('.resume-card button[aria-label*="ismiss" i], .resume-card button[aria-label*="lose" i]')?.click(), true`);
  await sleep(800);

  await capture(cdp, "01-nebula-ultra", "Nebula (default) in Ultra Visual: WebGL nebula background");
  await palette(cdp, "Open Appearance Settings");
  await sleep(800);
  await capture(cdp, "02-appearance-gallery", "Settings → Appearance: theme gallery with live miniatures and contrast tags");
  await clickSel(cdp, '[aria-label="Use theme Solar Light"]');
  await sleep(1500);
  await capture(cdp, "03-appearance-solar-light", "Solar Light (light theme, WCAG AA) applied from the gallery");
  await cdp.key("Escape");
  await sleep(600);
  await capture(cdp, "04-solar-light-workspace", "Workspace in Solar Light");
  await palette(cdp, "Open Theme Editor");
  await sleep(1200);
  await capture(cdp, "05-editor-colors", "Theme editor (live preview) — Colors tab with the WCAG contrast check");
  await cdp.eval(`document.querySelector('.th-color__swatch')?.click(), true`);
  await sleep(500);
  await capture(cdp, "06-editor-picker", "Custom HSV color picker");
  await clickText(cdp, ".th-editor__tab", "Background");
  await sleep(400);
  await clickText(cdp, ".ok-segmented__item", "Particles");
  await sleep(1500);
  await capture(cdp, "07-editor-background", "Background tab: procedural particles preview (live on the app too)");
  await clickText(cdp, ".th-editor__tab", "Effects");
  await sleep(600);
  await capture(cdp, "08-editor-effects", "Effects tab: radius, spacing, glass blur, glow, shadows");
  await cdp.eval(`document.querySelector('.th-editor button[aria-label="Close the editor"]')?.click(), true`);
  await sleep(400);
  await clickText(cdp, ".th-editor__confirm button", "Discard");
  await sleep(800);

  await rpc.call("settings.update", { patch: { language: "ru", performanceMode: "Performance" } });
  await sleep(1200);
  await palette(cdp, "Открыть настройки оформления");
  await sleep(800);
  await clickSel(cdp, '[aria-label="Применить тему Aurora"]');
  await sleep(1200);
  await capture(cdp, "09-ru-appearance-aurora-performance", "Russian UI: Aurora in Performance mode (static background, no blur)");
  await palette(cdp, "Открыть редактор тем");
  await sleep(1200);
  await capture(cdp, "10-ru-editor", "Russian UI: theme editor");
  await cdp.eval(`document.querySelector('.th-editor button[aria-label="Закрыть редактор"]')?.click(), true`);
  await sleep(400);
  await rpc.call("settings.update", { patch: { language: "en", performanceMode: "Balanced" } });
  await palette(cdp, "Open Appearance Settings");
  await sleep(600);
  await clickSel(cdp, '[aria-label="Use theme High Contrast"]');
  await sleep(1000);
  await cdp.key("Escape");
  await sleep(800);
  await capture(cdp, "11-high-contrast", "High Contrast (accessibility, AAA) workspace");
  await palette(cdp, "Open Appearance Settings");
  await sleep(600);
  await clickSel(cdp, '[aria-label="Use theme Carbon"]');
  await sleep(1000);
  await cdp.key("Escape");
  await sleep(1500);
  await capture(cdp, "12-carbon-grid", "Carbon: perspective grid background (Balanced, canvas 30 fps)");
  rpc.close();
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
