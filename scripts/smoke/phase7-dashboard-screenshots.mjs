#!/usr/bin/env node
// Phase 7 packaged-app walkthrough (infrastructure from phase3-*-screenshots.mjs):
// launches the custom-protocol desktop build with an isolated ORKESTR_HOME and
// the dev publisher key trusted, then drives the real UI through the WebView2
// DevTools protocol:
//
//   W1 Marketplace → Plugins: install hello-widget.orkp (signed fixture) and a
//      signed "Projects widget" (queries projects.list, probes the Tauri IPC)
//      from file, enable them; install hello-tool.orkp → permission prompt →
//      grant + enable; logs viewer; safe mode toggle; kill-all confirmation
//   W2 Dashboards: create a dashboard, add both plugin widgets + two built-ins
//   W3 the plugin widget iframe (sandbox="allow-scripts", orkestr-widget
//      scheme) rendered the project names it got from its projects.list query
//   W4 inside the widget frame the Tauri IPC is unusable (invoke gets no answer
//      / is refused) and the parent DOM is unreachable
//   W5 the dashboard is saved (dashboards.list) and the layout survives a reload
//   W6 Marketplace → Publish: sign + publish a package to a folder registry,
//      local publisher key fingerprint shown
//   screenshots (en + ru) of Dashboards, Plugins management and Publish into
//   docs/screenshots/phase7/ + manifest.json with the PASS/FAIL list.
//
// Usage (repo root, Windows) — no Vite needed, the frontend is bundled:
//   npm run build
//   cargo build -p orkestr-desktop --features custom-protocol
//   cargo build -p orkestr-plugins --example make-fixtures   (optional; else `cargo run`)
//   node scripts/smoke/phase7-dashboard-screenshots.mjs [--out docs/screenshots/phase7]
// ORKESTR_SMOKE_EXE overrides the executable (default: $CARGO_TARGET_DIR or
// ./target, debug), ORKESTR_SMOKE_CDP_PORT the DevTools port.

import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { buildPackage, widgetManifest } from "./fixtures/phase7/orkp.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const outArg = process.argv.indexOf("--out");
const outDir = resolve(repo, outArg > 0 ? process.argv[outArg + 1] : join("docs", "screenshots", "phase7"));
const targetDir = process.env.CARGO_TARGET_DIR ?? join(repo, "target");
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(targetDir, "debug", "orkestr-desktop.exe");
const cdpPort = Number(process.env.ORKESTR_SMOKE_CDP_PORT ?? 9357);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

const locale = (lang, ns) => JSON.parse(readFileSync(join(repo, "apps", "desktop", "src", "locales", lang, `${ns}.json`), "utf8"));
const L = {
  en: { shell: locale("en", "shell"), mp: locale("en", "marketplace"), dsh: locale("en", "dashboards") },
  ru: { shell: locale("ru", "shell"), mp: locale("ru", "marketplace"), dsh: locale("ru", "dashboards") },
};
L.en.layout = locale("en", "layout");
L.ru.layout = locale("ru", "layout");
const openPanelCmd = (lang, panel) => L[lang].shell.palette.openPanel.replace("{{title}}", L[lang].shell.panels[panel].title);

// ── checks ───────────────────────────────────────────────────────────────
const checks = [];
function check(id, label, ok, detail = "") {
  checks.push({ id, label, ok: !!ok, detail: String(detail).slice(0, 400) });
  console.log(`${ok ? "PASS" : "FAIL"} ${id} ${label}${detail ? ` — ${String(detail).slice(0, 200)}` : ""}`);
}

// ── temp dirs, fixtures, project ─────────────────────────────────────────
const base = mkdtempSync(join(tmpdir(), "orkestr-p7-shots-"));
const home = join(base, "home");
const fixtures = join(base, "fixtures");
const registry = join(base, "registry");
mkdirSync(registry, { recursive: true });
mkdirSync(home, { recursive: true });
const projects = ["hello-app", "billing-api"].map((name) => {
  const dir = join(base, name);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "README.md"), `# ${name}\n`);
  return { name, dir };
});

const fxBin = join(targetDir, "debug", "examples", "make-fixtures.exe");
if (existsSync(fxBin)) execFileSync(fxBin, [fixtures], { stdio: "ignore" });
else execFileSync("cargo", ["run", "-q", "-p", "orkestr-plugins", "--example", "make-fixtures", "--", fixtures], { cwd: repo, stdio: "ignore" });
const devPub = readFileSync(join(fixtures, "dev-publisher.pub"), "utf8").trim();

