#!/usr/bin/env node
// Floating orb + Agent Hub screenshots (ADR-0045). Launches the
// custom-protocol desktop build as a second instance (own ORKESTR_HOME +
// WebView2 profile), seeds two projects, then through the WebView2 DevTools
// protocol captures the orb window in every mood (forced through the
// debug-hooks flag), the open Agent Hub and Settings → Orb, in en + ru and
// dark + light themes, into docs/screenshots/orb/ + manifest.json.
//
// Usage (repo root, Windows):
//   npm run build && cargo build -p orkestr-desktop --features custom-protocol
//   node scripts/smoke/orb-screenshots.mjs [--out docs/screenshots/orb]
// ORKESTR_SMOKE_EXE overrides the executable (default: $CARGO_TARGET_DIR or
// ./target, debug), ORKESTR_SMOKE_CDP_PORT the DevTools port.

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "orb"));
const targetDir = process.env.CARGO_TARGET_DIR ?? join(repo, "target");
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(targetDir, "debug", "orkestr-desktop.exe");
const cdpPort = Number(process.env.ORKESTR_SMOKE_CDP_PORT ?? 9371);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

const checks = [];
function check(id, label, ok, detail = "") {
  checks.push({ id, label, ok: !!ok, detail: String(detail).slice(0, 300) });
  console.log(`${ok ? "PASS" : "FAIL"} ${id} ${label}${detail ? ` — ${String(detail).slice(0, 200)}` : ""}`);
}

const base = mkdtempSync(join(tmpdir(), "orkestr-orb-shots-"));
const home = join(base, "home");
mkdirSync(home, { recursive: true });
const projects = ["shop-frontend", "billing-api"].map((name) => {
  const dir = join(base, name);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "README.md"), `# ${name}\n`);
  return { name, dir };
});

const child = spawn(exe, [], {
  env: {
    ...process.env,
    ORKESTR_HOME: home,
    ORKESTR_SECRET_BACKEND: "vault-file",
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
      this.pending.set(id, { resolve: res, reject: rej });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  async eval(expr) {
    const r = await this.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  }
}

const shots = [];
async function capture(cdp, name, note) {
  await sleep(700);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
  const errors = cdp.console.splice(0);
  shots.push({ name: `${name}.png`, note, consoleMessages: errors });
  console.log(`shot ${name}${errors.length ? ` — ${errors.length} console message(s)` : ""}`);
}

async function pages() {
  const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
  return list.filter((t) => t.type === "page" && !t.url.startsWith("devtools"));
}

async function windowOf(kind) {
  for (const t of await pages()) {
    const cdp = new Cdp(t.webSocketDebuggerUrl);
    await cdp.open();
    const w = await cdp.eval("document.documentElement.dataset.okWindow ?? 'main'").catch(() => null);
    if (w === kind) return cdp;
    cdp.ws.close();
  }
  return null;
}

const MOODS = {
  idle: {},
  working: { working: 3 },
  approval: { approvals: 2, working: 1 },
  error: { errors: 1 },
  listening: { voicePhase: "listening" },
  speaking: { speaking: true },
  stopped: { allStopped: true },
};

let exitCode = 0;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  const { settings } = await rpc.call("settings.get", {});
  const darkTheme = settings.themeId ?? "nebula";
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, performanceMode: "Balanced", language: "en" } });
  for (const p of projects) p.id = (await rpc.call("projects.create", { name: p.name, rootPath: p.dir, approvalMode: "SemiAuto" })).project?.id;
  // A few agents for the hub (they may fail without a model provider — that shows the error state).
  for (const [i, tpl] of ["backend", "qa", "frontend"].entries()) {
    try {
      await rpc.call("agents.spawn", { projectId: projects[i % 2].id, templateId: tpl, newTask: { title: `Demo task ${i + 1}` } });
    } catch (e) {
      console.log(`spawn ${tpl}: ${e.message}`);
    }
  }
  const agents = (await rpc.call("agents.list", {})).agents;
  check("O0", "seeded projects and agents", projects.every((p) => p.id), `${agents.length} agents`);

  const main = await waitFor(() => windowOf("main"), 60_000, "main window");
  await main.eval(`localStorage.setItem("orkestr.debugHooks", "1"), true`);
  const orb = await waitFor(() => windowOf("orb"), 60_000, "orb window");
  check("O1", "the orb window opened (single window, label orb)", !!orb);
  await orb.send("Page.reload", { ignoreCache: true });
  await waitFor(() => orb.eval("!!document.querySelector('.orb') && !!window.__orkestrDebug?.orb"), 30_000, "orb rendered");
  await sleep(1500);
  check("O2", "no robot windows remain", !(await pages()).some((t) => t.url.includes("robot.html")));

  const setSignals = (s) => orb.eval(`window.__orkestrDebug.orb.setSignals(${JSON.stringify(s)}), true`);
  const openHub = () =>
    orb.eval(`(() => { const o = document.querySelector('.orb'); o.dispatchEvent(new PointerEvent('pointerdown', {bubbles: true, button: 0})); o.dispatchEvent(new PointerEvent('pointerup', {bubbles: true, button: 0})); return true; })()`);
  const hubOpen = () => orb.eval("!!document.querySelector('.orb-hub')");

  for (const [theme, themeId] of [
    ["dark", darkTheme],
    ["light", "solar-light"],
  ]) {
    await rpc.call("settings.update", { patch: { themeId, language: "en" } });
    await sleep(1500);
    for (const [mood, s] of Object.entries(MOODS)) {
      await setSignals(Object.keys(s).length ? s : null);
      await sleep(400);
      const shown = await orb.eval("document.querySelector('.orb')?.dataset.orbMood");
      check(`M-${mood}-${theme}`, `orb shows ${mood} (${theme})`, shown === mood, shown);
      await capture(orb, `orb-${mood}-${theme}`, `Orb, ${mood} mood, ${theme} theme`);
    }
    for (const lang of ["en", "ru"]) {
      await rpc.call("settings.update", { patch: { language: lang } });
      await setSignals({ working: 2, approvals: 1 });
      await sleep(1200);
      if (!(await hubOpen())) await openHub();
      await waitFor(hubOpen, 10_000, "hub open");
      await sleep(1200);
      const rows = await orb.eval("document.querySelectorAll('.orb-row').length");
      check(`H-${lang}-${theme}`, `Agent Hub opens next to the orb (${lang}, ${theme})`, true, `${rows} rows`);
      await capture(orb, `hub-${lang}-${theme}`, `Agent Hub, ${lang}, ${theme} theme`);
      await orb.eval(`document.querySelector('.orb-hub__head button:last-child')?.click(), true`);
      await orb.eval(`document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape'})), window.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape'})), true`);
      await sleep(800);
    }
  }
  await setSignals(null);

  // Settings → Orb (opened from the hub's settings button), en + ru.
  for (const lang of ["en", "ru"]) {
    await rpc.call("settings.update", { patch: { language: lang, themeId: darkTheme } });
    await sleep(800);
    if (!(await hubOpen())) await openHub();
    await waitFor(hubOpen, 10_000, "hub open");
    await orb.eval(`document.querySelectorAll('.orb-hub__head button')[1].click(), true`);
    await sleep(2000);
    const ok = await main.eval("!!document.querySelector('.orb-settings')");
    check(`S-${lang}`, `Settings → Orb opens from the hub (${lang})`, ok);
    await capture(main, `settings-orb-${lang}`, `Settings → Orb, ${lang}`);
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
