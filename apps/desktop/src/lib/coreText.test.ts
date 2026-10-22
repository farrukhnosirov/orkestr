// Display texts for the core's fixed English strings (lib/coreText.ts) and
// the system-check localizer (features/settings/sections/checkText.ts).
import type { SystemCheck } from "@orkestr/protocol";
import { afterEach, describe, expect, it } from "vitest";

import { checkTexts } from "../features/settings/sections/checkText";
import { resumeHeadline, resumeIssue, resumeLines } from "../panels/Manager/parts";
import { applyLanguagePreference, t } from "../i18n";
import { formatTokens, tokenPluralCount } from "../panels/Agents/model";
import {
  agentActivity,
  agentName,
  agentNote,
  agentStopDetail,
  agentToolSummary,
  builtinTeamName,
  fallbackReason,
  leadReason,
  managerToolSummary,
  mergeDetail,
  planWarning,
  runReason,
  runTaskError,
  snapshotLabel,
  taskReason,
  taskTag,
  templateDescription,
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
    expect(agentActivity("Reading the task")).toBe("Читает задачу");
    expect(agentActivity("write_file src/a.ts")).toBe("write_file src/a.ts");
    expect(agentNote("Started on “Test it” with mock-smart — C:\\w (agent/qa-1).")).toBe(
      "Начата работа над задачей «Test it», модель mock-smart — C:\\w (agent/qa-1).",
    );
    expect(agentToolSummary("created tests/a.test.mjs")).toBe("создан tests/a.test.mjs");
    expect(agentToolSummary("`node --test` → exit 0")).toBe("`node --test` → код 0");
  });

  it("translates fallback reasons, stop details and task reasons", async () => {
    await applyLanguagePreference("ru");
    expect(agentNote("Provider fallback: a → b (rate_limited). A structured handoff was created.")).toBe(
      "Переход на резервную модель: a → b (лимит запросов). Контекст передан в структурированном виде.",
    );
    expect(fallbackReason("network")).toBe("сетевая ошибка");
    expect(fallbackReason("something_new")).toBe("something_new");
    expect(agentNote("Stopped: stopped by the user")).toBe("Остановлен: остановлен пользователем");
    expect(agentStopDetail("interrupted by a core restart")).toBe("прерван перезапуском ядра");
    expect(agentStopDetail("LIMIT: too many tokens")).toBe("LIMIT: too many tokens");
    expect(taskReason("agent_result_partial")).toBe("Агент сообщил о частичном результате");
    expect(taskReason("EmergencyStop")).toBe("Остановлена экстренной остановкой");
    expect(taskReason("manual")).toBe("manual");
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

  it("translates team leads, built-in team names and lead stop reasons", async () => {
    await applyLanguagePreference("ru");
    expect(builtinTeamName("Web Team")).toBe("Веб-команда");
    expect(builtinTeamName("My crew")).toBe("My crew");
    expect(agentName("Web Team — Software Architect")).toBe(`Веб-команда — ${templateName("Software Architect")}`);
    expect(agentName("My crew — Lead")).toBe("My crew — Lead");
    expect(agentActivity("Coordinating executors")).toBe("Координирует исполнителей");
    expect(agentActivity("Delegated task done")).toBe("Делегированная задача выполнена");
    expect(leadReason("emergency stop")).toBe("Экстренная остановка");
    expect(nb(leadReason("budget exhausted: $1.2500 spent of $1.00"))).toBe("Бюджет исчерпан: потрачено 1,25 $ из 1,00 $");
    expect(leadReason("disk full")).toBe("disk full");
    const delegated = taskReason("delegated to the lead of Web Team") ?? "";
    expect(delegated).toContain("Веб-команда");
    expect(delegated).not.toContain("Web Team");
    expect(taskReason("waiting for the user's acceptance")).toBe("Ждёт вашей приёмки");
  });

  it("translates merge status details instead of showing Debug names", async () => {
    await applyLanguagePreference("ru");
    expect(mergeDetail("Rejected")).toBe(t("common:status.approval.Rejected"));
    expect(mergeDetail("Expired: no answer")).toBe(`${t("common:status.approval.Expired")}: no answer`);
    expect(mergeDetail("Orkestr stopped while the merge was running")).toBe("Orkestr остановился во время слияния");
    expect(mergeDetail("approval failed: db locked")).toBe("Не удалось запросить подтверждение: db locked");
    expect(mergeDetail("merging agent/x failed: boom")).toBe("merging agent/x failed: boom");
  });
});

