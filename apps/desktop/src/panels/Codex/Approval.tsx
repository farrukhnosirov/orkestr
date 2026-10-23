/**
 * Inline approval card of the Codex chat: the command (and its folder) or
 * the file changes Codex wants to apply, Orkestr's risk badge from the
 * Dangerous Action Guard, and «Allow» / «Allow for this session» (only when
 * the CLI can remember it and the guard saw no risk) / «Deny» / «Stop
 * turn». Forbidden commands are declined by Orkestr automatically. With
 * focus inside the card Enter allows once and Esc denies. Also the patch
 * card (`Edit` with `changes`) of file changes.
 */
import type { CodexApprovalDecision } from "@orkestr/protocol";
import { Badge, Button } from "@orkestr/ui";
import clsx from "clsx";
import { FilePlus2, FileMinus2, PencilLine, ShieldAlert, ShieldCheck, ShieldQuestion, ShieldX } from "lucide-react";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";

import { field, relativeTo } from "../../features/claudeCode/model";
import { patchFiles, patchLines, type CxApprovalItem, type PatchFile } from "../../features/codex/model";
import { dangerLabel } from "../../features/security/logic";
import { useT } from "../../i18n";

/** Diff lines shown per file before «…and N more». */
const PREVIEW_LINES = 14;

function str(v: unknown): string | null {
  return typeof v === "string" ? v : null;
}

function FileIcon({ kind }: { kind: string }) {
  if (kind === "add") return <FilePlus2 size={13} aria-hidden />;
  if (kind === "delete") return <FileMinus2 size={13} aria-hidden />;
  return <PencilLine size={13} aria-hidden />;
}

