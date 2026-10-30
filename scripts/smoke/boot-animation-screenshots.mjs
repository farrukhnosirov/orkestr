#!/usr/bin/env node
// Startup animation frames (wave 10, ADR-0053). Launches the custom-protocol
// desktop build as a second instance (own ORKESTR_HOME + WebView2 profile, no
// personal data), checks the real first start (full version) and a regular
// start (short version) end on their own, then replays both versions through
// the debug hook (`__orkestrDebug.bootPlay` / `.boot.freeze`) and captures
// frame sequences (dark + light, ru; a compact and an English frame) into
// docs/screenshots/boot-animation/. Only the app's own web content is captured.
//
// Usage (repo root, Windows):
//   npm run build && cargo build -p orkestr-desktop --features custom-protocol
//   node scripts/smoke/boot-animation-screenshots.mjs [--out docs/screenshots/boot-animation] [--quick]

import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "boot-animation"));
const quick = process.argv.includes("--quick");
const targetDir = process.env.CARGO_TARGET_DIR ?? join(repo, "target");
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(targetDir, "debug", "orkestr-desktop.exe");
const cdpPort = Number(process.env.ORKESTR_SMOKE_CDP_PORT ?? 9381);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

const checks = [];
function check(id, label, ok, detail = "") {
  checks.push({ id, label, ok: !!ok, detail: String(detail).slice(0, 300) });
  console.log(`${ok ? "PASS" : "FAIL"} ${id} ${label}${detail ? ` — ${String(detail).slice(0, 200)}` : ""}`);
}

const base = mkdtempSync(join(tmpdir(), "orkestr-boot-shots-"));
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
async function shot(cdp, name, note) {
  const { data } = await cdp.send("Page.captureScreenshot", { format: "jpeg", quality: 90 });
  writeFileSync(join(outDir, `${name}.jpg`), Buffer.from(data, "base64"));
  const errors = cdp.console.splice(0);
  shots.push({ name: `${name}.jpg`, note, consoleMessages: errors });
  console.log(`shot ${name}${errors.length ? ` — ${errors.length} console message(s): ${errors[0]}` : ""}`);
}

const OVERLAY = "document.querySelector('[data-testid=boot-overlay]')";

/** Frames: [name, frozen state, note] (animation seconds; `now` = t keeps the spin deterministic). */
const FULL = [
  ["01-grid", { phase: "intro", t: 0.4 }, "grid revealed from the centre, first ring drawing in"],
  ["02-rings", { phase: "intro", t: 0.95 }, "rings draw in (stroke-dash), ticks start lighting clockwise"],
  ["03-ticks", { phase: "intro", t: 1.35 }, "tick ring lighting clockwise, boot log typing"],
  ["04-coils", { phase: "intro", t: 1.8 }, "core ring and coils filling"],
  ["05-ignite", { phase: "intro", t: 2.12 }, "core ignition: bloom + shock wave"],
  ["06-holding", { phase: "holding", t: 2.9, hold: 1.4 }, "core still starting: idle scan, real step statuses"],
  ["07-online", { phase: "online", t: 3.2 }, "all systems nominal, greeting typing"],
  ["08-welcome", { phase: "online", t: 3.95 }, "welcome + time-of-day line"],
  ["09-exit", { phase: "exit", t: 4.3, exitP: 0.35 }, "reactor shrinks and flies to the title-bar logo"],
  ["10-exit", { phase: "exit", t: 4.6, exitP: 0.72 }, "main UI fades in"],
];
const SHORT = [
  ["01-assemble", { phase: "intro", t: 0.22 }, "short version: fast assembly"],
  ["02-ignite", { phase: "intro", t: 0.66 }, "short version: ignition"],
  ["03-welcome", { phase: "online", t: 1.3 }, "short version: greeting"],
  ["04-exit", { phase: "exit", t: 1.65, exitP: 0.45 }, "short version: fly to the title bar"],
];

async function frames(main, variant, list, prefix) {
  await main.eval(`window.__orkestrDebug.bootPlay(null), true`);
  await sleep(150);
  await main.eval(`window.__orkestrDebug.bootPlay(${JSON.stringify(variant)}), true`);
  await waitFor(() => main.eval("!!window.__orkestrDebug?.boot"), 10_000, "boot hook");
  await sleep(900); // steps report
  for (const [name, s, note] of list) {
    await main.eval(`window.__orkestrDebug.boot.freeze(${JSON.stringify(s)}, ${s.t}), true`);
    await sleep(220);
    await shot(main, `${prefix}-${name}`, `${variant}: ${note}`);
  }
  await main.eval(`window.__orkestrDebug.boot.freeze(null), window.__orkestrDebug.bootPlay(null), true`);
  await sleep(300);
}

