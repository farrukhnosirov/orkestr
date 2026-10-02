/** Commands contributed by the Terminal panel (appended to the command registry). */
import { SquareTerminal } from "lucide-react";

import { workspaceActions } from "../../components/workspaceActions";
import type { AppCommand } from "../../features/commands/registry";
import { useTerminalUi } from "./store";

export const terminalCommands: AppCommand[] = [
  {
    id: "terminal.open",
    title: "Open Terminal",
    group: "Terminal",
    icon: SquareTerminal,
    keywords: ["shell", "console", "pty", "powershell", "cmd", "bash", "терминал", "консоль"],
    shortcut: "Ctrl+`",
    run: () => {
      workspaceActions.openPanel("terminal");
      useTerminalUi.getState().requestOpen();
    },
  },
];
