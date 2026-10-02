#!/usr/bin/env node
// Russian UI review: launches the real desktop app (debug build, frontend from
// the Vite dev server) with an isolated ORKESTR_HOME and `language: "ru"`,
// seeds a project with a finished Manager run, a pending plan, memory, a
// decision and a budget, then opens every panel, settings section,
// onboarding step, the command palette and the main dialogs at the minimum
// window size (960×600) and captures the webview through the WebView2
// DevTools protocol (webview only — no other application can leak into the
// PNGs). Finally it switches to English live (no reload) and captures again.
//
// It also reports, per view, visible elements whose text is clipped
// (scrollWidth > clientWidth with overflow hidden) and Latin-only text
// fragments that look like untranslated English.
//
// Usage (repo root, Windows):
//   npm run dev -w @orkestr/desktop          # Vite on 127.0.0.1:1420
//   cargo build -p orkestr-desktop
//   node scripts/smoke/i18n-ru-screenshots.mjs [--out docs/screenshots/i18n-ru]

import { spawn, execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const arg = (name, dflt) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : dflt;
};
const outDir = resolve(repo, arg("--out", join("docs", "screenshots", "i18n-ru")));
const only = arg("--only", "");
const exe = process.env.ORKESTR_SMOKE_EXE ?? join(repo, "target", "debug", "orkestr-desktop.exe");
const cdpPort = 9334;
const WIDTH = Number(arg("--width", "960"));
const HEIGHT = Number(arg("--height", "600"));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(outDir, { recursive: true });

// ── seed data dir + project ───────────────────────────────────────────────
const base = mkdtempSync(join(tmpdir(), "orkestr-ru-"));
const home = join(base, "home");
const repoDir = join(base, "hello-app");
mkdirSync(home, { recursive: true });
mkdirSync(join(repoDir, "src"), { recursive: true });
const git = (cwd, ...a) => execFileSync("git", a, { cwd, encoding: "utf8" }).trim();
git(repoDir, "init", "-q", "-b", "main");
git(repoDir, "config", "user.email", "demo@orkestr.local");
git(repoDir, "config", "user.name", "Orkestr Demo");
writeFileSync(join(repoDir, "README.md"), "# hello-app\n\nДемо-проект для Orkestr.\n");
writeFileSync(join(repoDir, "package.json"), '{\n  "name": "hello-app",\n  "private": true,\n  "type": "module"\n}\n');
writeFileSync(
  join(repoDir, "src", "index.mjs"),
  '// Точка входа\nexport function main(argv = process.argv.slice(2)) {\n  const name = argv[0] ?? "мир";\n  console.log(`Привет, ${name}!`);\n}\n',
);
git(repoDir, "add", "-A");
git(repoDir, "commit", "-q", "-m", "initial");
writeFileSync(join(repoDir, "README.md"), "# hello-app\n\nДемо-проект для Orkestr.\n\nВ работе.\n");
writeFileSync(join(repoDir, "notes.txt"), "черновик\n");

