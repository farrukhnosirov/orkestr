/**
 * Inline permission request: Claude wants to run a tool. «Разрешить один
 * раз» / «Разрешить до конца сеанса» (not offered for dangerous commands) /
 * «Запретить» with an optional note. Commands flagged by Orkestr's guard show
 * their risk category; forbidden ones arrive already denied.
 */
import type { ClaudePermissionDecision } from "@orkestr/protocol";
import { Badge, Button } from "@orkestr/ui";
import clsx from "clsx";
import { ShieldAlert, ShieldCheck, ShieldQuestion, ShieldX } from "lucide-react";
import { useState } from "react";

import { useT } from "../../i18n";
import { field, relativeTo, toolSubject, type PermissionItem } from "../../features/claudeCode/model";
import { TextDiff } from "./InlineDiff";

export interface PermissionCardProps {
  item: PermissionItem;
  root: string | null;
  cwd: string | null;
  onRespond: (decision: ClaudePermissionDecision, message?: string) => void;
}

export function PermissionCard({ item, root, cwd, onRespond }: PermissionCardProps) {
  const { t } = useT("claudeCode");
  const [note, setNote] = useState("");
  const [denying, setDenying] = useState(false);
  const subject = toolSubject(item.toolName, item.input);
  const filePath = field(item.input, "file_path");
  const rel = typeof filePath === "string" ? relativeTo(root, filePath, cwd) : null;
  const pending = item.status === "pending";
  const risk = item.risk;
  const oldString = field(item.input, "old_string");
  const newString = field(item.input, "new_string");
  const content = field(item.input, "content");

  return (
    <div
      className={clsx("cc-perm", pending && "cc-perm--pending", risk && `cc-perm--${risk.level}`)}
      role={pending ? "alertdialog" : "group"}
      aria-label={t("permission.title", { tool: item.toolName })}
    >
      <div className="cc-perm__head">
        {risk ? <ShieldAlert size={16} /> : pending ? <ShieldQuestion size={16} /> : item.status === "deny" ? <ShieldX size={16} /> : <ShieldCheck size={16} />}
        <span className="cc-perm__title">{t("permission.title", { tool: item.toolName })}</span>
        {risk && (
          <Badge tone={risk.level === "forbidden" ? "danger" : "warning"} title={risk.reason}>
            {t(`permission.risk.${risk.level}`)} · {t(`security:danger.${risk.category}`)}
          </Badge>
        )}
      </div>
      {item.description && item.description !== subject && <div className="cc-muted">{item.description}</div>}
      {subject && <pre className="cc-command">{rel ?? subject}</pre>}
      {typeof oldString === "string" && typeof newString === "string" && <TextDiff before={oldString} after={newString} />}
      {typeof content === "string" && item.toolName === "Write" && <TextDiff before="" after={content} />}
      {item.blockedPath && <div className="cc-muted">{t("permission.blockedPath", { path: item.blockedPath })}</div>}
      {risk && <div className="cc-perm__reason">{risk.reason}</div>}
      {pending ? (
        <div className="cc-perm__actions">
          {denying ? (
            <form
              className="cc-perm__deny"
              onSubmit={(e) => {
                e.preventDefault();
                onRespond("deny", note);
              }}
            >
              <input
                className="cc-input"
                value={note}
                autoFocus
                onChange={(e) => setNote(e.target.value)}
                placeholder={t("permission.denyNotePlaceholder")}
                aria-label={t("permission.denyNote")}
              />
              <Button type="submit" variant="danger" size="sm">
                {t("permission.deny")}
              </Button>
              <Button variant="ghost" size="sm" onClick={() => setDenying(false)}>
                {t("common:actions.cancel")}
              </Button>
            </form>
          ) : (
            <>
              <Button variant="primary" size="sm" onClick={() => onRespond("allowOnce")}>
                {t("permission.allowOnce")}
              </Button>
              {item.canRemember && (
                <Button size="sm" onClick={() => onRespond("allowSession")} title={t("permission.allowSessionHint")}>
                  {t("permission.allowSession")}
                </Button>
              )}
              <Button variant="danger" size="sm" onClick={() => setDenying(true)}>
                {t("permission.denyWithNote")}
              </Button>
            </>
          )}
        </div>
      ) : (
        <div className="cc-perm__outcome">
          {item.byPolicy && item.status === "deny"
            ? t("permission.outcome.policy")
            : t(`permission.outcome.${item.status}`)}
          {item.message ? ` — ${item.message}` : ""}
        </div>
      )}
    </div>
  );
}
