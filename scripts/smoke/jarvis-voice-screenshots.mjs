#!/usr/bin/env node
// Jarvis voice card screenshots (wave 9, package A). Launches the
// custom-protocol desktop build as a second instance (own ORKESTR_HOME +
// WebView2 profile, no microphone, no personal data), forces each visual
// state through the debug hooks (`__orkestrDebug.jarvis.set`), and captures
// the orb-attached card (dark + light, ru + en), short frame sequences and
// the in-app voice HUD into docs/screenshots/jarvis-voice/. The orb window is
// transparent; a plain desktop-like backdrop is painted behind it for the
// pictures only.
//
// Usage (repo root, Windows):
//   npm run build && cargo build -p orkestr-desktop --features custom-protocol
//   node scripts/smoke/jarvis-voice-screenshots.mjs [--out docs/screenshots/jarvis-voice] [--quick]

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "jarvis-voice"));
const quick = process.argv.includes("--quick");
const targetDir = process.env.CARGO_TARGET_DIR ?? join(repo, "target");
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(targetDir, "debug", "orkestr-desktop.exe");
const cdpPort = Number(process.env.ORKESTR_SMOKE_CDP_PORT ?? 9373);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

const checks = [];
function check(id, label, ok, detail = "") {
  checks.push({ id, label, ok: !!ok, detail: String(detail).slice(0, 300) });
  console.log(`${ok ? "PASS" : "FAIL"} ${id} ${label}${detail ? ` — ${String(detail).slice(0, 200)}` : ""}`);
}

const base = mkdtempSync(join(tmpdir(), "orkestr-jarvis-shots-"));
const home = join(base, "home");
mkdirSync(home, { recursive: true });

const child = spawn(exe, [], {
  env: {
    ...process.env,
    ORKESTR_HOME: home,
    CODEX_HOME: join(base, "codex-home"),
    ORKESTR_SECRET_BACKEND: "vault-file",
    ORKESTR_COMPUTER_BACKEND: "mock",
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
      } else if (f.method === "Runtime.exceptionThrown") {
        const d = f.params.exceptionDetails;
        this.console.push(`exception: ${d.exception?.description ?? d.text}`);
      } else if (f.method === "Runtime.consoleAPICalled" && f.params.type === "error") {
        this.console.push(`error: ${f.params.args.map((a) => a.value ?? a.description ?? "").join(" ")}`);
      }
    };
    await this.send("Runtime.enable");
    await this.send("Page.enable");
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((res, rej) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        rej(new Error(`CDP ${method} timed out: ${String(params.expression ?? "").slice(0, 80)} @ ${this.ws.url.slice(-12)}`));
      }, 20_000);
      this.pending.set(id, {
        resolve: (v) => (clearTimeout(timer), res(v)),
        reject: (e) => (clearTimeout(timer), rej(e)),
      });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  async eval(expr) {
    const r = await this.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  }
}

async function pages() {
  const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
  return list.filter((t) => t.type === "page" && !t.url.startsWith("devtools"));
}

async function windowOf(kind) {
  const t = (await pages()).find((p) => (kind === "orb" ? p.url.endsWith("#/orb") : !p.url.includes("#/")));
  if (!t) return null;
  const cdp = new Cdp(t.webSocketDebuggerUrl);
  try {
    await cdp.open();
  } catch (e) {
    console.log(`attach ${kind}: ${e.message}`);
    cdp.ws.close();
    return null;
  }
  return cdp;
}


const shots = [];
async function shot(cdp, name, note, clip) {
  const params = { format: "png" };
  if (clip) params.clip = { ...clip, scale: 1 };
  const { data } = await cdp.send("Page.captureScreenshot", params);
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
  const errors = cdp.console.splice(0);
  shots.push({ name: `${name}.png`, note, consoleMessages: errors });
  console.log(`shot ${name}${errors.length ? ` — ${errors.length} console message(s): ${errors[0]}` : ""}`);
}

