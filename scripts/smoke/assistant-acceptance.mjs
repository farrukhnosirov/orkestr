#!/usr/bin/env node
// Computer assistant acceptance run (ADR-0035) — drives THIS Windows desktop
// for real through the core RPC (`computer.*`), exactly as the Assistant
// panel and the voice pipeline do:
//
//   A1 setup       headless orkestrd (temp ORKESTR_HOME) + offline Mock
//                  provider as the AI Manager model; computer.status
//   A2 notepad     "открой блокнот" → window appears; "напечатай …" types;
//                  select all + copy + "what is in the clipboard" proves the
//                  text arrived; 60 more lines typed; scroll up / down /
//                  page / to the top
//   A3 switching   "open calculator" → "переключись на блокнот" → Notepad is
//                  in front → "switch to calculator" → "закрой калькулятор"
//   A4 close       "закрой блокнот без сохранения" → NeedsConfirmation with a
//                  pending approval in the approvals inbox → computer.confirm
//                  → Notepad gone, no file written
//   A5 settings    "открой настройки экрана" → ms-settings:display window →
//                  "close windows settings" → gone
//   A6 AI path     unknown command → Mock plan (Notepad + text) → correction →
//                  approve → runs → "запомни это как …" → user command →
//                  same phrase again runs the remembered command → closes
//   A7 safety      kill explorer refused; shutdown asks; computer.stop
//                  cancels it; audit rows; remote never (policy)
//
// Usage (repo root, Windows, interactive desktop session):
//   cargo build -p orkestr-core --bin orkestrd
//   node scripts/smoke/assistant-acceptance.mjs [--out <dir>]
// Env: ORKESTR_SMOKE_BIN_DIR (default target/debug). Refuses to run while
// Notepad or Calculator are already open (it would close the user's windows).
// Saves and restores the clipboard. Writes <out>/acceptance.json.

import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const exeDir = process.env.ORKESTR_SMOKE_BIN_DIR ?? join(repo, "target", "debug");
const coreExe = join(exeDir, "orkestrd.exe");
const argOf = (n) => {
  const i = process.argv.indexOf(n);
  return i > 0 ? process.argv[i + 1] : undefined;
};
const outDir = argOf("--out") ? resolve(repo, argOf("--out")) : join(tmpdir(), "orkestr-assistant-acceptance");
mkdirSync(outDir, { recursive: true });
const tmp = mkdtempSync(join(tmpdir(), "orkestr-asst-"));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const checks = [];
function check(area, name, ok, detail = "") {
  checks.push({ area, name, ok: Boolean(ok), detail: String(detail ?? "") });
  console.log(`${ok ? "PASS" : "FAIL"}  [${area}] ${name}${detail ? ` — ${String(detail).slice(0, 260)}` : ""}`);
}
const describe = (e) => (e ? `${e.code ? `${e.code}: ` : ""}${e.message ?? e}` : "");
async function waitFor(fn, ms, label) {
  const end = Date.now() + ms;
  let last;
  for (;;) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (e) {
      last = e;
    }
    if (Date.now() > end) throw new Error(`timed out: ${label}${last ? ` (${describe(last)})` : ""}`);
    await sleep(250);
  }
}
async function section(id, title, fn) {
  console.log(`\n── ${id}. ${title}`);
  try {
    await fn();
  } catch (e) {
    check(id, "section ran to the end", false, describe(e));
  }
}

function ps(script) {
  return execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], { encoding: "utf8" }).trim();
}

