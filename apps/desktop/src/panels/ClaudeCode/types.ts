/**
 * Shared prop types of the Claude Code panel parts (shell ↔ tools ↔
 * composer/permissions). The chat model (event folding) lives in
 * `features/claudeCode/model.ts`; the store (RPC + events) in
 * `features/claudeCode/store.ts`.
 *
 * Reference implementations already exist in this folder (`ToolCard.tsx`,
 * `PermissionCard.tsx`, `Composer.tsx`, `InlineDiff.tsx`, `MonacoDiff.tsx`);
 * `tools/`, `composer/` and `permissions/` may refine and replace them while
 * keeping these contracts.
 */
import type {
  ClaudeModelOption,
  ClaudePermissionDecision,
  ClaudePermissionMode,
  ClaudeRisk,
  ClaudeSlashCommand,
  JsonValue,
} from "@orkestr/protocol";
import type { ReactNode } from "react";

import type { PermissionItem } from "../../features/claudeCode/model";

/** One tool call (from a `toolUse` event). */
export interface ToolCallView {
  /** Tool call id (`toolu_…`). */
  id: string;
  /** Tool name: `Read`, `Edit`, `Write`, `MultiEdit`, `NotebookEdit`, `Bash`, `PowerShell`, `Glob`, `Grep`,
   * `TodoWrite`, `WebFetch`, `WebSearch`, `Task` / `Agent`, `mcp__…`. */
  name: string;
  /** Raw tool input as the CLI sent it (e.g. `{file_path, old_string, new_string}`). */
  input: JsonValue;
}

/** Result of a tool call (`null` while it runs). Text is already truncated by the core. */
export type ToolResultView = { content: string; isError: boolean } | null;

export interface ToolCardProps {
  call: ToolCallView;
  result: ToolResultView;
  /** Project root (absolute) — to show / open project-relative paths. */
  root: string | null;
  /** Session working directory (absolute). */
  cwd: string | null;
  /** Opens a project-relative path in the Code panel. */
  onOpenFile: (relPath: string, line?: number) => void;
  /** Guard classification of a shell command, when a permission request carried one. */
  risk?: ClaudeRisk | null;
  /** Rendered items of a sub-agent (`Task`), nested in its card. */
  children?: ReactNode;
}

/** A permission request card (status `pending` until answered). */
export type PermissionRequestView = PermissionItem;

export interface PermissionCardProps {
  request: PermissionRequestView;
  root: string | null;
  cwd: string | null;
  onRespond: (decision: ClaudePermissionDecision, message?: string) => void;
}

/** What the composer sends. */
export interface ComposerSendPayload {
  /** Message text (slash commands pass through as typed; `@path` mentions inline). */
  text: string;
}

export interface ComposerProps {
  projectId: string;
  /** Session ended / not started. */
  disabled: boolean;
  /** Claude is working: Enter queues, Esc / the stop button interrupts. */
  running: boolean;
  /** Queued messages (sent one by one after each turn). */
  queue: string[];
  /** Slash commands from the CLI. */
  commands: ClaudeSlashCommand[];
  /** Text to insert (editor selection), or null. */
  insert: string | null;
  onInsertConsumed: () => void;
  /** Resolves true when sent or queued (the composer then clears). */
  onSend: (payload: ComposerSendPayload) => Promise<boolean>;
  onInterrupt: () => void;
  onDequeue: (index: number) => void;
}

export interface ModeSwitcherProps {
  mode: ClaudePermissionMode;
  disabled?: boolean;
  /** `bypassPermissions` must be confirmed by the user (danger dialog) → `confirmBypass: true`. */
  onChange: (mode: ClaudePermissionMode, confirmBypass?: boolean) => void;
}

export interface ModelPickerProps {
  /** Current model value (`default`, `sonnet`, `opus`, `haiku`, or a full id). */
  value: string;
  /** Models reported by the CLI (empty before the session answered). */
  models: ClaudeModelOption[];
  disabled?: boolean;
  onChange: (model: string) => void;
}