describe("AI Manager run reasons", () => {
  afterEach(async () => {
    await applyLanguagePreference("en");
  });

  it("are shown in Russian with locale numbers and money", async () => {
    await applyLanguagePreference("ru");
    expect(runReason("paused by the user — no new agents start")).toBe("Приостановлен вами — новые агенты не запускаются");
    expect(runReason("cancelled by the user")).toBe("Отменён вами");
    expect(runReason("emergency stop")).toBe("Экстренная остановка");
    expect(runReason("Orkestr stopped while the run was active — resume it to continue")).toMatch(/^Orkestr остановился во время запуска/);
    expect(runReason("paused by the user when Orkestr stopped — resume it to continue")).toMatch(/^Был приостановлен/);
    expect(nb(runReason("budget exhausted: $0.5012 spent of $0.50") ?? "")).toMatch(/^Бюджет исчерпан: потрачено 0,5012 \$ из 0,50 \$$/);
    expect(runReason("time limit of 30 min reached")).toBe("Достигнут лимит времени: 30 мин");
    expect(runReason("3 consecutive task failures (limit 3)")).toBe("3 задачи подряд завершились ошибкой (лимит 3)");
    expect(runReason("2 task(s) were cancelled")).toBe("2 задачи отменены");
    expect(runReason("1 task(s) wait for your review; 2 agent(s) wait for an approval")).toBe(
      "1 задача ждёт вашего ревью; 2 агента ждут подтверждения",
    );
    expect(runReason('no task can proceed: "Build API" (Failed), "Docs" (Blocked)')).toBe(
      `Ни одна задача не может продолжиться: «Build API» (${t("common:status.task.Failed")}), «Docs» (${t("common:status.task.Blocked")})`,
    );
    expect(runReason("waiting: the run is paused")).toBe(t("scheduler:reason.waiting", { reason: t("scheduler:reason.paused") }));
    expect(runReason("3 task(s) waiting: global parallel limit 2/2")).toBe(
      `3 задачи — ${t("scheduler:reason.waiting", { reason: t("scheduler:reason.globalLimit", { running: 2, limit: 2 }) })}`,
    );
    // Unknown parts (errors, notes) stay as written; known ones around them are translated.
    expect(runReason("1 task(s) wait for your review; something custom")).toBe("1 задача ждёт вашего ревью; something custom");
    expect(runReason("provider exploded")).toBe("provider exploded");
    expect(runReason(null)).toBeNull();
    // Team lead runs.
    expect(runReason("the Manager run was cancelled")).toBe("Запуск менеджера отменён");
    expect(runReason('sub-task "Write tests" failed after 2 attempt(s)')).toBe("Подзадача «Write tests» не выполнена после 2 попыток");
  });

  it("localizes per-task errors of a run", async () => {
    await applyLanguagePreference("ru");
    expect(runTaskError("the task was deleted")).toBe("Задача удалена");
    expect(runTaskError("ended Failed")).toBe(`Завершилась со статусом «${t("common:status.task.Failed")}»`);
    expect(runTaskError("stopped by the user")).toBe(taskReason("stopped by the user"));
    expect(runTaskError("disk full")).toBe("disk full");
    await applyLanguagePreference("en");
    expect(runReason("1 task(s) wait for your review")).toBe("1 task waits for your review");
    expect(runReason("budget exhausted: $0.5012 spent of $0.50")).toBe("Budget exhausted: $0.5012 spent of $0.50");
  });

  // Audit r2: run/project budget codes were shown raw, and known limit codes
  // embedded the core's English message (mixed language, US money format).
  it("localizes every run-limit reason and its numbers", async () => {
    await applyLanguagePreference("ru");
    expect(nb(taskReason("run_budget_exceeded: cost budget exhausted ($0.2501 spent of $0.25)")!)).toBe(
      "Бюджет запуска исчерпан: потрачено 0,2501 $ из 0,25 $",
    );
    expect(nb(taskReason("budget_exceeded: cost budget exhausted ($0.0042 spent of $0.00)")!)).toBe(
      "Бюджет исчерпан: потрачено 0,0042 $ из 0,00 $",
    );
    expect(taskReason("project_budget_exceeded: a project/global spending budget would be exceeded")).toBe(
      "Бюджет расходов исчерпан: следующий запрос превысил бы бюджет проекта или глобальный бюджет",
    );
    expect(nb(taskReason("budget_exceeded: token budget exhausted (120500 of 100000 tokens used)")!)).toBe(
      "Бюджет исчерпан: использовано токенов: 120 500 из 100 000",
    );
    expect(taskReason("max_turns: turn limit of 30 reached without submit_result")).toBe("Достигнут лимит ходов: нет результата за 30 ходов");
    expect(taskReason("max_turns: turn limit of 1 reached without submit_result")).toBe("Достигнут лимит ходов: нет результата за 1 ход");
    expect(nb(taskReason("time_limit: wall-clock limit reached after 125s")!)).toBe("Достигнут лимит времени: прошло 2 мин 5 с");
    // AgentStopped detail and the transcript note carry the same text.
    expect(agentStopDetail("run_budget_exceeded: cost budget exhausted ($0.2501 spent of $0.25)")).not.toContain("run_budget_exceeded");
    expect(agentNote("Stopped: max_turns: turn limit of 30 reached without submit_result")).toContain("нет результата за 30 ходов");
    expect(agentStopDetail("provider exploded")).toBe("provider exploded");
    await applyLanguagePreference("en");
    expect(taskReason("run_budget_exceeded: cost budget exhausted ($0.2501 spent of $0.25)")).toBe("Run budget exceeded: $0.2501 spent of $0.25");
    expect(taskReason("max_turns: turn limit of 1 reached without submit_result")).toBe("Turn limit reached: no result after 1 turn");
  });
});