// A signed widget that renders the result of its projects.list query and
// probes the Tauri IPC from inside the sandboxed frame.
const PROJECTS_WIDGET_ID = "orkestr-dev.projects-widget";
const PROJECTS_WIDGET_HTML = `<!doctype html><html><head><style>
html,body{height:100%;box-sizing:border-box;background:var(--ok-color-surface,#151726)}body{margin:0;padding:10px 12px;font:13px/1.45 var(--ok-font-sans,system-ui,sans-serif);color:var(--ok-color-text,#e6e6f0)}
h4{margin:0 0 6px;font-size:13px;color:var(--ok-color-text-muted,#9aa)}ul{margin:0;padding-left:18px}li{color:var(--ok-color-text,#e6e6f0);font-weight:600}
#ipc{margin-top:8px;font:11px var(--ok-font-mono,monospace);color:var(--ok-color-text-subtle,#889)}
</style></head><body>
<h4 id="t">Projects (projects.list)</h4><ul id="list"><li>loading…</li></ul><p id="ipc" data-result="pending">ipc: probing…</p>
<script>
(function(){
var root=document.documentElement,list=document.getElementById("list"),ipc=document.getElementById("ipc");
function theme(t){for(var k in t){root.style.setProperty("--ok-"+k,t[k]);}}
window.addEventListener("message",function(e){var d=e.data||{};
 if(d.type==="init"){theme(d.theme||{});parent.postMessage({type:"query",reqId:"q1",method:"projects.list",params:{}},"*");}
 else if(d.type==="theme"){theme(d.theme||{});}
 else if(d.type==="data"&&d.bindingId==="q1"){var ps=(d.result&&d.result.projects)||[];list.innerHTML="";
   ps.forEach(function(p){var li=document.createElement("li");li.textContent=p.name;list.appendChild(li);});
   document.body.setAttribute("data-rendered",String(ps.length));}
 else if(d.type==="error"){list.innerHTML="";var li=document.createElement("li");li.textContent="error: "+(d.error&&d.error.message);list.appendChild(li);}
});
parent.postMessage({type:"ready"},"*");
var parentDom;try{parentDom=!!window.parent.document.body;}catch(err){parentDom=false;}
ipc.setAttribute("data-parent-dom",String(parentDom));
function done(r,msg){if(ipc.getAttribute("data-result")!=="pending")return;ipc.setAttribute("data-result",r);ipc.textContent="ipc: "+msg;}
var ti=window.__TAURI_INTERNALS__;
ipc.setAttribute("data-internals",String(!!ti));
if(!ti||typeof ti.invoke!=="function"){done("blocked","unavailable");return;}
try{Promise.resolve(ti.invoke("core_connection")).then(function(){done("answered","ANSWERED (IPC reachable!)");},function(err){done("blocked","refused ("+String(err).slice(0,60)+")");});}
catch(err){done("blocked","threw ("+String(err).slice(0,60)+")");}
setTimeout(function(){done("blocked","blocked (invoke got no answer)");},5000);
})();
</script></body></html>`;
const projectsWidgetPath = join(fixtures, "projects-widget.orkp");
{
  const m = widgetManifest(PROJECTS_WIDGET_ID, "Projects widget", ["projects.list"]);
  m.widgets[0].sizes = [{ w: 4, h: 3 }];
  m.description = "Lists projects from its projects.list query (Phase 7 walkthrough).";
  writeFileSync(projectsWidgetPath, buildPackage(m, { "widgets/w.html": PROJECTS_WIDGET_HTML }));
}

