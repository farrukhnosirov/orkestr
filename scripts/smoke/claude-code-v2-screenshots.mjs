#!/usr/bin/env node
// Claude Code panel v2 in the real desktop app (bundled frontend:
// `npm run build -w @orkestr/desktop` + `cargo build -p orkestr-desktop
// --features custom-protocol`), with the user's own signed-in CLI (model
// haiku, one tiny prompt):
//   - plan usage over RPC (`claudeCode.usage`: probe process, then a live
//     session), context usage, MCP status, chat labels;
//   - the panel maximized at 1920×1080 and 960×600 (English + Russian):
//     welcome, a real chat, «Возможности», the context and usage popovers,
//     the commands drawer.
// Screenshots: app window only (CDP `Page.captureScreenshot`).
//
// Usage (repo root, Windows):
//   node scripts/smoke/claude-code-v2-screenshots.mjs --claude <path to claude.exe> [--out docs/screenshots/claude-code-v2]

import { execFileSync, spawn } from "node:child_process";
import { existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const arg = (name, dflt) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : dflt;
};
const outDir = resolve(repo, arg("--out", join("docs", "screenshots", "claude-code-v2")));
const claude = arg("--claude", null);
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(process.env.CARGO_TARGET_DIR ?? join(repo, "target"), "debug", "orkestr-desktop.exe");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
if (!claude || !existsSync(claude)) {
  console.error("--claude <path to the official claude executable> is required");
  process.exit(2);
}
mkdirSync(outDir, { recursive: true });

const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: Boolean(ok), detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
};

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
    await sleep(300);
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
  async key(key, { ctrl = false, shift = false, alt = false } = {}) {
    const modifiers = (alt ? 1 : 0) | (ctrl ? 2 : 0) | (shift ? 8 : 0);
    const code = key.length === 1 ? `Key${key.toUpperCase()}` : key;
    const vk = key.length === 1 ? key.toUpperCase().charCodeAt(0) : { Enter: 13, Escape: 27 }[key];
    for (const type of ["rawKeyDown", "keyUp"]) {
      await this.send("Input.dispatchKeyEvent", { type, modifiers, key, code, windowsVirtualKeyCode: vk });
    }
  }
  async type(text) {
    await this.send("Input.insertText", { text });
  }
  async size(width, height) {
    await this.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
    await sleep(700);
  }
}

async function shot(cdp, name) {
  // The "events since the beginning" toast would cover the panel.
  await cdp.eval(`(() => {
    const b = [...document.querySelectorAll('button')].find((x) => /Open logs|Открыть журнал/.test(x.textContent));
    let el = b;
    while (el && !el.querySelector('button[aria-label]')) el = el.parentElement;
    el?.querySelector('button[aria-label]')?.click();
    return true;
  })()`);
  await sleep(700);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
  console.log(`  screenshot ${name}.png`);
}
const click = (cdp, selector, text) =>
  cdp.eval(`(() => {
    const els = [...document.querySelectorAll(${JSON.stringify(selector)})].filter((e) => e.getClientRects().length);
    const text = ${JSON.stringify(text ?? null)};
    const el = text === null ? els[0] : els.find((e) => e.textContent.trim().includes(text) || e.getAttribute('aria-label') === text);
    if (!el) return false;
    el.click();
    return true;
  })()`);
const closePopovers = (cdp) => cdp.eval("document.body.dispatchEvent(new MouseEvent('mousedown', { bubbles: true })), true");

function stop(child) {
  child.kill();
  try {
    execFileSync("taskkill", ["/F", "/T", "/PID", String(child.pid)], { stdio: "ignore" });
  } catch {
    /* already gone */
  }
}

