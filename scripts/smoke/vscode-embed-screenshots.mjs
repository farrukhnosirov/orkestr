#!/usr/bin/env node
// VS Code inside the Orkestr window (ADR-0043) — live check of the packaged
// (custom-protocol) desktop build with an isolated ORKESTR_HOME, driven
// through the WebView2 DevTools protocol:
//
//   V1 the VS Code panel starts the VSCodium REH server and shows the real
//      workbench in a frame on http://vscode.tauri.localhost:<port> inside the
//      Orkestr main window (workbench DOM read from the frame's own target)
//   V2 Open VSX extensions installed through Orkestr (`vscode.installExtension`):
//      a theme (Dracula), a language extension with a language server
//      (redhat.vscode-yaml) and a webview extension (anthropic.claude-code)
//   V3 the extensions activate: YAML language-server diagnostics on a broken
//      file, the PDF custom editor renders in a webview, the Dracula theme
//      applies, their commands are in `vscode.commands` (runtime list)
//   V4 Orkestr's theme reaches the workbench (bridge `applyTheme`)
//   V5 Ctrl+Alt+K inside VS Code opens Orkestr's command palette
//   V6 Extensions tab: list with icons; disable → restart → disabled (folder
//      moved out), enable → restart → back
//   V7 "Open in VS Code" (`vscode.openFile`) reveals a file:line in the frame
//   V8 the panel works detached into its own Orkestr window
//   screenshots into docs/screenshots/vscode-embed/ + manifest.json
//
// Usage (repo root, Windows) — no Vite needed, the frontend is bundled:
//   npm run build
//   cargo build -p orkestr-desktop --features custom-protocol
//   node scripts/smoke/vscode-embed-screenshots.mjs [--out docs/screenshots/vscode-embed]
// Needs an installed VSCodium REH server: ORKESTR_SMOKE_REH (a version dir
// with installed.json one level up; default: the user's Orkestr data dir) is
// linked (junction) into the temporary home instead of downloading 130 MB.
// ORKESTR_SMOKE_EXE overrides the executable, ORKESTR_SMOKE_CDP_PORT the
// DevTools port.

import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "vscode-embed"));
const targetDir = process.env.CARGO_TARGET_DIR ?? join(repo, "target");
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(targetDir, "debug", "orkestr-desktop.exe");
const cdpPort = Number(process.env.ORKESTR_SMOKE_CDP_PORT ?? 9363);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

const locale = (lang, ns) => JSON.parse(readFileSync(join(repo, "apps", "desktop", "src", "locales", lang, `${ns}.json`), "utf8"));
const VS = { en: locale("en", "vscode"), ru: locale("ru", "vscode") };
const LAYOUT = locale("en", "layout");

const checks = [];
function check(id, label, ok, detail = "") {
  checks.push({ id, label, ok: !!ok, detail: String(detail).slice(0, 400) });
  console.log(`${ok ? "PASS" : "FAIL"} ${id} ${label}${detail ? ` — ${String(detail).slice(0, 200)}` : ""}`);
}

// ── REH server from the user's data dir (junction, no download) ─────────
const userRoot = join(process.env.APPDATA ?? "", "Orkestr", "Orkestr", "data", "vscode-server");
const installedJson = JSON.parse(readFileSync(join(userRoot, "installed.json"), "utf8"));
const rehDir = process.env.ORKESTR_SMOKE_REH ?? join(userRoot, installedJson.version);
if (!existsSync(join(rehDir, "out", "server-main.js"))) throw new Error(`no VSCodium REH server at ${rehDir}`);

const base = mkdtempSync(join(tmpdir(), "orkestr-vsx-"));
const home = join(base, "home");
const root = join(home, "vscode-server");
mkdirSync(root, { recursive: true });
execFileSync("cmd", ["/c", "mklink", "/J", join(root, installedJson.version), rehDir], { stdio: "ignore" });
writeFileSync(join(root, "installed.json"), JSON.stringify({ ...installedJson, dir: join(root, installedJson.version) }, null, 2));

