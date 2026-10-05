/**
 * "Quality" section of the Tasks panel task drawer (§58): gate checklist with
 * status and reason, Definition of Done, "Run checks" in the task's worktree
 * and the latest run's results (failures expandable, jump to file).
 */
import type { DodResult, GateReport, GateResult, Task } from "@orkestr/protocol";
import { Badge, Button, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { CircleCheck, CircleDashed, CircleHelp, CircleMinus, CircleX, Play, RefreshCw, Settings2, ShieldCheck, Square } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";
import { useSettingsStore } from "../settings/settingsStore";
import { RunResults } from "./RunResults";
import { gateTone, isFinal, targetKey } from "./logic";
import { cancelRun, evaluate, startRun, useQualityLive, useQualityStore } from "./store";
import "./quality.css";

function StatusIcon({ status }: { status: GateResult["status"] }) {
  switch (status) {
    case "Passed":
      return <CircleCheck size={14} className="q-icon q-icon--passed" aria-hidden="true" />;
    case "Failed":
      return <CircleX size={14} className="q-icon q-icon--failed" aria-hidden="true" />;
    case "Pending":
      return <CircleDashed size={14} className="q-icon q-icon--pending" aria-hidden="true" />;
    case "Unknown":
      return <CircleHelp size={14} className="q-icon q-icon--unknown" aria-hidden="true" />;
    default:
      return <CircleMinus size={14} className="q-icon q-icon--skipped" aria-hidden="true" />;
  }
}

function GateRow({ name, item }: { name: string; item: GateResult | DodResult }) {
  const { t } = useT("quality");
  const required = "required" in item ? item.required : null;
  return (
    <li className={clsx("q-gate", `q-gate--${item.status.toLowerCase()}`)}>
      <StatusIcon status={item.status} />
      <div className="q-gate__text">
        <span className="q-gate__name">
          {name}
          {required === false && <span className="q-muted"> · {t("section.optional")}</span>}
        </span>
        <span className="q-gate__reason">{t(`reason.${item.reason}`, { detail: item.detail })}</span>
      </div>
      <Badge tone={gateTone(item.status)}>{t(`gateStatus.${item.status}`)}</Badge>
    </li>
  );
}

export function QualitySection({ task }: { task: Task }) {
  const { t } = useT("quality");
  const fmt = useFormat();
  const connected = useAppStore((s) => s.connection.state === "open");
  const projectRuns = useQualityLive(task.projectId);
  const version = useQualityStore((s) => s.version);
  const [report, setReport] = useState<GateReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<"run" | "cancel" | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const runs = projectRuns.runs.filter((r) => r.taskId === task.id);
  const latest = runs[0] ?? null;
  const active = latest !== null && !isFinal(latest.status);

  const reload = useCallback(() => {
    if (!connected) return () => undefined;
    let cancelled = false;
    evaluate(task.projectId, task.id)
      .then((r) => {
        if (cancelled) return;
        setReport(r);
        setError(null);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(describeError(e));
      });
    return () => {
      cancelled = true;
    };
  }, [connected, task.projectId, task.id]);

  // Re-evaluate when the task changes (criteria, files) or a run progresses.
  useEffect(() => reload(), [reload, task.version, version]);

  const run = async () => {
    setBusy("run");
    setActionError(null);
    try {
      await startRun(task.projectId, { target: { type: "task", taskId: task.id } });
    } catch (e) {
      setActionError(describeError(e));
    } finally {
      setBusy(null);
    }
  };
  const cancel = async () => {
    if (!latest) return;
    setBusy("cancel");
    try {
      await cancelRun(latest.id);
    } catch (e) {
      setActionError(describeError(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="tasks-section q-section" aria-label={t("section.title")}>
      <h4 className="tasks-section__title">
        <ShieldCheck size={13} /> {t("section.title")}
        {report && (
          <span className="q-section__overall">
            <Badge tone={gateTone(report.overall)} title={t("section.overall")}>
              {t(`gateStatus.${report.overall}`)}
            </Badge>
          </span>
        )}
      </h4>

      <div className="q-actions">
        {active ? (
          <Button size="sm" variant="danger" leftIcon={<Square size={12} />} loading={busy === "cancel"} disabled={busy !== null} onClick={() => void cancel()}>
            {t("section.cancel")}
          </Button>
        ) : (
          <Button size="sm" variant="primary" leftIcon={<Play size={12} />} loading={busy === "run"} disabled={busy !== null || !connected} onClick={() => void run()}>
            {t("section.runChecks")}
          </Button>
        )}
        <Button size="sm" variant="ghost" leftIcon={<RefreshCw size={12} />} disabled={!connected} onClick={() => reload()}>
          {t("section.refresh")}
        </Button>
        <Button size="sm" variant="ghost" leftIcon={<Settings2 size={12} />} onClick={() => useSettingsStore.getState().openSettings("quality")}>
          {t("section.configure")}
        </Button>
      </div>
      {actionError && <p className="q-error" role="alert">{actionError}</p>}

      {error ? (
        <p className="q-error" role="alert">
          {t("section.evaluateFailed")}: {error}
        </p>
      ) : report === null ? (
        <div className="q-muted">
          <Spinner size={14} /> {t("section.evaluating")}
        </div>
      ) : (
        <>
          <h5 className="tasks-subtitle">{t("section.gates")}</h5>
          <ul className="q-gates">
            {report.gates
              .filter((g) => g.enabled)
              .map((g) => (
                <GateRow key={g.gate} name={t(`gate.${g.gate}`)} item={g} />
              ))}
          </ul>
          <h5 className="tasks-subtitle">{t("section.dod")}</h5>
          <ul className="q-gates">
            {report.dod.map((d) => (
              <GateRow key={d.item} name={t(`dod.${d.item}`)} item={d} />
            ))}
          </ul>
        </>
      )}

      <h5 className="tasks-subtitle">{t("section.checks")}</h5>
      {latest === null ? (
        <p className="q-muted">{t("section.noRuns")}</p>
      ) : (
        <>
          <p className="q-muted">
            {t("section.lastRun", { when: fmt.relative(latest.startedAt), target: t(`target.${targetKey(latest.target)}`) })}
          </p>
          <RunResults run={latest} />
        </>
      )}
    </section>
  );
}
