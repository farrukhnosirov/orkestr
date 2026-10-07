/**
 * Collapsible card of one tool call: Read (path), Edit / Write / MultiEdit
 * (diff + open file), Bash / PowerShell (command + output), Glob / Grep
 * (results), TodoWrite (checklist), WebFetch / WebSearch, Task (sub-agent,
 * nested items) and a generic JSON view for anything else (MCP tools).
 */
import { Badge, Spinner } from "@orkestr/ui";
import clsx from "clsx";
import {
  Bot,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Circle,
  CircleDot,
  FileText,
  FolderSearch,
  Globe,
  ListChecks,
  PencilLine,
  Search,
  SquareTerminal,
  Wrench,
  XCircle,
} from "lucide-react";
import type { ReactNode } from "react";
import { useState } from "react";

import type { ToolCallView, ToolCardProps } from "./types";

import { useT } from "../../i18n";
import { editPairs, field, relativeTo, todos, toolSubject } from "../../features/claudeCode/model";
import { InlineDiff } from "./InlineDiff";

const EDIT_TOOLS = new Set(["Edit", "MultiEdit", "Write", "NotebookEdit"]);
const SHELL_TOOLS = new Set(["Bash", "PowerShell"]);

function iconOf(name: string) {
  if (name === "Read") return <FileText size={14} />;
  if (EDIT_TOOLS.has(name)) return <PencilLine size={14} />;
  if (SHELL_TOOLS.has(name)) return <SquareTerminal size={14} />;
  if (name === "Glob") return <FolderSearch size={14} />;
  if (name === "Grep" || name === "WebSearch") return <Search size={14} />;
  if (name === "WebFetch") return <Globe size={14} />;
  if (name === "TodoWrite") return <ListChecks size={14} />;
  if (name === "Task" || name === "Agent") return <Bot size={14} />;
  return <Wrench size={14} />;
}

/** Tools whose card starts expanded. */
function openByDefault(name: string): boolean {
  return EDIT_TOOLS.has(name) || SHELL_TOOLS.has(name) || name === "TodoWrite" || name === "Task" || name === "Agent";
}

function Output({ text, isError }: { text: string; isError: boolean }) {
  const { t } = useT("claudeCode");
  const [all, setAll] = useState(false);
  if (!text.trim()) return <div className="cc-muted">{t("tool.noOutput")}</div>;
  const lines = text.split("\n");
  const shown = all ? text : lines.slice(0, 30).join("\n");
  return (
    <>
      <pre className={clsx("cc-output", isError && "cc-output--error")}>{shown}</pre>
      {!all && lines.length > 30 && (
        <button type="button" className="cc-link" onClick={() => setAll(true)}>
          {t("tool.showAllLines", { count: lines.length })}
        </button>
      )}
    </>
  );
}

function Todos({ item }: { item: ToolCallView }) {
  const { t } = useT("claudeCode");
  const list = todos(item.input);
  return (
    <ul className="cc-todos" aria-label={t("tool.todos")}>
      {list.map((td, i) => (
        <li key={i} className={clsx("cc-todo", `cc-todo--${td.status}`)}>
          {td.status === "completed" ? <CheckCircle2 size={14} /> : td.status === "in_progress" ? <CircleDot size={14} /> : <Circle size={14} />}
          <span>{td.content}</span>
        </li>
      ))}
    </ul>
  );
}

export function ToolCard({ call: item, result, root, cwd, onOpenFile, risk, children }: ToolCardProps) {
  const { t } = useT("claudeCode");
  const [open, setOpen] = useState(openByDefault(item.name));
  const subject = toolSubject(item.name, item.input);
  const filePath = typeof field(item.input, "file_path") === "string" ? (field(item.input, "file_path") as string) : null;
  const rel = filePath ? relativeTo(root, filePath, cwd) : null;
  const shownSubject = rel ?? subject;
  const done = result !== null;
  const failed = result?.isError ?? false;

  let body: ReactNode = null;
  if (EDIT_TOOLS.has(item.name)) {
    const pairs = editPairs(item.name, item.input);
    body = (
      <>
        {pairs.map(([before, after], i) => (
          <InlineDiff key={i} before={before} after={after} path={filePath ?? ""} />
        ))}
        {result?.isError && <Output text={result.content} isError />}
      </>
    );
  } else if (SHELL_TOOLS.has(item.name)) {
    const description = field(item.input, "description");
    body = (
      <>
        {typeof description === "string" && <div className="cc-muted">{description}</div>}
        {risk && (
          <Badge tone={risk.level === "forbidden" ? "danger" : "warning"} title={risk.reason}>
            {t(`permission.risk.${risk.level}`)}
          </Badge>
        )}
        <pre className="cc-command">{subject}</pre>
        {result && <Output text={result.content} isError={result.isError} />}
      </>
    );
  } else if (item.name === "TodoWrite") {
    body = <Todos item={item} />;
  } else if (item.name === "Task" || item.name === "Agent") {
    const prompt = field(item.input, "prompt");
    body = (
      <>
        {typeof prompt === "string" && <div className="cc-muted cc-clamp">{prompt}</div>}
        {children && <div className="cc-subagent">{children}</div>}
        {result && <Output text={result.content} isError={result.isError} />}
      </>
    );
  } else if (["Read", "Glob", "Grep", "WebFetch", "WebSearch", "LS"].includes(item.name)) {
    body = result ? <Output text={result.content} isError={result.isError} /> : null;
  } else {
    body = (
      <>
        <pre className="cc-output">{JSON.stringify(item.input, null, 2)}</pre>
        {result && <Output text={result.content} isError={result.isError} />}
      </>
    );
  }

  return (
    <div className={clsx("cc-tool", failed && "cc-tool--error")} data-tool={item.name}>
      <div className="cc-tool__head">
        <button
          type="button"
          className="cc-tool__toggle"
          aria-expanded={open}
          onClick={() => setOpen((v) => !v)}
          aria-label={open ? t("tool.collapse") : t("tool.expand")}
        >
          {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          {iconOf(item.name)}
          <span className="cc-tool__name">{item.name}</span>
        </button>
        {rel !== null && rel !== "" ? (
          <button type="button" className="cc-tool__subject cc-link" title={t("tool.openFile")} onClick={() => onOpenFile(rel)}>
            {shownSubject}
          </button>
        ) : (
          <span className="cc-tool__subject" title={subject}>
            {shownSubject}
          </span>
        )}
        <span className="cc-tool__state">
          {!done ? (
            <Spinner size={12} label={t("tool.running")} />
          ) : failed ? (
            <Badge tone="danger" icon={<XCircle size={12} />}>
              {t("tool.failed")}
            </Badge>
          ) : (
            <CheckCircle2 size={14} className="cc-ok" aria-label={t("tool.done")} />
          )}
        </span>
      </div>
      {open && body && <div className="cc-tool__body">{body}</div>}
    </div>
  );
}
