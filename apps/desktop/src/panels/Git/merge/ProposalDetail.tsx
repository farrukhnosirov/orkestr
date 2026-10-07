/**
 * One merge proposal: status + recommendation, blockers, predicted
 * conflicts, branch cards in merge order (drag / arrows to reorder,
 * validated against dependencies), execution details, conflict resolution
 * and the actions (execute → approval, continue, abort, re-analyse).
 */
import type { MergeManagerProposal } from "@orkestr/protocol";
import { Badge, Button, Input } from "@orkestr/ui";
import { CheckCircle2, GitMerge, Play, RefreshCw, RotateCcw, Square, Zap } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { getClient } from "../../../core/client";
import { useFormat, useT } from "../../../i18n";
import { mergeDetail } from "../../../lib/coreText";
import { ConfirmDialog, Notice } from "../components";
import { shortId } from "../model";
import { errorMessage } from "../state";
import { BranchCard } from "./BranchCard";
import { ConflictResolver } from "./ConflictResolver";
import {
  canAbort,
  canExecute,
  moveItem,
  orderViolations,
  orderedBranches,
  proposalTotals,
  recommendationTone,
  statusTone,
} from "./model";

interface Props {
  proposal: MergeManagerProposal;
  onUpdated: (p: MergeManagerProposal) => void;
}