// ── RPC ──────────────────────────────────────────────────────────────────
function rpcClient(url, token) {
  let ws;
  let seq = 0;
  const pending = new Map();
  const events = [];
  const frame = (o, id) =>
    new Promise((res, rej) => {
      pending.set(id, { resolve: res, reject: rej });
      ws.send(JSON.stringify(o));
    });
  return {
    events,
    open: () =>
      new Promise((res, rej) => {
        ws = new WebSocket(url);
        ws.onmessage = (m) => {
          const f = JSON.parse(m.data);
          if (f.type === "res") {
            const p = pending.get(f.id);
            pending.delete(f.id);
            if (f.ok) p?.resolve(f.result);
            else p?.reject(Object.assign(new Error(`${f.error.code}: ${f.error.message}`), { code: f.error.code, data: f.error.data }));
          } else if (f.type === "event") events.push(f.event);
        };
        ws.onopen = () => frame({ type: "auth", token, id: "a" }, "a").then(res, rej);
        ws.onerror = () => rej(new Error("ws error"));
      }),
    call: (method, params = {}) => {
      const id = `r${++seq}`;
      return frame({ type: "req", id, method, params }, id);
    },
    sub: (topics) => {
      const id = `s${++seq}`;
      return frame({ type: "sub", id, topics }, id);
    },
    close: () => ws?.close(),
  };
}

let core;
let rpc;
let clipFile = null;
const call = (m, p) => rpc.call(m, p);
const say = (text, lang = "ru", planUnknown = false) => call("computer.handle", { text, lang, planUnknown, source: "voice" });
const windows = async () => (await call("computer.windows", {})).windows;
const appWindows = async (re) => (await windows()).filter((w) => re.test(w.process) || re.test(w.title));
const NOTEPAD = /^notepad\.exe$/i;
const CALC = /calculator/i;
const SETTINGS = /^systemsettings\.exe$/i;
const executed = (r) => r.status === "Executed" && r.run.outcome === "Succeeded";
const summary = (r) => (r.status === "Executed" ? `${r.run.outcome}: ${r.run.title} [${r.run.steps.map((s) => `${s.action}${s.ok ? "" : `✗ ${s.detail}`}`).join(", ")}]` : r.status === "NeedsConfirmation" ? `NeedsConfirmation: ${r.pending.reasons.map((x) => x.code).join(",")}` : r.status === "Proposed" ? `Proposed: ${r.plan.explanation}` : JSON.stringify(r).slice(0, 200));

/** The user's clipboard text, kept in a file (exact bytes) while the run copies. */
function saveClipboard() {
  const file = join(tmp, "clipboard.txt");
  try {
    ps(`$t = Get-Clipboard -Raw; if ($null -eq $t) { $t = '' }; [IO.File]::WriteAllText('${file.replace(/'/g, "''")}', $t, (New-Object Text.UTF8Encoding $false))`);
    return file;
  } catch {
    return null;
  }
}
function restoreClipboard(file) {
  if (!file) return;
  try {
    ps(`$t = [IO.File]::ReadAllText('${file.replace(/'/g, "''")}', [Text.Encoding]::UTF8); if ($t.Length) { Set-Clipboard -Value $t } else { Set-Clipboard -Value $null }`);
  } catch {
    /* best effort */
  }
}

/** Safety net: never send keys unless the expected app is in front. */
async function front(re) {
  const a = (await windows()).find((w) => w.active);
  if (!a || !(re.test(a.process) || re.test(a.title))) throw new Error(`refusing to send input: active window is ${a?.process} “${a?.title}”`);
}

async function shot(name) {
  try {
    ps(`Add-Type -AssemblyName System.Windows.Forms,System.Drawing; $b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds; $bmp=New-Object System.Drawing.Bitmap $b.Width,$b.Height; $g=[System.Drawing.Graphics]::FromImage($bmp); $g.CopyFromScreen($b.Location,[System.Drawing.Point]::Empty,$b.Size); $bmp.Save('${join(outDir, name).replace(/'/g, "''")}'); $g.Dispose(); $bmp.Dispose()`);
  } catch {
    /* evidence only */
  }
}

