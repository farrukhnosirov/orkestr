import "./permissions.css";

/**
 * Inline permission request (`can_use_tool`): what Claude wants to do — the
 * command for Bash / PowerShell, the path and a mini diff for Edit / Write —
 * Orkestr's risk badge, and «Разрешить один раз» / «Разрешить в этом сеансе»
 * / «Запретить» (optionally with a reason for Claude). With focus inside the
 * card, Enter allows once and Esc denies. Answered requests show the outcome.
 */
import { Badge, Button } from "@orkestr/ui";
import clsx from "clsx";
import { ShieldAlert, ShieldCheck, ShieldQuestion, ShieldX } from "lucide-react";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";

import { editPairs, field, lineDiff, relativeTo, toolSubject, type DiffLine } from "../../../features/claudeCode/model";
import { dangerLabel } from "../../../features/security/logic";
import { useT } from "../../../i18n";
import type { PermissionCardProps } from "../types";

export { ModeSwitcher } from "./ModeSwitcher";

/** Changed lines shown in the preview before «…и ещё N строк». */
const DIFF_PREVIEW_LINES = 12;

const SHELL_TOOLS = new Set(["Bash", "PowerShell"]);
const EDIT_TOOLS = new Set(["Edit", "MultiEdit", "NotebookEdit"]);
const READ_TOOLS = new Set(["Read", "Glob", "Grep"]);

export interface PermissionCardViewProps extends PermissionCardProps {
  /** Focus the card when it appears pending (so Enter / Esc answer it). */
  autoFocus?: boolean;
}

/** Only the changed lines of an edit (capped). */
export function changedLines(name: string, input: PermissionCardProps["request"]["input"]): DiffLine[] {
  return editPairs(name, input).flatMap(([before, after]): DiffLine[] =>
    before === ""
      ? after.split("\n").map((text) => ({ type: "add", text }))
      : lineDiff(before, after).filter((l) => l.type !== "same"),
  );
}

function isTypingTarget(target: EventTarget): boolean {
  return target instanceof HTMLElement && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "BUTTON" || target.isContentEditable);
}