export function ProposalDetail({ proposal: p, onUpdated }: Props) {
  const { t } = useT(["merge", "common"]);
  const fmt = useFormat();
  const [order, setOrder] = useState<string[]>(p.order);
  const [drag, setDrag] = useState<{ from: number; over: number } | null>(null);
  const dragRef = useRef<{ from: number; over: number } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<"execute" | "abort" | null>(null);
  const [reason, setReason] = useState("");

  // The server's order wins whenever the proposal changes.
  const serverOrder = p.order.join("\n");
  useEffect(() => {
    setOrder(p.order);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serverOrder, p.id]);
  useEffect(() => {
    setError(null);
    setConfirm(null);
  }, [p.id]);

  const editable = p.status === "Proposed";
  const byName = useMemo(() => new Map(p.branches.map((b) => [b.branch, b] as const)), [p.branches]);
  const cards = editable ? order.flatMap((n) => byName.get(n) ?? []) : orderedBranches(p);
  const violations = orderViolations(order, p.branches);
  const reordered = order.join("\n") !== serverOrder;
  const totals = proposalTotals(p);
  const exec = p.execution;

  const act = async (tag: string, fn: () => Promise<MergeManagerProposal>) => {
    setBusy(tag);
    setError(null);
    try {
      onUpdated(await fn());
      setConfirm(null);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };
  const client = () => getClient();
  const applyOrder = (o: string[]) =>
    act("reorder", async () => (await client().call("merge.reorder", { proposalId: p.id, order: o })).proposal);
  const execute = () =>
    act("execute", async () => (await client().call("merge.execute", { proposalId: p.id, ...(reordered ? { order } : {}) })).proposal);
  const abort = () =>
    act("abort", async () => (await client().call("merge.abort", { proposalId: p.id, ...(reason.trim() ? { reason: reason.trim() } : {}) })).proposal);
  const approve = async (decision: "Approve" | "Reject") => {
    const approvalId = exec?.approvalId;
    if (!approvalId) return;
    setBusy(decision);
    setError(null);
    try {
      await client().call("approvals.resolve", { approvalId, decision });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const statusNotice = (() => {
    const detail = p.statusDetail ? ` ${t("notice.detail", { detail: mergeDetail(p.statusDetail) })}` : "";
    switch (p.status) {
      case "AwaitingApproval":
        return (
          <Notice
            tone="warning"
            action={
              exec?.approvalId ? (
                <span className="mm-inline-actions">
                  <Button size="sm" variant="primary" loading={busy === "Approve"} disabled={busy !== null} onClick={() => void approve("Approve")}>
                    {t("common:actions.approve")}
                  </Button>
                  <Button size="sm" variant="ghost" loading={busy === "Reject"} disabled={busy !== null} onClick={() => void approve("Reject")}>
                    {t("common:actions.reject")}
                  </Button>
                </span>
              ) : undefined
            }
          >
            {t("notice.awaitingApproval")}
          </Notice>
        );
      case "Merging":
        return <Notice tone="info">{t("notice.merging", { branch: exec?.integrationBranch ?? "" })}</Notice>;
      case "Testing":
        return <Notice tone="info">{t("notice.testing")}</Notice>;
      case "Conflicted":
        return <Notice tone="warning">{t("notice.conflicted", { branch: exec?.conflictBranch ?? p.statusDetail ?? "" })}</Notice>;
      case "Completed":
        return <Notice tone="success">{t("notice.completed", { target: p.target, commit: shortId(exec?.finalCommit) })}</Notice>;
      case "Failed":
        return <Notice tone="danger">{t("notice.failed") + detail}</Notice>;
      case "Aborted":
        return <Notice tone="neutral">{t("notice.aborted") + detail}</Notice>;
      case "Rejected":
        return <Notice tone="danger">{t("notice.rejected") + detail}</Notice>;
      case "Superseded":
        return <Notice tone="success">{t("notice.superseded")}</Notice>;
      default:
        return null;
    }
  })();

  return (
    <div className="mm-detail git-scroll">
      <header className="mm-detail__head">
        <div className="mm-detail__title">
          <GitMerge size={16} className="git-accent" />
          <span>{t("detail.target", { target: p.target })}</span>
          <Badge tone={statusTone(p.status)}>{t(`status.${p.status}`)}</Badge>
          {p.status === "Proposed" && <Badge tone={recommendationTone(p.recommendation)}>{t(`recommendation.${p.recommendation}`)}</Badge>}
          {p.autoMergeable && p.status === "Proposed" && (
            <Badge tone="success" icon={<Zap size={11} />} title={t("detail.autoHint")}>
              {t("detail.auto")}
            </Badge>
          )}
          {p.runId && <Badge tone="violet">{t("detail.run")}</Badge>}
        </div>
        <span className="git-small git-muted">
          {t("detail.totals", { files: fmt.number(totals.files), additions: fmt.number(totals.additions), deletions: fmt.number(totals.deletions) })}
          {" · "}
          {t("detail.updated", { time: fmt.relative(p.updatedAt) })}
        </span>
        <div className="mm-detail__actions">
          {p.status === "Proposed" && (
            <>
              <Button size="sm" variant="ghost" leftIcon={<RefreshCw size={13} />} loading={busy === "reorder"} disabled={busy !== null} onClick={() => void applyOrder(p.order)}>
                {t("detail.reanalyse")}
              </Button>
              <Button
                size="sm"
                variant="primary"
                leftIcon={<Play size={13} />}
                disabled={busy !== null || !canExecute(p) || violations.length > 0}
                onClick={() => setConfirm("execute")}
              >
                {t("detail.execute")}
              </Button>
            </>
          )}
          {p.status === "Conflicted" && (
            <Button size="sm" variant="primary" leftIcon={<Play size={13} />} loading={busy === "execute"} disabled={busy !== null || !canExecute(p)} onClick={() => void execute()}>
              {t("detail.continue")}
            </Button>
          )}
          {canAbort(p) && (
            <Button size="sm" variant="danger" leftIcon={<Square size={13} />} disabled={busy !== null} onClick={() => setConfirm("abort")}>
              {t("detail.abort")}
            </Button>
          )}
        </div>
      </header>

      {statusNotice}
      {error && <Notice tone="danger">{error}</Notice>}

      {p.blockers.length > 0 && (
        <section className="mm-section">
          <h3 className="mm-heading">{t("blockers.title")}</h3>
          <ul className="mm-bullets">
            {p.blockers.map((b, i) => (
              <li key={`${b.kind}:${b.branch ?? ""}:${i}`} className="mm-bullet mm-bullet--danger">
                {t(`blockers.${b.kind}`, { branch: b.branch ?? "" })}
                {b.detail && <span className="git-muted"> — {b.detail}</span>}
              </li>
            ))}
          </ul>
        </section>
      )}

      {p.status === "Conflicted" && <ConflictResolver proposal={p} onUpdated={onUpdated} />}

      <section className="mm-section">
        <h3 className="mm-heading">{t("conflicts.title")}</h3>
        {p.conflicts.length === 0 ? (
          <p className="git-muted git-small mm-none">
            <CheckCircle2 size={13} className="mm-ok" /> {t("conflicts.none")}
          </p>
        ) : (
          <ul className="mm-bullets">
            {p.conflicts.map((c, i) => (
              <li key={`${c.scope}:${c.branch}:${c.other ?? ""}:${i}`} className="mm-bullet mm-bullet--warning">
                {c.scope === "Pair" ? t("conflicts.pair", { branch: c.branch, other: c.other ?? "" }) : t("conflicts.order", { branch: c.branch })}
                <span className="git-muted mm-mono"> — {c.paths.join(", ")}</span>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="mm-section">
        <h3 className="mm-heading">
          {t("order.title")}
          {editable && reordered && (
            <span className="mm-heading__actions">
              <Button size="sm" variant="ghost" leftIcon={<RotateCcw size={13} />} onClick={() => setOrder(p.order)} disabled={busy !== null}>
                {t("order.reset")}
              </Button>
              <Button size="sm" variant="secondary" loading={busy === "reorder"} disabled={busy !== null || violations.length > 0} onClick={() => void applyOrder(order)}>
                {t("order.apply")}
              </Button>
            </span>
          )}
        </h3>
        {editable && <p className="git-muted git-small mm-hint">{t("order.hint")}</p>}
        {violations.map((v) => (
          <Notice key={`${v.branch}>${v.dependsOn}`} tone="danger">
            {t("order.violation", { branch: v.branch, dependsOn: v.dependsOn })}
          </Notice>
        ))}
        <ol className="mm-cards">
          {cards.map((b, i) => (
            <BranchCard
              key={b.branch}
              branch={b}
              index={i}
              total={cards.length}
              editable={editable && busy === null}
              dragging={drag?.from === i}
              dropTarget={drag !== null && drag.over === i && drag.from !== i}
              onMove={(from, to) => setOrder((o) => moveItem(o, from, to))}
              onDragStart={(from) => {
                dragRef.current = { from, over: from };
                setDrag(dragRef.current);
              }}
              onDragEnter={(over) => {
                if (!dragRef.current) return;
                dragRef.current = { ...dragRef.current, over };
                setDrag(dragRef.current);
              }}
              onDragEnd={() => {
                // Called by both `drop` (target) and `dragend` (source): act once.
                const d = dragRef.current;
                dragRef.current = null;
                setDrag(null);
                if (d && d.from !== d.over) setOrder((o) => moveItem(o, d.from, d.over));
              }}
            />
          ))}
        </ol>
      </section>

      {exec && (exec.snapshotId || exec.integrationBranch || exec.quality || exec.finalCommit || exec.autoApproved) && (
        <section className="mm-section">
          <h3 className="mm-heading">{t("execution.title")}</h3>
          <dl className="mm-facts">
            {exec.autoApproved && (
              <div>
                <dt>
                  <Zap size={12} />
                </dt>
                <dd>{t("execution.auto")}</dd>
              </div>
            )}
            {exec.snapshotId && (
              <div>
                <dt>{t("execution.snapshot")}</dt>
                <dd className="mm-mono">{shortId(exec.snapshotId)}</dd>
              </div>
            )}
            {exec.integrationBranch && (
              <div>
                <dt>{t("execution.integration")}</dt>
                <dd className="mm-mono">{exec.integrationBranch}</dd>
              </div>
            )}
            {exec.quality && (
              <div>
                <dt>{t("execution.quality")}</dt>
                <dd>
                  <Badge tone={exec.quality.passed ? "success" : "danger"}>
                    {exec.quality.passed ? t("execution.qualityPassed") : t("execution.qualityFailed")}
                  </Badge>
                  {exec.quality.checks.map((c) => (
                    <Badge key={c.name} tone={c.passed ? "success" : "danger"} title={c.detail ?? undefined}>
                      {c.name}
                    </Badge>
                  ))}
                </dd>
              </div>
            )}
            {exec.finalCommit && (
              <div>
                <dt>{t("execution.final")}</dt>
                <dd className="mm-mono">{shortId(exec.finalCommit)}</dd>
              </div>
            )}
          </dl>
          {exec.cleaned.length > 0 && <p className="git-muted git-small">{t("execution.cleaned", { items: exec.cleaned.join(", ") })}</p>}
        </section>
      )}

      {confirm === "execute" && (
        <ConfirmDialog
          title={t("executeDialog.title", { target: p.target })}
          description={t("executeDialog.description")}
          confirmLabel={t("executeDialog.confirm")}
          busy={busy === "execute"}
          error={error}
          onConfirm={() => void execute()}
          onCancel={() => setConfirm(null)}
        >
          <p className="git-small">{p.autoMergeable ? t("executeDialog.auto") : t("executeDialog.approval")}</p>
        </ConfirmDialog>
      )}
      {confirm === "abort" && (
        <ConfirmDialog
          title={t("abortDialog.title")}
          description={t("abortDialog.description")}
          confirmLabel={t("abortDialog.confirm")}
          danger
          busy={busy === "abort"}
          error={error}
          onConfirm={() => void abort()}
          onCancel={() => setConfirm(null)}
        >
          <Input value={reason} onChange={(e) => setReason(e.target.value)} placeholder={t("abortDialog.reason")} aria-label={t("abortDialog.reason")} />
        </ConfirmDialog>
      )}
    </div>
  );
}