async function closeNotepadDiscard(area) {
  const r = await say("закрой блокнот без сохранения");
  check(area, "\"закрой блокнот без сохранения\" → NeedsConfirmation (closeDiscard) + approval", r.status === "NeedsConfirmation" && r.pending.reasons.some((x) => x.code === "closeDiscard") && !!r.pending.approvalId, summary(r));
  if (r.status !== "NeedsConfirmation") return;
  const inbox = await call("approvals.list", { status: "Pending" });
  check(area, "the confirmation is in the approvals inbox (ComputerControl)", inbox.approvals.some((a) => a.id === r.pending.approvalId && a.capability === "ComputerControl"), `${inbox.approvals.length} pending`);
  const c = await call("computer.confirm", { pendingId: r.pending.pendingId, approve: true });
  check(area, "computer.confirm(approve) closes it", executed(c), summary(c));
  await waitFor(async () => (await appWindows(NOTEPAD)).length === 0, 15000, "Notepad closed").then(
    () => check(area, "no Notepad window left", true),
    (e) => check(area, "no Notepad window left", false, describe(e)),
  );
}

async function main() {
  if (process.platform !== "win32") throw new Error("Windows only");
  if (!existsSync(coreExe)) throw new Error(`missing ${coreExe} — cargo build -p orkestr-core --bin orkestrd`);
  clipFile = saveClipboard();

  await section("A1", "setup", async () => {
    const home = join(tmp, "core");
    mkdirSync(home, { recursive: true });
    const child = spawn(coreExe, ["--data-dir", home], {
      env: { ...process.env, ORKESTR_HOME: home, ORKESTR_SECRET_BACKEND: "vault-file", RUST_LOG: "info" },
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    });
    let log = "";
    child.stdout.on("data", (d) => (log = (log + d).slice(-20000)));
    child.stderr.on("data", (d) => (log = (log + d).slice(-20000)));
    core = { child, home, log: () => log };
    const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 60000, "core.json");
    const token = readFileSync(join(home, "core.token"), "utf8").trim();
    rpc = rpcClient(info.url, token);
    await waitFor(() => rpc.open().then(() => true), 30000, "core RPC");
    await rpc.sub(["global"]);
    const mock = (await call("providers.list")).providers.map((p) => p.provider ?? p).find((p) => p.kind === "Mock");
    const smart = { providerId: mock.id, model: "mock-smart" };
    await call("providers.setManager", { config: { model: smart, maxParallelAgents: 1, approvalPolicy: "SemiAuto" } }).catch(() => null);
    await call("providers.setRouting", { config: { mode: "Manual", rules: [], defaultFallback: [smart], preferLocal: true, maxRateLimitWaitSecs: 0 } });
    const open = [...(await appWindows(NOTEPAD)), ...(await appWindows(CALC)), ...(await appWindows(SETTINGS))];
    if (open.length) throw new Error(`close these windows first (the run closes them): ${open.map((w) => w.title).join(", ")}`);
    const st = await call("computer.status", {});
    check("A1", "computer.status: Windows backend available, catalogue ≥ 1000", st.available && st.backend === "windows" && st.catalogueCommands >= 1000, JSON.stringify(st));
    const m = await call("computer.match", { text: "открой блокнот", lang: "ru", limit: 3 });
    check("A1", "local matcher: \"открой блокнот\" → app.notepad.open (µs)", m.matches[0]?.commandId === "app.notepad.open", `${m.matches[0]?.commandId} ${m.matches[0]?.score} in ${m.elapsedUs} µs`);
  });

  if (checks.some((c) => !c.ok)) return; // setup failed: never drive the desktop blind

  await section("A2", "Notepad: open, type, scroll", async () => {
    const r = await say("открой блокнот");
    check("A2", "\"открой блокнот\" executed", executed(r), summary(r));
    const w = await waitFor(async () => (await appWindows(NOTEPAD))[0], 15000, "Notepad window");
    check("A2", "Notepad window appeared", !!w, w?.title);
    await sleep(800);
    const f = await say("переключись на блокнот");
    check("A2", "\"переключись на блокнот\" → Notepad in front", executed(f) && (await windows()).find((x) => x.active && NOTEPAD.test(x.process)), summary(f));
    // Windows 11 Notepad restores its previous session: work in a NEW tab so
    // the user's own restored documents never receive keystrokes.
    await front(NOTEPAD);
    const nt = await say("новый документ");
    check("A2", "\"новый документ\" → a fresh tab (Ctrl+N) for the test text", executed(nt), summary(nt));
    await sleep(700);
    await front(NOTEPAD);
    const typed = "Привет от ассистента Orkestr 123";
    const t1 = await say(`напечатай ${typed}`);
    check("A2", "\"напечатай …\" executed (Unicode SendInput)", executed(t1), summary(t1));
    await sleep(500);
    await front(NOTEPAD);
    const sel = await say("выдели всё");
    const cp = await say("скопируй");
    const clip = await say("что в буфере обмена");
    const shown = clip.status === "Executed" ? clip.run.effects.find((e) => e.type === "Show")?.lines.join("\n") ?? "" : "";
    check("A2", "select all + copy + clipboard read back the typed text", executed(sel) && executed(cp) && shown.includes(typed), JSON.stringify(shown).slice(0, 120));
    await front(NOTEPAD);
    await say("в конец документа");
    const lines = Array.from({ length: 60 }, (_, i) => `line ${i + 1}`).join("\n");
    const ex = await call("computer.execute", { title: "type 60 lines", steps: [{ type: "Keys", combo: "enter" }, { type: "TypeText", text: lines }], slots: {} });
    check("A2", "computer.execute types 60 lines", executed(ex), summary(ex));
    await shot("a2-notepad-bottom.png");
    await front(NOTEPAD);
    for (const phrase of ["прокрути вверх", "scroll down 3", "страница вверх", "прокрути в самый верх"]) {
      const s = await say(phrase, phrase.startsWith("scroll") ? "en" : "ru");
      check("A2", `"${phrase}" executed`, executed(s), summary(s));
      await sleep(250);
    }
    await shot("a2-notepad-top.png");
  });

  await section("A3", "switch windows", async () => {
    const r = await say("open calculator", "en");
    check("A3", "\"open calculator\" executed", executed(r), summary(r));
    await waitFor(async () => (await appWindows(CALC))[0], 15000, "Calculator window");
    await sleep(800);
    const n = await say("переключись на блокнот");
    const front1 = (await windows()).find((x) => x.active);
    check("A3", "\"переключись на блокнот\" → Notepad active", executed(n) && NOTEPAD.test(front1?.process ?? ""), `${summary(n)} active=${front1?.process}`);
    const c = await say("switch to calculator", "en");
    const front2 = (await windows()).find((x) => x.active);
    check("A3", "\"switch to calculator\" → Calculator active", executed(c) && CALC.test(`${front2?.process} ${front2?.title}`), `${summary(c)} active=${front2?.process}`);
    const k = await say("закрой калькулятор");
    check("A3", "\"закрой калькулятор\" executed", executed(k), summary(k));
    await waitFor(async () => (await appWindows(CALC)).length === 0, 10000, "Calculator closed").then(
      () => check("A3", "Calculator closed", true),
      (e) => check("A3", "Calculator closed", false, describe(e)),
    );
  });

  await section("A4", "close Notepad without saving (confirmation flow)", async () => {
    await closeNotepadDiscard("A4");
    const audit = await call("audit.list", { action: "computer.", limit: 200 });
    check("A4", "audit has computer.run rows (approved + executed)", audit.entries.filter((e) => e.event.action === "computer.run").length >= 3, `${audit.entries.length} rows`);
  });

  await section("A5", "Windows Settings page", async () => {
    const r = await say("открой настройки экрана");
    check("A5", "\"открой настройки экрана\" → ms-settings:display", executed(r) && r.run.steps.some((s) => /ms-settings:display/.test(s.detail)), summary(r));
    const w = await waitFor(async () => (await appWindows(SETTINGS))[0], 20000, "Settings window");
    check("A5", "Settings window appeared", !!w, w?.title);
    await sleep(1500);
    await shot("a5-settings.png");
    const c = await say("close windows settings", "en");
    check("A5", "\"close windows settings\" executed", executed(c), summary(c));
    await waitFor(async () => (await appWindows(SETTINGS)).length === 0, 15000, "Settings closed").then(
      () => check("A5", "Settings closed", true),
      (e) => check("A5", "Settings closed", false, describe(e)),
    );
  });

  await section("A6", "unknown command → AI plan → approve → remember → run again", async () => {
    const request = "запиши в блокнот список покупок молоко хлеб";
    const p = await say(request, "ru", true);
    check("A6", "unknown command → Proposed plan from the Mock provider", p.status === "Proposed" && p.plan.steps.length >= 2, summary(p));
    if (p.status !== "Proposed") return;
    check("A6", "plan uses the action vocabulary (Launch → Wait → Ctrl+N → Wait → TypeText)", p.plan.steps.map((s) => s.type).join(",") === "Launch,Wait,Keys,Wait,TypeText", JSON.stringify(p.plan.steps));
    const corrected = await call("computer.plan", { text: "", planId: p.plan.planId, correction: "яйца", lang: "ru" });
    const typedText = corrected.plan.steps.find((s) => s.type === "TypeText")?.text ?? "";
    check("A6", "correction → re-planned with the correction", typedText.endsWith("яйца") && corrected.plan.corrections.length === 1, typedText);
    const ran = await call("computer.approvePlan", { planId: p.plan.planId });
    check("A6", "approve → plan runs", executed(ran), summary(ran));
    await waitFor(async () => (await appWindows(NOTEPAD))[0], 15000, "Notepad from the plan");
    const rem = await say("запомни это как список покупок");
    check("A6", "\"запомни это как список покупок\" → Remembered in my-commands", rem.status === "Remembered" && rem.setId === "my-commands", JSON.stringify(rem).slice(0, 200));
    await closeNotepadDiscard("A6");
    const again = await say("список покупок");
    check("A6", "\"список покупок\" runs the remembered command", executed(again) && again.run.commandId?.startsWith("user."), summary(again));
    await waitFor(async () => (await appWindows(NOTEPAD))[0], 15000, "Notepad from the remembered command");
    await sleep(1200);
    await shot("a6-remembered.png");
    await closeNotepadDiscard("A6");
    const sets = await call("computer.listSets", {});
    check("A6", "user set listed with the command", sets.sets.some((s) => s.id === "my-commands" && s.commandCount === 1));
  });

  await section("A7", "safety", async () => {
    const k = await say("убей процесс explorer").catch((e) => e);
    check("A7", "killing explorer is forbidden", k instanceof Error && k.code === "forbidden", describe(k));
    const s = await say("выключи компьютер");
    check("A7", "shutdown asks first (Dangerous)", s.status === "NeedsConfirmation" && s.pending.reasons[0]?.level === "Dangerous", summary(s));
    const stop = await call("computer.stop", {});
    check("A7", "computer.stop cancels the pending shutdown", stop.pendingCancelled >= 1, JSON.stringify(stop));
    const pend = await call("approvals.list", { status: "Pending" });
    check("A7", "no pending approval left", !pend.approvals.some((a) => a.capability === "ComputerControl"), `${pend.approvals.length}`);
    const ev = rpc.events.filter((e) => e.kind === "ComputerCommand").length;
    check("A7", "ComputerCommand events published", ev > 5, `${ev} events`);
  });

}

main()
  .catch((e) => check("A0", "run", false, describe(e)))
  .finally(async () => {
    try {
      rpc?.close();
      core?.child.kill();
    } catch {
      /* gone */
    }
    restoreClipboard(clipFile);
    await sleep(500);
    rmSync(tmp, { recursive: true, force: true, maxRetries: 3 });
    const failed = checks.filter((c) => !c.ok);
    writeFileSync(join(outDir, "acceptance.json"), JSON.stringify({ at: new Date().toISOString(), passed: checks.length - failed.length, failed: failed.length, checks }, null, 2));
    console.log(`\n${checks.length - failed.length}/${checks.length} checks passed → ${join(outDir, "acceptance.json")}`);
    if (failed.length && core) console.error(`--- orkestrd log tail ---\n${core.log().slice(-2000)}`);
    process.exit(failed.length ? 1 : 0);
  });
