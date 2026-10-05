/**
 * "Reviews" section of the task drawer (§8, §17, §68.12): request an
 * automatic / adversarial review, see the verdict, findings grouped by
 * severity (with file links), the acceptance-criteria check, the quality
 * gate and resolve findings. Data: `reviews.*` + live `ReviewUpdated` events.
 */
import "./reviews.css";

import type { FindingStatus, Review, ReviewFinding, ReviewGate } from "@orkestr/protocol";
import { Badge, Button, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, CircleAlert, ClipboardCheck, FileCode2, ShieldAlert, ShieldCheck, Swords, X } from "lucide-react";
import { useState } from "react";

import { useFormat, useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { openInEditor } from "../files/actions";
import { reviewActions, useTaskReviews } from "./data";
import { SEVERITY_TONE, VERDICT_TONE, groupBySeverity, isActive, isBlocking, latestOf, modelLabel } from "./model";

export type ReviewToast = (tone: "success" | "danger" | "info", message: string) => void;

export function ReviewsSection({ taskId, onToast }: { taskId: string; onToast: ReviewToast }) {
  const { t } = useT(["reviews", "common"]);
  const { reviews, gate, loading, error, reload } = useTaskReviews(taskId);
  const [busy, setBusy] = useState<string | null>(null);
  const standard = latestOf(reviews, "Standard");
  const adversarial = latestOf(reviews, "Adversarial");
  const active = reviews.some(isActive);
  const earlier = reviews.filter((r) => r.id !== standard?.id && r.id !== adversarial?.id);

  const start = async (key: "std" | "both" | "adv") => {
    setBusy(key);
    try {
      if (key === "adv") {
        await reviewActions.adversarial({ taskId });
        onToast("info", t("adversarialRequested"));
      } else {
        await reviewActions.request(key === "both" ? { taskId, adversarial: true } : { taskId });
        onToast("info", t("requested"));
      }
      reload();
    } catch (e) {
      onToast("danger", describeError(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="tasks-section rv-section" aria-label={t("title")}>
      <h4 className="tasks-section__title">
        <ClipboardCheck size={13} /> {t("title")}
        {gate && (gate.latestReviewId || gate.latestAdversarialId) && <GateBadge gate={gate} />}
      </h4>
      <div className="tasks-actions">
        <Button size="sm" variant="secondary" loading={busy === "std"} disabled={busy !== null || active} onClick={() => void start("std")}>
          {t("request")}
        </Button>
        <Button size="sm" variant="ghost" loading={busy === "both"} disabled={busy !== null || active} onClick={() => void start("both")}>
          {t("requestBoth")}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          leftIcon={<Swords size={13} />}
          loading={busy === "adv"}
          disabled={busy !== null || active}
          onClick={() => void start("adv")}
        >
          {t("adversarial")}
        </Button>
      </div>
      {active && <p className="tasks-muted">{t("inProgress")}</p>}
      {error ? (
        <p className="tasks-error" role="alert">
          {error}{" "}
          <Button size="sm" variant="ghost" onClick={reload}>
            {t("common:actions.retry")}
          </Button>
        </p>
      ) : loading && reviews.length === 0 ? (
        <div className="tasks-muted">
          <Spinner size={14} /> {t("loading")}
        </div>
      ) : reviews.length === 0 ? (
        <p className="tasks-muted">{t("empty")}</p>
      ) : (
        <>
          {gate && <GateDetails gate={gate} />}
          {standard && <ReviewCard review={standard} onToast={onToast} />}
          {adversarial && <ReviewCard review={adversarial} onToast={onToast} />}
          {earlier.length > 0 && (
            <details className="rv-earlier">
              <summary>{t("earlier", { count: earlier.length })}</summary>
              {earlier.map((r) => (
                <ReviewCard key={r.id} review={r} onToast={onToast} compact />
              ))}
            </details>
          )}
        </>
      )}
    </section>
  );
}

function GateBadge({ gate }: { gate: ReviewGate }) {
  const { t } = useT("reviews");
  return (
    <Badge
      className="rv-gate-badge"
      tone={gate.passed ? "success" : "warning"}
      icon={gate.passed ? <ShieldCheck size={11} /> : <ShieldAlert size={11} />}
    >
      {gate.passed ? t("gate.passed") : t("gate.blocked")}
    </Badge>
  );
}

function GateDetails({ gate }: { gate: ReviewGate }) {
  const { t } = useT("reviews");
  return (
    <ul className="rv-gate" aria-label={t("gate.label")}>
      <li className={clsx(gate.reviewerApproved ? "rv-ok" : "rv-bad")}>
        {gate.reviewerApproved ? <Check size={12} /> : <X size={12} />} {gate.reviewerApproved ? t("gate.approved") : t("gate.notApproved")}
      </li>
      <li className={clsx(gate.noCriticalSecurity ? "rv-ok" : "rv-bad")}>
        {gate.noCriticalSecurity ? <Check size={12} /> : <X size={12} />}{" "}
        {gate.noCriticalSecurity ? t("gate.noCriticalSecurity") : t("gate.criticalSecurity")}
      </li>
      {gate.blockingFindings > 0 && (
        <li className="rv-bad">
          <CircleAlert size={12} /> {t("gate.blocking", { count: gate.blockingFindings })}
        </li>
      )}
    </ul>
  );
}

export function ReviewCard({ review, onToast, compact = false }: { review: Review; onToast: ReviewToast; compact?: boolean }) {
  const { t } = useT(["reviews", "common"]);
  const fmt = useFormat();
  const groups = groupBySeverity(review.findings);
  const adversarial = review.kind === "Adversarial";
  return (
    <article className={clsx("rv-card", adversarial && "rv-card--adversarial")} aria-label={t(`kind.${review.kind}`)}>
      <header className="rv-card__head">
        {adversarial && (
          <Badge tone="violet" icon={<Swords size={11} />}>
            {t("adversarialBadge")}
          </Badge>
        )}
        <strong>{t(`kind.${review.kind}`)}</strong>
        {review.verdict && review.status === "Finished" && <Badge tone={VERDICT_TONE[review.verdict]}>{t(`verdict.${review.verdict}`)}</Badge>}
        <span className="rv-card__spacer" />
        <time className="rv-muted" dateTime={review.createdAt} title={fmt.date(review.createdAt)}>
          {fmt.relative(review.createdAt)}
        </time>
      </header>
      {review.status === "Running" && (
        <p className="rv-muted rv-progress">
          <Spinner size={13} /> {t("running")}
        </p>
      )}
      {review.status === "Verifying" && (
        <p className="rv-muted rv-progress">
          <Spinner size={13} /> {t("verifying")}
        </p>
      )}
      {review.status === "Failed" && (
        <p className="tasks-error" role="alert">
          {t("failed", { error: review.error ?? "" })}
        </p>
      )}
      {review.summary && <p className="rv-summary">{review.summary}</p>}
      <div className="rv-meta">
        {review.model && <span>{t("model", { model: modelLabel(review.model) })}</span>}
        {review.verifierModel && <span>{t("verifier", { model: modelLabel(review.verifierModel) })}</span>}
        {review.diff.files > 0 && (
          <span>
            {t("files", { count: review.diff.files })} · {t("lines", { additions: review.diff.additions, deletions: review.diff.deletions })}
            {review.diff.truncated && <> · {t("truncated")}</>}
          </span>
        )}
        {review.usage.costUsd > 0 && <span>{t("cost", { cost: fmt.currency(review.usage.costUsd) })}</span>}
        {review.kind === "Standard" && review.linkedReviewId && <span>{t("linked")}</span>}
        {review.transitionedTo && <span>{t("moved", { status: t(`common:status.task.${review.transitionedTo}`) })}</span>}
      </div>
      {review.sameModelFallback && <p className="rv-note">{t("sameModel")}</p>}
      {!compact && review.status !== "Running" && (
        <>
          {review.status === "Finished" && groups.length === 0 && <p className="rv-muted">{t("noFindings")}</p>}
          {groups.map((g) => (
            <div key={g.severity} className="rv-group">
              <h5 className="rv-group__title">
                <Badge tone={SEVERITY_TONE[g.severity]}>{t(`severity.${g.severity}`)}</Badge>
                <span className="rv-muted">{t("findings", { count: g.findings.length })}</span>
              </h5>
              <ul className="rv-findings">
                {g.findings.map((f) => (
                  <FindingRow key={f.id} review={review} finding={f} onToast={onToast} />
                ))}
              </ul>
            </div>
          ))}
          {review.acCheck.length > 0 && (
            <div className="rv-group">
              <h5 className="rv-group__title">{t("acCheck")}</h5>
              <ul className="rv-ac">
                {review.acCheck.map((c, i) => (
                  <li key={`${i}-${c.criterion}`} className={clsx(c.met ? "rv-ok" : "rv-bad")}>
                    {c.met ? <Check size={12} aria-label={t("acMet")} /> : <X size={12} aria-label={t("acNotMet")} />}
                    <div>
                      <span>{c.criterion}</span>
                      {c.evidence && <small className="rv-muted">{c.evidence}</small>}
                    </div>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </>
      )}
    </article>
  );
}

function FindingRow({ review, finding: f, onToast }: { review: Review; finding: ReviewFinding; onToast: ReviewToast }) {
  const { t } = useT(["reviews", "common"]);
  const [busy, setBusy] = useState(false);
  const [dismissing, setDismissing] = useState(false);
  const [note, setNote] = useState("");
  const [noteError, setNoteError] = useState<string | null>(null);
  const editable = review.status === "Finished" || review.status === "Failed";

  const resolve = async (status: FindingStatus, withNote?: string) => {
    setBusy(true);
    try {
      await reviewActions.resolve(review.id, f.id, status, withNote);
      setDismissing(false);
      setNote("");
      onToast("success", t("updated"));
    } catch (e) {
      onToast("danger", describeError(e));
    } finally {
      setBusy(false);
    }
  };

  const confirmDismiss = () => {
    const text = note.trim();
    if (!text && f.severity === "Critical") {
      setNoteError(t("dismissNoteRequired"));
      return;
    }
    setNoteError(null);
    void resolve("Dismissed", text || undefined);
  };

  const where = f.file ? (f.line !== null ? t("location", { file: f.file, line: f.line }) : f.file) : null;
  const unresolved = f.status === "Open" || f.status === "Confirmed";

  return (
    <li className={clsx("rv-finding", isBlocking(f) && "rv-finding--blocking", !unresolved && "rv-finding--resolved")}>
      <div className="rv-finding__head">
        <span className="rv-finding__title">{f.title}</span>
        <span className="rv-chip">{f.category}</span>
        <span className={clsx("rv-chip", `rv-chip--${f.status.toLowerCase()}`)}>{t(`findingStatus.${f.status}`)}</span>
      </div>
      {where && f.file && (
        <button
          type="button"
          className="rv-file"
          title={t("openFile", { file: f.file })}
          onClick={() => openInEditor(f.file ?? "", f.line !== null ? { line: f.line } : undefined)}
        >
          <FileCode2 size={12} /> {where}
        </button>
      )}
      {f.detail && <p className="rv-finding__detail">{f.detail}</p>}
      {f.evidence && (
        <div className="rv-kv">
          <span>{t("evidence")}</span>
          <pre>{f.evidence}</pre>
        </div>
      )}
      {f.suggestion && (
        <div className="rv-kv">
          <span>{t("suggestion")}</span>
          <p>{f.suggestion}</p>
        </div>
      )}
      {f.response && (
        <div className="rv-kv">
          <span>{t("response")}</span>
          <p>{f.response}</p>
        </div>
      )}
      {f.resolutionNote && <p className="rv-muted">{t("resolutionNote", { note: f.resolutionNote })}</p>}
      {editable && !dismissing && (
        <div className="rv-finding__actions">
          {unresolved || f.status === "Rebutted" ? (
            <>
              <Button size="sm" variant="ghost" leftIcon={<Check size={12} />} loading={busy} onClick={() => void resolve("Fixed")}>
                {t("markFixed")}
              </Button>
              <Button size="sm" variant="ghost" disabled={busy} onClick={() => setDismissing(true)}>
                {t("dismiss")}
              </Button>
            </>
          ) : (
            <Button size="sm" variant="ghost" loading={busy} onClick={() => void resolve("Open")}>
              {t("reopen")}
            </Button>
          )}
        </div>
      )}
      {dismissing && (
        <form
          className="rv-dismiss"
          onSubmit={(e) => {
            e.preventDefault();
            confirmDismiss();
          }}
        >
          <textarea
            className="tasks-textarea"
            rows={2}
            value={note}
            maxLength={2000}
            aria-label={t("dismissNote")}
            placeholder={t("dismissNote")}
            onChange={(e) => setNote(e.target.value)}
          />
          {noteError && (
            <p className="tasks-error" role="alert">
              {noteError}
            </p>
          )}
          <div className="rv-finding__actions">
            <Button size="sm" variant="danger" type="submit" loading={busy}>
              {t("confirmDismiss")}
            </Button>
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => setDismissing(false)}>
              {t("common:actions.cancel")}
            </Button>
          </div>
        </form>
      )}
    </li>
  );
}
