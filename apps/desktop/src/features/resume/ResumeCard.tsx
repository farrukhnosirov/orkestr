import { Badge, Button, IconButton, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { History, ScrollText, X } from "lucide-react";
import { useEffect } from "react";

import { workspaceActions } from "../../components/workspaceActions";
import { useT } from "../../i18n";
import { describeEvent, severityOf } from "../../panels/Logs/describe";
import { useAppStore } from "../../store/app";
import { headline, summaryLines } from "./resume";
import "./resume.css";
import { useResumeStore } from "./resumeStore";

/** Auto-hide delay when nothing happened. */
const QUIET_TTL_MS = 8000;

/** Session Resume card (§57), shown when a project is opened. */
export function ResumeCard() {
  const { visible, loading, error, summary, projectId, dismiss } = useResumeStore();
  const projectName = useAppStore((s) => s.projects.find((p) => p.id === projectId)?.name ?? null);
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const { t } = useT("resume");

  const quiet = summary !== null && summary.totalEvents === 0 && !summary.recoveredFromCrash;
  useEffect(() => {
    if (!visible || !(quiet || error)) return;
    const timer = setTimeout(dismiss, QUIET_TTL_MS);
    return () => clearTimeout(timer);
  }, [visible, quiet, error, dismiss]);

  if (!visible || projectId === null || projectId !== activeProjectId) return null;

  return (
    <section className="resume-card" aria-label={t("card.label")} aria-live="polite">
      <header className="resume-card__head">
        <span className="resume-card__icon">
          <History size={16} />
        </span>
        <div className="resume-card__title">
          {loading ? t("card.catchingUp") : summary ? headline(summary, projectName) : t("card.label")}
        </div>
        <IconButton label={t("card.dismiss")} icon={<X size={14} />} size="sm" onClick={dismiss} />
      </header>
      {loading && (
        <div className="resume-card__body resume-card__loading">
          <Spinner size={14} /> {t("card.summarizing")}
        </div>
      )}
      {error && !loading && (
        <div className="resume-card__body resume-card__muted">{t("card.unavailable", { error })}</div>
      )}
      {summary && !loading && (
        <div className="resume-card__body">
          {summary.recoveredFromCrash && (
            <Badge tone="warning">{t("card.crashRecovered")}</Badge>
          )}
          {summary.totalEvents > 0 ? (
            <ul className="resume-card__lines">
              {summaryLines(summary).map((l) => (
                <li key={l}>{l}</li>
              ))}
            </ul>
          ) : (
            <p className="resume-card__muted">{t("card.nothingChanged")}</p>
          )}
          {summary.highlights.length > 0 && (
            <ul className="resume-card__highlights">
              {summary.highlights.slice(-4).map((ev) => (
                <li key={ev.id} className={clsx("resume-card__hl", `resume-card__hl--${severityOf(ev)}`)}>
                  {describeEvent(ev)}
                </li>
              ))}
            </ul>
          )}
          {summary.totalEvents > 0 && (
            <div className="resume-card__actions">
              <Button
                size="sm"
                variant="ghost"
                leftIcon={<ScrollText size={13} />}
                onClick={() => {
                  workspaceActions.openPanel("logs");
                  dismiss();
                }}
              >
                {t("card.openLogs")}
              </Button>
            </div>
          )}
        </div>
      )}
    </section>
  );
}
