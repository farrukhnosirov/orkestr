#!/usr/bin/env node
// Wave 8 — Orkestr's own context menu (no WebView2/Edge menu).
// Launches the custom-protocol desktop build as an isolated second instance
// (own ORKESTR_HOME + WebView2 profile), then right-clicks with real CDP mouse
// events in a text field, the Monaco editor, the terminal, the file tree, the
// task list and on selected text; checks that the Orkestr menu opens and that the browser menu was
// cancelled; screenshots (en + ru) → docs/screenshots/context-menu/.
//
// Usage (repo root, Windows):
//   npm run build -w @orkestr/desktop
//   cargo build -p orkestr-desktop --features custom-protocol
//   node scripts/smoke/wave8-context-menu-screenshots.mjs
// ORKESTR_SMOKE_EXE overrides the executable, ORKESTR_SMOKE_CDP_PORT the port.
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outDir = join(repo, "docs", "screenshots", "context-menu");
const targetDir = process.env.CARGO_TARGET_DIR ?? join(repo, "target");
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(targetDir, "debug", "orkestr-desktop.exe");
const cdpPort = Number(process.env.ORKESTR_SMOKE_CDP_PORT ?? 9383);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const J = JSON.stringify;
mkdirSync(outDir, { recursive: true });
const loc = (lang, ns) => JSON.parse(readFileSync(join(repo, "apps", "desktop", "src", "locales", lang, `${ns}.json`), "utf8"));
const L = Object.fromEntries(["en", "ru"].map((l) => [l, { shell: loc(l, "shell"), cm: loc(l, "contextMenu"), term: loc(l, "terminal") }]));

const checks = [];
const check = (id, label, ok, detail = "") => {
  checks.push({ id, label, ok: !!ok, detail: String(detail).slice(0, 300) });
  console.log(`${ok ? "PASS" : "FAIL"} ${id} ${label}${detail ? ` — ${String(detail).slice(0, 160)}` : ""}`);
};

const base = mkdtempSync(join(tmpdir(), "orkestr-ctx-shots-"));
const home = join(base, "home");
const proj = join(base, "demo-app");
mkdirSync(home, { recursive: true });
mkdirSync(join(proj, "src"), { recursive: true });
writeFileSync(join(proj, "README.md"), "# demo-app\n\nRight-click anywhere: Orkestr draws its own menu.\n");
writeFileSync(join(proj, "src", "main.ts"), 'export function greet(name: string): string {\n  return `Hello, ${name}!`;\n}\n');

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
  const frame = (o, id) => new Promise((res, rej) => (pending.set(id, { resolve: res, reject: rej }), ws.send(J(o))));
  return {
    open: () => new Promise((res, rej) => ((ws.onopen = () => frame({ type: "auth", token, id: "a" }, "a").then(res, rej)), (ws.onerror = rej))),
    call: (method, params = {}) => {
      const id = `r${++seq}`;
      return frame({ type: "req", id, method, params }, id);
    },
    close: () => ws.close(),
  };
}

class Cdp {
  constructor(url) {
    this.ws = new WebSocket(url);
    this.id = 0;
    this.pending = new Map();
  }
  async open() {
    await new Promise((r, j) => ((this.ws.onopen = r), (this.ws.onerror = j)));
    this.ws.onmessage = (m) => {
      const f = JSON.parse(m.data);
      const p = f.id && this.pending.get(f.id);
      if (p) {
        this.pending.delete(f.id);
        f.error ? p.reject(new Error(f.error.message)) : p.resolve(f.result);
      }
    };
    await this.send("Runtime.enable");
    await this.send("Page.enable");
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((res, rej) => (this.pending.set(id, { resolve: res, reject: rej }), this.ws.send(J({ id, method, params }))));
  }
  async eval(expr) {
    const r = await this.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  }
  async key(key, { ctrl = false } = {}) {
    const code = key.length === 1 ? `Key${key.toUpperCase()}` : key;
    const vk = key.length === 1 ? key.toUpperCase().charCodeAt(0) : { Enter: 13, Escape: 27 }[key];
    for (const type of ["rawKeyDown", "keyUp"]) await this.send("Input.dispatchKeyEvent", { type, modifiers: ctrl ? 2 : 0, key, code, windowsVirtualKeyCode: vk });
  }
  /** A real right-click (mousedown / mouseup → native contextmenu). */
  async rightClick(x, y) {
    await this.send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
    await this.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "right", buttons: 2, clickCount: 1 });
    await this.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "right", buttons: 0, clickCount: 1 });
  }
  async leftClick(x, y, clickCount = 1) {
    await this.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", buttons: 1, clickCount });
    await this.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", buttons: 0, clickCount });
  }
}

const shots = [];
async function capture(cdp, name, note) {
  await sleep(400);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
  shots.push({ name: `${name}.png`, note });
  console.log(`shot ${name}`);
}

