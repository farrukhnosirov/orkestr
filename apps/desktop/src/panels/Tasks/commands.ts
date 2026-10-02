/** Commands contributed by the Tasks panel (command registry §54). */
import { ListPlus, Network } from "lucide-react";

import { workspaceActions } from "../../components/workspaceActions";
import type { AppCommand } from "../../features/commands/registry";
import { useTasksUi } from "./store";

export const taskCommands: AppCommand[] = [
  {
    id: "tasks.create",
    title: "Create Task",
    group: "Tasks",
    icon: ListPlus,
    keywords: ["new", "add", "todo", "задача"],
    enabled: (ctx) => ctx.projectId !== null,
    run: () => {
      workspaceActions.openPanel("tasks");
      useTasksUi.getState().setCreateOpen(true);
    },
  },
  {
    id: "tasks.openGraph",
    title: "Open Task Graph",
    group: "Tasks",
    icon: Network,
    keywords: ["dag", "dependencies", "critical path", "граф"],
    enabled: (ctx) => ctx.projectId !== null,
    run: () => {
      useTasksUi.getState().setView("graph");
      workspaceActions.openPanel("tasks");
    },
  },
];
