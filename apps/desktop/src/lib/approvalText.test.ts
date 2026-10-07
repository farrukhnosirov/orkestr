/** Approval requests (core-written English summary / reason) read in the UI language. */
import { afterEach, describe, expect, it } from "vitest";

import { applyLanguagePreference } from "../i18n";
import { approvalReason, approvalSummary } from "./coreText";

afterEach(async () => {
  await applyLanguagePreference("en");
});

describe("approvalSummary / approvalReason", () => {
  it("keeps English readable and unknown text as is", async () => {
    await applyLanguagePreference("en");
    expect(approvalSummary("Backend Engineer #1 wants to run_command npm test")).toBe("Backend Engineer #1 wants to run a command: npm test");
    expect(approvalSummary("Merge 1 branch(es) into `main`")).toBe("Merge 1 branch into `main`");
    expect(approvalSummary("Deploy the docs site")).toBe("Deploy the docs site");
    expect(approvalReason("destructive git operation")).toBe("destructive git operation");
  });

  it("translates every fixed shape to Russian", async () => {
    await applyLanguagePreference("ru");
    expect(approvalSummary("Backend Engineer #1 wants to run_command npm test")).toMatch(/ #1 хочет выполнить команду: npm test$/);
    expect(approvalSummary("Backend Engineer #1 wants to run_command npm test")).not.toContain("Backend Engineer");
    expect(approvalSummary("Question from QA Engineer #2")).toMatch(/^Вопрос от агента .+ #2$/);
    expect(approvalSummary("Run command (agent terminal)")).toBe("Выполнить команду (терминал агента)");
    expect(approvalSummary("Run command")).toBe("Выполнить команду");
    expect(approvalSummary("Merge 2 branch(es) into `main`")).toBe("Слить 2 ветки в `main`");
    expect(approvalSummary("Merge 5 branch(es) into `main`")).toBe("Слить 5 веток в `main`");
    expect(approvalSummary('Self-healing gave up on "Add login" — continue fixing?')).toBe(
      "Самовосстановление не справилось с задачей «Add login» — продолжить исправление?",
    );
    expect(approvalSummary("Let Backend Engineer #1 run Codex in its sandbox")).toMatch(/^Разрешить агенту .+ #1 запускать Codex в его песочнице$/);
    expect(approvalSummary('Start a QA agent on "Fix tests"')).toBe("Запустить агента (Тестировщик) для задачи «Fix tests»");
    expect(approvalSummary("Create 3 task(s)")).toBe("Создать 3 задачи");

    expect(approvalReason("The agent asks: Which DB?")).toBe("Агент спрашивает: Which DB?");
    expect(approvalReason("The Merge Manager merges agent branches only with your approval (merge policy).")).toBe(
      "Менеджер слияний сливает ветки агентов только с вашего подтверждения (политика слияния).",
    );
    expect(approvalReason("the AI Manager's approval policy is Ask")).toBe("Для AI-менеджера выбран режим подтверждения «Спрашивать»");
    expect(approvalReason("the command uses the network (Network requires approval)")).toBe(
      "команда обращается к сети (тип действия «Сеть» требует подтверждения)",
    );
    expect(approvalReason("Terminal requires approval in SemiAuto mode")).toBe("тип действия «Терминал» требует подтверждения в режиме «Полуавтомат»");
    expect(approvalReason("a permission rule requires approval for Git")).toBe("правило разрешений требует подтверждения для типа действия «Git»");
    expect(
      approvalReason(
        "The quality pipeline could not fix the Test stage automatically: 3 of 3 fix attempt(s) used. Last failure: 2 tests fail. Approve to grant 3 more attempt(s) (a different model is tried); reject to mark the task failed.",
      ),
    ).toBe(
      "Конвейер качества не смог автоматически исправить этап «Тесты»: использовано попыток исправления: 3 из 3. Последний сбой: 2 tests fail. Подтвердите, чтобы выделить ещё 3 попытки (будет выбрана другая модель); отклоните, чтобы пометить задачу как неудачную.",
    );
  });
});
