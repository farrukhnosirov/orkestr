/**
 * Expanded bodies of the tool cards, one per tool family. They are rendered
 * only while a card is open, so parsing / diffing never runs for collapsed
 * cards (virtualized lists stay cheap).
 */
import type { JsonValue } from "@orkestr/protocol";
import clsx from "clsx";
import { CheckCircle2, Circle, CircleDot, ExternalLink, FileText, Folder } from "lucide-react";
import { useState, type ReactNode } from "react";

import { editPairs, field, todos } from "../../../features/claudeCode/model";
import { SafeMarkdown } from "../../../features/extensions/markdown";
import { useT } from "../../../i18n";
import type { ToolResultView } from "../types";
import {
  boolField,
  numField,
  parseGrepOutput,
  parseLsOutput,
  parsePathList,
  parseReadOutput,
  parseWebSearch,
  strField,
  type ResultEntry,
} from "./parse";
import { DiffBlock, Field, Output, PathButton, projectPath, type PathContext } from "./shared";

export interface BodyProps {
  name: string;
  input: JsonValue;
  result: ToolResultView;
  ctx: PathContext;
}

/** Error / plain result text under a body (nothing while running). */
function ResultText({ result }: { result: ToolResultView }) {
  if (!result) return null;
  return <Output text={result.content} isError={result.isError} />;
}

function OpenFileButton({ ctx, path, line }: { ctx: PathContext; path: string; line?: number }) {
  const { t } = useT("claudeCodeTools");
  const rel = projectPath(ctx, path);
  if (rel === null) return null;
  return (
    <button type="button" className="cct-btn" onClick={() => ctx.onOpenFile(rel, line)}>
      <ExternalLink size={12} aria-hidden="true" />
      {t("file.open")}
    </button>
  );
}

// ── Edit / MultiEdit / Write / NotebookEdit ──────────────────────────────

export function EditBody({ name, input, result, ctx }: BodyProps) {
  const { t } = useT("claudeCodeTools");
  const path = strField(input, name === "NotebookEdit" ? "notebook_path" : "file_path") ?? "";
  let pairs = editPairs(name, input);
  let note: string | null = null;
  if (name === "NotebookEdit") {
    const mode = strField(input, "edit_mode") ?? "replace";
    const src = strField(input, "new_source") ?? "";
    pairs = mode === "delete" ? [] : [["", src]];
    note = mode === "insert" ? t("notebook.insert") : mode === "delete" ? t("notebook.delete") : t("notebook.replace");
    const cell = strField(input, "cell_id");
    if (cell) note = `${note} · ${t("notebook.cell", { id: cell })}`;
  } else if (name === "Write") {
    note = t("file.newFile");
  } else if (boolField(input, "replace_all")) {
    note = t("diff.replaceAll");
  }
  return (
    <>
      <div className="cct-row">
        <PathButton ctx={ctx} path={path} />
        {note && <span className="cct-chip">{note}</span>}
        <span className="cct-spacer" />
        <OpenFileButton ctx={ctx} path={path} />
      </div>
      {pairs.map(([before, after], i) => (
        <DiffBlock
          key={i}
          before={before}
          after={after}
          path={path}
          title={pairs.length > 1 ? t("diff.editN", { n: i + 1, total: pairs.length }) : undefined}
        />
      ))}
      {result?.isError && <Output text={result.content} isError />}
    </>
  );
}

// ── Read ─────────────────────────────────────────────────────────────────

const READ_PREVIEW = 12;

export function readRange(input: JsonValue): { from: number; to: number | null } | null {
  const offset = numField(input, "offset");
  const limit = numField(input, "limit");
  if (offset === null && limit === null) return null;
  const from = offset ?? 1;
  return { from, to: limit !== null ? from + limit - 1 : null };
}

