/**
 * Quality runner UI (§17, §58): task drawer section, status bar indicator,
 * "Quality gates" settings section and palette commands
 * ("Run tests" Ctrl+Shift+T, "Run build", "Run all checks").
 */
import type { CheckKind } from "@orkestr/protocol";
import { FlaskConical, Hammer, ListChecks, ShieldCheck } from "lucide-react";

import { t } from "../../i18n";
import { describeError } from "../../i18n/errors";
import type { AppCommand, CommandContext } from "../commands/registry";
import { useNotificationsStore } from "../notifications/notificationsStore";
import type { SettingsSection } from "../settings/sections";
import { useSettingsStore } from "../settings/settingsStore";
import { BUILD_RUN_KINDS, TEST_RUN_KINDS } from "./logic";
import { QualitySettings } from "./QualitySettings";
import { startRun } from "./store";

export { QualityIndicator } from "./QualityIndicator";
export { QualitySection } from "./QualitySection";
export { RunResults } from "./RunResults";

export const qualitySettingsSection: SettingsSection = {
  id: "quality",
  get title() {
    return t("quality:settings.title");
  },
  icon: ShieldCheck,
  order: 30,
  component: QualitySettings,
};

type StartedKey = "quality:toast.testsStarted" | "quality:toast.buildStarted" | "quality:toast.checksStarted";

/** Starts checks of the active project from a command and reports the outcome as a toast. */
export async function runFromCommand(ctx: CommandContext, kinds: CheckKind[] | undefined, startedKey: StartedKey): Promise<void> {
  const notify = useNotificationsStore.getState().notify;
  if (!ctx.projectId) {
    notify("Warning", t("quality:toast.noProject"));
    return;
  }
  try {
    await startRun(ctx.projectId, kinds ? { kinds } : {});
    notify("Info", t(startedKey));
  } catch (e) {
    notify("Error", describeError(e));
  }
}

export const qualityCommands: AppCommand[] = [
  {
    id: "quality.runTests",
    get title() {
      return t("quality:commands.runTests");
    },
    get group() {
      return t("quality:commands.group");
    },
    icon: FlaskConical,
    shortcut: "Ctrl+Shift+T",
    keywords: ["test", "tests", "jest", "vitest", "pytest", "cargo test", "тесты", "запустить", "проверка"],
    enabled: (ctx) => ctx.projectId !== null,
    run: (ctx) => runFromCommand(ctx, TEST_RUN_KINDS, "quality:toast.testsStarted"),
  },
  {
    id: "quality.runBuild",
    get title() {
      return t("quality:commands.runBuild");
    },
    get group() {
      return t("quality:commands.group");
    },
    icon: Hammer,
    keywords: ["build", "compile", "cargo build", "npm run build", "сборка", "собрать", "компиляция"],
    enabled: (ctx) => ctx.projectId !== null,
    run: (ctx) => runFromCommand(ctx, BUILD_RUN_KINDS, "quality:toast.buildStarted"),
  },
  {
    id: "quality.runAll",
    get title() {
      return t("quality:commands.runAll");
    },
    get group() {
      return t("quality:commands.group");
    },
    icon: ListChecks,
    keywords: ["quality", "checks", "lint", "typecheck", "качество", "проверки", "линтер"],
    enabled: (ctx) => ctx.projectId !== null,
    run: (ctx) => runFromCommand(ctx, undefined, "quality:toast.checksStarted"),
  },
  {
    id: "quality.openSettings",
    get title() {
      return t("quality:commands.openSettings");
    },
    get group() {
      return t("quality:commands.group");
    },
    icon: ShieldCheck,
    keywords: ["quality gates", "definition of done", "критерии качества", "критерии готовности"],
    run: () => useSettingsStore.getState().openSettings("quality"),
  },
];