const SAMPLE = {
  ru: {
    partial: "Джарвис, открой YouTube и включи",
    final: "Открой YouTube и включи музыку",
    reply: "Открываю YouTube, сэр. Включаю вашу подборку.",
    error: "Не расслышал команду, сэр. Повторите, пожалуйста.",
    confirm: "Закрыть все окна Chrome, сэр?",
  },
  en: {
    partial: "Jarvis, open YouTube and play",
    final: "Open YouTube and play some music",
    reply: "Opening YouTube, sir. Playing your mix.",
    error: "I didn't catch that, sir. Please say it again.",
    confirm: "Close every Chrome window, sir?",
  },
};
const STATES = (s) => ({
  wake: { state: "wake", level: 0.15, partialText: "", micOpen: true, engine: "Vosk" },
  listening: { state: "listening", level: 0.7, partialText: s.partial, micOpen: true, engine: "Vosk" },
  thinking: { state: "thinking", phase: "thinking", level: 0.2, finalText: s.final, engine: "Vosk" },
  speaking: { state: "speaking", phase: "result", speaking: true, level: 0.65, finalText: s.final, replyText: s.reply, engine: "Silero" },
  done: { state: "done", phase: "result", level: 0, finalText: s.final, replyText: s.reply },
  confirm: { state: "listening", phase: "confirm", level: 0.3, finalText: s.final, confirmText: s.confirm },
  error: { state: "error", phase: "error", level: 0.1, finalText: "", replyText: s.error },
  chat: { state: "speaking", phase: "result", speaking: true, mode: "chat", level: 0.55, finalText: s.final, replyText: s.reply },
  cursor: { state: "listening", mode: "cursor", level: 0.5, partialText: s.partial, micOpen: true },
});

