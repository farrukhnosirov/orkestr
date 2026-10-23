#!/usr/bin/env node
// Window Library previews (Wave 8): launches the custom-protocol desktop build
// as a second, isolated instance (temp ORKESTR_HOME + WebView2 profile), seeds a
// demo project, opens every library entry through the Window Library itself
// and captures it over the WebView2 DevTools protocol as a small WebP into
// apps/desktop/src/assets/window-previews/<id>.<lang>.<dark|light>.webp.
// `--library` additionally (or with `--library-only`, only) captures the
// library itself into docs/screenshots/window-library/ (en + ru).
//
// Usage (repo root, Windows):
//   npm run build -w @orkestr/desktop
//   cargo build -p orkestr-desktop --features custom-protocol
//   node scripts/smoke/window-library-previews.mjs [--only code,git] [--library | --library-only]
// ORKESTR_SMOKE_EXE overrides the executable (default: $CARGO_TARGET_DIR or
// ./target, debug), ORKESTR_SMOKE_CDP_PORT the DevTools port.

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const arg = (name) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : undefined;
};
const only = arg("--only")?.split(",");
const libraryOnly = process.argv.includes("--library-only");
const libraryShots = libraryOnly || process.argv.includes("--library");
const previewDir = join(repo, "apps", "desktop", "src", "assets", "window-previews");
const shotDir = join(repo, "docs", "screenshots", "window-library");
const targetDir = process.env.CARGO_TARGET_DIR ?? join(repo, "target");
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(targetDir, "debug", "orkestr-desktop.exe");
const cdpPort = Number(process.env.ORKESTR_SMOKE_CDP_PORT ?? 9361);
const VIEW = { width: 1280, height: 800 };
const PREVIEW_WIDTH = 480;
const THEMES = { dark: "Nebula", light: "Solar Light" };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(previewDir, { recursive: true });
mkdirSync(shotDir, { recursive: true });

// ── seed data dir + project ───────────────────────────────────────────────
const base = mkdtempSync(join(tmpdir(), "orkestr-wl-"));
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

