#!/usr/bin/env node
// Assistant mode screenshots (ADR-0035), en + ru: launches the
// custom-protocol desktop build with an isolated ORKESTR_HOME, the recording
// computer backend (ORKESTR_COMPUTER_BACKEND=mock: nothing on this desktop is
// touched) and the offline Mock provider as the planner, then drives the real
// UI through the WebView2 DevTools protocol:
//
//   01 Assistant mode after a command ("open notepad" → done), recent list
//   02 an unknown command → AI plan card (correct / run / remember)
//   03 a risky command → confirmation card
//   04 Settings → Assistant: command sets + catalogue browser (Orchestra mode)
//
// Usage (repo root, Windows):
//   npm run build:packages && npm run build -w @orkestr/desktop
//   cargo build -p orkestr-desktop --features custom-protocol
//   node scripts/smoke/assistant-screenshots.mjs [--out docs/screenshots/assistant]
// ORKESTR_SMOKE_EXE overrides the executable; ORKESTR_SMOKE_CDP_PORT the port.

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "assistant"));
const targetDir = process.env.CARGO_TARGET_DIR ?? join(repo, "target");
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(targetDir, "debug", "orkestr-desktop.exe");
const cdpPort = Number(process.env.ORKESTR_SMOKE_CDP_PORT ?? 9371);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const J = JSON.stringify;
mkdirSync(outDir, { recursive: true });
const L = {
  en: JSON.parse(readFileSync(join(repo, "apps/desktop/src/locales/en/assistant.json"), "utf8")),
  ru: JSON.parse(readFileSync(join(repo, "apps/desktop/src/locales/ru/assistant.json"), "utf8")),
};

const S = {
  en: JSON.parse(readFileSync(join(repo, "apps/desktop/src/locales/en/shell.json"), "utf8")),
  ru: JSON.parse(readFileSync(join(repo, "apps/desktop/src/locales/ru/shell.json"), "utf8")),
};

const checks = [];
const check = (id, label, ok, detail = "") => {
  checks.push({ id, label, ok: !!ok, detail: String(detail).slice(0, 300) });
  console.log(`${ok ? "PASS" : "FAIL"}  [${id}] ${label}${detail ? ` — ${String(detail).slice(0, 160)}` : ""}`);
};

const base = mkdtempSync(join(tmpdir(), "orkestr-asst-shots-"));
const home = join(base, "home");
mkdirSync(home, { recursive: true });
const child = spawn(exe, [], {
  env: {
    ...process.env,
    ORKESTR_HOME: home,
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
      }
    };
    await this.send("Runtime.enable");
    await this.send("Page.enable");
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((res, rej) => {
      this.pending.set(id, { resolve: res, reject: rej });
      this.ws.send(J({ id, method, params }));
    });
  }
  async eval(expr) {
    const r = await this.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  }
}

async function capture(cdp, name) {
  await sleep(900);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
  console.log(`shot ${name}`);
}

/** Types into the assistant input (React-controlled) and submits. */
const runText = (cdp, text) =>
  cdp.eval(`(async () => {
    const input = document.querySelector(".as-input input");
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set;
    set.call(input, ${J(text)});
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await new Promise((r) => setTimeout(r, 400));
    input.closest("form").requestSubmit();
    return true;
  })()`);
const surfaceText = (cdp) => cdp.eval(`document.querySelector("[data-testid=assistant-surface]")?.innerText ?? ""`);