let exitCode = 0;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  const { settings } = await rpc.call("settings.get", {});
  const darkTheme = settings.themeId && settings.themeId !== "solar-light" ? settings.themeId : "nebula";
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, performanceMode: "Balanced", language: "ru" } });

  let main = await waitFor(() => windowOf("main"), 120_000, "main window");
  await main.eval(`localStorage.setItem("orkestr.debugHooks", "1"), true`);
  const orb = await waitFor(() => windowOf("orb"), 60_000, "orb window");
  await orb.send("Page.reload", { ignoreCache: true });
  await waitFor(() => orb.eval("!!document.querySelector('.orb') && !!window.__orkestrDebug?.jarvis"), 30_000, "orb + jarvis hook");
  check("J0", "orb window with the Jarvis debug hook", true);

  const setTheme = async (id) => {
    const { themes } = await rpc.call("ui.themes", {});
    const entry = themes.find((e) => (e.theme ?? e).id === id) ?? themes.find((e) => (e.theme ?? e).mode === (id === "solar-light" ? "Light" : "Dark"));
    const { config } = await rpc.call("ui.getConfig", {});
    await rpc.call("ui.setConfig", { config: { ...config, theme: entry.theme ?? entry } });
  };
  const force = (cdp, p) => cdp.eval(`window.__orkestrDebug.jarvis.set(${JSON.stringify(p)}), true`);
  const backdrop = (cdp, light) =>
    cdp.eval(`(() => { let s = document.getElementById('jv-shot-bg'); if (!s) { s = document.createElement('style'); s.id = 'jv-shot-bg'; document.head.appendChild(s); }
      s.textContent = 'html,body{background:${light ? "linear-gradient(135deg,#e9eef5,#d5dde8)" : "linear-gradient(135deg,#1b2333,#0d121c)"} !important}'; return true; })()`);
  const full = () => orb.eval("({ x: 0, y: 0, width: Math.ceil(window.innerWidth), height: Math.ceil(window.innerHeight) })");

  for (const [theme, themeId] of quick ? [["dark", darkTheme]] : [["dark", darkTheme], ["light", "solar-light"]]) {
    await setTheme(themeId);
    await sleep(1200);
    await backdrop(orb, theme === "light");
    for (const lang of quick || theme === "light" ? ["ru"] : ["ru", "en"]) {
      await rpc.call("settings.update", { patch: { language: lang } });
      await sleep(900);
      for (const [name, p] of Object.entries(STATES(SAMPLE[lang]))) {
        await force(orb, p);
        await waitFor(() => orb.eval("!!document.querySelector('.orb-panel--voice .jv')"), 10_000, "card open");
        await sleep(name === "wake" ? 260 : 1400);
        const st = await orb.eval("document.querySelector('.orb-panel--voice .jv')?.dataset.jarvisState");
        check(`S-${name}-${theme}-${lang}`, `card shows ${name} (${theme}, ${lang})`, st === p.state, st);
        await shot(orb, `orb-${name}-${theme}-${lang}`, `Orb Jarvis card: ${name}, ${theme} theme, ${lang}`, await full());
      }
    }
  }

  // Frame sequences (dark, ru): wake bloom, listening, thinking, speaking, collapse.
  await setTheme(darkTheme);
  await rpc.call("settings.update", { patch: { language: "ru" } });
  await sleep(1000);
  await backdrop(orb, false);
  await force(orb, null);
  await sleep(1200);
  const seq = STATES(SAMPLE.ru);
  await force(orb, seq.wake);
  for (let i = 0; i < 6; i++) {
    await sleep(110);
    await shot(orb, `seq-wake-${String(i).padStart(2, "0")}`, `Wake bloom frame ${i}`, await full());
  }
  for (const name of ["listening", "thinking", "speaking"]) {
    await force(orb, seq[name]);
    for (let i = 0; i < 4; i++) {
      await sleep(name === "speaking" ? 280 : 160);
      await shot(orb, `seq-${name}-${String(i).padStart(2, "0")}`, `${name} frame ${i}`, await full());
    }
  }
  await force(orb, null);
  await sleep(200);
  await shot(orb, "seq-collapse", "Collapsing back into the orb", await full());
  await sleep(1200);
  check("J1", "the card collapses and closes when the command ends", await orb.eval("!document.querySelector('.orb-panel--voice')"));

  // In-app voice HUD (main window), smaller reactor.
  main.send("Page.reload", { ignoreCache: true }).catch(() => undefined);
  await sleep(4000);
  main.ws.close();
  main = await waitFor(() => windowOf("main"), 60_000, "main window (after reload)");
  await waitFor(() => main.eval("!!document.querySelector('.app__body') && !!window.__orkestrDebug?.jarvis"), 60_000, "app shell + hook");
  for (const [theme, themeId] of quick ? [["dark", darkTheme]] : [["dark", darkTheme], ["light", "solar-light"]]) {
    await setTheme(themeId);
    await sleep(1200);
    for (const name of ["listening", "speaking", "error"]) {
      await force(main, STATES(SAMPLE.ru)[name]);
      await waitFor(() => main.eval("!!document.querySelector('.vx-hud .jv')"), 10_000, "hud");
      await sleep(1400);
      const r = await main.eval(
        "(() => { const b = document.querySelector('.vx-hud').getBoundingClientRect(); return { x: Math.max(0, b.x - 24), y: Math.max(0, b.y - 24), width: b.width + 48, height: b.height + 48 }; })()",
      );
      check(`H-${name}-${theme}`, `in-app HUD shows the Jarvis reactor (${name}, ${theme})`, await main.eval("!!document.querySelector('.vx-hud .jv-reactor')"));
      await shot(main, `hud-${name}-${theme}`, `In-app voice HUD: ${name}, ${theme} theme`, r);
    }
    await force(main, null);
    await sleep(600);
  }
  rpc.close();
} catch (e) {
  exitCode = 1;
  console.error(e.stack ?? e);
  console.error(log.slice(-2000));
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
    `${JSON.stringify({ date: new Date().toISOString(), result: `${checks.length - failed.length}/${checks.length} checks passed`, checks, screenshots: shots }, null, 2)}\n`,
  );
  console.log(`\n${checks.length - failed.length}/${checks.length} checks passed`);
  await sleep(1000);
  try {
    rmSync(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  } catch {
    /* ignore */
  }
  process.exit(exitCode);
}
