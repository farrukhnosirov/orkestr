#!/usr/bin/env node
// Claude Code panel, end to end in the real desktop app (bundled frontend:
// `npm run build` + `cargo build -p orkestr-desktop --features custom-protocol`).
//
// Phase "login" (isolated, never touches the user's Claude login):
//   - a hard link of the official binary is placed in the isolated
//     ORKESTR_HOME's VS Code extensions folder
//     (`vscode-server/extensions/anthropic.claude-code-<v>-<target>/resources/native-binary/`)
//     → `claudeCode.status` must discover it (source `vscodeExtension`);
//   - CLAUDE_CONFIG_DIR points to an empty folder → the CLI reports "not
//     signed in" → the panel shows the login screen; «Sign in to Claude»
//     opens a terminal running `claude auth login`.
// Phase "chat" (`--real-chat`, uses the user's own signed-in CLI and a little
//   of its usage, model haiku): a real conversation with a Write permission
//   card answered «Allow once», the file written, the result footer.
//
// Usage (repo root, Windows):
//   node scripts/smoke/claude-code-e2e.mjs --claude <path to claude.exe> [--real-chat] [--out docs/screenshots/claude-code]

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
const outDir = resolve(repo, arg("--out", join("docs", "screenshots", "claude-code")));
const claude = arg("--claude", null);
const realChat = process.argv.includes("--real-chat");
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
}

async function shot(cdp, name) {
  await sleep(800);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
  console.log(`  screenshot ${name}.png`);
}
const clickText = (cdp, selector, text) =>
  cdp.eval(`(() => {
    const els = [...document.querySelectorAll(${JSON.stringify(selector)})].filter((e) => e.getClientRects().length);
    const el = els.find((e) => e.textContent.trim().includes(${JSON.stringify(text)}) || e.getAttribute('aria-label') === ${JSON.stringify(text)});
    if (!el) return false;
    el.click();
    return true;
  })()`);
const skipWizard = (cdp) => clickText(cdp, "button", "Skip setup");