let exitCode = 0;
let child = null;
let log = "";
try {
  const base = mkdtempSync(join(tmpdir(), "orkestr-cc2-"));
  const home = join(base, "home");
  const proj = join(base, "shop-api");
  mkdirSync(home, { recursive: true });
  mkdirSync(proj, { recursive: true });
  writeFileSync(join(proj, "README.md"), "# shop-api\n\nA tiny demo service.\n");
  execFileSync("git", ["init", "-q"], { cwd: proj });
  const extDir = join(home, "vscode-server", "extensions", "anthropic.claude-code-2.1.292-win32-x64", "resources", "native-binary");
  mkdirSync(extDir, { recursive: true });
  linkSync(claude, join(extDir, basename(claude)));
  const cdpPort = 9361;
  const env = {
    ...process.env,
    ORKESTR_HOME: home,
    ORKESTR_SECRET_BACKEND: "vault-file",
    ORKESTR_ALLOW_SECOND_INSTANCE: "1",
    WEBVIEW2_USER_DATA_FOLDER: join(base, "webview2"),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${cdpPort}`,
  };
  child = spawn(exe, [], { env, stdio: ["ignore", "pipe", "pipe"] });
  child.stdout.on("data", (d) => (log += d));
  child.stderr.on("data", (d) => (log += d));
  const coreInfo = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 120_000, "core.json");
  const rpc = rpcClient(coreInfo.url, readFileSync(join(home, "core.token"), "utf8").trim());
  await rpc.open();
  await rpc.call("settings.update", { patch: { onboardingCompleted: true, language: "en" } });
  const { project } = await rpc.call("projects.create", { name: "shop-api", rootPath: proj });
  await rpc.call("projects.open", { projectId: project.id });

  // ── RPC: plan usage without a session (probe process) ─────────────────
  const st = await rpc.call("claudeCode.status", { refresh: true });
  check("signed in with the user's own CLI", st.auth?.loggedIn === true, st.auth?.subscriptionType ?? "");
  const t0 = Date.now();
  const u0 = await rpc.call("claudeCode.usage", { refresh: true });
  check(
    "claudeCode.usage (probe, no model tokens): session + weekly",
    u0.available && u0.session && u0.weekly,
    `session ${u0.session?.percent}% resets ${u0.session?.resetsAt} · weekly ${u0.weekly?.percent}% resets ${u0.weekly?.resetsAt} · ${Date.now() - t0} ms`,
  );

  const target = await waitFor(async () => {
    const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
    return list.find((t) => t.type === "page" && !t.url.includes("robot.html") && !t.url.includes("#/") && !/hud/i.test(t.url));
  }, 60_000, "webview DevTools target");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.size(1920, 1080);
  await cdp.eval("localStorage.setItem('orkestr.claudeCode.prefs', JSON.stringify({ model: 'haiku', mode: 'default', fallbackModel: '', sidebar: true })), true");
  await cdp.eval("location.reload(), true");
  await sleep(6000);
  await click(cdp, "button", "Skip setup");
  await cdp.eval("document.activeElement?.blur?.(), true");
  await cdp.key("c", { ctrl: true, alt: true });
  await waitFor(() => cdp.eval("!!document.querySelector('.cc2')"), 20_000, "panel v2");
  // Maximize the Claude Code panel (dock maximize button).
  await cdp.eval(`(() => {
    const panel = document.querySelector('.cc2');
    let el = panel;
    while (el && !el.querySelector?.('[aria-label="Maximize panel"]')) el = el.parentElement;
    const btn = el?.querySelector('[aria-label="Maximize panel"]');
    btn?.click();
    return !!btn;
  })()`);
  await sleep(1500);
  // Dismiss the "events since the beginning" toast so it does not cover the panel.
  await cdp.eval(`(() => {
    const b = [...document.querySelectorAll('button')].find((x) => /Open logs|Открыть журнал/.test(x.textContent));
    let el = b;
    while (el && !el.querySelector('button[aria-label]')) el = el.parentElement;
    el?.querySelector('button[aria-label]')?.click();
    return true;
  })()`);
  await waitFor(() => cdp.eval("document.querySelector('.cc2-meter')?.textContent.includes('%')"), 30_000, "usage meter");
  check("usage meter shows SESSION | WEEKLY", true, await cdp.eval("document.querySelector('.cc2-meter').textContent"));
  await shot(cdp, "01-welcome-1920");

  // ── a real chat (haiku, tiny prompt) ──────────────────────────────────
  await click(cdp, ".cc2-new");
  await waitFor(() => cdp.eval("!!document.querySelector('.ccx-composer__input')"), 30_000, "composer");
  await waitFor(() => cdp.eval("document.querySelectorAll('.cc2-feat').length > 5"), 30_000, "features bar");
  await cdp.eval("document.querySelector('.ccx-composer__input').focus(), true");
  await cdp.type("In one short sentence: what does README.md in this folder say?");
  await sleep(300);
  await cdp.key("Enter");
  await waitFor(() => cdp.eval("!!document.querySelector('.cc-result')"), 180_000, "turn result");
  await sleep(2500);
  const sessions = await rpc.call("claudeCode.status", {});
  const live = sessions.sessions.find((s) => s.state !== "exited");
  check("live session after the turn", Boolean(live), live?.sessionId ?? "");
  if (live) {
    const u1 = await rpc.call("claudeCode.usage", { sessionId: live.sessionId, refresh: true });
    check("claudeCode.usage via the live session", u1.available, `session ${u1.session?.percent}% · weekly ${u1.weekly?.percent}%`);
    const ctx = await rpc.call("claudeCode.contextUsage", { sessionId: live.sessionId });
    check("claudeCode.contextUsage", ctx.maxTokens > 0 && ctx.totalTokens > 0, `${ctx.totalTokens}/${ctx.maxTokens} (${ctx.percentage}%)`);
    const mcp = await rpc.call("claudeCode.mcpStatus", { sessionId: live.sessionId });
    check("claudeCode.mcpStatus", Array.isArray(mcp.servers), `${mcp.servers.length} servers`);
    const th = await rpc.call("claudeCode.setThinking", { sessionId: live.sessionId, enabled: false });
    check("claudeCode.setThinking", th.sessionId === live.sessionId);
  }
  await waitFor(() => cdp.eval("document.querySelectorAll('.cc2-chat').length > 0"), 20_000, "chat in the sidebar");
  await shot(cdp, "02-chat-1920");

  await click(cdp, ".cc2-feat", "Context");
  await waitFor(() => cdp.eval("!!document.querySelector('.cc2-ctx__list')"), 20_000, "context popover");
  await shot(cdp, "03-context-1920");
  await closePopovers(cdp);
  await click(cdp, ".cc2-meter");
  await sleep(500);
  await shot(cdp, "04-usage-1920");
  await closePopovers(cdp);

  // chat labels
  const hist = await rpc.call("claudeCode.sessions", { projectId: project.id });
  const cid = hist.history[0]?.claudeSessionId;
  if (cid) {
    const m = await rpc.call("claudeCode.setChatMeta", { claudeSessionId: cid, title: "README summary", pinned: true });
    check("claudeCode.setChatMeta (rename + pin, stored in Orkestr)", m.title === "README summary" && m.pinned);
    await cdp.eval("location.reload(), true");
    await sleep(6000);
    await waitFor(() => cdp.eval("!!document.querySelector('.cc2')"), 20_000, "panel after reload");
    await waitFor(() => cdp.eval("document.querySelector('.cc2-side')?.textContent.includes('README summary')"), 20_000, "renamed chat");
    check("renamed + pinned chat in the sidebar", true);
    await click(cdp, ".cc2-chat__main", "README summary");
    await sleep(2500);
  }

  await click(cdp, ".cc2-top button", "Commands");
  await waitFor(() => cdp.eval("document.querySelectorAll('.cc2-cmd').length > 5"), 20_000, "commands drawer");
  const ncmd = await cdp.eval("document.querySelectorAll('.cc2-cmd').length");
  check("commands drawer lists the CLI's commands as buttons", ncmd > 10, `${ncmd} cards`);
  await shot(cdp, "05-commands-1920");
  await cdp.key("Escape");

  // ── 960×600 ──────────────────────────────────────────────────────────
  await cdp.size(960, 600);
  await shot(cdp, "06-chat-960");
  await click(cdp, ".cc2-chats-btn");
  await sleep(600);
  await shot(cdp, "07-chats-overlay-960");
  await click(cdp, ".cc2-scrim");
  await click(cdp, ".cc2-top button", "Commands");
  await sleep(800);
  await shot(cdp, "08-commands-960");
  await cdp.key("Escape");

  // ── Russian ──────────────────────────────────────────────────────────
  await rpc.call("settings.update", { patch: { language: "ru" } });
  await sleep(2000);
  await cdp.size(1920, 1080);
  await shot(cdp, "09-chat-ru-1920");
  await click(cdp, ".cc2-top button", "Команды");
  await sleep(800);
  await shot(cdp, "10-commands-ru-1920");
  await cdp.key("Escape");
  await cdp.size(960, 600);
  await shot(cdp, "11-chat-ru-960");

  const errors = cdp.console.filter((c) => !c.includes("ResizeObserver"));
  check("no console errors", errors.length === 0, errors.slice(0, 3).join(" | "));
} catch (e) {
  console.error("smoke failed:", e);
  exitCode = 1;
} finally {
  writeFileSync(join(outDir, "report.json"), JSON.stringify({ results }, null, 2));
  if (child) stop(child);
  if (results.some((r) => !r.ok)) exitCode = 1;
  if (exitCode) console.log(log.slice(-3000));
  process.exit(exitCode);
}
