#!/usr/bin/env node
// Cursor capture mode (ADR-0050) UI walkthrough + screenshots — SAFE:
// the app runs with ORKESTR_COMPUTER_BACKEND=mock (no real input, no real
// screen capture; the "screenshots" the model sees are flat grey) and a
// scripted model (ORKESTR_CURSOR_MOCK_SCRIPT), an isolated ORKESTR_HOME /
// CODEX_HOME / WebView2 profile, and the source switched to "API keys"
// (the isolated vault is empty) so no real model is ever called.
// Only Orkestr's own webview is captured (CDP Page.captureScreenshot,
// cropped to the panel) into docs/screenshots/cursor-mode/.
//
//   npm run build
//   cargo build -p orkestr-desktop --features custom-protocol
//   node scripts/smoke/cursor-mode-screenshots.mjs [--out docs/screenshots/cursor-mode]
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "cursor-mode"));
const targetDir = process.env.CARGO_TARGET_DIR ?? join(repo, "target");
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(targetDir, "debug", "orkestr-desktop.exe");
const cdpPort = Number(process.env.ORKESTR_SMOKE_CDP_PORT ?? 9361);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const J = JSON.stringify;
mkdirSync(outDir, { recursive: true });

const checks = [];
function check(id, label, ok, detail = "") {
  checks.push({ id, label, ok: !!ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"} ${id} ${label}${detail ? ` — ${detail}` : ""}`);
}

const base = mkdtempSync(join(tmpdir(), "ok-cursor-shots-"));
const home = join(base, "home");
const script = join(base, "script.json");
writeFileSync(
  script,
  J([
    {
      thought: "Open the Bluetooth page of Settings",
      plan: ["Open Settings → Bluetooth", "Turn Bluetooth on", "Check that it is on"],
      say: "Открываю настройки Bluetooth, сэр.",
      action: { type: "launch", app: "ms-settings:bluetooth" },
    },
    { thought: "Wait for the page", action: { type: "wait", ms: 4000 } },
    { thought: "Flip the Bluetooth switch", risk: "system", say: "Включаю Bluetooth.", action: { type: "click", x: 900, y: 260 } },
    { thought: "Verify", action: { type: "wait", ms: 2500 } },
    { thought: "Bluetooth is on", action: { type: "done", summary: "Bluetooth включён.", success: true } },
  ]),
);

const child = spawn(exe, [], {
  env: {
    ...process.env,
    ORKESTR_HOME: home,
    CODEX_HOME: join(base, "codex-home"),
    ORKESTR_SECRET_BACKEND: "vault-file",
    ORKESTR_COMPUTER_BACKEND: "mock",
    ORKESTR_CURSOR_MOCK_SCRIPT: script,
    ORKESTR_ALLOW_SECOND_INSTANCE: "1",
    WEBVIEW2_USER_DATA_FOLDER: join(base, "webview2"),
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
      ws.send(J(o));
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
      } else if (f.method === "Runtime.exceptionThrown") {
        this.console.push(`exception: ${f.params.exceptionDetails.exception?.description ?? f.params.exceptionDetails.text}`);
      }
    };
    await this.send("Runtime.enable");
    await this.send("Page.enable");
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((res, rej) => {
      this.pending.set(id, { resolve: res, reject: (e) => rej(new Error(`${method}: ${e.message}`)) });
      this.ws.send(J({ id, method, params }));
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
      await this.send("Input.dispatchKeyEvent", { type, modifiers: ctrl ? 2 : 0, key, code, windowsVirtualKeyCode: vk });
    }
  }
  async type(text) {
    await this.send("Input.insertText", { text });
  }
}

async function findMainTarget(port) {
  const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  for (const t of list.filter((t) => t.type === "page" && !t.url.startsWith("devtools") && !t.url.includes("#/"))) {
    const c = new Cdp(t.webSocketDebuggerUrl);
    try {
      await c.open();
      if ((await c.eval("window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label ?? null")) === "main") return t;
    } catch {
      /* not ready */
    } finally {
      try {
        c.ws.close();
      } catch {
        /* closed */
      }
    }
  }
  return null;
}

const shots = [];
/** Captures the element matching `selector` (Orkestr UI only), padded. */
async function shot(cdp, name, selector, note) {
  await sleep(700);
  const rect = await cdp.eval(`(() => { const e = document.querySelector(${J(selector)}); if (!e) return null; const r = e.getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height }; })()`);
  const pad = 12;
  const clip = rect ? { x: Math.max(0, rect.x - pad), y: Math.max(0, rect.y - pad), width: rect.w + 2 * pad, height: rect.h + 2 * pad, scale: 1 } : undefined;
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png", ...(clip ? { clip } : {}) });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
  shots.push({ name: `${name}.png`, note, cropped: !!clip, errors: cdp.console.splice(0) });
  console.log(`shot ${name}${clip ? "" : " (full page)"}`);
}

const click = (cdp, selector, text) =>
  cdp.eval(`(() => {
    const els = [...document.querySelectorAll(${J(selector)})].filter((e) => e.getClientRects().length);
    const el = ${text === undefined ? "els[0]" : `els.find((e) => (e.getAttribute("aria-label") ?? e.textContent).trim().includes(${J(text ?? "")}))`};
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
  // A demo Anthropic provider with a dummy key: the API source then has a
  // backend that needs the one-time consent (cancelled — never called).
  const demo = await rpc.call("providers.create", { kind: "Anthropic", name: "Anthropic" });
  await rpc.call("providers.setCredential", { providerId: demo.provider.provider.id, apiKey: "sk-ant-demo-not-a-real-key-000000" });
  const target = await waitFor(() => findMainTarget(cdpPort), 90_000, "webview DevTools target");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();

  for (const lang of ["ru", "en"]) {
    const L = JSON.parse(readFileSync(join(repo, "apps", "desktop", "src", "locales", lang, "cursor.json"), "utf8"));
    const A = JSON.parse(readFileSync(join(repo, "apps", "desktop", "src", "locales", lang, "assistant.json"), "utf8"));
    await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, performanceMode: "Balanced", language: lang } });
    // API keys first: the demo key needs the consent card (no session will start)
    const st0 = await rpc.call("computer.cursorStatus", {});
    await rpc.call("computer.cursorSetSettings", { settings: { ...st0.settings, source: "api" } });
    await cdp.send("Page.reload", { ignoreCache: true });
    await waitFor(() => cdp.eval("!!document.querySelector('.app__body, .as-surface')"), 60_000, "app shell");
    await sleep(2500);
    if (await cdp.eval("!!document.querySelector('[data-testid=assistant-surface]')?.getClientRects().length")) {
      await cdp.eval("document.activeElement?.blur?.(), true");
      await cdp.key("k", { ctrl: true });
      await sleep(400);
      await cdp.type(A.commands.toOrchestra);
      await sleep(500);
      await cdp.key("Enter");
      await sleep(2500);
    }

    // Settings → Cursor mode
    await cdp.eval("document.activeElement?.blur?.(), true");
    await cdp.key("k", { ctrl: true });
    await sleep(400);
    await cdp.type(L.commands.settings);
    await sleep(500);
    await cdp.key("Enter");
    const settingsShown = await waitFor(() => cdp.eval("!!document.querySelector('[data-testid=cursor-settings] .cu-backend')"), 15_000, "cursor settings").catch(() => false);
    check(`S-${lang}`, "Settings → Cursor mode shows the source switch and ordered backends", settingsShown);
    if (settingsShown) await shot(cdp, `settings-${lang}`, "[data-testid=cursor-settings]", "source switch, priorities, consent, limits");
    await cdp.key("Escape");
    await sleep(500);

    // Assistant mode + cursor panel
    await cdp.eval("document.activeElement?.blur?.(), true");
    await cdp.key("k", { ctrl: true });
    await sleep(400);
    await cdp.type(L.commands.start);
    await sleep(500);
    await cdp.key("Enter");
    await sleep(800);
    await click(cdp, ".cu-session button", L.control.new);
    const panel = await waitFor(() => cdp.eval("!!document.querySelector('.cu-start textarea')"), 15_000, "cursor panel").catch(() => false);
    check(`P-${lang}`, "Assistant mode shows the cursor start panel", panel);
    await cdp.eval(`(() => { const t = document.querySelector('.cu-start textarea'); t.focus(); return true; })()`);
    await cdp.type(lang === "ru" ? "открой настройки блютуз и включи его" : "open Bluetooth settings and turn it on");
    await sleep(300);
    await shot(cdp, `start-${lang}`, ".cu-panel", "task box with the active model");
    await click(cdp, ".cu-start button[type=submit]");
    const consent = await waitFor(() => cdp.eval("!!document.querySelector('.cu-consent')"), 10_000, "consent").catch(() => false);
    check(`C-${lang}`, "the first real backend asks for the one-time privacy consent", consent);
    if (consent) await shot(cdp, `consent-${lang}`, ".cu-panel", "privacy note + one-time consent");
    await click(cdp, ".cu-consent button", L.consent.cancel);

    // subscriptions (none signed in in the isolated home) → only the scripted mock remains
    const st1 = await rpc.call("computer.cursorStatus", {});
    await rpc.call("computer.cursorSetSettings", { settings: { ...st1.settings, source: "subscriptions" } });
    await sleep(300);
    await click(cdp, ".cu-start button[type=submit]");
    const live = await waitFor(() => cdp.eval("!!document.querySelector('[data-testid=cursor-session] .cu-plan') && !document.querySelector('.cu-confirm')"), 20_000, "live session").catch(() => false);
    check(`L-${lang}`, "a running session shows plan, step log and screenshot", live);
    await sleep(500);
    if (live) await shot(cdp, `running-${lang}`, ".cu-panel", "live plan, current step, screenshot, Pause/Stop");
    const confirm = await waitFor(() => cdp.eval("!!document.querySelector('.cu-confirm .cu-shot__target')"), 20_000, "confirmation").catch(() => false);
    check(`K-${lang}`, "a risky step waits for a confirmation with the target highlighted", confirm);
    if (confirm) await shot(cdp, `confirm-${lang}`, ".cu-panel", "system-settings change needs confirmation");
    await click(cdp, ".cu-confirm button", L.confirm.approve);
    const done = await waitFor(async () => (await rpc.call("computer.cursorStatus", {})).session?.state === "Done", 30_000, "done").catch(() => false);
    check(`D-${lang}`, "the session finishes and hands control back", done);
    await sleep(800);
    if (done) await shot(cdp, `done-${lang}`, ".cu-panel", "finished: summary + step log");
    await click(cdp, ".cu-session button", L.control.new);
  }
  rpc.close();
} catch (e) {
  console.error(e);
  exitCode = 1;
} finally {
  try {
    execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
  } catch {
    /* gone */
  }
  const failed = checks.filter((c) => !c.ok);
  if (failed.length) exitCode = 1;
  writeFileSync(
    join(outDir, "manifest.json"),
    `${J({ date: new Date().toISOString(), result: `${checks.length - failed.length}/${checks.length} checks passed`, safety: "mock computer + cursor backends, scripted model, isolated home; Orkestr UI only", checks, screenshots: shots }, null, 2)}\n`,
  );
  console.log(`\n${checks.length - failed.length}/${checks.length} checks passed`);
  if (exitCode) console.log(log.split("\n").slice(-30).join("\n"));
  await sleep(800);
  try {
    rmSync(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  } catch {
    /* busy */
  }
  process.exit(exitCode);
}
