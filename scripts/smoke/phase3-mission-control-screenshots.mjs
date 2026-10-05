#!/usr/bin/env node
// Phase 3 (mission-control) UI review: starts a headless core (`orkestrd`)
// in an isolated data dir, seeds it over RPC (project, a finished Manager
// run, a pending plan, a budget, a terminal), serves the built frontend with
// a small static server in browser-dev mode (`?core=…&token=…`) and drives it in a
// headless Edge through the DevTools protocol. Captures are DevTools captures
// of the page only, so no other application's content can leak into the PNGs.
//
// Ports are configurable so it can run next to other worktrees' dev servers.
//
// Usage (repo root, Windows):
//   npm run build
//   cargo build -p orkestr-core --bin orkestrd
//   node scripts/smoke/phase3-mission-control-screenshots.mjs [--out docs/screenshots/phase3-mission-control]

import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const arg = (name, dflt) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : dflt;
};
const outDir = resolve(repo, arg("--out", join("docs", "screenshots", "phase3-mission-control")));
const webPort = Number(arg("--web-port", "1477"));
const cdpPort = Number(arg("--cdp-port", "9377"));
const coreExe = process.env.ORKESTRD_EXE ?? join(repo, "target", "debug", "orkestrd.exe");
const edge = process.env.EDGE_EXE ?? "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

const base = mkdtempSync(join(tmpdir(), "orkestr-mc-shots-"));
const home = join(base, "home");
const repoDir = join(base, "hello-app");
mkdirSync(home, { recursive: true });
mkdirSync(join(repoDir, "src"), { recursive: true });
const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();
git(repoDir, "init", "-q", "-b", "main");
git(repoDir, "config", "user.email", "demo@orkestr.local");
git(repoDir, "config", "user.name", "Orkestr Demo");
writeFileSync(join(repoDir, "README.md"), "# hello-app\n");
writeFileSync(join(repoDir, "src", "index.mjs"), "export const hello = () => 'hi';\n");
git(repoDir, "add", "-A");
git(repoDir, "commit", "-q", "-m", "initial");

const procs = [];
const servers = [];
const launch = (cmd, args, opts = {}) => {
  const p = spawn(cmd, args, { stdio: ["ignore", "pipe", "pipe"], ...opts });
  let log = "";
  p.stdout?.on("data", (d) => (log += d));
  p.stderr?.on("data", (d) => (log += d));
  procs.push(p);
  return { p, log: () => log };
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
    await sleep(200);
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
      await this.send("Input.dispatchKeyEvent", { type, modifiers, key: shift && key.length === 1 ? key.toUpperCase() : key, code, windowsVirtualKeyCode: vk });
    }
  }
  async type(text) {
    await this.send("Input.insertText", { text });
  }
}

const shots = [];
async function capture(cdp, name, note) {
  await sleep(1500);
  const file = join(outDir, `${name}.png`);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(file, Buffer.from(data, "base64"));
  const errors = cdp.console.splice(0);
  shots.push({ name, note, errors });
  console.log(`${name}${errors.length ? `  — ${errors.length} console error(s)` : ""}`);
  for (const e of errors) console.log(`    ${e.slice(0, 300)}`);
}

async function palette(cdp, text) {
  await cdp.eval("document.activeElement && document.activeElement.blur && document.activeElement.blur(), true");
  await cdp.key("k", { ctrl: true });
  await sleep(300);
  await cdp.type(text);
  await sleep(400);
  await cdp.key("Enter");
  await sleep(700);
}