export function PermissionCard({ request, root, cwd, onRespond, autoFocus = false }: PermissionCardViewProps) {
  const { t } = useT("claudeCodeComposer");
  const ref = useRef<HTMLDivElement>(null);
  const [reasonOpen, setReasonOpen] = useState(false);
  const [reason, setReason] = useState("");
  const pending = request.status === "pending";
  const risk = request.risk;
  const tool = request.toolName;

  useEffect(() => {
    if (autoFocus && pending) ref.current?.focus();
  }, [autoFocus, pending]);

  const subject = toolSubject(tool, request.input);
  const filePath = field(request.input, "file_path") ?? field(request.input, "notebook_path");
  const isFileTool = typeof filePath === "string";
  const rel = isFileTool ? relativeTo(root, filePath, cwd) : null;
  const shell = SHELL_TOOLS.has(tool);
  const lines = changedLines(tool, request.input);
  const shown = lines.slice(0, DIFF_PREVIEW_LINES);
  const hidden = lines.length - shown.length;

  const verb = shell
    ? t("permission.wantsRun")
    : tool === "Write"
      ? t("permission.wantsWrite")
      : EDIT_TOOLS.has(tool)
        ? t("permission.wantsEdit")
        : READ_TOOLS.has(tool) && isFileTool
          ? t("permission.wantsRead")
          : t("permission.wantsOther");

  const allowSession = request.canRemember && risk === null;

  const deny = (message?: string) => {
    const note = message?.trim();
    if (note) onRespond("deny", note);
    else onRespond("deny");
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!pending) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      if (reasonOpen) setReasonOpen(false);
      else deny();
      return;
    }
    if (e.key === "Enter" && !e.shiftKey && !reasonOpen && !isTypingTarget(e.target)) {
      e.preventDefault();
      onRespond("allowOnce");
    }
  };

  const outcomeKey = request.byPolicy && request.status === "deny" ? "policy" : request.status === "pending" ? "deny" : request.status;

  return (
    <div
      ref={ref}
      className={clsx(
        "ccx-perm",
        pending ? "ccx-perm--pending" : `ccx-perm--${request.status}`,
        risk && `ccx-perm--${risk.level}`,
      )}
      role="group"
      aria-label={t("permission.title", { tool })}
      tabIndex={pending ? 0 : -1}
      onKeyDown={onKeyDown}
    >
      <div className="ccx-perm__head">
        <span className="ccx-perm__icon" aria-hidden>
          {risk ? (
            <ShieldAlert size={16} />
          ) : pending ? (
            <ShieldQuestion size={16} />
          ) : request.status === "deny" ? (
            <ShieldX size={16} />
          ) : (
            <ShieldCheck size={16} />
          )}
        </span>
        <span className="ccx-perm__title">{t("permission.title", { tool })}</span>
        {risk && (
          <Badge tone="danger" className="ccx-perm__risk" title={risk.reason}>
            {t(`permission.riskLevel.${risk.level}`)} · {dangerLabel(risk.category)}
          </Badge>
        )}
      </div>

      <div className="ccx-perm__what">
        <span className="ccx-perm__verb">{verb}</span>
        {isFileTool ? (
          <code className="ccx-perm__path">
            {rel ?? filePath}
            {rel === null && root && <span className="ccx-perm__outside"> · {t("permission.outsideProject")}</span>}
          </code>
        ) : (
          subject && !shell && <code className="ccx-perm__path">{subject}</code>
        )}
      </div>
      {shell && subject && <pre className="ccx-perm__command">{subject}</pre>}
      {request.description && request.description !== subject && <div className="ccx-perm__desc">{request.description}</div>}

      {shown.length > 0 && (
        <div className="ccx-perm__diff" role="figure">
          {shown.map((l, i) => (
            <div key={i} className={`ccx-perm__line ccx-perm__line--${l.type}`}>
              <span className="ccx-perm__sign" aria-hidden>
                {l.type === "add" ? "+" : "-"}
              </span>
              <span className="ccx-perm__code">{l.text || " "}</span>
            </div>
          ))}
          {hidden > 0 && <div className="ccx-perm__more">{t("permission.diffMore", { count: hidden })}</div>}
        </div>
      )}

      {request.blockedPath && <div className="ccx-perm__desc">{t("permission.blockedPath", { path: request.blockedPath })}</div>}
      {risk && <div className="ccx-perm__reason">{risk.reason}</div>}

      {pending ? (
        reasonOpen ? (
          <form
            className="ccx-perm__deny"
            onSubmit={(e) => {
              e.preventDefault();
              deny(reason);
            }}
          >
            <textarea
              className="ccx-perm__reasonInput"
              value={reason}
              rows={2}
              autoFocus
              aria-label={t("permission.reason")}
              placeholder={t("permission.reasonPlaceholder")}
              onChange={(e) => setReason(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  deny(reason);
                }
              }}
            />
            <div className="ccx-perm__actions">
              <Button type="submit" variant="danger" size="sm">
                {t("permission.deny")}
              </Button>
              <Button variant="ghost" size="sm" onClick={() => setReasonOpen(false)}>
                {t("permission.cancel")}
              </Button>
            </div>
          </form>
        ) : (
          <>
            <div className="ccx-perm__actions">
              <Button variant="primary" size="sm" onClick={() => onRespond("allowOnce")}>
                {t("permission.allowOnce")}
              </Button>
              {allowSession && (
                <Button size="sm" title={t("permission.allowSessionHint")} onClick={() => onRespond("allowSession")}>
                  {t("permission.allowSession")}
                </Button>
              )}
              <Button variant="danger" size="sm" onClick={() => deny()}>
                {t("permission.deny")}
              </Button>
              <Button variant="ghost" size="sm" onClick={() => setReasonOpen(true)}>
                {t("permission.denyWithReason")}
              </Button>
            </div>
            <div className="ccx-perm__keys">{t("permission.keys")}</div>
          </>
        )
      ) : (
        <div className="ccx-perm__outcome" role="status">
          {t(`permission.outcome.${outcomeKey}`)}
          {request.message ? ` — ${request.message}` : ""}
        </div>
      )}
    </div>
  );
}
