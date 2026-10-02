/** Commands contributed by the Tasks panel (command registry §54). */
import { ListPlus, Network } from "lucide-react";

import { workspaceActions } from "../../components/workspaceActions";
import type { AppCommand } from "../../features/commands/registry";
import { t } from "../../i18n";
import { useTasksUi } from "./store";

export const taskCommands: AppCommand[] = [
  {
    id: "tasks.create",
    get title() {
      return t("tasks:commands.create");
    },
    get group() {
      return t("tasks:commands.group");
    },
    icon: ListPlus,
    keywords: ["task", "new", "add", "todo", "задача", "создать", "новая"],
    enabled: (ctx) => ctx.projectId !== null,
    run: () => {
      workspaceActions.openPanel("tasks");
      useTasksUi.getState().setCreateOpen(true);
    },
  },
  {
    id: "tasks.openGraph",
    get title() {
      return t("tasks:commands.openGraph");
    },
    get group() {
      return t("tasks:commands.group");
    },
    icon: Network,
    keywords: ["dag", "dependencies", "critical path", "граф", "зависимости", "критический путь"],
    enabled: (ctx) => ctx.projectId !== null,
    run: () => {
      useTasksUi.getState().setView("graph");
      workspaceActions.openPanel("tasks");
    },
  },
];
