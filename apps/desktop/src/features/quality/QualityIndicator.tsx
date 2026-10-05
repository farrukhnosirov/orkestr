/**
 * Status bar build/test indicator (§17): the latest build and test status of
 * the active project; click opens a popover with the latest run's results.
 */
import type { CheckKind, QualityCheckStatus } from "@orkestr/protocol";
import { Button, StatusDot } from "@orkestr/ui";
import { FlaskConical, Hammer, Play } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { RunResults } from "./RunResults";
import { BUILD_RUN_KINDS, latestStatus, runTotals, statusDot, TEST_KINDS, TEST_RUN_KINDS } from "./logic";
import { startRun, useQualityLive } from "./store";
import "./quality.css";

const BUILD_KINDS: ReadonlySet<CheckKind> = new Set<CheckKind>(["Build", "Typecheck"]);

export function QualityIndicator({ projectId }: { projectId: string }) {
  const { t } = useT("quality");
  const fmt = useFormat();
  const { runs, loading } = useQualityLive(projectId);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const build = latestStatus(runs, BUILD_KINDS);
  const tests = latestStatus(runs, TEST_KINDS);
  const latest = runs[0] ?? null;
  const statusText = (s: { status: QualityCheckStatus } | null) => (s ? t(`status.${s.status}`) : t("statusBar.notRun"));
  const buildText = t("statusBar.build", { status: statusText(build) });
  const testsText = t("statusBar.tests", { status: statusText(tests) });

  const start = async (kinds: CheckKind[] | undefined) => {
    setError(null);
    try {
      await startRun(projectId, kinds ? { kinds } : {});
    } catch (e) {
      setError(describeError(e));
    }
  };

  return (
    <div className="q-indicator" ref={rootRef}>
      <button
        type="button"
        className="statusbar__item q-indicator__button"
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={`${t("statusBar.label")}: ${buildText}, ${testsText}`}
        title={t("statusBar.label")}
        onClick={() => setOpen((o) => !o)}
      >
        <Hammer size={12} aria-hidden="true" />
        <StatusDot status={statusDot(build?.status ?? null)} pulse={build?.status === "Running"} />
        <FlaskConical size={12} aria-hidden="true" />
        <StatusDot status={statusDot(tests?.status ?? null)} pulse={tests?.status === "Running"} />
      </button>
      {open && (
        <div className="q-popover" role="dialog" aria-label={t("statusBar.details")}>
          <div className="q-popover__head">
            <strong>{t("statusBar.details")}</strong>
            <span className="q-muted">{buildText}</span>
            <span className="q-muted">{testsText}</span>
          </div>
          <div className="q-actions">
            <Button size="sm" variant="primary" leftIcon={<Play size={12} />} onClick={() => void start(TEST_RUN_KINDS)}>
              {t("commands.runTests")}
            </Button>
            <Button size="sm" variant="secondary" leftIcon={<Hammer size={12} />} onClick={() => void start(BUILD_RUN_KINDS)}>
              {t("commands.runBuild")}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => void start(undefined)}>
              {t("commands.runAll")}
            </Button>
          </div>
          {error && <p className="q-error" role="alert">{error}</p>}
          <div className="q-popover__body">
            {latest === null ? (
              <p className="q-muted">{loading ? t("statusBar.loading") : t("statusBar.noRuns")}</p>
            ) : (
              <>
                <p className="q-muted">
                  {t("run.title", { when: fmt.relative(latest.startedAt) })} ·{" "}
                  {t("run.checks", { passed: runTotals(latest.results).checksPassed, total: latest.results.length })}
                  {latest.durationMs > 0 ? ` · ${t("run.duration", { duration: fmt.duration(latest.durationMs) })}` : ""}
                </p>
                <RunResults run={latest} />
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