const child = spawn(exe, [], {
  env: {
    ...process.env,
    ORKESTR_HOME: home,
    // Never capture the real ChatGPT / Codex account into the bundled previews.
    CODEX_HOME: join(base, "codex-home"),
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
    await sleep(200);
  }
}

function rpcClient(url, token) {
  const ws = new WebSocket(url);
  let seq = 0;
  const pending = new Map();
  ws.onmessage = (m) => {
    const f = JSON.parse(m.data);
    if (f.type !== "res") return;
    const p = pending.get(f.id);
    pending.delete(f.id);
    f.ok ? p?.resolve(f.result) : p?.reject(new Error(`${f.error.code}: ${f.error.message}`));
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
    const vk = key.length === 1 ? key.toUpperCase().charCodeAt(0) : { Enter: 13, Escape: 27, ArrowRight: 39, ArrowDown: 40 }[key];
    for (const type of ["rawKeyDown", "keyUp"]) {
      await this.send("Input.dispatchKeyEvent", { type, modifiers, key, code, windowsVirtualKeyCode: vk });
    }
  }
  type(text) {
    return this.send("Input.insertText", { text });
  }
}

const blur = (cdp) => cdp.eval("document.activeElement && document.activeElement.blur && document.activeElement.blur(), true");

async function palette(cdp, text) {
  await blur(cdp);
  await cdp.key("k", { ctrl: true });
  await sleep(300);
  await cdp.type(text);
  await sleep(400);
  await cdp.key("Enter");
  await sleep(700);
}

async function openLibrary(cdp) {
  if (await cdp.eval("!!document.querySelector('.wl')")) return;
  await blur(cdp);
  await cdp.key("L", { ctrl: true, shift: true });
  await waitFor(() => cdp.eval("!!document.querySelector('.wl')"), 5000, "library");
  await sleep(400);
}

/** Closes any dialog windows (settings, editors) and the library. */
async function closeDialogs(cdp) {
  for (let i = 0; i < 3; i++) {
    await cdp.key("Escape");
    await sleep(250);
  }
  // Theme editor: discard confirmation, then its close button.
  await cdp.eval(`(() => { const b = [...document.querySelectorAll('.th-editor__confirm button')].pop(); b?.click(); return true; })()`);
  await cdp.eval(`(() => { document.querySelector('.th-editor button[aria-label]')?.click(); return true; })()`);
  await sleep(300);
}

async function webp(cdp, rect, file) {
  const scale = PREVIEW_WIDTH / rect.width;
  const { data } = await cdp.send("Page.captureScreenshot", {
    format: "webp",
    quality: 62,
    clip: { x: rect.x, y: rect.y, width: rect.width, height: rect.height, scale },
  });
  writeFileSync(file, Buffer.from(data, "base64"));
  return Buffer.byteLength(data, "base64");
}

const rectOf = (cdp, sel) =>
  cdp.eval(`(() => { const el = document.querySelector(${JSON.stringify(sel)}); if (!el) return null;
    const r = el.getBoundingClientRect(); return { x: Math.round(r.x), y: Math.round(r.y), width: Math.round(r.width), height: Math.round(r.height) }; })()`);

async function capturePreview(cdp, entry, lang, mode) {
  await closeDialogs(cdp);
  await openLibrary(cdp);
  const clicked = await cdp.eval(`(() => { const b = document.querySelector('[data-wl-open="${entry.id}"]'); if (!b) return false; b.click(); return true; })()`);
  if (!clicked) throw new Error(`no library card for ${entry.id}`);
  await sleep(1800);
  let rect;
  if (entry.kind === "panel") {
    // Maximize the panel's group so it fills the workspace.
    const ws = await rectOf(cdp, ".app__workspace");
    const group = await rectOf(cdp, ".dv-active-group");
    if (group && ws && group.width * group.height < ws.width * ws.height * 0.8) {
      await cdp.eval(`(() => { const b = [...document.querySelectorAll('.dv-active-group .dock-actions__btn')].pop(); b?.click(); return true; })()`);
      await sleep(900);
    }
    if (entry.id === "code") {
      // Show real code rather than the empty state: Ctrl+P quick open.
      await blur(cdp);
      await cdp.key("p", { ctrl: true });
      await sleep(400);
      await cdp.type("index.mjs");
      await sleep(600);
      await cdp.key("Enter");
      await sleep(1500);
    }
    rect = (await rectOf(cdp, ".dv-active-group")) ?? ws;
  } else {
    rect = { x: 0, y: 0, ...VIEW };
  }
  // 16:10 crop from the top-left (the cards show the top of the window).
  const height = Math.min(rect.height, Math.round(rect.width / 1.6));
  const file = join(previewDir, `${entry.id}.${lang}.${mode}.webp`);
  const bytes = await webp(cdp, { ...rect, height }, file);
  console.log(`${entry.id}.${lang}.${mode}: ${(bytes / 1024).toFixed(1)} KB`);
}

async function setLanguage(rpc, cdp, lang) {
  await rpc.call("settings.update", { patch: { language: lang } });
  await waitFor(() => cdp.eval(`document.documentElement.lang === ${JSON.stringify(lang)}`), 10_000, `language ${lang}`).catch(() => {});
  await sleep(1200);
}

async function setMode(rpc, cdp, mode) {
  await setLanguage(rpc, cdp, "en");
  await closeDialogs(cdp);
  await palette(cdp, `Theme: ${THEMES[mode]}`);
  await sleep(1500);
}

async function libraryShot(cdp, name) {
  await sleep(900);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(shotDir, `${name}.png`), Buffer.from(data, "base64"));
  console.log(`screenshot ${name}.png`);
}

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
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, performanceMode: "Performance", language: "en" } });
  await rpc.call("projects.create", { name: "hello-app", rootPath: repoDir, approvalMode: "SemiAuto" });

  const target = await waitFor(() => findMainTarget(cdpPort), 60_000, "webview DevTools target");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send("Emulation.setDeviceMetricsOverride", { ...VIEW, deviceScaleFactor: 1, mobile: false });
  await cdp.send("Page.reload", { ignoreCache: true });
  await waitFor(() => cdp.eval("!!document.querySelector('.app__body')"), 60_000, "app shell");
  await sleep(2500);
  await cdp.eval(`document.querySelector('[aria-label="Open project hello-app"]')?.click(), true`);
  await sleep(1500);
  await cdp.eval(`document.querySelector('.resume-card button[aria-label*="ismiss" i], .resume-card button[aria-label*="lose" i]')?.click(), true`);
  await sleep(600);

  // Entry list straight from the library DOM (registry order).
  await openLibrary(cdp);
  const entries = await cdp.eval(
    `[...document.querySelectorAll('[data-wl-entry]')].map((el) => ({ id: el.dataset.wlEntry, kind: el.querySelector('[data-wl-open]') && el.querySelector('.wl-card__action:not(.wl-card__action--primary)') ? "panel" : "window" }))`,
  );
  console.log(`${entries.length} entries`);

  if (!libraryOnly) {
    for (const mode of ["dark", "light"]) {
      await setMode(rpc, cdp, mode);
      for (const lang of ["en", "ru"]) {
        await setLanguage(rpc, cdp, lang);
        for (const e of entries) {
          if (only && !only.includes(e.id)) continue;
          await capturePreview(cdp, e, lang, mode);
        }
      }
    }
  }

  if (libraryShots) {
    await setMode(rpc, cdp, "dark");
    for (const lang of ["en", "ru"]) {
      await setLanguage(rpc, cdp, lang);
      await closeDialogs(cdp);
      await openLibrary(cdp);
      await cdp.eval("document.querySelector('.wl-cat--all')?.click(), true");
      await sleep(300);
      await cdp.key("ArrowRight");
      await libraryShot(cdp, `01-library-${lang}`);
      await cdp.type(lang === "en" ? "agent" : "агент");
      await sleep(500);
      await libraryShot(cdp, `02-search-${lang}`);
      await cdp.key("Escape");
      await sleep(300);
      await cdp.key("PageDown");
      await cdp.key("PageDown");
      await cdp.key("PageDown");
      await cdp.key("PageDown");
      await cdp.key("PageDown");
      await libraryShot(cdp, `03-category-code-${lang}`);
    }
    await setMode(rpc, cdp, "light");
    await closeDialogs(cdp);
    await openLibrary(cdp);
    await cdp.eval("document.querySelector('.wl-cat--all')?.click(), true");
    await libraryShot(cdp, "04-library-light-en");
  }
  rpc.close();
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