async function palette(cdp, text) {
  await cdp.eval("document.activeElement?.blur?.(), true");
  await cdp.key("k", { ctrl: true });
  await sleep(350);
  await cdp.send("Input.insertText", { text });
  await sleep(450);
  await cdp.key("Enter");
  await sleep(1200);
}
const openPanel = (cdp, lang, panel) => palette(cdp, L[lang].shell.palette.openPanel.replace("{{title}}", L[lang].shell.panels[panel].title));

/** Center of the first visible element matching `selector` (optionally containing `text`). */
const center = (cdp, selector, text = null) =>
  cdp.eval(`(() => {
    const els = [...document.querySelectorAll(${J(selector)})].filter((e) => e.getClientRects().length && (${J(text)} === null || e.textContent.includes(${J(text)})));
    const el = els[0];
    if (!el) return null;
    el.scrollIntoView({ block: "nearest" });
    const r = el.getBoundingClientRect();
    return { x: Math.round(r.x + Math.min(r.width / 2, 80)), y: Math.round(r.y + Math.min(r.height / 2, 14)) };
  })()`);

// Records the contextmenu events (capture phase: components may stop propagation);
// defaultPrevented is read after dispatch = whether the native menu was cancelled.
const PROBE = `(() => { if (!window.__ctxProbe) { window.__ctxProbe = []; window.addEventListener("contextmenu", (e) => window.__ctxProbe.push(e), true); } window.__ctxProbe.length = 0; return true; })()`;
const menuState = (cdp) =>
  cdp.eval(`({
    prevented: window.__ctxProbe.length > 0 && window.__ctxProbe.every((e) => e.defaultPrevented),
    orkestr: [...document.querySelectorAll(".ok-ctxmenu .ok-ctxmenu__label")].map((n) => n.textContent),
    monaco: !!document.querySelector(".monaco-menu-container, .context-view .monaco-menu") || [...document.querySelectorAll(".shadow-root-host")].some((h) => h.shadowRoot?.querySelector(".monaco-menu")),
  })`);
const closeMenu = async (cdp) => {
  await cdp.key("Escape");
  await sleep(300);
};

async function scenario(cdp, lang, id, label, at, expect, shot) {
  if (!at) {
    const dbg = await cdp.eval(`({ tabs: [...document.querySelectorAll('.dv-tab')].map((t) => t.textContent.trim()).join("|"), tree: document.querySelectorAll('[role=tree] [role=treeitem]').length, term: document.querySelectorAll('.term-view').length, dialog: !!document.querySelector('[role=dialog]') })`).catch(() => null);
    await capture(cdp, `dbg-${id}-${lang}`, "debug");
    return check(id, `${label} (${lang})`, false, `target not found ${J(dbg)}`);
  }
  await cdp.eval(PROBE);
  await cdp.rightClick(at.x, at.y);
  await sleep(700);
  const st = await menuState(cdp);
  const ok = st.prevented && expect(st);
  check(id, `${label} (${lang})`, ok, J(st));
  if (shot) await capture(cdp, `${shot}-${lang}`, `${label} (${lang})`);
  await closeMenu(cdp);
  return ok;
}

