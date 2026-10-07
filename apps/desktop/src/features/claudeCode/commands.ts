/** Palette commands of the Claude Code panel. */
import { MessageSquarePlus, MessageSquareQuote, OctagonX, Sparkle } from "lucide-react";

import type { AppCommand } from "../commands/registry";
import { t } from "../../i18n";
import { askClaudeAboutCurrentSelection, openClaudePanel } from "./bridge";
import { useClaudeStore } from "./store";

/** Opens the Claude Code chat (non-clashing with the registry; checked by tests). */
export const CLAUDE_SHORTCUT = "Ctrl+Alt+C";

function activeSession(projectId: string | null): string | null {
  if (!projectId) return null;
  const st = useClaudeStore.getState();
  return st.active[projectId] ?? null;
}

const group = () => t("claudeCode:commands.group");

export const claudeCodeCommands: AppCommand[] = [
  {
    id: "claudeCode.open",
    get title() {
      return t("claudeCode:commands.open");
    },
    get group() {
      return group();
    },
    icon: Sparkle,
    keywords: ["claude", "claude code", "anthropic", "chat", "agent", "ai", "клод", "чат", "агент", "открыть"],
    shortcut: CLAUDE_SHORTCUT,
    run: () => openClaudePanel(),
  },
  {
    id: "claudeCode.newChat",
    get title() {
      return t("claudeCode:commands.newChat");
    },
    get group() {
      return group();
    },
    icon: MessageSquarePlus,
    keywords: ["claude", "new chat", "session", "клод", "новый чат", "сеанс"],
    enabled: (ctx) => ctx.projectId !== null,
    run: async (ctx) => {
      openClaudePanel();
      if (ctx.projectId) await useClaudeStore.getState().start(ctx.projectId);
    },
  },
  {
    id: "claudeCode.askSelection",
    get title() {
      return t("claudeCode:commands.askSelection");
    },
    get group() {
      return group();
    },
    icon: MessageSquareQuote,
    keywords: ["claude", "selection", "ask", "explain", "code", "клод", "выделенное", "спросить", "код"],
    run: () => askClaudeAboutCurrentSelection(),
  },
  {
    id: "claudeCode.interrupt",
    get title() {
      return t("claudeCode:commands.interrupt");
    },
    get group() {
      return group();
    },
    icon: OctagonX,
    keywords: ["claude", "interrupt", "stop", "cancel", "клод", "прервать", "остановить"],
    enabled: (ctx) => {
      const id = activeSession(ctx.projectId);
      return id !== null && (useClaudeStore.getState().sessions[id]?.chat.running ?? false);
    },
    run: async (ctx) => {
      const id = activeSession(ctx.projectId);
      if (id) await useClaudeStore.getState().interrupt(id);
    },
  },
];
