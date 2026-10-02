import type { Approval } from "@orkestr/protocol";
import { Badge, Button, EmptyState, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { Bot, Check, Inbox, ShieldAlert, ShieldCheck, Terminal, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { capabilityLabel, dangerLabel, riskLabel, riskTone, shortId } from "./logic";
import { errorMessage, refreshPending, resolveApproval, useSecurityStore } from "./store";

/** Title-bar button showing the live number of pending approvals. */
export function ApprovalsBadge() {
  const count = useSecurityStore((s) => s.pending.length);
  const open = useSecurityStore((s) => s.inboxOpen);
  const setOpen = useSecurityStore((s) => s.setInboxOpen);
  const { t } = useT("security");
  const label = count === 0 ? t("inbox.badgeEmpty") : t("inbox.badgePending", { count });
  return (
    <button
      type="button"
      className={clsx("sec-badge-btn", count > 0 && "sec-badge-btn--active")}
      aria-label={label}
      aria-expanded={open}
      title={label}
      onClick={() => setOpen(!open)}
    >
      <Inbox size={15} />
      {count > 0 && (
        <span className="sec-badge-btn__count" aria-hidden="true">
          {count > 99 ? "99+" : count}
        </span>
      )}
    </button>
  );
}

function ApprovalCard({ approval }: { approval: Approval }) {
  const { t } = useT(["security", "common"]);
  const fmt = useFormat();
  const projectName = useAppStore((s) => s.projects.find((p) => p.id === approval.projectId)?.name);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState<"Approve" | "Reject" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const act = async (decision: "Approve" | "Reject") => {
    setBusy(decision);
    setError(null);
    try {
      await resolveApproval(approval.id, decision, note);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <article className="sec-approval" aria-label={t("approval.ariaLabel", { summary: approval.actionSummary })}>
      <header className="sec-approval__head">
        <Badge tone={riskTone(approval.risk)}>{riskLabel(approval.risk)}</Badge>
        <Badge tone="violet">{capabilityLabel(approval.capability)}</Badge>
        {approval.danger && (
          <Badge tone="danger" icon={<ShieldAlert size={12} />}>
            {dangerLabel(approval.danger)}
          </Badge>
        )}
        <span className="sec-approval__time" title={fmt.date(approval.requestedAt, "dateTime")}>
          {fmt.relative(approval.requestedAt)}
        </span>
      </header>
      <h3 className="sec-approval__title">{approval.actionSummary}</h3>
      <div className="sec-approval__meta">
        <span>
          <Bot size={12} /> {approval.agentId ? t("approval.agent", { id: shortId(approval.agentId) }) : t("approval.managerOrSystem")}
        </span>
        {projectName && <span>· {projectName}</span>}
      </div>
      {approval.commandPreview && (
        <pre className="sec-approval__preview" aria-label={t("approval.commandPreview")}>
          <Terminal size={12} aria-hidden="true" /> {approval.commandPreview}
        </pre>
      )}
      <p className="sec-approval__reason">{approval.reason}</p>
      <label className="sec-approval__note">
        <span className="sr-only">{t("approval.note")}</span>
        <input
          className="sec-input"
          placeholder={t("approval.notePlaceholder")}
          value={note}
          maxLength={2000}
          onChange={(e) => setNote(e.target.value)}
        />
      </label>
      {error && (
        <p className="sec-error" role="alert">
          {error}
        </p>
      )}
      <footer className="sec-approval__actions">
        <Button
          variant="danger"
          size="sm"
          leftIcon={<X size={14} />}
          loading={busy === "Reject"}
          disabled={busy !== null}
          onClick={() => void act("Reject")}
        >
          {t("common:actions.reject")}
        </Button>
        <Button
          variant="primary"
          size="sm"
          leftIcon={<Check size={14} />}
          loading={busy === "Approve"}
          disabled={busy !== null}
          onClick={() => void act("Approve")}
        >
          {t("common:actions.approve")}
        </Button>
      </footer>
    </article>
  );
}

/** Slide-over drawer listing pending approvals. */
export function ApprovalsDrawer() {
  const open = useSecurityStore((s) => s.inboxOpen);
  const setOpen = useSecurityStore((s) => s.setInboxOpen);
  const pending = useSecurityStore((s) => s.pending);
  const loaded = useSecurityStore((s) => s.pendingLoaded);
  const error = useSecurityStore((s) => s.pendingError);
  const { t } = useT(["security", "common"]);
  const ref = useRef<HTMLElement>(null);

  useEffect(() => {
    if (open) {
      void refreshPending();
      requestAnimationFrame(() => ref.current?.focus());
    }
  }, [open]);

  if (!open) return null;
  return (
    <div className="sec-drawer-overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && setOpen(false)}>
      <aside
        ref={ref}
        className="sec-drawer"
        role="dialog"
        aria-modal="true"
        aria-labelledby="sec-drawer-title"
        tabIndex={-1}
        onKeyDown={(e) => e.key === "Escape" && setOpen(false)}
      >
        <header className="sec-drawer__head">
          <div className="dialog__icon">
            <ShieldCheck size={18} />
          </div>
          <div>
            <h2 id="sec-drawer-title" className="dialog__title">
              {t("inbox.title")}
            </h2>
            <p className="dialog__subtitle">
              {pending.length === 0 ? t("inbox.subtitleEmpty") : t("inbox.subtitlePending", { count: pending.length })}
            </p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("inbox.close")} onClick={() => setOpen(false)}>
            <X size={16} />
          </button>
        </header>
        <div className="sec-drawer__body">
          {!loaded ? (
            <div className="sec-center">
              <Spinner label={t("inbox.loading")} />
            </div>
          ) : error ? (
            <EmptyState
              icon={<ShieldAlert size={22} />}
              title={t("inbox.loadFailed")}
              description={error}
              action={
                <Button size="sm" onClick={() => void refreshPending()}>
                  {t("common:actions.retry")}
                </Button>
              }
            />
          ) : pending.length === 0 ? (
            <EmptyState
              icon={<ShieldCheck size={22} />}
              title={t("inbox.emptyTitle")}
              description={t("inbox.emptyDescription")}
            />
          ) : (
            pending.map((a) => <ApprovalCard key={a.id} approval={a} />)
          )}
        </div>
      </aside>
    </div>
  );
}