const child = spawn(exe, [], {
  env: {
    ...process.env,
    ORKESTR_HOME: home,
    ORKESTR_SECRET_BACKEND: "vault-file",
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
      } else if (f.method === "Runtime.consoleAPICalled" && ["error"].includes(f.params.type)) {
        const text = f.params.args.map((a) => a.value ?? a.description ?? "").join(" ");
        this.console.push(`${f.params.type}: ${text}`);
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

// Visible text that is clipped, plus Latin-only phrases (possible untranslated English).
const AUDIT = `(() => {
  const vis = (el) => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none' && r.bottom > 0 && r.right > 0 && r.top < innerHeight && r.left < innerWidth; };
  const clipped = [];
  for (const el of document.querySelectorAll('body *')) {
    if (!vis(el) || el.closest('.monaco-editor, .xterm, svg, .react-flow__edge, pre, code, textarea, input')) continue;
    const own = [...el.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent).join('').trim();
    if (!own) continue;
    const s = getComputedStyle(el);
    const hidesX = ['hidden', 'clip'].includes(s.overflowX) || s.textOverflow === 'ellipsis';
    if (hidesX && el.scrollWidth > el.clientWidth + 1) clipped.push(own.slice(0, 80));
    else if (s.overflowY === 'hidden' && el.scrollHeight > el.clientHeight + 2 && s.webkitLineClamp === 'none' && el.clientHeight > 0) clipped.push('[v] ' + own.slice(0, 80));
  }
  const latin = new Set();
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let n; (n = walker.nextNode()); ) {
    const p = n.parentElement; if (!p || !vis(p) || p.closest('.monaco-editor, .xterm, pre, code, kbd, script, style, textarea, input, [data-user-content]')) continue;
    const t = n.textContent.trim();
    if (/[A-Za-z]{3,}\\s+[A-Za-z]{3,}/.test(t) && !/[А-Яа-яЁё]/.test(t)) latin.add(t.slice(0, 100));
  }
  const hscroll = document.documentElement.scrollWidth > innerWidth + 1;
  return { clipped: [...new Set(clipped)].slice(0, 40), latin: [...latin].slice(0, 40), hscroll };
})()`;

const shots = [];
async function capture(cdp, name, note) {
  if (only && !name.includes(only)) return;
  await sleep(900);
  const file = join(outDir, `${name}.png`);
  const { data } = await cdp.send("Page.captureScreenshot", { format: "png" });
  writeFileSync(file, Buffer.from(data, "base64"));
  let audit = { clipped: [], latin: [], hscroll: false };
  try {
    audit = await cdp.eval(AUDIT);
  } catch (e) {
    audit.error = String(e.message ?? e);
  }
  const errors = cdp.console.splice(0);
  shots.push({ name: `${name}.png`, note, consoleErrors: errors, ...audit });
  console.log(`${name}${errors.length ? `  — ${errors.length} console error(s)` : ""}`);
  for (const e of errors) console.log(`    ! ${e.slice(0, 300)}`);
  for (const c of audit.clipped) console.log(`    clipped: ${c}`);
  for (const l of audit.latin) console.log(`    latin: ${l}`);
  if (audit.hscroll) console.log("    horizontal page scroll!");
}

async function blur(cdp) {
  await cdp.eval("document.activeElement && document.activeElement.blur && document.activeElement.blur(), true");
}
async function palette(cdp, text) {
  await blur(cdp);
  await cdp.key("k", { ctrl: true });
  await sleep(300);
  await cdp.type(text);
  await sleep(450);
  await cdp.key("Enter");
  await sleep(700);
}
async function escape(cdp, n = 1) {
  for (let i = 0; i < n; i++) {
    await cdp.key("Escape");
    await sleep(250);
  }
}
const clickText = (cdp, selector, text, exact = false) =>
  cdp.eval(`(() => {
    const els = [...document.querySelectorAll(${JSON.stringify(selector)})].filter((e) => e.getClientRects().length);
    const want = ${JSON.stringify(text)};
    const el = els.find((e) => ${exact ? "e.textContent.trim() === want" : "e.textContent.trim().includes(want)"});
    if (!el) return false;
    el.click();
    return true;
  })()`);
const clickSel = (cdp, selector) =>
  cdp.eval(`(() => { const el = [...document.querySelectorAll(${JSON.stringify(selector)})].find((e) => e.getClientRects().length); if (!el) return false; el.click(); return true; })()`);

async function step(label, fn) {
  try {
    await fn();
  } catch (e) {
    console.log(`  step failed (${label}): ${String(e.message ?? e).slice(0, 200)}`);
  }
}

let exitCode = 0;
try {
  const info = await waitFor(() => JSON.parse(readFileSync(join(home, "core.json"), "utf8")), 120_000, "core.json");
  const token = readFileSync(join(home, "core.token"), "utf8").trim();
  const rpc = rpcClient(info.url, token);
  await rpc.open();
  await rpc.sub(["*"]);

  await rpc.call("settings.update", { patch: { profileName: "Анна", onboardingCompleted: true, language: "ru" } });
  const providers = (await rpc.call("providers.list")).providers.map((p) => p.provider ?? p);
  const mock = providers.find((p) => p.kind === "Mock");
  const smart = { providerId: mock.id, model: "mock-smart" };
  await rpc.call("providers.setManager", { config: { model: smart, maxParallelAgents: 2, approvalPolicy: "SemiAuto" } });
  await rpc.call("providers.setRouting", {
    config: { mode: "Manual", rules: [], defaultFallback: [smart], preferLocal: true, maxRateLimitWaitSecs: 0 },
  });
  const { project } = await rpc.call("projects.create", { name: "hello-app", rootPath: repoDir, approvalMode: "SemiAuto" });
  const projectId = project.id;
  const chat = async (text) => {
    const sent = await rpc.call("manager.chat", { projectId, text, options: { language: "ru" } });
    return waitFor(
      () => rpc.events.find((e) => e.kind === "ManagerMessageAdded" && e.payload.message.id === sent.replyMessageId)?.payload.message,
      60_000,
      "manager reply",
    );
  };
  const r1 = await chat("Создай модуль hello с юнит-тестом");
  const ap = await rpc.call("manager.approvePlan", { planId: r1.planId });
  await waitFor(
    async () => ["Completed", "Failed", "Cancelled"].includes((await rpc.call("manager.status", { runId: ap.run.id })).runs[0].status),
    120_000,
    "run",
  );
  await chat("Создай модуль greeter с тестами");
  await rpc.call("memory.create", {
    scope: { scope: "Project", projectId },
    kind: "Convention",
    title: "Только ES-модули",
    content: "Все исходники — ES-модули .mjs; тесты на node:test.",
    tags: ["стиль"],
  });
  await step("decision", () =>
    rpc.call("decisions.create", {
      projectId,
      title: "SQLite для метаданных",
      decision: "Храним метаданные в SQLite (WAL).",
      context: "Нужна локальная БД без отдельного сервера.",
      alternatives: [{ title: "Postgres", summary: "Полноценная серверная СУБД", rejectedReason: "нужен отдельный сервер" }],
      reasoningSummary: "Встраивается в приложение, есть транзакции и полнотекстовый поиск.",
      consequences: "Миграции ведёт orkestr-storage.",
      status: "Accepted",
    }),
  );
  await step("budget", () =>
    rpc.call("budgets.set", { name: "Проект — месяц", scope: { scope: "Project", projectId }, period: "Monthly", limitUsd: 100, warnAt: 0.8 }),
  );
  await step("snapshot", () => rpc.call("snapshots.create", { projectId, label: "Перед рефакторингом" }));
  rpc.close();
  console.log("seeded");

  const target = await waitFor(async () => {
    const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
    return list.find((t) => t.type === "page" && t.url.includes("127.0.0.1:1420"));
  }, 60_000, "webview DevTools target (is Vite running on :1420?)");
  const cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.open();
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: WIDTH, height: HEIGHT, deviceScaleFactor: 1, mobile: false });
  // The WebView2 profile outlives the temp ORKESTR_HOME: start from the default layout.
  await cdp.eval("(() => { try { localStorage.removeItem('orkestr.layout.v3'); } catch {} return true; })()");
  await cdp.send("Page.reload", { ignoreCache: true });
  await waitFor(() => cdp.eval("!!document.querySelector('.app__body')"), 60_000, "app shell");
  await waitFor(() => cdp.eval("document.documentElement.lang === 'ru'"), 20_000, "Russian UI");
  await sleep(2500);
  await cdp.eval(`document.querySelector('[aria-label="Открыть проект «hello-app»"]')?.click(), true`);
  await sleep(1500);

  await step("manager", async () => {
    await palette(cdp, "Открыть панель: Менеджер");
    await capture(cdp, "01-manager", "Менеджер: сводка сеанса, завершённый запуск и карточка плана");
    await cdp.eval(`document.querySelector('.resume-card button[aria-label="Скрыть"]')?.click(), true`);
    await palette(cdp, "Развернуть или восстановить панель");
    await capture(cdp, "02-manager-maximized", "Менеджер (развёрнут)");
    await palette(cdp, "Развернуть или восстановить панель");
  });
  await step("agents", async () => {
    await palette(cdp, "Открыть панель: Агенты");
    await palette(cdp, "Развернуть или восстановить панель");
    await clickSel(cdp, ".agent-list [role=option], .agent-list li button, .agent-list li");
    await capture(cdp, "03-agents", "Агенты (развёрнуто)");
    await clickText(cdp, "[role=tab]", "Шаблоны");
    await capture(cdp, "04-agents-templates", "Шаблоны агентов");
    await clickText(cdp, "button", "Новый шаблон");
    await capture(cdp, "05-agents-template-editor", "Редактор шаблона");
    await escape(cdp);
    await clickText(cdp, "[role=tab]", "Агенты", true);
    await clickText(cdp, "button", "Запустить агента");
    await capture(cdp, "06-agents-spawn-dialog", "Диалог запуска агента");
    await escape(cdp);
    await palette(cdp, "Развернуть или восстановить панель");
  });
  await step("tasks", async () => {
    await palette(cdp, "Открыть панель: Задачи");
    await palette(cdp, "Развернуть или восстановить панель");
    await clickText(cdp, "[role=tab], [role=radio], button", "Список", true);
    await capture(cdp, "07-tasks-list", "Задачи: список");
    await clickText(cdp, "[role=tab], [role=radio], button", "Доска", true);
    await capture(cdp, "08-tasks-board", "Задачи: доска");
    await clickText(cdp, "[role=tab], [role=radio], button", "Граф", true);
    await sleep(1200);
    await capture(cdp, "09-tasks-graph", "Задачи: граф зависимостей");
    await clickText(cdp, "[role=tab], [role=radio], button", "Список", true);
    await clickSel(cdp, "table tbody tr, [role=row][tabindex], .task-row");
    await capture(cdp, "10-tasks-drawer", "Сведения о задаче");
    await escape(cdp);
    await clickText(cdp, "button", "Новая задача");
    await capture(cdp, "11-tasks-create", "Диалог создания задачи");
    await escape(cdp);
    await palette(cdp, "Развернуть или восстановить панель");
  });
  await step("code", async () => {
    await blur(cdp);
    await cdp.key("p", { ctrl: true });
    await sleep(500);
    await cdp.type("index.mjs");
    await sleep(800);
    await capture(cdp, "12-code-quick-open", "Переход к файлу");
    await cdp.key("Enter");
    await sleep(1500);
    await capture(cdp, "13-code", "Редактор кода");
    await palette(cdp, "Найти в проекте");
    await cdp.type("name");
    await sleep(1200);
    await capture(cdp, "14-code-search", "Поиск по проекту");
  });
  await step("files", async () => {
    await palette(cdp, "Открыть панель: Файлы");
    await capture(cdp, "15-files", "Файлы проекта");
  });
  await step("terminal", async () => {
    await palette(cdp, "Открыть панель: Терминал");
    await palette(cdp, "Развернуть или восстановить панель");
    await sleep(1500);
    await capture(cdp, "16-terminal", "Терминал (развёрнут)");
    await palette(cdp, "Развернуть или восстановить панель");
  });
  await step("git", async () => {
    await palette(cdp, "Открыть панель: Git");
    await palette(cdp, "Развернуть или восстановить панель");
    for (const [tab, n] of [["Изменения", "17-git-changes"], ["История", "18-git-history"], ["Ветки", "19-git-branches"], ["Рабочие копии", "20-git-worktrees"], ["Снимки", "21-git-snapshots"]]) {
      await clickText(cdp, "[role=tab]", tab, true);
      await sleep(900);
      if (tab === "Изменения") await clickSel(cdp, ".git-file, [role=treeitem], .changes li button, .changes li");
      await capture(cdp, n, `Git: ${tab}`);
    }
    await clickText(cdp, "button", "Откатить…");
    await sleep(800);
    await capture(cdp, "22-git-rollback-dialog", "Откат рабочей области");
    await escape(cdp);
    await palette(cdp, "Развернуть или восстановить панель");
  });
  await step("memory", async () => {
    await palette(cdp, "Открыть панель: Память");
    await palette(cdp, "Развернуть или восстановить панель");
    await capture(cdp, "23-memory", "Память проекта");
    await clickText(cdp, "button", "Добавить", true);
    await capture(cdp, "24-memory-editor", "Новая запись памяти");
    await escape(cdp);
    await clickText(cdp, "[role=tab]", "Журнал решений");
    await capture(cdp, "25-memory-decisions", "Журнал решений");
    await clickText(cdp, "[role=tab]", "База знаний");
    await capture(cdp, "26-memory-knowledge", "База знаний");
    await palette(cdp, "Развернуть или восстановить панель");
  });
  await step("costs", async () => {
    await palette(cdp, "Открыть панель: Расходы");
    await palette(cdp, "Развернуть или восстановить панель");
    await capture(cdp, "27-costs", "Расходы");
    await clickText(cdp, "button", "Добавить бюджет");
    await capture(cdp, "28-costs-budget-editor", "Новый бюджет");
    await escape(cdp);
    await palette(cdp, "Развернуть или восстановить панель");
  });
  await step("logs", async () => {
    await palette(cdp, "Открыть журнал");
    await palette(cdp, "Развернуть или восстановить панель");
    await capture(cdp, "29-logs", "Журнал событий");
    await palette(cdp, "Развернуть или восстановить панель");
  });
  await step("placeholders", async () => {
    await palette(cdp, "Открыть панель: Мониторинг");
    await capture(cdp, "30-placeholder-monitoring", "Мониторинг (появится на этапе N)");
  });
  await step("palette", async () => {
    await blur(cdp);
    await cdp.key("k", { ctrl: true });
    await sleep(500);
    await capture(cdp, "31-command-palette", "Палитра команд");
    await cdp.type("/hello");
    await sleep(800);
    await capture(cdp, "32-command-palette-search", "Палитра: поиск проектов");
    await escape(cdp);
  });
  await step("create-project", async () => {
    await palette(cdp, "Создать проект");
    await capture(cdp, "33-create-project", "Новый проект");
    await escape(cdp);
  });
  await step("notifications", async () => {
    await clickSel(cdp, 'button[aria-label^="Уведомления"]');
    await capture(cdp, "34-notifications", "Центр уведомлений");
    await escape(cdp);
    await clickSel(cdp, 'button[aria-label^="Запросы на подтверждение"]');
    await capture(cdp, "35-approvals", "Запросы на подтверждение");
    await escape(cdp);
    await clickSel(cdp, 'button[aria-label="Экстренная остановка всей работы AI"]');
    await capture(cdp, "36-emergency-stop", "Экстренная остановка (диалог)");
    await escape(cdp);
  });
  await step("settings", async () => {
    await palette(cdp, "Открыть настройки");
    await sleep(500);
    const sections = [
      ["Общие", "37-settings-general"],
      ["Система", "38-settings-system"],
      ["AI-провайдеры", "39-settings-providers"],
      ["AI-менеджер", "40-settings-manager"],
      ["Безопасность и разрешения", "41-settings-permissions"],
      ["Хранилище секретов", "42-settings-secrets"],
      ["О программе", "43-settings-about"],
    ];
    for (const [title, name] of sections) {
      await clickText(cdp, "[role=tab], nav button, .settings__nav button", title, true);
      await sleep(title === "Система" ? 3500 : 900);
      await capture(cdp, name, `Настройки → ${title}`);
    }
    await escape(cdp);
  });
  await step("onboarding", async () => {
    await palette(cdp, "Запустить мастер настройки");
    await sleep(600);
    for (let i = 1; i <= 8; i++) {
      await capture(cdp, `${String(43 + i).padStart(2, "0")}-onboarding-step-${i}`, `Мастер настройки, шаг ${i}`);
      const ok = await clickText(cdp, ".wizard button, [role=dialog] button", "Далее");
      if (!ok) await clickText(cdp, ".wizard button, [role=dialog] button", "Пропустить", true);
      await sleep(i === 7 ? 3000 : 700);
    }
    await clickSel(cdp, 'button[aria-label="Закрыть мастер настройки"]');
    await sleep(400);
  });
  // Live switch to English (no reload): the shell must re-render in place.
  await step("switch-en", async () => {
    const before = await cdp.eval("performance.timeOrigin");
    await palette(cdp, "Сменить язык");
    await sleep(800);
    const lang = await cdp.eval("document.documentElement.lang");
    const after = await cdp.eval("performance.timeOrigin");
    console.log(`switched: lang=${lang}, reloaded=${before !== after}`);
    await palette(cdp, "Open Panel: Manager");
    await capture(cdp, "52-switched-to-english", `Live switch to English (lang=${lang}, page reloaded: ${before !== after})`);
    await palette(cdp, "Switch Language");
    await sleep(800);
    await capture(cdp, "53-switched-back-to-russian", `Back to Russian (lang=${await cdp.eval("document.documentElement.lang")})`);
  });

  writeFileSync(join(outDir, "manifest.json"), `${JSON.stringify(shots, null, 2)}\n`);
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
