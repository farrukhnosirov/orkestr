#!/usr/bin/env node
// Codex (ChatGPT subscription) panel + parallel-engines settings in the real
// desktop app (bundled frontend: `npm run build -w @orkestr/desktop` +
// `cargo build -p orkestr-desktop --features custom-protocol`), driven by the
// scripted `orkestr-fake-codex` (default: no account data, no usage spent) or
// a real CLI (`--codex <path>`), English + Russian (ADR-0047).
//
// Usage (repo root, Windows):
//   node scripts/smoke/codex-screenshots.mjs [--codex <path>] [--out docs/screenshots/codex]

import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const arg = (name, dflt) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : dflt;
};
const target = process.env.CARGO_TARGET_DIR ?? join(repo, "target");
const outDir = resolve(repo, arg("--out", join("docs", "screenshots", "codex")));
const codex = arg("--codex", join(target, "debug", "orkestr-fake-codex.exe"));
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(target, "debug", "orkestr-desktop.exe");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
if (!existsSync(codex) || !existsSync(exe)) {
  console.error(`missing ${existsSync(codex) ? exe : codex}`);
  process.exit(2);
}
mkdirSync(outDir, { recursive: true });
const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok: Boolean(ok) });
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
    call: (method, params = {}) => frame({ type: "req", id: `r${++seq}`, method, params }, `r${seq}`),
  };
}
class Cdp {
  constructor(u) {
    this.ws = new WebSocket(u);
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
      const p = f.id && this.pending.get(f.id);
      if (p) {
        this.pending.delete(f.id);
        f.error ? p.reject(new Error(f.error.message)) : p.resolve(f.result);
      }
    };
    await this.send("Runtime.enable");
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
  async key(key, { ctrl = false } = {}) {
    const code = key.length === 1 ? `Key${key.toUpperCase()}` : key;
    const vk = key.length === 1 ? key.toUpperCase().charCodeAt(0) : { Enter: 13, Escape: 27 }[key];
    for (const type of ["rawKeyDown", "keyUp"]) {
      await this.send("Input.dispatchKeyEvent", { type, modifiers: ctrl ? 2 : 0, key, code, windowsVirtualKeyCode: vk });
    }
  }
}
const q = (s) => JSON.stringify(s);
const click = (cdp, selector, text) =>
  cdp.eval(`(() => {
    const els = [...document.querySelectorAll(${q(selector)})].filter((e) => e.getClientRects().length);
    const t = ${q(text ?? null)};
    const el = t === null ? els[0] : els.find((e) => e.textContent.trim().includes(t));
    if (!el) return false;
    el.scrollIntoView({ block: 'center' });
    el.click();
    return true;
  })()`);
async function shot(cdp, name) {
  await sleep(800);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(outDir, `${name}.png`), Buffer.from(data, "base64"));
  console.log(`  screenshot ${name}.png`);
}
async function palette(cdp, text) {
  await cdp.eval("document.activeElement?.blur?.(), true");
  await cdp.key("k", { ctrl: true });
  await sleep(600);
  await cdp.send("Input.insertText", { text });
  await sleep(800);
  await cdp.key("Enter");
  await sleep(1500);
}
const maximize = (cdp, sel) =>
  cdp.eval(`(() => {
    let el = document.querySelector(${q(sel)});
    while (el && !el.querySelector?.('[aria-label="Maximize panel"], [aria-label="Развернуть панель"]')) el = el.parentElement;
    el?.querySelector('[aria-label="Maximize panel"], [aria-label="Развернуть панель"]')?.click();
    return true;
  })()`);

