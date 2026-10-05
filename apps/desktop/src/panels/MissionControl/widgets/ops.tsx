import type { Approval } from "@orkestr/protocol";
import { Badge, Button, type Tone } from "@orkestr/ui";
import { clsx } from "clsx";
import {
  AlertOctagon,
  AlertTriangle,
  BellRing,
  Check,
  CheckCircle2,
  Hammer,
  Info,
  Loader2,
  Rocket,
  ShieldCheck,
  TestTube2,
  X,
  XCircle,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";

import { useFormat, useT } from "../../../i18n";
import { describeError } from "../../../i18n/errors";
import { useAppStore } from "../../../store/app";
import { resolveApproval, useSecurityStore } from "../../../features/security/store";
import { type WidgetProps, useMission } from "../context";
import { useFeedStore, useMissionFeed } from "../data";
import type { Alert, AlertSeverity, QualityRun } from "../feed";
import { WidgetEmpty } from "./common";

const RISK_TONE: Record<Approval["risk"], Tone> = { Low: "neutral", Medium: "info", High: "warning", Critical: "danger" };

function ApprovalRow({ a }: { a: Approval }) {
  const { t } = useT(["missionControl", "security", "common"]);
  const fmt = useFormat();
  const projects = useAppStore((s) => s.projects);
  const [busy, setBusy] = useState<"Approve" | "Reject" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const decide = async (d: "Approve" | "Reject") => {
    setBusy(d);
    setError(null);
    try {
      await resolveApproval(a.id, d);
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(null);
    }
  };
  const project = projects.find((p) => p.id === a.projectId)?.name;
  return (
    <li className={clsx("mc-approval", `mc-approval--${a.risk.toLowerCase()}`)}>
      <div className="mc-approval__head">
        <Badge tone={RISK_TONE[a.risk]}>{t(`security:risk.${a.risk}`)}</Badge>
        <span className="mc-approval__summary">{a.actionSummary}</span>
      </div>
      {a.commandPreview && <code className="mc-approval__cmd">{a.commandPreview}</code>}
      <div className="mc-approval__foot">
        <span className="mc-approval__meta">
          {project ? `${project} · ` : ""}
          {fmt.relative(a.requestedAt)}
        </span>
        <Button size="sm" variant="ghost" disabled={busy !== null} onClick={() => void decide("Reject")} aria-label={t("approvals.rejectLabel", { action: a.actionSummary })}>
          <X size={13} aria-hidden="true" />
          {t("common:actions.reject")}
        </Button>
        <Button size="sm" variant="primary" disabled={busy !== null} onClick={() => void decide("Approve")} aria-label={t("approvals.approveLabel", { action: a.actionSummary })}>
          <Check size={13} aria-hidden="true" />
          {t("common:actions.approve")}
        </Button>
      </div>
      {error && <div className="mc-error" role="alert">{error}</div>}
    </li>
  );
}

/** Pending approvals with inline approve / reject (dangerous actions always land here). */
export function ApprovalsWidget(_props: WidgetProps) {
  const { t } = useT("missionControl");
  const pending = useSecurityStore((s) => s.pending);
  const { budget } = useMission();
  if (pending.length === 0) return <WidgetEmpty icon={<ShieldCheck size={20} />} title={t("approvals.empty")} hint={t("approvals.emptyHint")} />;
  return (
    <ul className="mc-approvals" aria-label={t("approvals.listLabel", { count: pending.length })}>
      <AnimatePresence initial={false}>
        {pending.map((a) => (
          <motion.div
            key={a.id}
            layout={budget.animate}
            initial={budget.animate ? { opacity: 0, y: -6 } : false}
            animate={{ opacity: 1, y: 0 }}
            exit={budget.animate ? { opacity: 0, x: 24 } : { opacity: 0 }}
            transition={{ duration: budget.animate ? 0.2 : 0 }}
          >
            <ApprovalRow a={a} />
          </motion.div>
        ))}
      </AnimatePresence>
    </ul>
  );
}

const QUALITY_TONE: Record<QualityRun["status"], Tone> = { running: "info", passed: "success", failed: "danger" };

function QualityRow({ r }: { r: QualityRun }) {
  const { t } = useT("missionControl");
  const fmt = useFormat();
  const Icon = r.status === "running" ? Loader2 : r.status === "passed" ? CheckCircle2 : XCircle;
  const KindIcon = r.kind === "build" ? Hammer : TestTube2;
  return (
    <li className={clsx("mc-quality", `mc-quality--${r.status}`)}>
      <KindIcon size={14} aria-hidden="true" className="mc-quality__kind" />
      <span className="mc-quality__label" title={r.label}>
        {r.label || t(`quality.kind.${r.kind}`)}
      </span>
      {r.kind === "test" && r.passed !== null && (
        <span className="mc-quality__counts">{t("quality.counts", { passed: r.passed, failed: r.failed ?? 0 })}</span>
      )}
      {r.durationMs !== null && <span className="mc-quality__time">{fmt.duration(r.durationMs)}</span>}
      <Badge tone={QUALITY_TONE[r.status]}>
        <Icon size={12} aria-hidden="true" className={clsx(r.status === "running" && "mc-spin")} />
        {t(`quality.status.${r.status}`)}
      </Badge>
    </li>
  );
}

/** Latest builds and test runs. INTEGRATOR(P2): bind quality-pipeline runs (see `feed.ts`). */
export function QualityWidget(_props: WidgetProps) {
  const { t } = useT("missionControl");
  const { quality } = useMissionFeed();
  if (quality.length === 0) {
    return <WidgetEmpty icon={<TestTube2 size={20} />} title={t("quality.empty")} hint={t("quality.emptyHint")} />;
  }
  return (
    <ul className="mc-qualities" aria-label={t("quality.listLabel")}>
      {quality.slice(0, 8).map((r) => (
        <QualityRow key={r.id} r={r} />
      ))}
    </ul>
  );
}

/** Deployments arrive in Phase 7 (§26). */
export function DeploymentsWidget(_props: WidgetProps) {
  const { t } = useT(["missionControl", "common"]);
  return (
    <WidgetEmpty
      icon={<Rocket size={20} />}
      title={t("deployments.empty")}
      hint={t("deployments.hint")}
      badge={<Badge tone="violet">{t("common:phase", { phase: 7 })}</Badge>}
    />
  );
}

const SEVERITY_ICON: Record<AlertSeverity, typeof Info> = { critical: AlertOctagon, warning: AlertTriangle, info: Info };

function AlertRow({ a }: { a: Alert }) {
  const { t } = useT(["missionControl", "common"]);
  const fmt = useFormat();
  const projects = useAppStore((s) => s.projects);
  const Icon = SEVERITY_ICON[a.severity];
  const project = a.projectId ? projects.find((p) => p.id === a.projectId)?.name : null;
  const params: Record<string, string | number> = { ...a.params };
  if (a.kind === "budget") {
    params["spent"] = fmt.currency(Number(a.params["spent"] ?? 0));
    params["limit"] = fmt.currency(Number(a.params["limit"] ?? 0));
  }
  if (a.kind === "rateLimited") params["secs"] = fmt.duration(Number(a.params["secs"] ?? 0) * 1000);
  return (
    <li className={clsx("mc-alert", `mc-alert--${a.severity}`)}>
      <Icon size={15} className="mc-alert__icon" aria-hidden="true" />
      <div className="mc-alert__body">
        <span className="mc-alert__title">
          <span className="mc-sr">{t(`alerts.severity.${a.severity}`)}: </span>
          {t(`alerts.kind.${a.kind}`, params)}
        </span>
        {a.detail && <span className="mc-alert__detail">{a.detail}</span>}
        <span className="mc-alert__meta">
          {project ? `${project} · ` : ""}
          {fmt.relative(a.ts)}
        </span>
      </div>
      <button
        type="button"
        className="mc-icon-btn"
        aria-label={t("alerts.dismiss")}
        title={t("alerts.dismiss")}
        onClick={() => useFeedStore.getState().dismiss(a.id)}
      >
        <X size={13} />
      </button>
    </li>
  );
}

/** Alerts feed: failures, rate limits, budget thresholds, risky approvals, findings. */
export function AlertsWidget(_props: WidgetProps) {
  const { t } = useT("missionControl");
  const { alerts } = useMissionFeed();
  if (alerts.length === 0) return <WidgetEmpty icon={<BellRing size={20} />} title={t("alerts.empty")} hint={t("alerts.emptyHint")} />;
  return (
    <div className="mc-alerts">
      <div className="mc-alerts__bar">
        <span>{t("alerts.count", { count: alerts.length })}</span>
        <button type="button" className="mc-link" onClick={() => useFeedStore.getState().clearAll()}>
          {t("alerts.clear")}
        </button>
      </div>
      <ul aria-label={t("alerts.listLabel")}>
        {alerts.slice(0, 30).map((a) => (
          <AlertRow key={a.id} a={a} />
        ))}
      </ul>
    </div>
  );
}
