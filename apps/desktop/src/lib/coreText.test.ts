// Display texts for the core's fixed English strings (lib/coreText.ts) and
// the system-check localizer (features/settings/sections/checkText.ts).
import type { SystemCheck } from "@orkestr/protocol";
import { afterEach, describe, expect, it } from "vitest";

import type { BubbleSpec } from "../features/robots/model/bubble";
import { bubbleText } from "../features/robots/model/bubbleText";
import { checkTexts } from "../features/settings/sections/checkText";
import { resumeHeadline, resumeIssue, resumeLines } from "../panels/Manager/parts";
import { applyLanguagePreference, t } from "../i18n";
import { formatTokens, tokenPluralCount } from "../panels/Agents/model";
import {
  agentActivity,
  agentName,
  agentNote,
  agentToolSummary,
  managerToolSummary,
  planWarning,
  snapshotLabel,
  taskTag,
  templateDescription,
  taskReason,
  templateName,
  terminalTitle,
} from "./coreText";

const nb = (s: string) => s.replace(/\u00a0/g, " ");

const check = (c: Partial<SystemCheck>): SystemCheck => ({ id: "git", title: "Git", status: "ok", detail: "", hint: null, version: null, ...c });

describe("core text in Russian", () => {
  afterEach(async () => {
    await applyLanguagePreference("en");
  });

  it("translates built-in templates, agents, activity and notes", async () => {
    await applyLanguagePreference("ru");
    expect(templateName("QA Engineer")).toBe("Тестировщик");
    expect(templateName("My agent")).toBe("My agent");
    expect(agentName("Backend Engineer #2")).toBe("Бэкенд-разработчик #2");
    expect(agentName("Custom bot #1")).toBe("Custom bot #1");
    expect(templateDescription("Researcher", "Investigates the codebase and documentation to answer questions and compare options; read-only.")).toMatch(/^Изучает код/);
    expect(templateDescription("Researcher", "My own description")).toBe("My own description");
    expect(agentActivity("Finished")).toBe("Работа завершена");
    expect(agentActivity("Waiting for approval: git push")).toBe("Ждёт подтверждения: git push");
    expect(agentActivity("write_file src/a.ts")).toBe("write_file src/a.ts");
    // Fixed activity strings of the agent runtime (runtime.rs, run.rs, cli.rs).
    expect(agentActivity("Spawned")).toBe("Запущен");
    expect(agentActivity("Stopped")).toBe("Остановлен");
    expect(agentActivity("Interrupted: Orkestr stopped while the agent was running")).toBe("Прерван: Orkestr остановился во время работы агента");
    expect(agentActivity("stopped by the user")).toBe("Остановлен пользователем");
    expect(agentActivity("stopped by the user (task cancelled)")).toBe("Остановлен пользователем (задача отменена)");
    expect(agentActivity("emergency stop")).toBe("Экстренная остановка");
    expect(agentActivity("the core is shutting down")).toBe("Ядро завершает работу");
    expect(agentActivity("Running ClaudeCode CLI")).toBe("Работает через CLI Claude Code");
    expect(agentActivity("Running Codex CLI")).toBe("Работает через CLI Codex");
    // Robot speech bubbles localize the core's activity lines they carry.
    const bubble = (key: BubbleSpec["key"], values: BubbleSpec["values"]): BubbleSpec => ({ key, values, tone: "info", ttlMs: 0, important: false, chatty: false });
    expect(bubbleText(bubble("activity", { activity: "Reading the task" }))).toBe("Читает задачу");
    expect(bubbleText(bubble("errorDetail", { detail: "emergency stop" }))).toBe("Ошибка: Экстренная остановка");
    expect(bubbleText(bubble("activity", { activity: "write_file src/a.ts" }))).toBe("write_file src/a.ts");
    expect(agentNote("Started on “Test it” with mock-smart — C:\\w (agent/qa-1).")).toBe(
      "Начата работа над задачей «Test it», модель mock-smart — C:\\w (agent/qa-1).",
    );
    expect(agentToolSummary("created tests/a.test.mjs")).toBe("создан tests/a.test.mjs");
    expect(agentToolSummary("`node --test` → exit 0")).toBe("`node --test` → код 0");
  });

  it("translates snapshot labels, tags, warnings, terminal titles and tool summaries", async () => {
    await applyLanguagePreference("ru");
    expect(snapshotLabel("Before QA Engineer #1 changes")).toBe("Перед изменениями: Тестировщик #1");
    expect(snapshotLabel("Product Manager #1 finished")).toBe("Продакт-менеджер #1: работа завершена");
    expect(snapshotLabel("Before discarding 3 path(s)")).toBe("Перед отменой изменений в 3 файлах");
    expect(snapshotLabel("Before rollback to “Manual snapshot”")).toBe("Перед откатом к „Снимок вручную“");
    expect(snapshotLabel("Мой снимок")).toBe("Мой снимок");
    expect(taskTag("Product Manager")).toBe("Продакт-менеджер");
    expect(taskTag("epic:Hello module")).toBe("эпик: Hello module");
    expect(taskTag("frontend-ui")).toBe("frontend-ui");
    expect(planWarning("task t2: dependency on itself dropped")).toBe("Задача t2: зависимость от самой себя удалена.");
    expect(terminalTitle("Agent commands")).toBe("Команды агента");
    expect(managerToolSummary("plan with 4 task(s)")).toBe("план из 4 задач");
    expect(managerToolSummary("plan with 1 task(s)")).toBe("план из 1 задачи");
    expect(managerToolSummary("5 task(s), 2 running agent(s)")).toBe("задач: 5, работающих агентов: 2");
  });

  it("translates merge snapshots and the remaining agent / Manager tool summaries", async () => {
    await applyLanguagePreference("ru");
    expect(snapshotLabel("Before merge into main")).toBe("Перед слиянием в main");
    expect(agentToolSummary("listed src (12 entries)")).toBe("содержимое src (элементов: 12)");
    expect(agentToolSummary("3 match(es) for /foo/")).toBe("совпадений с /foo/: 3");
    expect(managerToolSummary("started QA Engineer #1")).toBe("запущен агент Тестировщик #1");
    expect(managerToolSummary("stopped Backend Engineer #2")).toBe("остановлен агент Бэкенд-разработчик #2");
    expect(managerToolSummary("QA Engineer #1 is Coding")).toBe("Тестировщик #1: Пишет код");
    expect(managerToolSummary('recorded "Use SQLite"')).toBe("записано решение «Use SQLite»");
    expect(managerToolSummary("Approved")).toBe("Подтверждено");
    expect(managerToolSummary("UI change proposed: Compact layout")).toBe("предложено изменение интерфейса: Compact layout");
    await applyLanguagePreference("en");
    expect(snapshotLabel("Before merge into main")).toBe("Before merge into main");
    expect(managerToolSummary("started QA Engineer #1")).toBe("started QA Engineer #1");
  });

  it("builds the Manager resume banner from its counters", async () => {
    await applyLanguagePreference("ru");
    const base = {
      projectId: "p",
      headline: "Last session: …",
      lines: [],
      tasksTotal: 5,
      tasksCompleted: 3,
      tasksInProgress: 0,
      tasksFailed: 0,
      tasksBlocked: 0,
      tasksCancelled: 1,
      percentComplete: 75,
      goals: [{ planId: "x", goal: "Модуль hello", completed: 3, total: 3, percent: 100 }],
      issues: [],
      pendingApprovals: 0,
      lastSnapshot: null,
      activeRun: null,
      lastActivityAt: null,
      isEmpty: false,
    };
    expect(nb(resumeHeadline(base))).toBe("Прошлый сеанс: выполнено задач — 3 из 4 (75 %).");
    expect(resumeLines(base)).toEqual(["Модуль hello — выполнено"]);
    expect(resumeHeadline({ ...base, isEmpty: true })).toBe("В этом проекте ещё не было сеансов.");
    expect(resumeIssue('"Implement it" is Failed: tests failed')).toBe("«Implement it»: Сбой — tests failed");
    expect(resumeIssue("2 task(s) wait for review")).toBe("2 задачи ждут ревью");
  });

  it("localizes system checks by id and message shape", async () => {
    await applyLanguagePreference("ru");
    expect(checkTexts(check({ detail: "git 2.45.1 at C:\\Git\\git.exe" }))).toEqual({ title: "Git", detail: "git 2.45.1 — C:\\Git\\git.exe" });
    const dir = checkTexts(check({ id: "dataDir", title: "Data directory", detail: "C:\\data — 12.5 GB free", hint: "Free up disk space: snapshots and worktrees need room." }));
    expect(dir.title).toBe("Каталог данных");
    expect(nb(dir.detail)).toBe("C:\\data — свободно 12,5 ГБ");
    expect(dir.hint).toMatch(/^Освободите место/);
    expect(checkTexts(check({ id: "keyring", title: "Secure credential store", detail: "Windows Credential Manager is available" })).detail).toBe(
      "Диспетчер учётных данных Windows: доступно",
    );
    expect(checkTexts(check({ id: "shell", title: "Terminal shell", detail: "PowerShell (default), cmd" })).detail).toBe("PowerShell (по умолчанию), cmd");
    expect(checkTexts(check({ id: "future", title: "Future check", detail: "something new" }))).toEqual({ title: "Future check", detail: "something new" });
  });

  it("uses the genitive plural after compact token counts", async () => {
    await applyLanguagePreference("ru");
    const label = (n: number) => nb(t("agents:list.tokens", { count: tokenPluralCount(n), value: formatTokens(n, "ru") }));
    expect(label(6551)).toBe("6,6 тыс. токенов");
    expect(label(10_302)).toBe("10,3 тыс. токенов");
    expect(label(21)).toBe("21 токен");
    expect(label(3)).toBe("3 токена");
  });

  it("keeps the core's English in English", async () => {
    await applyLanguagePreference("en");
    expect(agentName("QA Engineer #1")).toBe("QA Engineer #1");
    expect(snapshotLabel("Before QA Engineer #1 changes")).toBe("Before QA Engineer #1 changes");
    expect(checkTexts(check({ detail: "git was not found on PATH" })).detail).toBe("git was not found on PATH");
  });

  it("turns task status reasons into readable text", async () => {
    await applyLanguagePreference("ru");
    expect(taskReason(null)).toBeNull();
    expect(taskReason("dependency_pending")).toBe("Ждёт задачи, от которых зависит");
    expect(taskReason("code review requested changes: missing tests; lint")).toBe("Ревью кода запросило доработку: missing tests; lint");
    expect(taskReason("self-healing stopped (attempt limit); the user declined further attempts")).toBe(
      "Самовосстановление остановлено (attempt limit); вы отказались от новых попыток",
    );
    expect(taskReason("max_turns: 40 turns used")).toBe("Достигнут лимит ходов: 40 turns used");
    expect(taskReason("EmergencyStop")).toBe("Остановлена экстренной остановкой");
    expect(taskReason("brand_new_code")).toBeNull();
    expect(taskReason("tests failed in CI")).toBe("tests failed in CI");
    await applyLanguagePreference("en");
    expect(taskReason("agent_result_done")).toBe("The agent reported the task done");
  });
});