let exitCode = 0;
try {
  if (!existsSync(coreExe)) throw new Error(`missing ${coreExe} (cargo build -p orkestr-core --bin orkestrd)`);
  const core = launch(coreExe, ["--data-dir", home], { env: { ...process.env, ORKESTR_HOME: home, ORKESTR_SECRET_BACKEND: "vault-file" } });
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 90_000, "core.json").catch((e) => {
    console.error(core.log().slice(-2000));
    throw e;
  });
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.sub(["*"]);

  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, language: "en" } });
  const providers = (await rpc.call("providers.list")).providers.map((p) => p.provider ?? p);
  const mock = providers.find((p) => p.kind === "Mock");
  const smart = { providerId: mock.id, model: "mock-smart" };
  await rpc.call("providers.setManager", { config: { model: smart, maxParallelAgents: 2, approvalPolicy: "SemiAuto" } });
  await rpc.call("providers.setRouting", {
    config: { mode: "Manual", rules: [], defaultFallback: [smart], preferLocal: true, maxRateLimitWaitSecs: 0 },
  });
  const { project } = await rpc.call("projects.create", { name: "hello-app", rootPath: repoDir, approvalMode: "SemiAuto" });
  const projectId = project.id;
  await rpc.call("projects.create", { name: "billing-service", rootPath: join(base, "billing"), approvalMode: "Ask" }).catch(() => null);
  const chat = async (text) => {
    const sent = await rpc.call("manager.chat", { projectId, text });
    return waitFor(
      () => rpc.events.find((e) => e.kind === "ManagerMessageAdded" && e.payload.message.id === sent.replyMessageId)?.payload.message,
      60_000,
      "manager reply",
    );
  };
  const r1 = await chat("Create a hello module with a unit test");
  const ap = await rpc.call("manager.approvePlan", { planId: r1.planId });
  await waitFor(
    async () => ["Completed", "Failed", "Cancelled"].includes((await rpc.call("manager.status", { runId: ap.run.id })).runs[0].status),
    120_000,
    "run",
  ).catch(() => console.warn("run still going — capturing it live"));
  await chat("Create a greeter module with tests");
  await rpc.call("budgets.set", { name: "Monthly cap", scope: { scope: "Global" }, period: "Monthly", limitUsd: 25, warnAt: 0.8 }).catch((e) => console.warn("budget", e.message));
  await rpc.call("budgets.set", { name: "hello-app daily", scope: { scope: "Project", projectId }, period: "Daily", limitUsd: 2, warnAt: 0.8 }).catch((e) => console.warn("budget", e.message));
  await rpc.call("terminal.open", { projectId, title: "dev shell", cols: 100, rows: 30 }).catch((e) => console.warn("terminal", e.message));
  rpc.close();
  console.log("seeded");

  // Serve the built frontend.
  // Serve the built frontend and bridge the RPC WebSocket. The core only
  // accepts the Tauri / Vite-dev origins, so the bridge (test harness only)
  // forwards the handshake with the dev origin; the core itself is unchanged.
  const coreUrl = new URL(info.url);
  const dist = join(repo, "apps", "desktop", "dist");
  const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".json": "application/json", ".woff2": "font/woff2", ".wasm": "application/wasm" };
  const server = createServer((req, res) => {
    const path = decodeURIComponent(new URL(req.url, "http://x").pathname);
    let file = join(dist, path === "/" ? "index.html" : path);
    if (!file.startsWith(dist) || !existsSync(file) || statSync(file).isDirectory()) file = join(dist, "index.html");
    res.writeHead(200, { "content-type": types[extname(file)] ?? "application/octet-stream" });
    res.end(readFileSync(file));
  });
  server.on("upgrade", (req, socket, head) => {
    const upstream = connect(Number(coreUrl.port), coreUrl.hostname, () => {
      const headers = Object.entries(req.headers)
        .filter(([k]) => !["origin", "host"].includes(k))
        .map(([k, v]) => `${k}: ${v}`);
      upstream.write(`GET ${coreUrl.pathname}${coreUrl.search} HTTP/1.1\r\nhost: ${coreUrl.host}\r\norigin: http://127.0.0.1:1420\r\n${headers.join("\r\n")}\r\n\r\n`);
      if (head?.length) upstream.write(head);
      upstream.pipe(socket);
      socket.pipe(upstream);
    });
    upstream.on("error", () => socket.destroy());
    socket.on("error", () => upstream.destroy());
  });
  await new Promise((r) => server.listen(webPort, "127.0.0.1", r));
  servers.push(server);
  const bridged = `ws://127.0.0.1:${webPort}${coreUrl.pathname}`;
  const url = `http://127.0.0.1:${webPort}/?core=${encodeURIComponent(bridged)}&token=${encodeURIComponent(token)}`;
  launch(edge, [
    "--headless=new",
    `--remote-debugging-port=${cdpPort}`,
    `--user-data-dir=${join(base, "edge")}`,
    "--window-size=1600,1000",
    "--force-device-scale-factor=1",
    "--no-first-run",
    "--disable-extensions",
    "about:blank",
  ]);
  const target = await waitFor(async () => {
    const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
    return list.find((t) => t.type === "page");
  }, 30_000, "edge DevTools target");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: 1600, height: 1000, deviceScaleFactor: 1, mobile: false });
  await cdp.send("Page.navigate", { url });
  await waitFor(() => cdp.eval("!!document.querySelector('.app__body')"), 60_000, "app shell").catch(async (e) => {
    console.error(await cdp.eval("document.body.innerText.slice(0, 800)").catch(() => ""));
    console.error(cdp.console.join(" | ").slice(0, 2000));
    throw e;
  });
  await sleep(2500);
  await cdp.eval(`document.querySelector('[aria-label="Open project hello-app"]')?.click(), true`);
  await sleep(1500);
  await sleep(1500);
  await cdp.eval(`[...document.querySelectorAll('.resume-card button')].find((b) => /dismiss|close|закрыть|скрыть/i.test(b.getAttribute('aria-label') ?? b.textContent ?? ''))?.click(), true`);

  await cdp.key("d", { ctrl: true, shift: true });
  await sleep(4000);
  await capture(cdp, "01-mission-control-fullscreen", "Mission Control full screen (Ctrl+Shift+D)");
  await cdp.eval(`[...document.querySelectorAll('.mc-overlay button')].find((b) => b.textContent.trim() === 'Customize')?.click(), true`);
  await sleep(600);
  await capture(cdp, "02-mission-control-customize", "Customize mode: drag handles, resize/hide controls");
  await cdp.eval(`[...document.querySelectorAll('.mc-overlay button')].find((b) => b.textContent.trim() === 'Done')?.click(), true`);
  await cdp.key("Escape");
  await sleep(600);

  await palette(cdp, "Open Panel: Monitoring");
  await palette(cdp, "Toggle Maximize Panel");
  await sleep(6000);
  await capture(cdp, "03-monitoring", "Monitoring panel (maximized): resources, core services, processes, error tail");
  await palette(cdp, "Toggle Maximize Panel");

  await palette(cdp, "Open Panel: Mission Control");
  await sleep(1500);
  await capture(cdp, "04-mission-control-docked", "Mission Control docked as a panel in the Coding layout");

  // Russian
  const rpc2 = rpcClient(info.url, token);
  await rpc2.open();
  await rpc2.call("settings.update", { patch: { language: "ru" } });
  rpc2.close();
  await sleep(1500);
  await cdp.key("d", { ctrl: true, shift: true });
  await sleep(3000);
  await capture(cdp, "05-mission-control-ru", "Mission Control in Russian");
  await cdp.key("Escape");

  writeFileSync(join(outDir, "manifest.json"), `${JSON.stringify(shots.map(({ name, note, errors }) => ({ name: `${name}.png`, note, consoleErrors: errors })), null, 2)}\n`);
} catch (e) {
  exitCode = 1;
  console.error(e.stack ?? e);
} finally {
  for (const srv of servers) srv.close();
  for (const p of procs.reverse()) {
    try {
      execFileSync("taskkill", ["/PID", String(p.pid), "/T", "/F"], { stdio: "ignore" });
    } catch {
      /* gone */
    }
  }
  await sleep(1000);
  try {
    rmSync(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  } catch {
    /* ignore */
  }
  process.exit(exitCode);
}