const project = join(base, "demo-project");
mkdirSync(project, { recursive: true });
writeFileSync(join(project, "README.md"), "# demo-project\n\nVS Code inside Orkestr.\n");
writeFileSync(join(project, "config.yaml"), "name: demo\nservices:\n  web: [unclosed\n  port: 8080\n");
// A minimal one-page PDF (custom editor webview of tomoki1207.pdf).
{
  const objs = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 420 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    null,
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
  ];
  const stream = "BT /F1 28 Tf 40 100 Td (Orkestr PDF webview) Tj ET";
  objs[3] = `<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`;
  let pdf = "%PDF-1.4\n";
  const offsets = [];
  objs.forEach((o, i) => {
    offsets.push(pdf.length);
    pdf += `${i + 1} 0 obj\n${o}\nendobj\n`;
  });
  const xref = pdf.length;
  pdf += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n${offsets.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("")}`;
  pdf += `trailer\n<< /Size ${objs.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  writeFileSync(join(project, "sample.pdf"), pdf, "latin1");
}

// ── launch ───────────────────────────────────────────────────────────────
const child = spawn(exe, [], {
  env: {
    ...process.env,
    ORKESTR_HOME: home,
    CODEX_HOME: join(base, "codex-home"), // never the real ChatGPT account
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
      }
    };
    await this.send("Runtime.enable");
    await this.send("Page.enable");
  }
  send(method, params = {}, sessionId) {
    const id = ++this.id;
    return new Promise((res, rej) => {
      this.pending.set(id, { resolve: res, reject: rej });
      this.ws.send(JSON.stringify(sessionId ? { id, method, params, sessionId } : { id, method, params }));
    });
  }
  async eval(expr, ctx) {
    // ctx: a frame context (`{ contextId }`, same-process frame) or an OOPIF session id.
    const contextId = ctx && typeof ctx === "object" ? ctx.contextId : undefined;
    const sessionId = typeof ctx === "string" ? ctx : undefined;
    const r = await this.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true, ...(contextId ? { contextId } : {}) }, sessionId);
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
    return r.result.value;
  }
  async key(key, { ctrl = false, alt = false, shift = false } = {}, sessionId) {
    const code = key.length === 1 ? `Key${key.toUpperCase()}` : key;
    const vk = key.length === 1 ? key.toUpperCase().charCodeAt(0) : { Enter: 13, Escape: 27 }[key];
    const modifiers = (alt ? 1 : 0) | (ctrl ? 2 : 0) | (shift ? 8 : 0);
    for (const type of ["rawKeyDown", "keyUp"]) {
      await this.send("Input.dispatchKeyEvent", { type, modifiers, key, code, windowsVirtualKeyCode: vk }, sessionId);
    }
  }
  async type(text, sessionId) {
    await this.send("Input.insertText", { text }, sessionId);
  }
}

const J = JSON.stringify;
const shots = [];
async function capture(cdp, name, note) {
  await sleep(800);
  const file = join(outDir, `${name}.png`);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(file, Buffer.from(data, "base64"));
  shots.push({ name, note, errors: cdp.console.splice(0) });
  console.log(`shot ${name}`);
}

/**
 * Execution context of the workbench frame. The frame is same-site with the
 * app, so WebView2 keeps it in the page's process (no OOPIF target): an
 * isolated world in that frame reads its DOM. Falls back to an OOPIF session.
 */
async function frameContext(cdp) {
  const { frameTree } = await cdp.send("Page.getFrameTree");
  const walk = (n, acc) => {
    for (const c of n.childFrames ?? []) {
      acc.push(c.frame);
      walk(c, acc);
    }
    return acc;
  };
  const f = walk(frameTree, []).find((x) => x.url.includes("vscode.tauri.localhost"));
  if (f) {
    const { executionContextId } = await cdp.send("Page.createIsolatedWorld", { frameId: f.id, worldName: "vsx-check" });
    return { contextId: executionContextId, url: f.url };
  }
  const { targetInfos } = await cdp.send("Target.getTargets");
  const t = targetInfos.find((x) => x.type === "iframe" && x.url.includes("vscode.tauri.localhost"));
  if (!t) return null;
  const { sessionId } = await cdp.send("Target.attachToTarget", { targetId: t.targetId, flatten: true });
  return sessionId;
}

const clickText = (cdp, selector, text) =>
  cdp.eval(`(() => {
    const els = [...document.querySelectorAll(${J(selector)})].filter((e) => e.getClientRects().length);
    const el = els.find((e) => (e.getAttribute("aria-label") ?? "").trim() === ${J(text)}) ?? els.find((e) => e.textContent.trim() === ${J(text)});
    if (!el) return false;
    el.click();
    return true;
  })()`);

