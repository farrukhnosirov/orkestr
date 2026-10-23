/**
 * Command registry (§54). Features append commands here; the command palette,
 * keyboard shortcuts and voice commands (Phase 3) all read from it.
 */
import type { LucideIcon } from "lucide-react";

import { coreCommands } from "./coreCommands";
import { providerCommands } from "../providers";
import { taskCommands } from "../../panels/Tasks/commands";
import { agentCommands } from "../../panels/Agents/commands";
import { managerCommands } from "../../panels/Manager/commands";
import { GIT_COMMANDS } from "../../panels/Git/commands";
import { terminalCommands } from "../../panels/Terminal/commands";
import { FILE_COMMANDS } from "../files/commands";
import { qualityCommands } from "../quality";
import { hudCommands } from "../hud/commands";
import { voiceCommands } from "../voice";
import { WORKBENCH_COMMANDS } from "../workbench/commands";
import { vscodeCommands } from "../vscode";
import { claudeCodeCommands } from "../claudeCode/commands";
import { codexCommands } from "../codex/commands";

export interface CommandContext {
  projectId: string | null;
}

export interface AppCommand {
  /** Stable id, e.g. `project.create`. */
  id: string;
  /**
   * Localized title. Define it as a getter so it follows live language
   * switches: `get title() { return t("shell:commands.projectCreate"); }`.
   */
  readonly title: string;
  /** Localized grouping in the palette (getter, like `title`). */
  readonly group: string;
  icon?: LucideIcon;
  /** Extra search terms: include both English and Russian words so either finds the command. */
  keywords?: string[];
  /** Shortcut, e.g. `Ctrl+Shift+T` (dispatched globally via `matchesShortcut`). */
  shortcut?: string;
  /**
   * Whether the shortcut also fires while focus is in a terminal. By default
   * terminal keystrokes go to the shell (Ctrl+P is previous-history, Ctrl+1..5
   * and Ctrl+Shift+… are used by TUIs), so only keys the shell never needs —
   * the terminal toggle, the emergency stop, F11 — opt in.
   */
  inTerminal?: boolean;
  /** Whether the command can run now. */
  enabled?: (ctx: CommandContext) => boolean;
  run: (ctx: CommandContext) => void | Promise<void>;
}

export const commands: AppCommand[] = [...coreCommands];
commands.push(...providerCommands);
commands.push(...taskCommands);
commands.push(...agentCommands);
commands.push(...managerCommands);

// Git / snapshots (Phase 1, vcs-snapshots)
commands.push(...GIT_COMMANDS);
commands.push(...terminalCommands);

/** Layout-independent shortcut matching (works on Cyrillic layouts too). */
export { matchesShortcut, shortcutKey } from "./keys";

// files-editor (Phase 1)
commands.push(...FILE_COMMANDS);

// quality runner (Phase 2)
commands.push(...qualityCommands);
// HUD, Performance Mode, AI-editable UI (Phase 3)
commands.push(...hudCommands);
// voice-sound (Phase 3)
commands.push(...voiceCommands);
// File menu / Welcome page (VS Code-like project & file management)
commands.push(...WORKBENCH_COMMANDS);
// Embedded VS Code
commands.push(...vscodeCommands);
// Claude Code panel
commands.push(...claudeCodeCommands);
// Codex panel (ChatGPT subscription)
commands.push(...codexCommands);
