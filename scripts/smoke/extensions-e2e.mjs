#!/usr/bin/env node
// VS Code extensions in the native editor (ADR-0025), end to end with the real
// Open VSX registry and the real desktop app:
//
//   1. launches the desktop app (bundled frontend: `npm run build` +
//      `cargo build -p orkestr-desktop --features custom-protocol`) with an
//      isolated ORKESTR_HOME and a seeded Rust project;
//   2. installs from Open VSX over the RPC: dracula-theme.theme-dracula,
//      PKief.material-icon-theme, tamasfe.even-better-toml, golang.go and
//      rust-lang.rust-analyzer (platform build with a bundled server);
//   3. selects Dracula as the editor theme and Material as the icon theme;
//   4. drives the webview through the WebView2 DevTools protocol: opens
//      main.rs and Cargo.toml in the Code panel, checks the Dracula editor
//      background and TextMate-colored tokens, the Material icons in the Files
//      panel / tabs, approves rust-analyzer in the editor banner and waits for
//      its diagnostics and hover; captures screenshots of every step.
//
// Usage (repo root, Windows): node scripts/smoke/extensions-e2e.mjs [--out docs/screenshots/extensions]

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const arg = (name, dflt) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : dflt;
};
const outDir = resolve(repo, arg("--out", join("docs", "screenshots", "extensions")));
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(process.env.CARGO_TARGET_DIR ?? join(repo, "target"), "debug", "orkestr-desktop.exe");
const cdpPort = 9336;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

const base = mkdtempSync(join(tmpdir(), "orkestr-ext-"));
const home = join(base, "home");
const proj = join(base, "hello-rs");
mkdirSync(home, { recursive: true });
mkdirSync(join(proj, "src"), { recursive: true });
writeFileSync(join(proj, "Cargo.toml"), '[package]\nname = "hello-rs"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n');
writeFileSync(
  join(proj, "src", "main.rs"),
  '// Entry point\nmod missing_module;\n\nfn greet(name: &str) -> String {\n    format!("Hello, {name}!")\n}\n\nfn main() {\n    let count: i32 = "not a number";\n    println!("{}", greet("world"));\n}\n',
);
writeFileSync(join(proj, "README.md"), "# hello-rs\n");
execFileSync("git", ["init", "-q"], { cwd: proj });

const child = spawn(exe, [], {
  env: {
    ...process.env,
    ORKESTR_HOME: home,
    CODEX_HOME: join(base, "codex-home"), // never the real ChatGPT account
    ORKESTR_SECRET_BACKEND: "vault-file",
    // Debug builds: run next to an already running Orkestr (own ORKESTR_HOME).
    ORKESTR_ALLOW_SECOND_INSTANCE: "1",
    // Own WebView2 profile: a running instance may use other browser arguments.
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
        this.console.push(`${f.params.type}: ${f.params.args.map((a) => a.value ?? a.description ?? "").join(" ")}`);
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
      await this.send("Input.dispatchKeyEvent", { type, modifiers, key, code, windowsVirtualKeyCode: vk });
    }
  }
  async type(text) {
    await this.send("Input.insertText", { text });
  }
}

const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: Boolean(ok), detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
};
async function shot(cdp, name) {
  await sleep(700);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
  console.log(`  screenshot ${name}.png`);
}
const clickText = (cdp, selector, text) =>
  cdp.eval(`(() => {
    const els = [...document.querySelectorAll(${JSON.stringify(selector)})].filter((e) => e.getClientRects().length);
    const el = els.find((e) => e.textContent.trim().includes(${JSON.stringify(text)}));
    if (!el) return false;
    el.click();
    return true;
  })()`);
// The onboarding wizard can open if the UI read the settings before the seed
// marked onboarding as done; close it so it does not cover the views.
const skipWizard = (cdp) => clickText(cdp, "button", "Skip setup");
async function palette(cdp, text) {
  await skipWizard(cdp);
  await cdp.eval("document.activeElement?.blur?.(), true");
  await cdp.key("k", { ctrl: true });
  await sleep(400);
  await cdp.type(text);
  await sleep(600);
  await cdp.key("Enter");
  await sleep(900);
}
async function quickOpen(cdp, text) {
  await skipWizard(cdp);
  await cdp.key("p", { ctrl: true });
  await sleep(500);
  await cdp.type(text);
  await sleep(1200);
  await cdp.key("Enter");
  await sleep(1500);
}