let exitCode = 0;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 60_000, "core.json");
  const rpc = rpcClient(info.url, readFileSync(join(home, "core.token"), "utf8").trim());
  await rpc.open();
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, performanceMode: "Balanced", language: "en" } });
  const project = (await rpc.call("projects.create", { name: "demo-app", rootPath: proj, approvalMode: "SemiAuto" })).project;
  for (const title of ["Add login form validation", "Write release notes", "Fix flaky terminal test"]) await rpc.call("tasks.create", { projectId: project.id, title }).catch((e) => console.log("task:", e.message));

  const target = await waitFor(async () => (await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json()).find((t) => t.type === "page" && !t.url.startsWith("devtools") && !t.url.includes("#/")), 60_000, "DevTools target");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send("Page.reload", { ignoreCache: true });
  await waitFor(() => cdp.eval("!!document.querySelector('.app__body')"), 60_000, "app shell");
  await sleep(2500);
  await cdp.eval(`document.querySelector('[aria-label="Open project demo-app"]')?.click(), true`);
  await sleep(1500);

  for (const lang of ["en", "ru"]) {
    const cm = L[lang].cm;
    if (lang === "ru") {
      await rpc.call("settings.update", { patch: { language: "ru" } });
      await sleep(1500);
    }
    // 1. Text field (command palette input is always there; use the Manager composer).
    await openPanel(cdp, lang, "manager");
    const field = await cdp.eval(`(() => {
      const el = [...document.querySelectorAll("textarea:not(.xterm-helper-textarea)")].find((e) => e.getClientRects().length && !e.readOnly && !e.disabled);
      if (!el) return null;
      el.focus();
      const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, "value").set.call(el, ${J(lang === "ru" ? "Проверить контекстное меню" : "Check the context menu")});
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.setSelectionRange(0, 9);
      const r = el.getBoundingClientRect();
      return { x: Math.round(r.x + 30), y: Math.round(r.y + Math.min(r.height / 2, 14)) };
    })()`);
    await scenario(cdp, lang, "INPUT", "text field → Undo/Redo/Cut/Copy/Paste/Select all", field, (s) => [cm.edit.undo, cm.edit.cut, cm.edit.paste, cm.edit.selectAll].every((l) => s.orkestr.includes(l)), "01-input");
    await cdp.eval(`(() => { const el = document.activeElement; if (el instanceof HTMLTextAreaElement) { Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set.call(el, ""); el.dispatchEvent(new Event("input", { bubbles: true })); el.blur(); } return true; })()`);

    // 2. File tree.
    await openPanel(cdp, lang, "files");
    await waitFor(() => center(cdp, "[role=treeitem]", "README.md"), 10_000, "file tree").catch(() => null);
    await scenario(cdp, lang, "LIST", "file tree row → file menu", await center(cdp, "[role=treeitem]", "README.md"), (s) => s.orkestr.length >= 5, "02-file-tree");

    // 3. Monaco editor: Monaco's own menu is off; the Code panel contributes Orkestr items.
    const row = await center(cdp, "[role=treeitem]", "main.ts").catch(() => null);
    if (!row) {
      const src = await center(cdp, "[role=treeitem]", "src");
      if (src) await cdp.leftClick(src.x, src.y);
      await sleep(800);
    }
    const mainTs = await center(cdp, "[role=treeitem]", "main.ts");
    if (mainTs) await cdp.leftClick(mainTs.x, mainTs.y);
    await waitFor(() => center(cdp, ".monaco-editor .view-lines"), 15_000, "editor").catch(() => null);
    await sleep(1200);
    await scenario(cdp, lang, "EDITOR", "Monaco editor → Orkestr editor menu (Monaco's own menu off)", await center(cdp, ".monaco-editor .view-lines"), (s) => !s.monaco && s.orkestr.includes(cm.editor.goToDefinition) && s.orkestr.includes(cm.edit.paste), "03-editor");

    // 4. Terminal.
    await openPanel(cdp, lang, "terminal");
    if (!(await center(cdp, ".term-view:not([hidden]) .term-view__host"))) {
      const btn = await center(cdp, ".term-panel button", L[lang].term.new);
      if (btn) await cdp.leftClick(btn.x, btn.y);
    }
    await waitFor(() => center(cdp, ".term-view:not([hidden]) .term-view__host"), 15_000, "terminal").catch(() => null);
    await sleep(1500);
    await scenario(cdp, lang, "TERMINAL", "terminal → Copy/Paste/Select all/Clear", await center(cdp, ".term-view:not([hidden]) .term-view__host"), (s) => [cm.terminal.paste, cm.terminal.clear].every((l) => s.orkestr.includes(l)), "04-terminal");

    // 5. Task list.
    await openPanel(cdp, lang, "tasks");
    await waitFor(() => center(cdp, ".tasks-row"), 10_000, "task rows").catch(() => null);
    await scenario(cdp, lang, "TASKS", "task row → Open/Ask Manager/Copy", await center(cdp, ".tasks-row"), (s) => s.orkestr.includes(cm.tasks.askManager), "05-tasks");

    // 6. Selected text (non-editable).
    const sel = await cdp.eval(`(() => {
      const el = document.querySelector(".tasks-row__name");
      if (!el) return null;
      const r = document.createRange(); r.selectNodeContents(el);
      getSelection().removeAllRanges(); getSelection().addRange(r);
      const b = el.getBoundingClientRect();
      return { x: Math.round(b.x + 10), y: Math.round(b.y + b.height / 2) };
    })()`);
    await scenario(cdp, lang, "SELECTION", "selected text → Copy/Search/Ask Manager", sel, (s) => s.orkestr.includes(cm.selection.copy) && s.orkestr.includes(cm.selection.askManager), "06-selection");
  }
  await rpc.call("settings.update", { patch: { language: "en" } });
  const policyWarn = /could not apply the web view policy/.test(log);
  check("POLICY", "WebView2 policy applied (AreDefaultContextMenusEnabled=false) without warnings", !policyWarn, policyWarn ? "warning in log" : "");
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
  writeFileSync(join(outDir, "manifest.json"), `${J({ date: new Date().toISOString(), result: `${checks.length - failed.length}/${checks.length} checks passed`, checks, screenshots: shots }, null, 2)}\n`);
  console.log(`\n${checks.length - failed.length}/${checks.length} checks passed`);
  await sleep(1000);
  try {
    rmSync(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  } catch {
    /* ignore */
  }
  process.exit(exitCode);
}