export function ReadBody({ input, result, ctx }: BodyProps) {
  const { t } = useT("claudeCodeTools");
  const path = strField(input, "file_path") ?? "";
  const range = readRange(input);
  const rangeText = !range ? t("read.wholeFile") : range.to === null ? t("read.fromLine", { from: range.from }) : t("read.lines", range);
  const lines = result && !result.isError ? parseReadOutput(result.content) : [];
  const shown = lines.slice(0, READ_PREVIEW);
  return (
    <>
      <div className="cct-row">
        <PathButton ctx={ctx} path={path} line={range?.from} />
        <span className="cct-chip">{rangeText}</span>
        <span className="cct-spacer" />
        <OpenFileButton ctx={ctx} path={path} line={range?.from} />
      </div>
      {result?.isError && <Output text={result.content} isError />}
      {shown.length > 0 && (
        <div className="cct-snippet" role="figure" aria-label={t("read.preview")}>
          {shown.map((l) => (
            <div key={l.line} className="cct-snippet__line">
              <span className="cct-snippet__no" aria-hidden="true">
                {l.line}
              </span>
              <span className="cct-snippet__text">{l.text}</span>
            </div>
          ))}
          {lines.length > shown.length && <div className="cct-muted">{t("read.moreLines", { count: lines.length - shown.length })}</div>}
        </div>
      )}
      {result && !result.isError && lines.length === 0 && <Output text={result.content} isError={false} lines={READ_PREVIEW} />}
    </>
  );
}

// ── Bash / PowerShell / BashOutput / KillShell ───────────────────────────

export function BashBody({ input, result }: BodyProps) {
  const { t } = useT("claudeCodeTools");
  const command = strField(input, "command") ?? "";
  const description = strField(input, "description");
  return (
    <>
      {description && <div className="cct-muted">{description}</div>}
      {boolField(input, "run_in_background") && (
        <div className="cct-row">
          <span className="cct-chip">{t("bash.background")}</span>
        </div>
      )}
      <pre className="cct-command">{command}</pre>
      <ResultText result={result} />
    </>
  );
}

export function ShellRefBody({ input, result }: BodyProps) {
  const { t } = useT("claudeCodeTools");
  const id = strField(input, "bash_id") ?? strField(input, "shell_id");
  const filter = strField(input, "filter");
  return (
    <>
      <div className="cct-row">
        {id && <span className="cct-chip">{t("bash.shell", { id })}</span>}
        {filter && <span className="cct-chip">{t("bash.filter", { filter })}</span>}
      </div>
      <ResultText result={result} />
    </>
  );
}

// ── Glob / Grep / LS ─────────────────────────────────────────────────────

const LIST_PREVIEW = 30;

function ResultList({ entries, ctx, base }: { entries: ResultEntry[]; ctx: PathContext; base: string | null }) {
  const { t } = useT("claudeCodeTools");
  const [all, setAll] = useState(false);
  if (entries.length === 0) return <div className="cct-muted">{t("results.none")}</div>;
  const shown = all ? entries : entries.slice(0, LIST_PREVIEW);
  const resolve = (p: string) => (base && !/^([a-zA-Z]:[\\/]|\/)/.test(p) ? `${base.replace(/[\\/]+$/, "")}/${p}` : p);
  return (
    <>
      <ul className="cct-list" aria-label={t("results.list")}>
        {shown.map((e, i) => (
          <li key={i} className="cct-list__item">
            <FileText size={12} aria-hidden="true" className="cct-list__icon" />
            <PathButton ctx={ctx} path={resolve(e.path)} line={e.line} />
            {e.text !== undefined && <code className="cct-list__text">{e.text}</code>}
            {e.count !== undefined && <span className="cct-chip">{t("results.matches", { count: e.count })}</span>}
          </li>
        ))}
      </ul>
      {entries.length > LIST_PREVIEW && (
        <button type="button" className="cct-link" aria-expanded={all} onClick={() => setAll((v) => !v)}>
          {all ? t("output.showLess") : t("results.showAll", { count: entries.length })}
        </button>
      )}
    </>
  );
}