let child = null;
let code = 0;
try {
  const base = mkdtempSync(join(tmpdir(), "orkestr-codex-"));
  const home = join(base, "home");
  const proj = join(base, "shop-api");
  mkdirSync(home, { recursive: true });
  mkdirSync(proj, { recursive: true });
  writeFileSync(join(proj, "README.md"), "# shop-api\n\nA tiny demo service.\n");
  execFileSync("git", ["init", "-q"], { cwd: proj });
  const cdpPort = 9367;
  const env = {
    ...process.env,
    ORKESTR_HOME: home,
    ORKESTR_SECRET_BACKEND: "vault-file",
    ORKESTR_ALLOW_SECOND_INSTANCE: "1",
    WEBVIEW2_USER_DATA_FOLDER: join(base, "webview2"),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${cdpPort}`,
  };
  child = spawn(exe, [], { env, stdio: ["ignore", "ignore", "ignore"] });
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 120_000, "core.json");
  const rpc = rpcClient(info.url, readFileSync(join(home, "core.token"), "utf8").trim());
  await rpc.open();
  await rpc.call("settings.update", { patch: { onboardingCompleted: true, language: "en" } });
  const { project } = await rpc.call("projects.create", { name: "shop-api", rootPath: proj });
  await rpc.call("projects.open", { projectId: project.id });
  const list = await rpc.call("providers.list", {});
  const existing = list.providers.find((p) => p.provider.kind === "Codex");
  if (existing) await rpc.call("providers.update", { providerId: existing.provider.id, patch: { binaryPath: codex } });
  else await rpc.call("providers.create", { kind: "Codex", name: "Codex", binaryPath: codex });
  const st = await rpc.call("codex.status", { refresh: true });
  check("codex.status", st.binary && st.loggedIn, `${st.binary?.version} · ${st.account?.plan ?? "?"}`);
  const lim = await rpc.call("codex.limits", { refresh: true });
  check("codex.limits", lim.available, `${lim.primary?.percent}% / ${lim.secondary?.percent}%`);

  const tgt = await waitFor(async () => {
    const l = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
    return l.find((t) => t.type === "page" && !t.url.includes("robot.html") && !t.url.includes("#/") && !/hud/i.test(t.url));
  }, 60_000, "webview target");
  const cdp = new Cdp(tgt.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: 1600, height: 960, deviceScaleFactor: 1, mobile: false });
  for (const lang of ["en", "ru"]) {
    await rpc.call("settings.update", { patch: { language: lang } });
    await cdp.eval("location.reload(), true");
    await sleep(6000);
    await click(cdp, "button", lang === "en" ? "Skip setup" : "Пропустить");
    await palette(cdp, "Codex");
    await waitFor(() => cdp.eval("!!document.querySelector('[data-testid=codex-panel]')"), 20_000, "codex panel");
    await maximize(cdp, "[data-testid=codex-panel]");
    await sleep(1500);
    await shot(cdp, `${lang}-01-codex-panel`);
    if (await click(cdp, "[data-testid=codex-new-chat]")) {
      await sleep(2500);
      await cdp.eval(`(() => { const i = document.querySelector('[data-testid=codex-panel] textarea'); i?.focus(); return !!i; })()`);
      await cdp.send("Input.insertText", { text: lang === "en" ? "Please write the hello file" : "Пожалуйста, write файл hello" });
      await cdp.key("Enter");
      const ok = await waitFor(() => cdp.eval("!!document.querySelector('[data-testid=codex-approval]')"), 30_000, "approval card").catch(() => false);
      check(`${lang}: approval card`, ok);
      await shot(cdp, `${lang}-02-codex-approval`);
      await click(cdp, "[data-testid=codex-approval-allow]");
      await sleep(2500);
      await click(cdp, "[data-testid=codex-limits-meter]");
      await shot(cdp, `${lang}-03-codex-chat-limits`);
      await cdp.key("Escape");
    }
    await palette(cdp, lang === "en" ? "providers" : "провайдер");
    const card = await waitFor(() => cdp.eval("!!document.querySelector('[data-testid=engines-card]')"), 20_000, "engines card").catch(() => false);
    check(`${lang}: engines card`, card);
    await cdp.eval("document.querySelector('[data-testid=codex-engine-card]')?.scrollIntoView({block:'start'}), true");
    await shot(cdp, `${lang}-04-codex-subscription-card`);
    await cdp.eval("document.querySelector('[data-testid=engines-card]')?.scrollIntoView({block:'start'}), true");
    await click(cdp, "[data-testid=engines-mode-split]");
    await shot(cdp, `${lang}-05-engines-settings`);
  }
} catch (e) {
  console.error(e);
  code = 1;
} finally {
  if (child) {
    try {
      execFileSync("taskkill", ["/F", "/T", "/PID", String(child.pid)], { stdio: "ignore" });
    } catch {
      /* gone */
    }
  }
}
const failed = results.filter((r) => !r.ok).length;
console.log(`${results.length - failed}/${results.length} checks passed`);
process.exit(code || (failed ? 1 : 0));
