/** Commands contributed by the Terminal panel (appended to the command registry). */
import { SquareTerminal } from "lucide-react";

import { workspaceActions } from "../../components/workspaceActions";
import type { AppCommand } from "../../features/commands/registry";
import { t } from "../../i18n";
import { useTerminalUi } from "./store";

export const terminalCommands: AppCommand[] = [
  {
    id: "terminal.open",
    get title() {
      return t("terminal:commands.open");
    },
    get group() {
      return t("terminal:commands.group");
    },
    icon: SquareTerminal,
    keywords: ["terminal", "shell", "console", "pty", "powershell", "cmd", "bash", "терминал", "консоль", "оболочка", "открыть"],
    shortcut: "Ctrl+`",
    inTerminal: true,
    run: () => {
      workspaceActions.openPanel("terminal");
      useTerminalUi.getState().requestOpen();
    },
  },
];