let exitCode = 0;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 120_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.sub(["global"]);
  await rpc.call("settings.update", { patch: { onboardingCompleted: true, language: "en" } });
  const { project } = await rpc.call("projects.create", { name: "hello-rs", rootPath: proj });
  await rpc.call("projects.open", { projectId: project.id });

  const installed = {};
  for (const [ns, name] of [
    ["dracula-theme", "theme-dracula"],
    ["PKief", "material-icon-theme"],
    ["tamasfe", "even-better-toml"],
    ["golang", "Go"],
    ["rust-lang", "rust-analyzer"],
  ]) {
    const t0 = Date.now();
    const { extension } = await rpc.call("extensions.install", { namespace: ns, name });
    installed[extension.id] = extension;
    check(`install ${extension.id}`, true, `${extension.version} ${extension.targetPlatform ?? ""} sha256 ${extension.sha256.slice(0, 12)}… in ${Date.now() - t0} ms`);
  }
  const progress = rpc.events.filter((e) => e.kind === "ExtensionInstallProgress").length;
  check("install progress events", progress > 0, `${progress} events`);
  const dracula = installed["dracula-theme.theme-dracula"];
  const material = installed["pkief.material-icon-theme"];
  const draculaTheme = dracula.contributions.themes.find((t) => t.label === "Dracula Theme") ?? dracula.contributions.themes[0];
  await rpc.call("extensions.setPrefs", {
    editorTheme: { extensionId: dracula.id, themeId: draculaTheme.id },
    iconTheme: { extensionId: material.id, themeId: material.contributions.iconThemes[0].id },
  });
  const servers = await rpc.call("lsp.servers", {});
  const ra = servers.servers.find((s) => s.id === "rust-analyzer");
  check("rust-analyzer found in the installed extension", ra?.available && ra.source?.kind === "extension", ra?.command ?? "");

  const target = await waitFor(async () => {
    const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
    return list.find((t) => t.type === "page" && !t.url.includes("robot.html") && !t.url.includes("#/") && !/hud/i.test(t.url));
  }, 60_000, "webview DevTools target");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: 1400, height: 860, deviceScaleFactor: 1, mobile: false });
  await sleep(4000);

  // Extensions panel.
  await palette(cdp, "Extensions");
  await clickText(cdp, "[role=tab]", "Installed");
  await sleep(1500);
  await clickText(cdp, ".ext-item", "Go");
  await sleep(2500);
  const compat = await cdp.eval("document.querySelector('.ext-compat')?.textContent ?? ''");
  check("compatibility details for golang.go", compat.includes("Needs the full VS Code") && compat.includes("debuggers"), compat.slice(0, 160));
  await shot(cdp, "01-extensions-installed-go");
  await clickText(cdp, "[role=tab]", "Open VSX");
  await sleep(500);
  await cdp.eval("(() => { const i = document.querySelector('.ext-search input'); i.focus(); return true; })()");
  await cdp.type("dracula");
  await sleep(4000);
  const hits = await cdp.eval("document.querySelectorAll('.ext-list .ext-item').length");
  check("Open VSX search in the panel", hits > 0, `${hits} results`);
  await clickText(cdp, ".ext-item", "Dracula Theme Official");
  await sleep(4000);
  await shot(cdp, "02-extensions-search-dracula");

  // Code panel with Dracula + TextMate + Material icons.
  await palette(cdp, "Code");
  await quickOpen(cdp, "main.rs");
  await sleep(4000);
  const bg = await cdp.eval("getComputedStyle(document.querySelector('.monaco-editor .monaco-editor-background') ?? document.body).backgroundColor");
  check("Dracula editor background", bg === "rgb(40, 42, 54)", bg);
  const colors = await cdp.eval(`(() => {
    const out = {};
    for (const s of document.querySelectorAll('.monaco-editor .view-lines span span')) {
      const t = s.textContent.trim(); if (!t) continue;
      out[t] = getComputedStyle(s).color;
    }
    return out;
  })()`);
  check("Dracula token colors in Rust (fn → pink)", colors.fn === "rgb(255, 121, 198)", `fn=${colors.fn}`);
  check("Dracula comment color", Object.entries(colors).some(([k, v]) => k.startsWith("//") && v === "rgb(98, 114, 164)"), JSON.stringify(Object.entries(colors).slice(0, 6)));
  const icons = await cdp.eval("[...document.querySelectorAll('img.ok-file-icon')].length");
  check("Material file icons rendered", icons > 0, `${icons} icons`);
  await shot(cdp, "03-code-dracula-material");

  // rust-analyzer: approve once in the editor banner, then diagnostics + hover.
  const banner = await waitFor(() => cdp.eval("document.querySelector('.code-lsp-banner')?.textContent ?? ''"), 30_000, "LSP approval banner");
  check("approval banner for rust-analyzer", banner.includes("rust-analyzer"), banner.slice(0, 120));
  await skipWizard(cdp);
  await shot(cdp, "04-lsp-approval");
  await clickText(cdp, ".code-lsp-banner button", "Allow");
  const running = await waitFor(async () => (await rpc.call("lsp.servers", {})).sessions.find((s) => s.serverId === "rust-analyzer" && s.state === "running"), 60_000, "rust-analyzer running");
  check("rust-analyzer session running", Boolean(running), `pid ${running?.pid}`);
  const diag = await waitFor(
    () => cdp.eval("document.querySelectorAll('.monaco-editor .squiggly-error').length").then((n) => (n > 0 ? n : 0)),
    240_000,
    "rust-analyzer diagnostics",
  ).catch(() => 0);
  check("rust-analyzer diagnostics shown as markers", diag > 0, `${diag} error squiggles`);
  await shot(cdp, "05-lsp-diagnostics");
  await skipWizard(cdp);
  // Hover over the `greet` call: the hover comes from rust-analyzer (`fn greet(name: &str) -> String`).
  const pos = await cdp.eval(`(() => {
    const spans = [...document.querySelectorAll('.monaco-editor .view-lines span span')].filter((s) => s.textContent.trim() === 'greet');
    const s = spans[spans.length - 1]; if (!s) return null;
    const r = s.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  })()`);
  let hover = "";
  if (pos) {
    for (let i = 0; i < 20 && !hover.includes("fn greet"); i++) {
      await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", x: pos.x + (i % 2), y: pos.y });
      await sleep(1500);
      hover = await cdp.eval("[...document.querySelectorAll('.monaco-hover')].map((h) => h.textContent).join(' | ')");
    }
  }
  const sessions = (await rpc.call("lsp.servers", {})).sessions.map((x) => `${x.serverId}:${x.state}:r${x.restarts}${x.error ? `:${x.error.slice(0, 200)}` : ""}`);
  check("rust-analyzer hover", hover.includes("fn greet"), `${hover.slice(0, 160)} [pos ${JSON.stringify(pos)}] sessions ${sessions.join(", ")}`);
  await shot(cdp, "05b-lsp-hover");

  // TOML: a language Monaco lacks, highlighted by the extension's grammar.
  await quickOpen(cdp, "Cargo.toml");
  await sleep(3000);
  const tomlColors = await cdp.eval(`(() => [...new Set([...document.querySelectorAll('.monaco-editor .view-lines span span')].map((s) => getComputedStyle(s).color))])()`);
  check("TOML highlighted by the even-better-toml grammar", tomlColors.length >= 3, tomlColors.join(" "));
  await shot(cdp, "06-toml-grammar");

  // Settings → Editor.
  await palette(cdp, "Settings");
  await clickText(cdp, "button, [role=tab], a", "Editor");
  await sleep(2500);
  await shot(cdp, "07-settings-editor");

  const errors = cdp.console.filter((c) => !c.startsWith("warning"));
  check("no console errors", errors.length === 0, errors.slice(0, 5).join(" | "));
  rpc.close();
} catch (e) {
  console.error("smoke failed:", e);
  exitCode = 1;
} finally {
  writeFileSync(join(outDir, "report.json"), JSON.stringify({ results, home }, null, 2));
  child.kill();
  try {
    execFileSync("taskkill", ["/F", "/T", "/PID", String(child.pid)], { stdio: "ignore" });
  } catch {
    /* already gone */
  }
  if (results.some((r) => !r.ok)) exitCode = 1;
  if (exitCode) console.log(log.slice(-4000));
  process.exit(exitCode);
}