export function SearchBody({ name, input, result, ctx }: BodyProps) {
  const { t } = useT("claudeCodeTools");
  const pattern = strField(input, "pattern") ?? "";
  const path = strField(input, "path");
  const filter = strField(input, "glob") ?? strField(input, "type");
  const entries = result && !result.isError ? (name === "Grep" ? parseGrepOutput(result.content) : parsePathList(result.content)) : [];
  return (
    <>
      <div className="cct-row">
        <code className="cct-code">{pattern}</code>
        {path && <span className="cct-muted">{t("results.in", { path })}</span>}
        {filter && <span className="cct-chip">{t("results.filter", { filter })}</span>}
        {result && !result.isError && <span className="cct-chip">{t("results.count", { count: entries.length })}</span>}
      </div>
      {result?.isError && <Output text={result.content} isError />}
      {result && !result.isError && <ResultList entries={entries} ctx={ctx} base={path ?? ctx.cwd} />}
    </>
  );
}

export function LsBody({ input, result, ctx }: BodyProps) {
  const { t } = useT("claudeCodeTools");
  const [all, setAll] = useState(false);
  const path = strField(input, "path") ?? "";
  if (!result) return <PathButton ctx={ctx} path={path} />;
  if (result.isError) return <Output text={result.content} isError />;
  const entries = parseLsOutput(result.content);
  if (entries.length === 0) return <Output text={result.content} isError={false} />;
  const shown = all ? entries : entries.slice(0, LIST_PREVIEW);
  return (
    <>
      <ul className="cct-list cct-tree" aria-label={t("results.list")}>
        {shown.map((e, i) => (
          <li key={i} className="cct-list__item" style={{ paddingInlineStart: `${e.depth * 14}px` }}>
            {e.dir ? <Folder size={12} aria-hidden="true" className="cct-list__icon" /> : <FileText size={12} aria-hidden="true" className="cct-list__icon" />}
            {e.dir ? <span className="cct-tree__dir">{e.name}</span> : <PathButton ctx={ctx} path={e.path} label={e.name} />}
          </li>
        ))}
      </ul>
      {entries.length > LIST_PREVIEW && (
        <button type="button" className="cct-link" aria-expanded={all} onClick={() => setAll((v) => !v)}>
          {all ? t("output.showLess") : t("results.showAll", { count: entries.length })}
        </button>
      )}
    </>
  );
}

// ── TodoWrite ────────────────────────────────────────────────────────────

export function TodoBody({ input }: BodyProps) {
  const { t } = useT("claudeCodeTools");
  const list = todos(input);
  if (list.length === 0) return <div className="cct-muted">{t("todo.empty")}</div>;
  return (
    <ul className="cct-todos" aria-label={t("todo.label")}>
      {list.map((td, i) => (
        <li key={i} className={clsx("cct-todo", `cct-todo--${td.status}`)}>
          {td.status === "completed" ? (
            <CheckCircle2 size={14} aria-hidden="true" />
          ) : td.status === "in_progress" ? (
            <CircleDot size={14} aria-hidden="true" />
          ) : (
            <Circle size={14} aria-hidden="true" />
          )}
          <span className="cct-todo__text">{td.content}</span>
          <span className="cct-sr">{t(`todo.state.${td.status}`)}</span>
        </li>
      ))}
    </ul>
  );
}

/** "2 of 5 done" of a TodoWrite input. */
export function todoProgress(input: JsonValue): { done: number; total: number } {
  const list = todos(input);
  return { done: list.filter((td) => td.status === "completed").length, total: list.length };
}

// ── WebFetch / WebSearch ─────────────────────────────────────────────────

const MD_PREVIEW_BLOCKS = 12;

/** Untrusted Markdown (tool / sub-agent text) through the app's safe renderer, truncated. */
export function MarkdownResult({ text, isError }: { text: string; isError: boolean }) {
  const { t } = useT("claudeCodeTools");
  const [all, setAll] = useState(false);
  if (isError) return <Output text={text} isError />;
  if (!text.trim()) return <div className="cct-muted">{t("output.none")}</div>;
  const blocks = text.split(/\n\s*\n/).length;
  return (
    <div className="cct-md">
      <SafeMarkdown source={text} maxBlocks={all ? 400 : MD_PREVIEW_BLOCKS} />
      {blocks > MD_PREVIEW_BLOCKS && (
        <button type="button" className="cct-link" aria-expanded={all} onClick={() => setAll((v) => !v)}>
          {all ? t("output.showLess") : t("output.showAll", { count: text.split("\n").length })}
        </button>
      )}
    </div>
  );
}

