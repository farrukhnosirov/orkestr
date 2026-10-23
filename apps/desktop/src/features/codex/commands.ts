/**
 * Codex commands: the panel's slash commands (handled by Orkestr, never sent
 * to the model as text) and the command palette entries.
 */
import type { ClaudeSlashCommand } from "@orkestr/protocol";
import { MessageSquarePlus, OctagonX, Bot } from "lucide-react";

import { workspaceActions } from "../../components/workspaceActions";
import { t } from "../../i18n";
import type { AppCommand } from "../commands/registry";
import { useCodexStore } from "./store";

/** Panel id. */
export const CODEX_PANEL_ID = "codex";

/** Opens the Codex panel. */
export function openCodexPanel(): void {
  workspaceActions.openPanel(CODEX_PANEL_ID);
}

/** Slash commands of the Codex composer. */
export const CODEX_SLASH = ["new", "resume", "fork", "compact", "model", "status"] as const;
export type CodexSlash = (typeof CODEX_SLASH)[number];

export interface ParsedSlash {
  command: CodexSlash;
  arg: string;
}

/** `/model gpt-5` → `{command: "model", arg: "gpt-5"}`; null for ordinary text / unknown commands. */
export function parseCodexSlash(text: string): ParsedSlash | null {
  const m = /^\/([a-z]+)(?:\s+([\s\S]*))?$/i.exec(text.trim());
  if (!m || !m[1]) return null;
  const name = m[1].toLowerCase();
  if (!(CODEX_SLASH as readonly string[]).includes(name)) return null;
  return { command: name as CodexSlash, arg: (m[2] ?? "").trim() };
}

/** The slash commands in the composer's suggestion format (localized descriptions). */
export function codexSlashCommands(): ClaudeSlashCommand[] {
  return CODEX_SLASH.map((name) => ({
    name,
    description: t(`codex:slash.${name}`),
    argumentHint: name === "model" ? t("codex:slash.modelArg") : name === "resume" ? t("codex:slash.resumeArg") : null,
    builtin: true,
  }));
}

function activeSession(projectId: string | null): string | null {
  return projectId ? (useCodexStore.getState().active[projectId] ?? null) : null;
}

const group = () => t("codex:commands.group");

export const codexCommands: AppCommand[] = [
  {
    id: "codex.open",
    get title() {
      return t("codex:commands.open");
    },
    get group() {
      return group();
    },
    icon: Bot,
    keywords: ["codex", "chatgpt", "openai", "gpt", "chat", "agent", "кодекс", "чат", "подписка"],
    run: () => openCodexPanel(),
  },
  {
    id: "codex.newChat",
    get title() {
      return t("codex:commands.newChat");
    },
    get group() {
      return group();
    },
    icon: MessageSquarePlus,
    keywords: ["codex", "chatgpt", "new chat", "кодекс", "новый чат"],
    enabled: (ctx) => ctx.projectId !== null,
    run: async (ctx) => {
      openCodexPanel();
      if (ctx.projectId) await useCodexStore.getState().start(ctx.projectId);
    },
  },
  {
    id: "codex.interrupt",
    get title() {
      return t("codex:commands.interrupt");
    },
    get group() {
      return group();
    },
    icon: OctagonX,
    keywords: ["codex", "interrupt", "stop", "кодекс", "прервать", "остановить"],
    enabled: (ctx) => {
      const id = activeSession(ctx.projectId);
      return id !== null && (useCodexStore.getState().sessions[id]?.chat.running ?? false);
    },
    run: async (ctx) => {
      const id = activeSession(ctx.projectId);
      if (id) await useCodexStore.getState().interrupt(id);
    },
  },
];