/** One changed file with a (capped) unified diff. */
export function PatchFileView({ file, root, cwd, onOpen, preview = PREVIEW_LINES }: { file: PatchFile; root: string | null; cwd: string | null; onOpen?: (rel: string) => void; preview?: number }) {
  const { t } = useT("codex");
  const [all, setAll] = useState(false);
  const rel = relativeTo(root, file.path, cwd);
  const lines = file.diff ? patchLines(file.diff) : [];
  const shown = all ? lines : lines.slice(0, preview);
  const hidden = lines.length - shown.length;
  const kindLabel = file.kind === "add" ? t("patch.added") : file.kind === "delete" ? t("patch.deleted") : t("patch.updated");
  return (
    <div className="cx-patch__file">
      <div className="cx-patch__head">
        <FileIcon kind={file.kind} />
        {rel && onOpen ? (
          <button type="button" className="cc-chip" title={t("patch.open")} onClick={() => onOpen(rel)}>
            {rel}
          </button>
        ) : (
          <code className="cx-patch__path">{rel ?? file.path}</code>
        )}
        <span className="cx-patch__kind">{kindLabel}</span>
      </div>
      {shown.length > 0 && (
        <div className="ccx-perm__diff" role="figure" aria-label={t("patch.diff", { path: rel ?? file.path })}>
          {shown.map((l, i) =>
            l.type === "hunk" ? (
              <div key={i} className="cx-patch__hunk">
                {l.text}
              </div>
            ) : (
              <div key={i} className={clsx("ccx-perm__line", l.type !== "same" && `ccx-perm__line--${l.type}`)}>
                <span className="ccx-perm__sign" aria-hidden>
                  {l.type === "add" ? "+" : l.type === "del" ? "-" : " "}
                </span>
                <span className="ccx-perm__code">{l.text || " "}</span>
              </div>
            ),
          )}
          {hidden > 0 && (
            <button type="button" className="ccx-perm__more cx-patch__more" onClick={() => setAll(true)}>
              {t("patch.more", { count: hidden })}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/** Card of a Codex patch tool call (`Edit` with `changes`). */
export function PatchCard({ files, result, root, cwd, onOpen }: { files: PatchFile[]; result: { content: string; isError: boolean } | null; root: string | null; cwd: string | null; onOpen: (rel: string) => void }) {
  const { t } = useT("codex");
  return (
    <div className={clsx("cx-patch", result?.isError && "cx-patch--error")} data-testid="codex-patch">
      <div className="cx-patch__title">
        <PencilLine size={14} aria-hidden />
        <span>{t("patch.title", { count: files.length })}</span>
        <span className="cx-patch__state">{result === null ? t("patch.running") : result.isError ? t("patch.failed") : t("patch.applied")}</span>
      </div>
      {files.map((f) => (
        <PatchFileView key={f.path} file={f} root={root} cwd={cwd} onOpen={onOpen} />
      ))}
      {result?.isError && result.content && <pre className="cx-patch__error">{result.content}</pre>}
    </div>
  );
}

function isTypingTarget(target: EventTarget): boolean {
  return target instanceof HTMLElement && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "BUTTON" || target.isContentEditable);
}

const STATUS_CLASS: Record<CodexApprovalDecision, string> = {
  allow: "allowOnce",
  allowSession: "allowSession",
  deny: "deny",
  abort: "deny",
};

export interface ApprovalCardProps {
  request: CxApprovalItem;
  root: string | null;
  cwd: string | null;
  onRespond: (decision: CodexApprovalDecision) => void;
  onOpenFile: (rel: string) => void;
  autoFocus?: boolean;
}

export function ApprovalCard({ request, root, cwd, onRespond, onOpenFile, autoFocus = false }: ApprovalCardProps) {
  const { t } = useT("codex");
  const ref = useRef<HTMLDivElement>(null);
  const pending = request.status === "pending";
  const risk = request.risk;
  const forbidden = risk?.level === "forbidden";
  const command = request.approval === "command" ? str(field(request.input, "command")) : null;
  const commandCwd = str(field(request.input, "cwd"));
  const files = request.approval === "fileChange" ? patchFiles(request.input) : [];
  const singlePath = files.length === 0 ? str(field(request.input, "file_path")) : null;
  const allowSession = request.canRemember && risk === null;

  useEffect(() => {
    if (autoFocus && pending && !forbidden) ref.current?.focus();
  }, [autoFocus, pending, forbidden]);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!pending || forbidden) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onRespond("deny");
    } else if (e.key === "Enter" && !e.shiftKey && !isTypingTarget(e.target)) {
      e.preventDefault();
      onRespond("allow");
    }
  };

  const outcome =
    request.status === "pending"
      ? null
      : request.byPolicy && (request.status === "deny" || request.status === "abort")
        ? t("approval.outcome.policy")
        : t(`approval.outcome.${request.status}`);

  return (
    <div
      ref={ref}
      className={clsx("ccx-perm", pending ? "ccx-perm--pending" : `ccx-perm--${STATUS_CLASS[request.status as CodexApprovalDecision]}`, risk && `ccx-perm--${risk.level}`)}
      role="group"
      aria-label={request.approval === "command" ? t("approval.titleCommand") : t("approval.titleFiles")}
      tabIndex={pending ? 0 : -1}
      onKeyDown={onKeyDown}
      data-testid="codex-approval"
    >
      <div className="ccx-perm__head">
        <span className="ccx-perm__icon" aria-hidden>
          {risk ? <ShieldAlert size={16} /> : pending ? <ShieldQuestion size={16} /> : request.status === "deny" || request.status === "abort" ? <ShieldX size={16} /> : <ShieldCheck size={16} />}
        </span>
        <span className="ccx-perm__title">{request.approval === "command" ? t("approval.titleCommand") : t("approval.titleFiles")}</span>
        {risk && (
          <Badge tone="danger" className="ccx-perm__risk" title={risk.reason}>
            {t(`approval.risk.${risk.level}`)} · {dangerLabel(risk.category)}
          </Badge>
        )}
      </div>

      {command && <pre className="ccx-perm__command">{command}</pre>}
      {command && commandCwd && (
        <div className="ccx-perm__desc">{t("approval.inFolder", { path: relativeTo(root, commandCwd, cwd) || commandCwd })}</div>
      )}
      {singlePath && <code className="ccx-perm__path">{relativeTo(root, singlePath, cwd) ?? singlePath}</code>}
      {files.map((f) => (
        <PatchFileView key={f.path} file={f} root={root} cwd={cwd} onOpen={onOpenFile} preview={10} />
      ))}
      {request.reason && <div className="ccx-perm__desc">{t("approval.reason", { reason: request.reason })}</div>}
      {risk && <div className="ccx-perm__reason">{risk.reason}</div>}

      {pending && forbidden ? (
        <div className="ccx-perm__outcome" role="status">
          {t("approval.autoDenied")}
        </div>
      ) : pending ? (
        <>
          <div className="ccx-perm__actions">
            <Button variant="primary" size="sm" onClick={() => onRespond("allow")} data-testid="codex-approval-allow">
              {t("approval.allow")}
            </Button>
            {allowSession && (
              <Button size="sm" title={t("approval.allowSessionHint")} onClick={() => onRespond("allowSession")} data-testid="codex-approval-session">
                {t("approval.allowSession")}
              </Button>
            )}
            <Button variant="danger" size="sm" onClick={() => onRespond("deny")} data-testid="codex-approval-deny">
              {t("approval.deny")}
            </Button>
            <Button variant="ghost" size="sm" title={t("approval.abortHint")} onClick={() => onRespond("abort")} data-testid="codex-approval-abort">
              {t("approval.abort")}
            </Button>
          </div>
          <div className="ccx-perm__keys">{t("approval.keys")}</div>
        </>
      ) : (
        <div className="ccx-perm__outcome" role="status">
          {outcome}
          {request.message ? ` — ${request.message}` : ""}
        </div>
      )}
    </div>
  );
}