/** The main window's DevTools target (not the orb, HUD or a detached panel:
 *  the orb's target can be listed before its `#/orb` URL commits, so the
 *  Tauri window label is checked in the page itself). */
async function findMainTarget(port) {
  const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  for (const t of list.filter((t) => t.type === "page" && !t.url.startsWith("devtools") && !t.url.includes("#/"))) {
    const c = new Cdp(t.webSocketDebuggerUrl);
    try {
      await c.open();
      // Tauri's own window label is authoritative (the URL hash may not be set yet).
      const label = await c.eval("window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label ?? null");
      if (label === "main") return t;
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

let exitCode = 0;
let rpc = null;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, performanceMode: "Balanced", language: "en" } });
  const created = await rpc.call("projects.create", { name: "demo-project", rootPath: project, approvalMode: "SemiAuto" });
  const projectId = created.project?.id ?? created.id;
  const status0 = await rpc.call("vscode.status");
  check("V0", "the temporary home sees the installed VSCodium server", status0.state === "stopped" && status0.installed?.version, status0.state);

  // V2: install three Open VSX extensions through Orkestr.
  const want = ["dracula-theme.theme-dracula", "redhat.vscode-yaml", "anthropic.claude-code"];
  for (const id of want) {
    const t0 = Date.now();
    await rpc.call("vscode.installExtension", { id });
    console.log(`installed ${id} in ${Math.round((Date.now() - t0) / 1000)} s`);
  }
  const list = (await rpc.call("vscode.listExtensions")).extensions.map((e) => e.id);
  check("V2", "theme, language-server and webview extensions installed from Open VSX", want.every((w) => list.includes(w)), list.join(", "));

  // DevTools target of the main window.
  const target = await waitFor(() => findMainTarget(cdpPort), 60_000, "devtools target");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  await waitFor(() => cdp.eval(`!!document.querySelector('[role="tab"]')`), 60_000, "workspace");
  await cdp.eval(`document.querySelector(".resume-card__head button")?.click(), true`);
  // A fresh home shows onboarding before the settings update lands: skip it.
  await sleep(1500);
  await clickText(cdp, "button", "Skip setup");
  await sleep(800);

  // V1: open the VS Code panel (palette command "Open VS Code panel").
  await cdp.key("k", { ctrl: true });
  await sleep(400);
  await cdp.type(VS.en.commands.open);
  await sleep(600);
  await cdp.key("Enter");
  const frameUrl = await waitFor(
    () => cdp.eval(`document.querySelector('[data-testid="vscode-embed"]')?.src ?? null`),
    120_000,
    "embedded workbench frame",
  );
  check("V1a", "the panel shows VS Code in a frame on vscode.tauri.localhost", /^http:\/\/vscode\.tauri\.localhost:\d+\//.test(frameUrl), frameUrl.replace(/tkn=[^&]+/, "tkn=…"));
  // Maximize the panel's dock group for readable screenshots.
  await cdp.eval(`(() => {
    let g = document.querySelector('[data-testid="vscode-embed"]');
    while (g && g !== document.body) {
      const b = [...g.querySelectorAll("button")].find((x) => x.getAttribute("aria-label") === ${J(LAYOUT.actions.maximize)});
      if (b) { b.click(); return true; }
      g = g.parentElement;
    }
    return false;
  })()`);
  await waitFor(async () => (await rpc.call("vscode.status")).bridgeConnected, 120_000, "bridge connected");
  let frame = await waitFor(() => frameContext(cdp), 60_000, "frame context");
  const F = () => frame;
  /** VS Code's own theme picker: trusted key input into its quick input. */
  const pickTheme = async (name) => {
    await rpc.call("vscode.execute", { commandId: "workbench.action.selectTheme", waitMs: 5000 }).catch(() => null);
    await waitFor(() => cdp.eval(`(() => { const i = document.querySelector(".quick-input-widget input"); return !!i && i.getClientRects().length > 0; })()`, F()), 15_000, "theme picker").catch(() => null);
    await cdp.eval(`document.querySelector('[data-testid="vscode-embed"]').focus(), true`);
    await cdp.eval(`document.querySelector(".quick-input-widget input").focus(), true`, F());
    await cdp.type(name);
    await sleep(1500);
    await cdp.key("Enter");
    await sleep(2500);
  };
  const workbench = await waitFor(
    () => cdp.eval(`!!document.querySelector(".monaco-workbench .part.activitybar") && !!document.querySelector(".explorer-folders-view .monaco-list-row")`, F()),
    120_000,
    "workbench DOM",
  ).catch(() => false);
  check("V1b", "the real VS Code workbench renders inside the Orkestr window (explorer shows the project)", workbench);
  const folder = await cdp.eval(`[...document.querySelectorAll(".explorer-folders-view .monaco-list-row")].map((r) => r.textContent.trim()).join(",")`, F());
  check("V1c", "the project folder is open in the workbench", folder.includes("config.yaml") && folder.includes("sample.pdf"), folder);

  // Orkestr projects open trusted by default (config.workspaceTrust = false).
  const trustNow = (await rpc.call("vscode.commands", { locale: "en" })).workspaceTrusted;
  check("V1d", "the project opens trusted (no Restricted Mode), so every extension can run", trustNow === true, String(trustNow));
  await rpc.call("vscode.execute", { commandId: "workbench.action.closeAllEditors", waitMs: 5000 }).catch(() => null);

  // V4: Orkestr theme → workbench colors (the panel pushes them on connect).
  const okBg = await cdp.eval(`getComputedStyle(document.documentElement).getPropertyValue("--ok-color-bg").trim()`);
  const SIDE = `getComputedStyle(document.querySelector(".monaco-workbench")).getPropertyValue("--vscode-sideBar-background").trim()`;
  const sideBg = await waitFor(() => cdp.eval(SIDE, F()), 30_000, "sidebar color");
  const hex = (rgb) => {
    const m = /rgba?\((\d+),\s*(\d+),\s*(\d+)/.exec(rgb ?? "");
    return m ? `#${[m[1], m[2], m[3]].map((n) => Number(n).toString(16).padStart(2, "0")).join("")}` : rgb;
  };
  const parseOk = (v) => (v.startsWith("#") ? v.slice(0, 7).toLowerCase() : hex(v));
  await waitFor(
    async () => parseOk(await cdp.eval(SIDE, F())) === parseOk(okBg),
    30_000,
    "theme sync",
  ).catch(() => null);
  const sideNow = parseOk(await cdp.eval(SIDE, F()));
  check("V4", "Orkestr's theme colors reach the workbench (--vscode-sideBar-background = --ok-color-bg)", sideNow === parseOk(okBg), `side bar ${sideNow} (first seen ${sideBg}), Orkestr bg ${okBg}`);
  if (sideNow !== parseOk(okBg)) {
    await rpc.call("vscode.execute", { commandId: "workbench.action.openSettingsJson", waitMs: 5000 }).catch((e) => console.log(e.message));
    await sleep(2500);
    console.log("DEBUG settings.json:", await cdp.eval(`document.querySelector(".part.editor .view-lines")?.innerText ?? "(none)"`, F()));
    console.log("DEBUG vars:", await cdp.eval(`(() => { const cs = getComputedStyle(document.querySelector(".monaco-workbench")); return ["editor-background","activityBar-background","sideBar-background","focusBorder","button-background"].map((k) => k + "=" + cs.getPropertyValue("--vscode-" + k).trim()).join(" "); })()`, F()));
    if (process.env.STOP_AFTER_V4) throw new Error("stop");
    console.log("DEBUG theme class:", await cdp.eval(`document.querySelector(".monaco-workbench").className`, F()));
  }
  await capture(cdp, "01-workbench-in-orkestr", "VS Code workbench inside the Orkestr main window, Orkestr theme applied");

  // V3a: YAML language server diagnostics.
  await rpc.call("vscode.openFile", { path: join(project, "config.yaml"), line: 3, column: 8, waitMs: 30_000 });
  const diag = await waitFor(
    () => cdp.eval(`document.querySelectorAll(".squiggly-error, .squiggly-warning").length`, F()),
    90_000,
    "yaml diagnostics",
  ).catch(() => 0);
  check("V3a", "the YAML extension's language server reports diagnostics in the open file", diag > 0, `${diag} squiggles`);
  const tab = await cdp.eval(`document.querySelector(".tabs-container .tab.active")?.textContent ?? ""`, F());
  check("V7", "vscode.openFile (Open in VS Code) shows the file in the embedded workbench", tab.includes("config.yaml"), tab);
  await capture(cdp, "02-yaml-language-server", "redhat.vscode-yaml: language-server diagnostics in the embedded workbench");

  // V3b: a webview extension: Claude Code's chat as an editor tab (a webview panel).
  await rpc.call("vscode.openView", { extensionId: "anthropic.claude-code", waitMs: 30_000 }).catch((e) => console.log(`openView claude: ${e.message}`));
  await sleep(3000);
  await rpc.call("vscode.execute", { commandId: "claude-vscode.editor.open", waitMs: 30_000 }).catch((e) => console.log(`claude editor: ${e.message}`));
  const webview = await waitFor(() => cdp.eval(`document.querySelectorAll("iframe.webview.ready").length`, F()), 60_000, "claude webview").catch(() => 0);
  await sleep(6000);
  // The webview host (vscode-cdn.net, its own service worker) runs nested in the
  // embedded frame and received the extension's document with VS Code's (= Orkestr's) theme.
  let webviewDoc = null;
  const { targetInfos } = await cdp.send("Target.getTargets");
  const wv = targetInfos.find((t) => t.type === "iframe" && t.url.includes("vscode-cdn.net"));
  const sw = targetInfos.some((t) => t.type === "service_worker" && t.url.includes("vscode-cdn.net"));
  if (wv) {
    const { sessionId } = await cdp.send("Target.attachToTarget", { targetId: wv.targetId, flatten: true });
    webviewDoc = await cdp.eval(`(() => { const d = document.querySelector("#active-frame")?.contentDocument; if (!d) return null; return { themed: (d.documentElement.getAttribute("style") ?? "").includes("--vscode-foreground"), text: (d.body?.innerText ?? "").trim().slice(0, 120), html: d.body?.innerHTML.length ?? 0 }; })()`, sessionId).catch(() => null);
  }
  check("V3b", "the Claude Code extension's webview runs inside the embedded workbench (webview host + service worker, extension document loaded, theme passed)", webview > 0 && sw && webviewDoc?.themed && webviewDoc.html > 0, `${webview} ready webview(s), sw=${sw}, doc=${J(webviewDoc)}`);
  await capture(cdp, "03-claude-code-webview", "Claude Code (webview extension from Open VSX) inside the embedded workbench");

  // V3c: runtime commands of the installed extensions.
  const catalog = await rpc.call("vscode.commands", { locale: "en" });
  const yaml = catalog.extensions.find((e) => e.id === "redhat.vscode-yaml");
  const pdf = catalog.extensions.find((e) => e.id === "anthropic.claude-code");
  check("V3c", "extension catalog lists the installed extensions; bridge connected", catalog.bridgeConnected && !!yaml && !!pdf && catalog.extensions.some((e) => e.id === "dracula-theme.theme-dracula"), `${catalog.extensions.map((e) => `${e.id}(${e.commands.length})`).join(", ")}`);

  // V3d: switch to the Dracula theme through VS Code's own theme picker.
  const EDITOR_BG = `getComputedStyle(document.querySelector(".monaco-workbench")).getPropertyValue("--vscode-editor-background").trim().toLowerCase()`;
  for (let attempt = 0; attempt < 3; attempt++) {
    await pickTheme("Dracula Theme");
    if ((await cdp.eval(EDITOR_BG, F())) === "#282a36") break;
    await cdp.key("Escape");
  }
  const draculaBg = await cdp.eval(`getComputedStyle(document.querySelector(".monaco-workbench")).getPropertyValue("--vscode-editor-background").trim().toLowerCase()`, F());
  const themeClass = await cdp.eval(`document.querySelector(".monaco-workbench").className`, F());
  check("V3d", "the Dracula theme extension applies (workbench repainted)", /dracula/i.test(themeClass) || draculaBg === "#282a36", `${draculaBg}; ${themeClass.match(/vscode-theme-[\w-]+/)?.[0] ?? ""}`);
  await capture(cdp, "04-dracula-theme", "Dracula theme from Open VSX applied in the embedded workbench");
  // Back to the Orkestr-synced default theme.
  await pickTheme("Dark Modern");

  // V5: Ctrl+Alt+K inside VS Code → Orkestr palette.
  await cdp.eval(`document.querySelector(".monaco-editor textarea, .monaco-workbench")?.focus(), true`, F());
  await cdp.eval(`document.querySelector('[data-testid="vscode-embed"]').focus(), true`);
  await sleep(300);
  await cdp.key("k", { ctrl: true, alt: true });
  const paletteOpened = await waitFor(() => cdp.eval(`!!document.querySelector('[role="dialog"] input, .palette input, [cmdk-input]')`), 15_000, "palette").catch(() => false);
  check("V5", "Ctrl+Alt+K inside the embedded VS Code opens Orkestr's command palette", paletteOpened);
  await capture(cdp, "05-palette-from-vscode", "Orkestr command palette opened from inside VS Code (Ctrl+Alt+K)");
  await cdp.key("Escape");
  await sleep(400);

  // V6: Extensions tab, disable → restart → disabled.
  await clickText(cdp, '[role="tab"]', VS.en.tabs.extensions);
  await waitFor(() => cdp.eval(`document.querySelectorAll(".vsc-ext").length >= 3`), 60_000, "extension list");
  await sleep(1500);
  const icons = await cdp.eval(`document.querySelectorAll(".vsc-ext img.vsc-ext__icon").length`);
  check("V6a", "the Extensions tab lists the installed extensions with their icons", icons >= 2, `${icons} icons`);
  await capture(cdp, "06-extensions-tab", "Extensions tab: installed VS Code extensions managed from Orkestr");
  await cdp.eval(`(() => { const row = [...document.querySelectorAll(".vsc-ext")].find((r) => r.textContent.includes("dracula-theme.theme-dracula")); const b = row && [...row.querySelectorAll("button")].find((x) => x.textContent.trim() === ${J(VS.en.manager.disable)}); b?.click(); return !!b; })()`);
  await waitFor(() => cdp.eval(`!!document.querySelector(".vsc-notice")`), 60_000, "restart notice").catch(() => null);
  await sleep(800);
  await capture(cdp, "07-disable-restart-needed", "Disable while running → restart needed");
  const restartBtn = await clickText(cdp, "button", VS.en.manager.restartNow);
  await waitFor(async () => {
    const l = (await rpc.call("vscode.listExtensions")).extensions.find((e) => e.id === "dracula-theme.theme-dracula");
    return l && l.disabled && !l.restartRequired;
  }, 180_000, "disabled after restart").catch(() => null);
  const after = (await rpc.call("vscode.listExtensions")).extensions.find((e) => e.id === "dracula-theme.theme-dracula");
  console.log(log.split(String.fromCharCode(10)).filter((l) => /disabled|enabled|state of an extension/.test(l)).slice(-6).join(" | "));
  const moved = existsSync(join(root, "disabled-extensions")) && readdirSync(join(root, "disabled-extensions")).some((d) => d.startsWith("dracula-theme.theme-dracula"));
  check("V6b", "disable + Restart VS Code moves the extension out; it is reported disabled", restartBtn && after?.disabled && !after.restartRequired && moved, J(after));
  await waitFor(() => cdp.eval(`!!document.querySelector('[data-testid="vscode-embed"]')`), 120_000, "frame after restart").catch(() => null);
  await clickText(cdp, '[role="tab"]', VS.en.tabs.extensions);
  await waitFor(() => cdp.eval(`document.querySelectorAll(".vsc-ext").length >= 3`), 60_000, "extension list again").catch(() => null);
  await cdp.eval(`(() => { const row = [...document.querySelectorAll(".vsc-ext")].find((r) => r.textContent.includes("dracula-theme.theme-dracula")); const b = row && [...row.querySelectorAll("button")].find((x) => x.textContent.trim() === ${J(VS.en.manager.enable)}); b?.click(); return !!b; })()`);
  const restartAgain = `[...document.querySelectorAll(".vsc-notice button")].find((b) => b.textContent.trim() === ${J(VS.en.manager.restartNow)})`;
  await waitFor(() => cdp.eval(`!!${restartAgain}`), 60_000, "restart notice (enable)").catch(() => null);
  await cdp.eval(`${restartAgain}?.click(), true`);
  await waitFor(async () => {
    const l = (await rpc.call("vscode.listExtensions")).extensions.find((e) => e.id === "dracula-theme.theme-dracula");
    return l && !l.disabled && !l.restartRequired;
  }, 180_000, "enabled after restart").catch(() => null);
  const back = (await rpc.call("vscode.listExtensions")).extensions.find((e) => e.id === "dracula-theme.theme-dracula");
  console.log(log.split(String.fromCharCode(10)).filter((l) => /dracula|state of an extension|stopped|ready on/.test(l)).slice(-8).join(" | "));
  check("V6c", "enable + restart brings it back", back && !back.disabled && !back.restartRequired, J(back));
  await clickText(cdp, '[role="tab"]', VS.en.tabs.workbench);
  await waitFor(async () => (await rpc.call("vscode.status")).bridgeConnected, 120_000, "bridge after restart").catch(() => null);
  await sleep(4000);
  frame = (await frameContext(cdp).catch(() => null)) ?? frame;
  await capture(cdp, "08-after-restart", "Workbench back after the restart");

  // V8: detach the panel into its own Orkestr window.
  const detached = await cdp.eval(`(() => {
    let g = document.querySelector('[data-testid="vscode-embed"]');
    while (g && g !== document.body) {
      const b = [...g.querySelectorAll("button")].find((x) => x.getAttribute("aria-label") === ${J(LAYOUT.actions.detach)});
      if (b) { b.click(); return true; }
      g = g.parentElement;
    }
    return false;
  })()`);
  let detachedOk = false;
  if (detached) {
    const pt = await waitFor(async () => {
      const r = await fetch(`http://127.0.0.1:${cdpPort}/json`);
      return (await r.json()).find((x) => x.type === "page" && x.url.includes("#/panel"));
    }, 30_000, "detached window").catch(() => null);
    if (pt) {
      const dcdp = new Cdp(pt.webSocketDebuggerUrl);
      await dcdp.open();
      const src = await waitFor(() => dcdp.eval(`document.querySelector('[data-testid="vscode-embed"]')?.src ?? null`), 120_000, "detached frame").catch(() => null);
      await sleep(8000);
      detachedOk = !!src && /vscode\.tauri\.localhost/.test(src);
      await capture(dcdp, "09-detached-window", "VS Code panel detached into its own Orkestr window");
      await dcdp.eval(`document.querySelector('[aria-label=${J(LAYOUT.actions.redock)}]')?.click(), true`).catch(() => null);
      dcdp.ws.close();
    }
  }
  check("V8", "the VS Code panel works detached into its own Orkestr window", detachedOk);

  // Russian UI of the panel.
  await rpc.call("settings.update", { patch: { language: "ru" } });
  await sleep(1500);
  await clickText(cdp, '[role="tab"]', VS.ru.tabs.extensions);
  await sleep(1500);
  await capture(cdp, "10-extensions-tab-ru", "Extensions tab in Russian");
  await rpc.call("settings.update", { patch: { language: "en" } });
  cdp.ws.close();
} catch (e) {
  console.error(e);
  exitCode = 1;
} finally {
  writeFileSync(join(outDir, "manifest.json"), JSON.stringify({ when: new Date().toISOString(), checks, shots }, null, 2));
  try {
    await rpc?.call("vscode.stop");
  } catch {
    /* ignore */
  }
  rpc?.close();
  child.kill();
  await sleep(2500);
  // A hard-killed shell leaves its VS Code server behind: stop node processes started from the temp home.
  try {
    execFileSync("powershell", ["-NoProfile", "-Command", `Get-CimInstance Win32_Process -Filter "Name='node.exe'" | Where-Object { $_.CommandLine -like '*${base.replace(/'/g, "''")}*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`], { stdio: "ignore" });
  } catch {
    /* ignore */
  }
  await sleep(1500);
  try {
    execFileSync("cmd", ["/c", "rmdir", join(root, installedJson.version)], { stdio: "ignore" });
  } catch {
    /* ignore */
  }
  // Never recurse through the junction into the user's real server install.
  if (existsSync(join(root, installedJson.version))) {
    console.log(`left ${base} in place: the server junction could not be removed`);
  } else {
    try {
      rmSync(base, { recursive: true, force: true });
    } catch {
      /* ignore */
    }
  }
  const failed = checks.filter((c) => !c.ok);
  console.log(`\n${checks.length - failed.length}/${checks.length} checks passed`);
  if (failed.length) exitCode = 1;
  if (exitCode && log) console.log(log.split("\n").slice(-40).join("\n"));
  process.exit(exitCode);
}
