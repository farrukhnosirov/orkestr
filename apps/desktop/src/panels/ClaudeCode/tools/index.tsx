/**
 * Claude Code tool cards: one compact, collapsible card per tool call of the
 * CLI (Read, Write, Edit, MultiEdit, NotebookEdit, Bash / PowerShell,
 * BashOutput, KillShell, Glob, Grep, LS, TodoWrite, WebFetch, WebSearch,
 * Task / Agent, ExitPlanMode, `mcp__*` and an unknown-tool fallback).
 *
 * The header (icon, verb, key argument, status) is cheap; the body — diffs
 * (lazy Monaco), parsed results, Markdown — renders only while expanded.
 */
import { Badge, Spinner } from "@orkestr/ui";
import clsx from "clsx";
import {
  Bot,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  ClipboardList,
  ExternalLink,
  FileText,
  FilePlus2,
  FolderSearch,
  FolderTree,
  Globe,
  ListChecks,
  NotebookPen,
  PencilLine,
  Plug,
  Search,
  SquareTerminal,
  Wrench,
  XCircle,
  type LucideIcon,
} from "lucide-react";
import { useId, useState, type ReactNode } from "react";

import { editPairs } from "../../../features/claudeCode/model";
import { useT } from "../../../i18n";
import type { ToolCardProps } from "../types";
import {
  BashBody,
  EditBody,
  GenericBody,
  LsBody,
  PlanBody,
  ReadBody,
  SearchBody,
  ShellRefBody,
  TaskBody,
  todoProgress,
  TodoBody,
  WebFetchBody,
  WebSearchBody,
  type BodyProps,
} from "./bodies";
import { diffStats, mcpParts, strField, toolKind, type ToolKind } from "./parse";
import { projectPath, RiskBadge, type PathContext } from "./shared";
import "./tools.css";

export type { ToolCallView, ToolCardProps, ToolResultView } from "../types";
export { toolKind, type ToolKind } from "./parse";

const ICONS: Record<ToolKind, LucideIcon> = {
  read: FileText,
  write: FilePlus2,
  edit: PencilLine,
  multiEdit: PencilLine,
  notebookEdit: NotebookPen,
  bash: SquareTerminal,
  bashOutput: SquareTerminal,
  killShell: SquareTerminal,
  glob: FolderSearch,
  grep: Search,
  ls: FolderTree,
  todo: ListChecks,
  webFetch: Globe,
  webSearch: Search,
  task: Bot,
  plan: ClipboardList,
  mcp: Plug,
  unknown: Wrench,
};

const FILE_KINDS: ReadonlySet<ToolKind> = new Set(["read", "write", "edit", "multiEdit", "notebookEdit"]);
const EDIT_KINDS: ReadonlySet<ToolKind> = new Set(["write", "edit", "multiEdit", "notebookEdit"]);

/** Cards that start expanded (cheap bodies the user wants to see). Failures always do. */
function openByDefault(kind: ToolKind, failed: boolean): boolean {
  return failed || kind === "todo" || kind === "plan" || kind === "task";
}

/** Path argument of a file tool. */
function filePathOf(kind: ToolKind, input: ToolCardProps["call"]["input"]): string | null {
  if (kind === "notebookEdit") return strField(input, "notebook_path");
  if (FILE_KINDS.has(kind)) return strField(input, "file_path");
  if (kind === "ls") return strField(input, "path");
  return null;
}

function firstLine(s: string): string {
  const i = s.indexOf("\n");
  return i < 0 ? s : `${s.slice(0, i)} …`;
}

/** Extra props (not in the shared contract): initial expansion override. */
export interface ToolCardExtraProps {
  /** Start expanded / collapsed regardless of the per-tool default. */
  defaultExpanded?: boolean;
}