/** Launches an isolated app; returns { child, rpc, cdp, home, proj, log() }. */
async function launch(tag, { emptyClaudeConfig, cdpPort }) {
  const base = mkdtempSync(join(tmpdir(), `orkestr-cc-${tag}-`));
  const home = join(base, "home");
  const proj = join(base, "demo");
  mkdirSync(home, { recursive: true });
  mkdirSync(proj, { recursive: true });
  writeFileSync(join(proj, "README.md"), "# demo\n");
  execFileSync("git", ["init", "-q"], { cwd: proj });
  // The binary "installed" with the VS Code extension (hard link: no copy).
  const extDir = join(home, "vscode-server", "extensions", "anthropic.claude-code-2.1.292-win32-x64", "resources", "native-binary");
  mkdirSync(extDir, { recursive: true });
  linkSync(claude, join(extDir, basename(claude)));
  const env = {
    ...process.env,
    ORKESTR_HOME: home,
    ORKESTR_SECRET_BACKEND: "vault-file",
    ORKESTR_ALLOW_SECOND_INSTANCE: "1",
    WEBVIEW2_USER_DATA_FOLDER: join(base, "webview2"),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${cdpPort}`,
  };
  if (emptyClaudeConfig) {
    const cfg = join(base, "empty-claude-config");
    mkdirSync(cfg, { recursive: true });
    env.CLAUDE_CONFIG_DIR = cfg;
  }
  const child = spawn(exe, [], { env, stdio: ["ignore", "pipe", "pipe"] });
  let log = "";
  child.stdout.on("data", (d) => (log += d));
  child.stderr.on("data", (d) => (log += d));
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 120_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.call("settings.update", { patch: { onboardingCompleted: true, language: "en" } });
  const { project } = await rpc.call("projects.create", { name: "demo", rootPath: proj });
  await rpc.call("projects.open", { projectId: project.id });
  const target = await waitFor(async () => {
    const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
    return list.find((t) => t.type === "page" && !t.url.includes("robot.html") && !t.url.includes("#/") && !/hud/i.test(t.url));
  }, 60_000, "webview DevTools target");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: 1400, height: 860, deviceScaleFactor: 1, mobile: false });
  await sleep(4000);
  await skipWizard(cdp);
  return { child, rpc, cdp, home, proj, project, log: () => log };
}

function stop(child) {
  child.kill();
  try {
    execFileSync("taskkill", ["/F", "/T", "/PID", String(child.pid)], { stdio: "ignore" });
  } catch {
    /* already gone */
  }
}

async function openPanel(cdp) {
  await cdp.eval("document.activeElement?.blur?.(), true");
  await cdp.key("c", { ctrl: true, alt: true }); // Claude Code: Open chat
  await sleep(1500);
  return cdp.eval("!!document.querySelector('.cc-center, .cc-panel')");
}

let exitCode = 0;
const apps = [];
try {
  // ── Phase 1: discovery + not signed in + login terminal ────────────────
  const a = await launch("login", { emptyClaudeConfig: true, cdpPort: 9341 });
  apps.push(a);
  const st = await a.rpc.call("claudeCode.status", { refresh: true });
  check("binary discovered in the VS Code extensions folder", st.binary?.source === "vscodeExtension", `${st.binary?.path} v${st.binary?.version}`);
  check("binary version", st.binary?.version === "2.1.292", st.binary?.version ?? "");
  check("not signed in (empty CLAUDE_CONFIG_DIR)", st.auth && st.auth.loggedIn === false, JSON.stringify(st.auth));
  check("panel opens with Ctrl+Alt+C", await openPanel(a.cdp));
  const loginText = await waitFor(() => a.cdp.eval("document.querySelector('.cc-center')?.textContent ?? ''").then((t) => (t.includes("Sign in") ? t : "")), 20_000, "login screen");
  check("login screen shown", loginText.includes("Sign in to Claude"), loginText.slice(0, 120));
  await shot(a.cdp, "01-not-signed-in");
  await clickText(a.cdp, "button", "Sign in to Claude");
  const term = await waitFor(async () => {
    const { terminals } = await a.rpc.call("terminal.list", {});
    return terminals.find((t) => t.title.includes("login"));
  }, 20_000, "login terminal");
  check("«Sign in» opens a terminal running `claude auth login`", Boolean(term), `${term?.title} pid ${term?.pid}`);
  await sleep(4000);
  await shot(a.cdp, "02-login-terminal");
  const errors = a.cdp.console.filter((c) => !c.includes("ResizeObserver"));
  check("no console errors (login phase)", errors.length === 0, errors.slice(0, 3).join(" | "));
  stop(a.child);

  // ── Phase 2: a real conversation (opt-in) ─────────────────────────────────
  if (realChat) {
    const b = await launch("chat", { emptyClaudeConfig: false, cdpPort: 9342 });
    apps.push(b);
    const st2 = await b.rpc.call("claudeCode.status", { refresh: true });
    check("signed in with the user's own CLI login", st2.auth?.loggedIn === true, st2.auth?.subscriptionType ?? "");
    await openPanel(b.cdp);
    // Haiku keeps the check cheap.
    await b.cdp.eval("localStorage.setItem('orkestr.claudeCode.prefs', JSON.stringify({ model: 'haiku', mode: 'default' })), true");
    await b.cdp.eval("location.reload(), true");
    await sleep(5000);
    await skipWizard(b.cdp);
    await openPanel(b.cdp);
    await waitFor(() => clickText(b.cdp, ".cc-welcome button", "New chat"), 20_000, "new chat button");
    await waitFor(() => b.cdp.eval("!!document.querySelector('.ccx-composer__input')"), 20_000, "composer");
    await b.cdp.eval("document.querySelector('.ccx-composer__input').focus(), true");
    await b.cdp.type("Create a file named notes.md containing the line 'hello from Orkestr' using the Write tool, then summarize in one sentence.");
    await sleep(300);
    await b.cdp.key("Enter");
    await waitFor(() => b.cdp.eval("[...document.querySelectorAll('button')].some((x) => x.textContent.trim() === 'Allow once')"), 120_000, "permission card");
    check("permission card for Write", true);
    await shot(b.cdp, "03-permission-card");
    await clickText(b.cdp, "button", "Allow once");
    await waitFor(() => b.cdp.eval("!!document.querySelector('.cc-result')"), 180_000, "turn result");
    const written = existsSync(join(b.proj, "notes.md")) ? readFileSync(join(b.proj, "notes.md"), "utf8") : "";
    check("file written after «Allow once»", written.includes("hello from Orkestr"), written.trim());
    const footer = await b.cdp.eval("document.querySelector('.cc-result')?.textContent ?? ''");
    check("result footer (tokens, changed files)", footer.includes("tokens") && footer.includes("notes.md"), footer.slice(0, 160));
    await shot(b.cdp, "04-real-chat-result");
    const hist = await b.rpc.call("claudeCode.sessions", { projectId: b.project.id });
    check("conversation listed in history", hist.history.length > 0, hist.history[0]?.firstPrompt ?? "");
    const errors2 = b.cdp.console.filter((c) => !c.includes("ResizeObserver"));
    check("no console errors (chat phase)", errors2.length === 0, errors2.slice(0, 3).join(" | "));
    stop(b.child);
  }
} catch (e) {
  console.error("smoke failed:", e);
  exitCode = 1;
} finally {
  writeFileSync(join(outDir, "report.json"), JSON.stringify({ results }, null, 2));
  for (const app of apps) stop(app.child);
  if (results.some((r) => !r.ok)) exitCode = 1;
  if (exitCode) for (const app of apps) console.log(app.log().slice(-3000));
  process.exit(exitCode);
}