let exitCode = 0;
let cdpRef = null;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json");
  const rpc = rpcClient(info.url, readFileSync(join(home, "core.token"), "utf8").trim());
  await rpc.open();
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, performanceMode: "Balanced", language: "en" } });
  const mock = (await rpc.call("providers.list")).providers.map((p) => p.provider ?? p).find((p) => p.kind === "Mock");
  const smart = { providerId: mock.id, model: "mock-smart" };
  await rpc.call("providers.setManager", { config: { model: smart, maxParallelAgents: 1, approvalPolicy: "SemiAuto" } }).catch(() => null);
  await rpc.call("providers.setRouting", { config: { mode: "Manual", rules: [], defaultFallback: [smart], preferLocal: true, maxRateLimitWaitSecs: 0 } });
  // a user set so Settings shows one next to the catalogue
  await rpc.call("computer.importSet", {
    json: J({ format: "orkestr-command-set", version: 1, set: { id: "morning", name: "Morning routine", description: "Mail, calendar and news", commands: [
      { id: "start-day", category: "custom", title: { en: "Start my day", ru: "Начать день" }, phrases: ["начни день", "start my day"], safety: "Safe", steps: [
        { type: "OpenUrl", url: "mail.google.com" }, { type: "Wait", ms: 800 }, { type: "OpenUrl", url: "calendar.google.com" }, { type: "Speak", text: "Good morning" } ] } ] } }),
    replace: true,
  });

  const target = await waitFor(async () => {
    const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
    return list.find((t) => t.type === "page" && !t.url.startsWith("devtools") && !t.url.includes("#/"));
  }, 60_000, "DevTools target");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  cdpRef = cdp;
  await cdp.send("Page.reload", { ignoreCache: true });
  await waitFor(() => cdp.eval("!!document.querySelector('.app__body')"), 60_000, "app shell");
  await sleep(2500);

  for (const [lang, n] of [["en", 0], ["ru", 10]]) {
    const t = L[lang];
    await rpc.call("settings.update", { patch: { language: lang } });
    await sleep(1500);
    // Assistant mode via the title-bar switch
    await cdp.eval(`document.querySelectorAll("[data-testid=mode-toggle] button")[1].click(), true`);
    await waitFor(() => cdp.eval("!!document.querySelector('[data-testid=assistant-surface]')"), 10_000, "assistant surface");
    check(`${lang}-mode`, "title-bar switch → Assistant mode (workspace hidden)", await cdp.eval(`getComputedStyle(document.querySelector(".app__body")).display === "none"`));
    await sleep(1200);
    await runText(cdp, lang === "ru" ? "открой блокнот" : "open notepad");
    await waitFor(async () => (await surfaceText(cdp)).includes(t.reply.done.split(":")[0]), 15_000, "done reply");
    await runText(cdp, lang === "ru" ? "громкость 30 процентов" : "volume 30 percent");
    await waitFor(async () => (await surfaceText(cdp)).toLowerCase().includes(t.surface.recent.toLowerCase()), 10_000, "recent list");
    await sleep(800);
    check(`${lang}-run`, "typed command ran and the reply shows", (await surfaceText(cdp)).includes(t.reply.done.split(":")[0]));
    await capture(cdp, `${String(n + 1).padStart(2, "0")}-assistant-${lang}`);

    await runText(cdp, lang === "ru" ? "запиши в блокнот список покупок молоко хлеб" : "prepare my shopping list: milk, bread");
    await waitFor(() => cdp.eval("!!document.querySelector('.as-plan')"), 30_000, "plan card");
    check(`${lang}-plan`, "unknown command → AI plan card", (await surfaceText(cdp)).includes(t.plan.title));
    await capture(cdp, `${String(n + 2).padStart(2, "0")}-assistant-plan-${lang}`);
    await cdp.eval(`[...document.querySelectorAll(".as-plan button")].find((b) => b.textContent.includes(${J(t.plan.dismiss)}))?.click(), true`);

    await runText(cdp, lang === "ru" ? "закрой блокнот без сохранения" : "close notepad without saving");
    await waitFor(() => cdp.eval("!!document.querySelector('.as-confirm')"), 15_000, "confirmation card");
    check(`${lang}-confirm`, "risky command → confirmation card", (await surfaceText(cdp)).includes(t.confirm.title));
    await capture(cdp, `${String(n + 3).padStart(2, "0")}-assistant-confirm-${lang}`);
    await cdp.eval(`[...document.querySelectorAll(".as-confirm button")].find((b) => b.textContent.includes(${J(t.confirm.reject)}))?.click(), true`);
    await sleep(800);

    // back to Orchestra mode, Settings → Assistant
    await cdp.eval(`[...document.querySelectorAll(".as-foot button")].find((b) => b.textContent.includes(${J(t.mode.toOrchestra)}))?.click(), true`);
    await waitFor(() => cdp.eval(`getComputedStyle(document.querySelector(".app__body")).display !== "none"`), 10_000, "orchestra");
    check(`${lang}-back`, "footer button → Orchestra mode restored", true);
    await sleep(1500);
    await cdp.eval(`document.querySelector(${J(`.titlebar [aria-label="${S[lang].titleBar.settings}"]`)})?.click(), true`);
    await sleep(800);
    await cdp.eval(`[...document.querySelectorAll("[role=tab]")].find((x) => x.textContent.trim() === ${J(t.settings.title)})?.click(), true`);
    await waitFor(() => cdp.eval("!!document.querySelector('.as-sets .as-set')"), 15_000, "command sets");
    await sleep(1200);
    check(`${lang}-sets`, "Settings → Assistant lists the sets", (await cdp.eval(`document.querySelectorAll(".as-set").length`)) >= 15);
    await capture(cdp, `${String(n + 4).padStart(2, "0")}-assistant-settings-${lang}`);
    await cdp.eval(`document.querySelector(".settings-overlay")?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })), true`);
    await waitFor(() => cdp.eval("!document.querySelector('.settings-overlay')"), 5000, "settings closed");
    await sleep(800);
  }
  await rpc.call("settings.update", { patch: { language: "en" } });
  rpc.close();
} catch (e) {
  exitCode = 1;
  console.error(e.stack ?? e);
  if (cdpRef) {
    try {
      console.error("surface:", await surfaceText(cdpRef));
      const { data } = await cdpRef.send("Page.captureScreenshot", { format: "png" });
      writeFileSync(join(tmpdir(), "orkestr-assistant-shots-error.png"), Buffer.from(data, "base64"));
    } catch {
      /* ignore */
    }
  }
  console.error(log.slice(-800));
} finally {
  try {
    execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
  } catch {
    /* gone */
  }
  const failed = checks.filter((c) => !c.ok);
  if (failed.length) exitCode = 1;
  writeFileSync(join(outDir, "manifest.json"), `${J({ date: new Date().toISOString(), result: `${checks.length - failed.length}/${checks.length} checks passed`, backend: "mock (no desktop input)", checks }, null, 2)}\n`);
  console.log(`\n${checks.length - failed.length}/${checks.length} checks passed`);
  await sleep(1000);
  try {
    rmSync(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  } catch {
    /* ignore */
  }
  process.exit(exitCode);
}