let exitCode = 0;
try {
  // Real first start: the full version plays while the core starts and ends on its own.
  let main = await waitFor(() => windowOf("main"), 120_000, "main window");
  const firstSeen = await waitFor(() => main.eval(`(document.getElementById("root")?.children.length ?? 0) > 0 && (${OVERLAY}?.className ?? "gone")`), 30_000, "overlay state");
  check("B0", "first start after install: the full version plays", String(firstSeen).includes("boot--full"), firstSeen);
  const t0 = Date.now();
  await waitFor(() => main.eval(`!${OVERLAY}`), 30_000, "overlay ends");
  check("B1", "the full version ends on its own once the core is ready", true, `${Date.now() - t0} ms after attach`);
  check("B2", "the app is usable under it (main body mounted)", await waitFor(() => main.eval("!!document.querySelector('.app__body')"), 60_000, "app body"));

  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  const { settings } = await rpc.call("settings.get", {});
  const darkTheme = settings.themeId && settings.themeId !== "solar-light" ? settings.themeId : "nebula";
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, performanceMode: "Balanced", language: "ru" } });
  await main.eval(`localStorage.setItem("orkestr.debugHooks", "1"), true`);

  // Regular start (reload): the short version, then gone within ~2 s.
  main.send("Page.reload", { ignoreCache: true }).catch(() => undefined);
  await sleep(700);
  main.ws.close();
  main = await waitFor(() => windowOf("main"), 60_000, "main window after reload");
  const second = await waitFor(() => main.eval(`(document.getElementById("root")?.children.length ?? 0) > 0 && (${OVERLAY}?.className ?? "gone")`), 20_000, "overlay after reload");
  check("B3", "a regular start plays the short version (or it already ended)", String(second).includes("boot--short") || second === "gone", second);
  const t1 = Date.now();
  await waitFor(() => main.eval(`!${OVERLAY}`), 10_000, "short overlay ends");
  check("B4", "the short version ends within ~2 s", Date.now() - t1 < 2600, `${Date.now() - t1} ms after attach`);
  await waitFor(() => main.eval("!!window.__orkestrDebug?.bootPlay"), 20_000, "bootPlay hook");

  const setTheme = async (id) => {
    const { themes } = await rpc.call("ui.themes", {});
    const entry = themes.find((e) => (e.theme ?? e).id === id) ?? themes.find((e) => (e.theme ?? e).mode === (id === "solar-light" ? "Light" : "Dark"));
    const { config } = await rpc.call("ui.getConfig", {});
    await rpc.call("ui.setConfig", { config: { ...config, theme: entry.theme ?? entry } });
  };

  for (const [theme, themeId] of quick ? [["dark", darkTheme]] : [["dark", darkTheme], ["light", "solar-light"]]) {
    await setTheme(themeId);
    await sleep(900);
    await frames(main, "full", FULL, `full-${theme}-ru`);
    await frames(main, "short", SHORT, `short-${theme}-ru`);
  }
  await setTheme(darkTheme);
  await sleep(600);
  await frames(main, "static", [["01-static", { phase: "online", t: 1 }, "reduced motion / Performance mode: static version"]], "static-dark-ru");

  // Compact (Assistant-sized window).
  await main.send("Emulation.setDeviceMetricsOverride", { width: 440, height: 560, deviceScaleFactor: 1, mobile: false });
  await sleep(400);
  await frames(main, "short", [["01-compact", { phase: "online", t: 1.3 }, "compact layout (Assistant mode / small window)"]], "compact-dark-ru");
  await main.send("Emulation.clearDeviceMetricsOverride");

  // English.
  await rpc.call("settings.update", { patch: { language: "en" } });
  await sleep(800);
  await frames(main, "full", [["08-welcome", FULL[7][1], "English greeting"]], "full-dark-en");
  await rpc.call("settings.update", { patch: { language: "ru" } });

  check("B5", "no console errors while capturing", shots.every((s) => s.consoleMessages.length === 0), shots.flatMap((s) => s.consoleMessages).slice(0, 3).join(" | "));
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