function domains(input: JsonValue, key: string): string | null {
  const v = field(input, key);
  return Array.isArray(v) && v.length > 0 ? v.filter((d): d is string => typeof d === "string").join(", ") : null;
}

export function WebFetchBody({ input, result }: BodyProps) {
  const { t } = useT("claudeCodeTools");
  const url = strField(input, "url") ?? "";
  const prompt = strField(input, "prompt");
  return (
    <>
      <code className="cct-code cct-url" title={url}>
        {url}
      </code>
      {prompt && <Field label={t("web.prompt")}>{prompt}</Field>}
      {result && <MarkdownResult text={result.content} isError={result.isError} />}
    </>
  );
}

export function WebSearchBody({ input, result }: BodyProps) {
  const { t } = useT("claudeCodeTools");
  const query = strField(input, "query") ?? "";
  const allowed = domains(input, "allowed_domains");
  const blocked = domains(input, "blocked_domains");
  const parsed = result && !result.isError ? parseWebSearch(result.content) : null;
  return (
    <>
      <div className="cct-row">
        <code className="cct-code">{query}</code>
        {allowed && <span className="cct-chip">{t("web.allowed", { list: allowed })}</span>}
        {blocked && <span className="cct-chip">{t("web.blocked", { list: blocked })}</span>}
      </div>
      {result?.isError && <Output text={result.content} isError />}
      {parsed && parsed.links.length > 0 && (
        <ul className="cct-list" aria-label={t("web.sources")}>
          {parsed.links.slice(0, LIST_PREVIEW).map((l, i) => (
            <li key={i} className="cct-list__item cct-link-item" title={l.url}>
              <span className="cct-link-item__title">{l.title}</span>
              <span className="cct-link-item__url">{l.url}</span>
            </li>
          ))}
        </ul>
      )}
      {parsed && parsed.text && <MarkdownResult text={parsed.text} isError={false} />}
    </>
  );
}

// ── Task (sub-agent) ─────────────────────────────────────────────────────

export function TaskBody({ input, result, children }: BodyProps & { children?: ReactNode }) {
  const { t } = useT("claudeCodeTools");
  const [full, setFull] = useState(false);
  const prompt = strField(input, "prompt");
  return (
    <>
      {prompt && (
        <div className="cct-prompt">
          <div className="cct-field__label">{t("task.prompt")}</div>
          <div className={clsx("cct-prompt__text", !full && "cct-clamp")}>{prompt}</div>
          {prompt.length > 240 && (
            <button type="button" className="cct-link" aria-expanded={full} onClick={() => setFull((v) => !v)}>
              {full ? t("output.showLess") : t("task.showPrompt")}
            </button>
          )}
        </div>
      )}
      {children && (
        <div className="cct-subagent" role="group" aria-label={t("task.steps")}>
          {children}
        </div>
      )}
      {result && (
        <div className="cct-task-result">
          <div className="cct-field__label">{t("task.result")}</div>
          <MarkdownResult text={result.content} isError={result.isError} />
        </div>
      )}
    </>
  );
}

// ── ExitPlanMode ─────────────────────────────────────────────────────────

export function PlanBody({ input, result }: BodyProps) {
  const { t } = useT("claudeCodeTools");
  const plan = strField(input, "plan") ?? "";
  return (
    <>
      <div className="cct-plan" role="document" aria-label={t("plan.label")}>
        {plan.trim() ? <SafeMarkdown source={plan} /> : <div className="cct-muted">{t("plan.empty")}</div>}
      </div>
      {result && <Output text={result.content} isError={result.isError} lines={6} />}
    </>
  );
}

// ── MCP / unknown ────────────────────────────────────────────────────────

export function GenericBody({ name, input, result }: BodyProps) {
  const { t } = useT("claudeCodeTools");
  const json = JSON.stringify(input, null, 2) ?? "";
  return (
    <>
      <div className="cct-field__label">{name.startsWith("mcp__") ? t("mcp.arguments") : t("generic.input")}</div>
      <Output text={json} isError={false} lines={16} />
      {result && (
        <>
          <div className="cct-field__label">{t("generic.result")}</div>
          <Output text={result.content} isError={result.isError} />
        </>
      )}
    </>
  );
}