// ── launch ───────────────────────────────────────────────────────────────
function running() {
  try {
    return execFileSync("tasklist", ["/FI", "IMAGENAME eq orkestr-desktop.exe", "/NH"], { encoding: "utf8" }).includes("orkestr-desktop.exe");
  } catch {
    return false;
  }
}
const preexisting = running();
const child = spawn(exe, [], {
  env: {
    ...process.env,
    ORKESTR_HOME: home,
    ORKESTR_SECRET_BACKEND: "vault-file",
    ORKESTR_PLUGINS_TRUSTED_KEYS: devPub,
    // Debug builds: run next to a running Orkestr (own home + own WebView2
    // profile, so the DevTools port below is honoured).
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

// ── RPC ──────────────────────────────────────────────────────────────────
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

// ── CDP ──────────────────────────────────────────────────────────────────
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
  send(method, params = {}, sessionId) {
    const id = ++this.id;
    return new Promise((res, rej) => {
      this.pending.set(id, { resolve: res, reject: rej });
      this.ws.send(JSON.stringify(sessionId ? { id, method, params, sessionId } : { id, method, params }));
    });
  }
  async eval(expr, contextId) {
    const r = await this.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true, ...(contextId ? { contextId } : {}) });
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
  async type(text) {
    await this.send("Input.insertText", { text });
  }
}

const shots = [];
async function capture(cdp, name, note) {
  await cdp.eval(`document.querySelector(".resume-card__head button")?.click(), true`);
  await sleep(1000);
  const file = join(outDir, `${name}.png`);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(file, Buffer.from(data, "base64"));
  const errors = cdp.console.splice(0);
  shots.push({ name, note, errors });
  console.log(`shot ${name}${errors.length ? `  — ${errors.length} console message(s)` : ""}`);
}

async function palette(cdp, text) {
  await cdp.eval("document.activeElement && document.activeElement.blur && document.activeElement.blur(), true");
  await cdp.key("k", { ctrl: true });
  await sleep(350);
  await cdp.type(text);
  await sleep(450);
  await cdp.key("Enter");
  await sleep(900);
}

const J = JSON.stringify;
/** Clicks the first visible element matching `selector` whose text or aria-label equals / contains `text`. */
const click = (cdp, selector, text, exact = false) =>
  cdp.eval(`(() => {
    const want = ${J(text)};
    const els = [...document.querySelectorAll(${J(selector)})].filter((e) => e.offsetParent !== null || e.getClientRects().length);
    const match = (s) => (${exact} ? s === want : s.includes(want));
    const el = els.find((e) => match((e.getAttribute("aria-label") ?? "").trim())) ?? els.find((e) => match(e.textContent.trim()));
    if (!el) return false;
    el.scrollIntoView({ block: "center" });
    el.click();
    return true;
  })()`);
/** Focuses an input by its accessible label and replaces its value via typing. */
const fill = async (cdp, labelText, value) => {
  const ok = await cdp.eval(`(() => {
    const label = [...document.querySelectorAll("label")].find((l) => l.textContent.trim().startsWith(${J(labelText)}));
    const input = label && (label.control ?? document.getElementById(label.htmlFor));
    if (!input) return false;
    input.focus();
    input.select && input.select();
    return true;
  })()`);
  if (!ok) return false;
  await cdp.type(value);
  await sleep(200);
  return true;
};
/** Sets a <select> (found by its label text) through React's value setter. */
const choose = (cdp, labelText, value) =>
  cdp.eval(`(() => {
    const label = [...document.querySelectorAll("label")].find((l) => l.textContent.trim().startsWith(${J(labelText)}));
    const sel = label && label.querySelector("select");
    if (!sel) return false;
    Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(sel, ${J(value)});
    sel.dispatchEvent(new Event("change", { bubbles: true }));
    return true;
  })()`);
const text = (cdp, selector) => cdp.eval(`[...document.querySelectorAll(${J(selector)})].map((e) => e.textContent.trim()).join(" | ")`);

/** Reads the DOM of every widget frame (same-process via an isolated world, OOPIF via an attached session). */
async function readWidgetFrames(cdp) {
  const expr = `JSON.stringify({ url: location.href, text: document.body ? document.body.innerText : "", rendered: document.body && document.body.getAttribute("data-rendered"), items: [...document.querySelectorAll("#list li")].map((l) => l.textContent), ipc: (() => { const p = document.getElementById("ipc"); return p ? { result: p.getAttribute("data-result"), text: p.textContent, internals: p.getAttribute("data-internals"), parentDom: p.getAttribute("data-parent-dom") } : null; })() })`;
  const out = [];
  const { frameTree } = await cdp.send("Page.getFrameTree");
  const walk = (n, acc) => {
    for (const c of n.childFrames ?? []) {
      acc.push(c.frame);
      walk(c, acc);
    }
    return acc;
  };
  for (const f of walk(frameTree, [])) {
    try {
      const { executionContextId } = await cdp.send("Page.createIsolatedWorld", { frameId: f.id, worldName: "p7-walkthrough" });
      out.push({ via: "isolated-world", frameUrl: f.url, ...JSON.parse(await cdp.eval(expr, executionContextId)) });
    } catch {
      /* out-of-process frame: handled below */
    }
  }
  if (out.length === 0) {
    const { targetInfos } = await cdp.send("Target.getTargets");
    for (const t of targetInfos.filter((x) => x.type === "iframe")) {
      try {
        const { sessionId } = await cdp.send("Target.attachToTarget", { targetId: t.targetId, flatten: true });
        const r = await cdp.send("Runtime.evaluate", { expression: expr, returnByValue: true }, sessionId);
        out.push({ via: "oopif-session", frameUrl: t.url, ...JSON.parse(r.result.value) });
      } catch {
        /* ignore */
      }
    }
  }
  return out;
}

/** Opens a panel: its dock tab when present (pointer events), else the command palette. */
const PANEL_ROOT = { marketplace: ".mp-panel", dashboards: ".dsh" };
async function openPanel(cdp, lang, panel) {
  const title = L[lang].shell.panels[panel].title;
  const mounted = () => cdp.eval(`(() => { const el = document.querySelector(${J(PANEL_ROOT[panel])}); return !!el && el.getClientRects().length > 0; })()`);
  await cdp.eval(`(() => {
    const el = [...document.querySelectorAll('[role="tab"]')].find((t) => t.textContent.trim() === ${J(title)} && !t.closest(".mp-panel"));
    if (!el) return false;
    const r = el.getBoundingClientRect();
    const o = { bubbles: true, cancelable: true, clientX: r.x + r.width / 2, clientY: r.y + r.height / 2, button: 0, pointerId: 1, isPrimary: true };
    el.dispatchEvent(new PointerEvent("pointerdown", o));
    el.dispatchEvent(new MouseEvent("mousedown", o));
    el.dispatchEvent(new PointerEvent("pointerup", o));
    el.dispatchEvent(new MouseEvent("mouseup", o));
    el.click();
    return true;
  })()`);
  await sleep(600);
  if (!(await mounted())) await palette(cdp, openPanelCmd(lang, panel));
  await waitFor(mounted, 10_000, `${panel} panel`).catch(() => null);
  await sleep(400);
}

/** Maximizes (`on`) or restores the dock group that contains `selector` (closest group header button). */
const maximize = (cdp, lang, selector, on) =>
  cdp.eval(`(() => {
    const want = ${J(L[lang].layout.actions.maximize)}, back = ${J(L[lang].layout.actions.restore)};
    let g = document.querySelector(${J(selector)});
    while (g && g !== document.body) {
      const b = [...g.querySelectorAll("button")].find((x) => [want, back].includes(x.getAttribute("aria-label")));
      if (b) {
        if ((b.getAttribute("aria-label") === want) === ${on ? "true" : "false"}) b.click();
        return true;
      }
      g = g.parentElement;
    }
    return false;
  })()`);

const dismissNotice = (cdp) => cdp.eval(`document.querySelector(".mp-notice__close")?.click(), true`);

async function openPlugins(cdp, lang) {
  return waitFor(async () => {
    if (!(await cdp.eval(`!!document.querySelector(".mp-panel")`))) await openPanel(cdp, lang, "marketplace");
    await click(cdp, ".mp-panel [role=tab]", L[lang].mp.tabs.plugins);
    await sleep(400);
    return cdp.eval(`!!document.querySelector(".pl-manager")`);
  }, 30_000, "plugins tab").catch(() => false);
}

let exitCode = 0;
let closedUserApp = false;
let cdpRef = null;
try {
  let info;
  try {
    info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 60_000, "core.json");
  } catch (e) {
    if (!preexisting) throw e;
    // Single-instance: an already running Orkestr swallowed our launch.
    console.log("another orkestr-desktop is running (single instance) — closing it and relaunching");
    execFileSync("powershell", ["-NoProfile", "-Command", "Stop-Process -Name orkestr-desktop -Force"], { stdio: "ignore" });
    closedUserApp = true;
    throw new Error("relaunch needed: rerun the script (the running instance was closed)");
  }
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.call("settings.update", { patch: { profileName: "Demo", onboardingCompleted: true, performanceMode: "Balanced", language: "en" } });
  for (const p of projects) p.id = (await rpc.call("projects.create", { name: p.name, rootPath: p.dir, approvalMode: "SemiAuto" })).project?.id;
  const source = (await rpc.call("marketplace.addSource", { kind: "Directory", location: registry, name: "Team folder registry" })).source;
  console.log("seeded");

  const target = await waitFor(async () => {
    const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
    return list.find((t) => t.type === "page" && !t.url.startsWith("devtools") && !t.url.includes("#/"));
  }, 60_000, "webview DevTools target");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  cdpRef = cdp;
  await cdp.send("Page.reload", { ignoreCache: true });
  await waitFor(() => cdp.eval("!!document.querySelector('.app__body')"), 60_000, "app shell");
  await sleep(2500);
  await cdp.eval(`document.querySelector('[aria-label="Open project hello-app"]')?.click(), true`);
  await sleep(1500);
  await cdp.eval(`document.querySelector('.resume-card button[aria-label*="ismiss" i], .resume-card button[aria-label*="lose" i]')?.click(), true`);
  await sleep(600);

  // ── W1 plugins management ─────────────────────────────────────────────
  const en = L.en.mp;
  check("W1", "Marketplace → Plugins tab opens", await openPlugins(cdp, "en"));
  await waitFor(() => cdp.eval(`!!document.querySelector(".pl-manager")`), 10_000, "plugins manager");
  const installFile = async (file) => {
    await fill(cdp, en.plugins.installFile, file);
    await click(cdp, ".pl-install button[type=submit]", en.plugins.install, true);
    await sleep(500);
  };
  const stateOf = async (pkg) => (await rpc.call("plugins.list", {})).plugins.find((p) => p.plugin.packageId === pkg);
  const rowShown = (pkg) => cdp.eval(`!!document.querySelector('.pl-row[data-plugin=${J(pkg)}]')`);
  const enableRow = async (pkg) =>
    cdp.eval(`(() => { const row = document.querySelector('.pl-row[data-plugin=${J(pkg)}]'); const b = row && [...row.querySelectorAll("button")].find((x) => x.textContent.trim() === ${J(en.plugins.enable)}); if (!b) return false; b.click(); return true; })()`);
  const installAndEnable = async (file, pkg) => {
    const t0 = Date.now();
    await installFile(file);
    await waitFor(() => rowShown(pkg), 180_000, `${pkg} row`).catch(() => null);
    const installedMs = Date.now() - t0;
    await sleep(500);
    await enableRow(pkg);
    const info = await waitFor(async () => {
      const i = await stateOf(pkg);
      return i?.plugin.state === "Enabled" ? i : null;
    }, 120_000, `${pkg} enabled`).catch(() => stateOf(pkg));
    return { info, installedMs };
  };

  const hwr = await installAndEnable(join(fixtures, "hello-widget.orkp"), "orkestr-dev.hello-widget");
  const hw = hwr.info;
  check("W1", "hello-widget.orkp (signed fixture) installed from file + enabled via the UI", hw?.plugin.state === "Enabled" && !!hw.signerFingerprint, `${hw?.plugin.state}; install ${hwr.installedMs} ms; signer ${hw?.signerFingerprint?.slice(0, 16)}…`);

  const pwr = await installAndEnable(projectsWidgetPath, PROJECTS_WIDGET_ID);
  const pw = pwr.info;
  check("W1", "projects-widget.orkp (signed with the dev key) installed + enabled", pw?.plugin.state === "Enabled", `${pw?.plugin.state}; install ${pwr.installedMs} ms`);

  const t0 = Date.now();
  await installFile(join(fixtures, "hello-tool.orkp"));
  const prompt = await waitFor(() => cdp.eval(`!!document.querySelector('[role=dialog][aria-labelledby="mp-perm-title"]')`), 180_000, "permission prompt").catch(() => false);
  const notice = await text(cdp, ".mp-notice, .mp-error");
  check("W1", "hello-tool.orkp → pending grants → marketplace permission prompt", prompt, prompt ? `after ${Date.now() - t0} ms` : notice);
  await capture(cdp, "03-plugins-permission-prompt-en", "Plugins: permission prompt after installing hello-tool.orkp from file (Network example.com)");
  await cdp.eval(`(() => { const d = document.querySelector('[aria-labelledby="mp-perm-title"]'); d && d.querySelectorAll("input[type=checkbox]").forEach((c) => c.click()); return true; })()`);
  await click(cdp, '[aria-labelledby="mp-perm-title"] button', en.permissions.apply, true);
  const ht = await waitFor(async () => {
    const i = await stateOf("orkestr-dev.hello-tool");
    return i?.plugin.state === "Enabled" ? i : null;
  }, 120_000, "hello-tool enabled").catch(() => stateOf("orkestr-dev.hello-tool"));
  await sleep(800);
  check("W1", "grant + enable from the prompt → hello-tool Enabled with Network grant", ht?.plugin.state === "Enabled" && ht.grants.some((g) => g.capability === "Network" && g.allowed), `${ht?.plugin.state}; ${JSON.stringify(ht?.grants?.map((g) => [g.capability, g.scope, g.allowed]))}`);
  if (ht) await rpc.call("plugins.invoke", { pluginId: ht.plugin.id, tool: "hello", input: { name: "walkthrough" } }).catch(() => null);
  const rows = await text(cdp, ".pl-row strong");
  check("W1", "plugin list shows all three plugins with state badges and signer fingerprints", ["Hello", "Projects widget"].every((s) => rows.includes(s)) && (await cdp.eval(`document.querySelectorAll(".pl-row .mp-fingerprint").length`)) >= 3, rows);
  await maximize(cdp, "en", ".mp-panel", true);
  await capture(cdp, "02-plugins-management-en", "Marketplace → Plugins: state, kinds, signer fingerprint, grants (revoke), enable/disable, logs, uninstall, kill switch and safe mode");
  await click(cdp, ".pl-row button", en.plugins.logsOf.replace("{{name}}", ht?.plugin.name ?? "Hello Tool"), true);
  await sleep(1200);
  const logText = await text(cdp, ".pl-logs");
  check("W1", "logs viewer shows plugin log lines (plugins.logs)", /hello/i.test(logText), logText.slice(0, 120));
  await capture(cdp, "04-plugin-logs-en", "Plugin logs viewer (plugins.logs)");
  await cdp.eval(`document.querySelector(".pl-logs-dialog .dialog__close")?.click(), true`);
  await sleep(400);

  // ── W2 dashboards ─────────────────────────────────────────────────────
  const dsh = L.en.dsh;
  await openPanel(cdp, "en", "dashboards");
  await waitFor(() => cdp.eval(`!!document.querySelector(".dsh")`), 10_000, "dashboards panel");
  await click(cdp, ".dsh button", dsh.actions.create, true);
  await sleep(500);
  await fill(cdp, dsh.naming.createLabel, "Phase 7 walkthrough");
  await click(cdp, ".dsh-naming button[type=submit]", dsh.actions.create, true);
  await sleep(1500);
  const editing = await cdp.eval(`document.querySelector(".dsh")?.dataset.editing === "on"`);
  if (!editing) await click(cdp, ".dsh-toolbar button", dsh.actions.edit, true);
  await waitFor(() => cdp.eval(`document.querySelectorAll(".dsh-palette__item").length > 0`), 10_000, "widget palette");
  const paletteItems = await cdp.eval(`[...document.querySelectorAll(".dsh-palette__item")].map((b) => ({ label: b.getAttribute("aria-label"), plugin: !!b.querySelector(".ok-badge") }))`);
  const builtins = paletteItems.filter((p) => !p.plugin).slice(0, 2);
  const pluginItems = paletteItems.filter((p) => p.plugin);
  check("W2", "widget palette lists built-ins + both plugin widgets", builtins.length === 2 && pluginItems.some((p) => /Projects widget/.test(p.label)) && pluginItems.some((p) => /Hello/.test(p.label)), paletteItems.map((p) => p.label).join(", "));
  // The palette follows widgets.list order; pick the hello-widget *package* (hello-tool also ships a "Hello widget").
  const catalog = (await rpc.call("widgets.list", {})).widgets.map((w) => w.id);
  const nth = (id) => cdp.eval(`(() => { const b = document.querySelectorAll(".dsh-palette__item")[${catalog.findIndex((w) => w.startsWith(id))}]; if (!b) return false; b.click(); return true; })()`);
  await nth(`${PROJECTS_WIDGET_ID}/`);
  await sleep(900);
  for (const p of builtins) {
    await click(cdp, ".dsh-palette__item", p.label, true);
    await sleep(900);
  }
  await nth("orkestr-dev.hello-widget/");
  await sleep(900);
  await click(cdp, ".dsh-toolbar button", dsh.actions.done, true);
  await sleep(4000);
  const listBoards = async () => (await rpc.call("dashboards.list", { includeGlobal: true, ...(projects[0].id ? { projectId: projects[0].id } : {}) })).dashboards ?? [];
  const saved = await listBoards();
  const board = saved.find((d) => d.name === "Phase 7 walkthrough");
  const full = board ? (await rpc.call("dashboards.get", { dashboardId: board.id }).catch(() => null))?.dashboard ?? board : null;
  const layout = (full?.widgets ?? []).map((w) => `${w.widgetId}@${w.x},${w.y} ${w.w}x${w.h}`);
  check("W2", "dashboard saved with the hello-widget + projects widgets and 2 built-ins (dashboards.list/get)", (full?.widgets ?? []).length === 4 && [`${PROJECTS_WIDGET_ID}/`, "orkestr-dev.hello-widget/"].every((id) => full.widgets.some((w) => w.widgetId.startsWith(id))), layout.join("; "));

  // ── W3 / W4 widget frame ──────────────────────────────────────────────
  await waitFor(() => cdp.eval(`document.querySelectorAll("iframe[data-widget-frame]").length >= 2`), 15_000, "widget iframes").catch(() => null);
  const frameAttrs = await cdp.eval(`[...document.querySelectorAll("iframe[data-widget-frame]")].map((f) => ({ id: f.dataset.widgetFrame, sandbox: f.getAttribute("sandbox"), src: f.getAttribute("src"), srcdoc: f.hasAttribute("srcdoc") }))`);
  check("W3", "plugin widgets render in sandboxed iframes (sandbox=allow-scripts, orkestr-widget scheme)", frameAttrs.length >= 2 && frameAttrs.every((f) => f.sandbox === "allow-scripts"), JSON.stringify(frameAttrs));
  let frames = [];
  await waitFor(async () => {
    frames = await readWidgetFrames(cdp);
    const pf = frames.find((f) => f.ipc);
    return pf && pf.rendered && pf.ipc.result !== "pending";
  }, 20_000, "widget content").catch(() => null);
  const pf = frames.find((f) => f.ipc);
  check("W3", "projects widget rendered the project names from its projects.list query (read via CDP inside the frame)", pf && projects.every((p) => pf.items.includes(p.name)), pf ? `${pf.via}: [${pf.items.join(", ")}] url ${pf.frameUrl}` : `frames: ${JSON.stringify(frames).slice(0, 200)}`);
  const hf = frames.find((f) => /Hello/.test(f.text) && !f.ipc);
  check("W3", "hello-widget fixture rendered inside its frame", !!hf, hf ? hf.text.slice(0, 60) : "not found");
  check("W4", "inside the widget frame the Tauri IPC fails (invoke refused / no answer)", pf?.ipc?.result === "blocked", pf?.ipc ? `${pf.ipc.text}; __TAURI_INTERNALS__ present: ${pf.ipc.internals}` : "no probe");
  check("W4", "the widget frame cannot reach the parent DOM (opaque origin)", pf?.ipc?.parentDom === "false", pf?.ipc?.parentDom);
  await sleep(1500);
  await maximize(cdp, "en", ".dsh", true);
  await sleep(1500);
  await capture(cdp, "01-dashboard-en", "Dashboards: plugin widgets (Projects widget rendering projects.list, hello-widget) next to built-ins, in sandboxed iframes");
  await maximize(cdp, "en", ".dsh", false);
  // ── W5 reload → layout persists ───────────────────────────────────────
  await cdp.send("Page.reload", { ignoreCache: true });
  await waitFor(() => cdp.eval("!!document.querySelector('.app__body')"), 60_000, "app shell after reload");
  await sleep(2500);
  if (!(await cdp.eval(`!!document.querySelector(".dsh")`))) await openPanel(cdp, "en", "dashboards");
  await waitFor(() => cdp.eval(`!!document.querySelector(".dsh-board")`), 15_000, "dashboard after reload").catch(() => null);
  await sleep(2500);
  const afterReload = await cdp.eval(`({ name: document.querySelector(".dsh-select")?.selectedOptions[0]?.textContent, cards: document.querySelectorAll(".dsh-board [aria-label]").length, frames: document.querySelectorAll("iframe[data-widget-frame]").length })`);
  const reloaded = (await listBoards()).find((d) => d.id === board?.id);
  const full2 = reloaded ? (await rpc.call("dashboards.get", { dashboardId: reloaded.id }).catch(() => null))?.dashboard ?? reloaded : null;
  const layout2 = (full2?.widgets ?? []).map((w) => `${w.widgetId}@${w.x},${w.y} ${w.w}x${w.h}`);
  check("W5", "after a reload the dashboard and its layout persist (same widgets / positions, frames re-rendered)", afterReload.name === "Phase 7 walkthrough" && afterReload.frames >= 2 && J(layout2) === J(layout), `${J(afterReload)}; layout equal: ${J(layout2) === J(layout)}`);

  // ── W6 publish ────────────────────────────────────────────────────────
  await openPanel(cdp, "en", "marketplace");
  await waitFor(async () => {
    if (!(await cdp.eval(`!!document.querySelector(".mp-panel")`))) await openPanel(cdp, "en", "marketplace");
    await click(cdp, ".mp-panel [role=tab]", en.tabs.publish);
    await sleep(400);
    return cdp.eval(`!!document.querySelector(".mp-publish form")`);
  }, 30_000, "publish tab");
  await maximize(cdp, "en", ".mp-panel", true);
  await sleep(800);
  await fill(cdp, en.publish.file, join(fixtures, "hello-widget.orkp"));
  await choose(cdp, en.publish.target, source.id);
  await fill(cdp, en.publish.publisher, "Orkestr Dev");
  await capture(cdp, "06-publish-form-en", "Marketplace → Publish: package file, kind, writable target source, publisher (no key yet)");
  await click(cdp, ".mp-publish button[type=submit]", en.publish.submit, true);
  await waitFor(() => cdp.eval(`!!document.querySelector(".mp-publish__result, .mp-publish .mp-error")`), 20_000, "publish result").catch(() => null);
  const pubResult = await text(cdp, ".mp-publish__result, .mp-publish .mp-error");
  const keys = await rpc.call("marketplace.keys", {});
  const fpShown = await cdp.eval(`[...document.querySelectorAll(".mp-publish .mp-fingerprint code")].map((c) => c.textContent).join(" ")`);
  check("W6", "Publish signs + adds the package to the folder registry; result shown", /Published/.test(pubResult), pubResult.slice(0, 160));
  check("W6", "local publisher key fingerprint shown in the Publish tab (marketplace.keys localPublisher)", !!keys.localPublisher && fpShown.replace(/\s+/g, " ").trim() === keys.localPublisher.fingerprint.replace(/\s+/g, " ").trim(), fpShown);
  await capture(cdp, "05-publish-en", "Marketplace → Publish: signed with the vault-held publisher key (fingerprint) and published to the folder registry");

  // ── safe mode + kill switch (en) ──────────────────────────────────────
  await click(cdp, ".mp-panel [role=tab]", en.tabs.plugins);
  await sleep(1000);
  await cdp.eval(`document.querySelector('.pl-switch input')?.click(), true`);
  await sleep(1200);
  const sm = await rpc.call("plugins.list", {});
  check("W1", "safe mode toggle → plugins.setSafeMode(true), warning shown", sm.safeMode === true && (await text(cdp, ".pl-controls")).includes(en.plugins.safeModeActive), `safeMode ${sm.safeMode}`);
  await capture(cdp, "07-plugins-safe-mode-en", "Safe mode on: explanation + warning, Enable is blocked");
  await cdp.eval(`document.querySelector('.pl-switch input')?.click(), true`);
  await sleep(1000);
  await click(cdp, ".pl-controls button", en.plugins.killAll, true);
  await sleep(600);
  const killDialog = await cdp.eval(`!!document.querySelector('[role=alertdialog]')`);
  await capture(cdp, "08-plugins-kill-confirm-en", "Kill all plugins: confirmation");
  await click(cdp, "[role=alertdialog] button", en.plugins.killAll, true);
  await sleep(1500);
  const killed = (await rpc.call("plugins.list", {})).plugins;
  check("W1", "Kill all (confirmed) → every plugin Killed", killDialog && killed.every((p) => p.plugin.state === "Killed"), killed.map((p) => `${p.plugin.name}:${p.plugin.state}`).join(", "));
  await dismissNotice(cdp);
  await maximize(cdp, "en", ".mp-panel", false);
  // re-enable for the ru pass
  for (const p of killed) await rpc.call("plugins.enable", { pluginId: p.plugin.id }).catch(() => null);
  await sleep(800);

  // ── ru ────────────────────────────────────────────────────────────────
  const ru = L.ru.mp;
  await rpc.call("settings.update", { patch: { language: "ru" } });
  await sleep(1500);
  await openPanel(cdp, "ru", "dashboards");
  await waitFor(() => cdp.eval(`document.querySelectorAll("iframe[data-widget-frame]").length >= 2`), 15_000, "ru widget frames").catch(() => null);
  await sleep(1000);
  await sleep(2500);
  await maximize(cdp, "ru", ".dsh", true);
  await sleep(2000);
  await capture(cdp, "11-dashboard-ru", "Дашборды (RU): виджеты плагинов и встроенные виджеты");
  await maximize(cdp, "ru", ".dsh", false);
  await openPlugins(cdp, "ru");
  await sleep(1200);
  await dismissNotice(cdp);
  await maximize(cdp, "ru", ".mp-panel", true);
  const ruPlugins = await text(cdp, ".pl-controls");
  check("I18N", "Plugins management renders in Russian", ruPlugins.includes(ru.plugins.killAll) && ruPlugins.includes(ru.plugins.safeMode), ruPlugins.slice(0, 120));
  await capture(cdp, "12-plugins-management-ru", "Управление плагинами (RU)");
  await click(cdp, ".mp-panel [role=tab]", ru.tabs.publish);
  await sleep(1500);
  const ruPublish = await text(cdp, ".mp-publish");
  check("I18N", "Publish tab renders in Russian", ruPublish.includes(ru.publish.title), ruPublish.slice(0, 120));
  await capture(cdp, "13-publish-ru", "Публикация пакета (RU)");
  await rpc.call("settings.update", { patch: { language: "en" } });

  rpc.close();
} catch (e) {
  exitCode = 1;
  console.error(e.stack ?? e);
  if (cdpRef) {
    try {
      const { data } = await cdpRef.send("Page.captureScreenshot", { format: "png" });
      writeFileSync(join(base, "..", "orkestr-p7-walkthrough-error.png"), Buffer.from(data, "base64"));
      console.error(`error screenshot: ${join(base, "..", "orkestr-p7-walkthrough-error.png")}`);
      console.error(await cdpRef.eval(`[...document.querySelectorAll('[role="tab"]')].map((t) => t.textContent.trim()).join(" | ")`));
    } catch {
      /* ignore */
    }
  }
  console.error(log.slice(-3000));
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
    `${J(
      {
        date: new Date().toISOString(),
        result: `${checks.length - failed.length}/${checks.length} checks passed`,
        closedRunningApp: closedUserApp,
        checks,
        screenshots: shots.map(({ name, note, errors }) => ({ name: `${name}.png`, note, consoleMessages: errors })),
      },
      null,
      2,
    )}\n`,
  );
  console.log(`\n${checks.length - failed.length}/${checks.length} checks passed${closedUserApp ? " (closed a running orkestr-desktop)" : ""}`);
  await sleep(1000);
  try {
    rmSync(base, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
  } catch {
    /* ignore */
  }
  process.exit(exitCode);
}
