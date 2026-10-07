/**
 * Building blocks shared by the tool card bodies: truncated output, path
 * buttons (open in the Code panel), the edit diff (text ↔ lazy Monaco) and
 * the shell risk badge.
 */
import type { ClaudeRisk } from "@orkestr/protocol";
import { Badge } from "@orkestr/ui";
import clsx from "clsx";
import { ShieldAlert, ShieldX, TriangleAlert } from "lucide-react";
import { lazy, Suspense, useState, type ReactNode } from "react";

import { relativeTo, lineDiff } from "../../../features/claudeCode/model";
import { dangerLabel } from "../../../features/security/logic";
import { useT } from "../../../i18n";
import { looksRisky } from "./parse";

const MonacoDiffEditor = lazy(() => import("./MonacoDiffEditor"));

/** Where paths are resolved: project root + session cwd + the open callback. */
export interface PathContext {
  root: string | null;
  cwd: string | null;
  onOpenFile: (relPath: string, line?: number) => void;
}

/** Project-relative form of a path the CLI reported (null when outside the project). */
export function projectPath(ctx: PathContext, path: string): string | null {
  const rel = relativeTo(ctx.root, path, ctx.cwd);
  return rel === "" ? null : rel;
}

/** A path that opens in the Code panel when it lies inside the project. */
export function PathButton({ ctx, path, line, label }: { ctx: PathContext; path: string; line?: number; label?: string }) {
  const { t } = useT("claudeCodeTools");
  const rel = projectPath(ctx, path);
  const text = label ?? rel ?? path;
  if (rel === null) {
    return (
      <span className="cct-path cct-path--outside" title={t("file.outside")}>
        {text}
      </span>
    );
  }
  const title = line !== undefined ? t("file.openAtLine", { path: rel, line }) : t("file.openAt", { path: rel });
  return (
    <button type="button" className="cct-path" title={title} onClick={() => ctx.onOpenFile(rel, line)}>
      {text}
      {line !== undefined && <span className="cct-path__line">:{line}</span>}
    </button>
  );
}

const PREVIEW_LINES = 24;

/** Tool output text, truncated with "show all"; error styling for failures. */
export function Output({ text, isError, lines = PREVIEW_LINES }: { text: string; isError: boolean; lines?: number }) {
  const { t } = useT("claudeCodeTools");
  const [all, setAll] = useState(false);
  if (!text.trim()) return <div className="cct-muted">{t("output.none")}</div>;
  const split = text.split("\n");
  const long = split.length > lines;
  const shown = all || !long ? text : split.slice(0, lines).join("\n");
  return (
    <div className="cct-output-wrap">
      <pre className={clsx("cct-output", isError && "cct-output--error")} aria-label={t("output.label")} tabIndex={0}>
        {shown}
      </pre>
      {long && (
        <button type="button" className="cct-link" aria-expanded={all} onClick={() => setAll((v) => !v)}>
          {all ? t("output.showLess") : t("output.showAll", { count: split.length })}
        </button>
      )}
    </div>
  );
}

const TEXT_DIFF_LINES = 60;

/** Light text diff (renders immediately; also the Monaco fallback while it loads). */
export function TextDiff({ before, after, label }: { before: string; after: string; label: string }) {
  const { t } = useT("claudeCodeTools");
  const [all, setAll] = useState(false);
  const lines = lineDiff(before, after);
  if (lines.length === 0) return <div className="cct-muted">{t("diff.empty")}</div>;
  const shown = all ? lines : lines.slice(0, TEXT_DIFF_LINES);
  return (
    <div className="cct-diff" role="figure" aria-label={label}>
      {shown.map((l, i) => (
        <div key={i} className={`cct-diff__line cct-diff__line--${l.type}`}>
          <span className="cct-diff__sign" aria-hidden="true">
            {l.type === "add" ? "+" : l.type === "del" ? "−" : " "}
          </span>
          <span className="cct-diff__text">{l.text}</span>
        </div>
      ))}
      {lines.length > TEXT_DIFF_LINES && (
        <button type="button" className="cct-link" aria-expanded={all} onClick={() => setAll((v) => !v)}>
          {all ? t("output.showLess") : t("output.showAll", { count: lines.length })}
        </button>
      )}
    </div>
  );
}

/** Edit diff: Monaco's read-only inline diff (lazy chunk) with a switch to the text view. */
export function DiffBlock({ before, after, path, title }: { before: string; after: string; path: string; title?: string }) {
  const { t } = useT("claudeCodeTools");
  const [editor, setEditor] = useState(true);
  const label = t("diff.label", { path });
  return (
    <div className="cct-diffblock">
      <div className="cct-diffblock__bar">
        {title && <span className="cct-muted">{title}</span>}
        <div className="cct-seg" role="group">
          <button type="button" className="cct-seg__btn" aria-pressed={editor} onClick={() => setEditor(true)}>
            {t("diff.editorView")}
          </button>
          <button type="button" className="cct-seg__btn" aria-pressed={!editor} onClick={() => setEditor(false)}>
            {t("diff.textView")}
          </button>
        </div>
      </div>
      {editor ? (
        <Suspense fallback={<TextDiff before={before} after={after} label={label} />}>
          <MonacoDiffEditor before={before} after={after} path={path} label={label} />
        </Suspense>
      ) : (
        <TextDiff before={before} after={after} label={label} />
      )}
    </div>
  );
}

/** Guard classification badge, or a clearly labelled local estimate when none was given. */
export function RiskBadge({ risk, command }: { risk: ClaudeRisk | null | undefined; command: string }) {
  const { t } = useT("claudeCodeTools");
  if (risk) {
    const forbidden = risk.level === "forbidden";
    const category = dangerLabel(risk.category);
    return (
      <Badge tone={forbidden ? "danger" : "warning"} icon={forbidden ? <ShieldX size={12} /> : <ShieldAlert size={12} />} title={risk.reason}>
        {forbidden ? t("risk.forbidden", { category }) : t("risk.dangerous", { category })}
      </Badge>
    );
  }
  if (!looksRisky(command)) return null;
  return (
    <Badge tone="warning" icon={<TriangleAlert size={12} />} title={t("risk.heuristicHint")} data-heuristic="true">
      {t("risk.heuristic")}
    </Badge>
  );
}

/** Small "label: value" row. */
export function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="cct-field">
      <span className="cct-field__label">{label}</span>
      <span className="cct-field__value">{children}</span>
    </div>
  );
}
