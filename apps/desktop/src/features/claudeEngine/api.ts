/**
 * Claude Code as Orkestr's engine (`claudeEngine.*`): provider status, the
 * one-click "use Claude Code for everything", and Claude Code's sub-agents.
 * All data comes from the core over RPC.
 */
import type {
  ClaudeAgentFileResult,
  ClaudeAgentScope,
  ClaudeAgentsResult,
  ClaudeEngineStatus,
  SaveClaudeAgentParams,
  UseClaudeForEverythingResult,
} from "@orkestr/protocol";

import { getClient } from "../../core/client";

/** Model aliases offered for Claude Code. */
export const CLAUDE_MODELS = ["sonnet", "opus", "haiku"] as const;

export function engineStatus(refresh = false): Promise<ClaudeEngineStatus> {
  return getClient().call("claudeEngine.status", { refresh });
}

/** Saves the Claude Code model (survives restarts; moves what already runs on Claude Code). */
export function setClaudeModel(model: string): Promise<ClaudeEngineStatus> {
  return getClient().call("claudeEngine.setModel", { model });
}

export function applyClaudeForEverything(model: string): Promise<UseClaudeForEverythingResult> {
  return getClient().call("claudeEngine.useForEverything", { model });
}

export function listClaudeAgents(projectId: string | null, refresh = false): Promise<ClaudeAgentsResult> {
  return getClient().call("claudeEngine.agents", { projectId, refresh });
}

export function saveClaudeAgent(params: SaveClaudeAgentParams): Promise<ClaudeAgentFileResult> {
  return getClient().call("claudeEngine.saveAgent", params);
}

export function deleteClaudeAgent(
  projectId: string | null,
  scope: ClaudeAgentScope,
  name: string,
): Promise<ClaudeAgentFileResult> {
  return getClient().call("claudeEngine.deleteAgent", { projectId, scope, name, confirmed: true });
}

/** Valid file name of a Claude Code agent (`code-reviewer`). */
export function isValidAgentName(name: string): boolean {
  return /^[a-z0-9][a-z0-9_-]{0,63}$/.test(name);
}

/** "Read, Grep" → ["Read", "Grep"]. */
export function parseTools(text: string): string[] {
  return text
    .split(/[,\n]/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}