export function ToolCard({ call, result, root, cwd, onOpenFile, risk, children, defaultExpanded }: ToolCardProps & ToolCardExtraProps) {
  const { t } = useT("claudeCodeTools");
  const bodyId = useId();
  const kind = toolKind(call.name);
  const failed = result?.isError ?? false;
  const done = result !== null;
  const [userOpen, setUserOpen] = useState<boolean | null>(defaultExpanded ?? null);
  const open = userOpen ?? openByDefault(kind, failed);
  const ctx: PathContext = { root, cwd, onOpenFile };
  const input = call.input;

  // ── header pieces (cheap) ──
  let verb: string = t(`verb.${kind}`);
  let subject = "";
  let subjectTitle: string | undefined;
  const extras: ReactNode[] = [];
  const path = filePathOf(kind, input);
  const rel = path ? projectPath(ctx, path) : null;

  switch (kind) {
    case "read":
    case "write":
    case "edit":
    case "multiEdit":
    case "notebookEdit":
    case "ls":
      subject = rel ?? path ?? "";
      subjectTitle = path ?? undefined;
      break;
    case "bash": {
      const command = strField(input, "command") ?? "";
      subject = firstLine(command);
      subjectTitle = command;
      const r = <RiskBadge key="risk" risk={risk} command={command} />;
      extras.push(r);
      break;
    }
    case "bashOutput":
    case "killShell":
      subject = strField(input, "bash_id") ?? strField(input, "shell_id") ?? "";
      break;
    case "glob":
    case "grep":
      subject = strField(input, "pattern") ?? "";
      break;
    case "todo": {
      const p = todoProgress(input);
      subject = t("todo.progress", p);
      break;
    }
    case "webFetch":
      subject = strField(input, "url") ?? "";
      break;
    case "webSearch":
      subject = strField(input, "query") ?? "";
      break;
    case "task": {
      subject = strField(input, "description") ?? "";
      const type = strField(input, "subagent_type");
      if (type) extras.push(<span key="agent" className="cct-chip">{t("task.agent", { type })}</span>);
      break;
    }
    case "plan":
      break;
    case "mcp": {
      const { server, tool } = mcpParts(call.name);
      verb = t("mcp.server", { server });
      subject = tool;
      break;
    }
    case "unknown":
      subject = call.name;
      break;
  }

  if (EDIT_KINDS.has(kind)) {
    const pairs = editPairs(call.name, input);
    if (kind === "multiEdit") extras.push(<span key="n" className="cct-chip">{t("diff.edits", { count: pairs.length })}</span>);
    if (pairs.length > 0) {
      let added = 0;
      let removed = 0;
      for (const [b, a] of pairs) {
        const s = diffStats(b, a);
        added += s.added;
        removed += s.removed;
      }
      extras.push(
        <span key="stats" className="cct-stats">
          <span className="cct-stats__add" aria-label={t("diff.added", { count: added })}>
            +{added}
          </span>
          <span className="cct-stats__del" aria-label={t("diff.removed", { count: removed })}>
            −{removed}
          </span>
        </span>,
      );
    }
  }

  const Icon = ICONS[kind];
  const toggleLabel = open ? t("card.collapse", { tool: call.name }) : t("card.expand", { tool: call.name });

  // ── body (only while open) ──
  let body: ReactNode = null;
  if (open) {
    const props: BodyProps = { name: call.name, input, result, ctx };
    switch (kind) {
      case "read":
        body = <ReadBody {...props} />;
        break;
      case "write":
      case "edit":
      case "multiEdit":
      case "notebookEdit":
        body = <EditBody {...props} />;
        break;
      case "bash":
        body = <BashBody {...props} />;
        break;
      case "bashOutput":
      case "killShell":
        body = <ShellRefBody {...props} />;
        break;
      case "glob":
      case "grep":
        body = <SearchBody {...props} />;
        break;
      case "ls":
        body = <LsBody {...props} />;
        break;
      case "todo":
        body = <TodoBody {...props} />;
        break;
      case "webFetch":
        body = <WebFetchBody {...props} />;
        break;
      case "webSearch":
        body = <WebSearchBody {...props} />;
        break;
      case "task":
        body = <TaskBody {...props}>{children}</TaskBody>;
        break;
      case "plan":
        body = <PlanBody {...props} />;
        break;
      case "mcp":
      case "unknown":
        body = <GenericBody {...props} />;
        break;
    }
  }

  return (
    <div className={clsx("cct-card", failed && "cct-card--error", !done && "cct-card--running")} data-tool={call.name} data-kind={kind}>
      <div className="cct-head">
        <button
          type="button"
          className="cct-head__toggle"
          aria-expanded={open}
          aria-controls={bodyId}
          aria-label={toggleLabel}
          onClick={() => setUserOpen(!open)}
        >
          {open ? <ChevronDown size={14} aria-hidden="true" /> : <ChevronRight size={14} aria-hidden="true" />}
          <Icon size={14} aria-hidden="true" className="cct-head__icon" />
          <span className="cct-head__verb">{verb}</span>
          {subject && (
            <span className={clsx("cct-head__subject", (kind === "bash" || kind === "glob" || kind === "grep") && "cct-mono")} title={subjectTitle ?? subject}>
              {subject}
            </span>
          )}
        </button>
        {extras.length > 0 && <span className="cct-head__extras">{extras}</span>}
        {rel !== null && FILE_KINDS.has(kind) && (
          <button
            type="button"
            className="cct-icon-btn"
            title={t("file.openAt", { path: rel })}
            aria-label={t("file.openAt", { path: rel })}
            onClick={() => onOpenFile(rel)}
          >
            <ExternalLink size={13} aria-hidden="true" />
          </button>
        )}
        <span className="cct-head__state">
          {!done ? (
            <Spinner size={12} label={t("status.running")} />
          ) : failed ? (
            <Badge tone="danger" icon={<XCircle size={12} aria-hidden="true" />}>
              {t("status.error")}
            </Badge>
          ) : (
            <CheckCircle2 size={14} className="cct-ok" role="img" aria-label={t("status.ok")} />
          )}
        </span>
      </div>
      {open && body && (
        <div className="cct-body" id={bodyId}>
          {body}
        </div>
      )}
    </div>
  );
}

export default ToolCard;
