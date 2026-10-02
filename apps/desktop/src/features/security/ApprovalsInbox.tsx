import type { Approval } from "@orkestr/protocol";
import { Badge, Button, EmptyState, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { Bot, Check, Inbox, ShieldAlert, ShieldCheck, Terminal, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { useAppStore } from "../../store/app";
import { dangerLabel, relativeTime, riskTone, shortId } from "./logic";
import { errorMessage, refreshPending, resolveApproval, useSecurityStore } from "./store";

/** Title-bar button showing the live number of pending approvals. */
export function ApprovalsBadge() {
  const count = useSecurityStore((s) => s.pending.length);
  const open = useSecurityStore((s) => s.inboxOpen);
  const setOpen = useSecurityStore((s) => s.setInboxOpen);
  const label = count === 0 ? "Approvals inbox (empty)" : `Approvals inbox: ${count} pending`;
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
    <article className="sec-approval" aria-label={`Approval: ${approval.actionSummary}`}>
      <header className="sec-approval__head">
        <Badge tone={riskTone(approval.risk)}>{approval.risk} risk</Badge>
        <Badge tone="violet">{approval.capability}</Badge>
        {approval.danger && (
          <Badge tone="danger" icon={<ShieldAlert size={12} />}>
            {dangerLabel(approval.danger)}
          </Badge>
        )}
        <span className="sec-approval__time" title={approval.requestedAt}>
          {relativeTime(approval.requestedAt)}
        </span>
      </header>
      <h3 className="sec-approval__title">{approval.actionSummary}</h3>
      <div className="sec-approval__meta">
        <span>
          <Bot size={12} /> {approval.agentId ? `Agent ${shortId(approval.agentId)}` : "Manager / system"}
        </span>
        {projectName && <span>· {projectName}</span>}
      </div>
      {approval.commandPreview && (
        <pre className="sec-approval__preview" aria-label="Command preview">
          <Terminal size={12} aria-hidden="true" /> {approval.commandPreview}
        </pre>
      )}
      <p className="sec-approval__reason">{approval.reason}</p>
      <label className="sec-approval__note">
        <span className="sr-only">Note</span>
        <input
          className="sec-input"
          placeholder="Optional note for the agent…"
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
          Reject
        </Button>
        <Button
          variant="primary"
          size="sm"
          leftIcon={<Check size={14} />}
          loading={busy === "Approve"}
          disabled={busy !== null}
          onClick={() => void act("Approve")}
        >
          Approve
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
              Approvals
            </h2>
            <p className="dialog__subtitle">
              {pending.length === 0 ? "Nothing is waiting for you." : `${pending.length} action(s) waiting for your decision`}
            </p>
          </div>
          <button type="button" className="dialog__close" aria-label="Close approvals" onClick={() => setOpen(false)}>
            <X size={16} />
          </button>
        </header>
        <div className="sec-drawer__body">
          {!loaded ? (
            <div className="sec-center">
              <Spinner label="Loading approvals" />
            </div>
          ) : error ? (
            <EmptyState
              icon={<ShieldAlert size={22} />}
              title="Could not load approvals"
              description={error}
              action={
                <Button size="sm" onClick={() => void refreshPending()}>
                  Retry
                </Button>
              }
            />
          ) : pending.length === 0 ? (
            <EmptyState
              icon={<ShieldCheck size={22} />}
              title="All clear"
              description="Agents ask here before risky actions: dangerous commands, network, secrets, deploys."
            />
          ) : (
            pending.map((a) => <ApprovalCard key={a.id} approval={a} />)
          )}
        </div>
      </aside>
    </div>
  );
}
