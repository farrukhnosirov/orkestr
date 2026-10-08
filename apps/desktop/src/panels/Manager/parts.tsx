/**
 * Smaller Manager panel parts: message rows, the session resume banner
 * (§57), the model indicator with switcher and toasts.
 */
import type { ManagerChatMessage, ManagerConfig, ManagerResumeSummary, ModelRef } from "@orkestr/protocol";
import { Badge, Button, IconButton, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { ChevronDown, Cpu, History, Info, Play, Sparkles, User, Wrench, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { instanceName, ModelPicker, useProvidersLive } from "../../features/providers";
import { formatDate, formatPercent, t as translate, useLanguage, useT } from "../../i18n";
import { managerToolSummary, runReason, snapshotLabel } from "../../lib/coreText";
import { Markdown } from "../Memory/markdown";
import { errorMessage, managerActions } from "./data";
import { formatUsd, modelLabel } from "./model";
import { PlanCard } from "./PlanCard";
import { RunCard } from "./RunCard";
import type { ChatState } from "./model";
import { useManagerUi } from "./store";

function time(iso: string, lng: string): string {
  return Number.isNaN(new Date(iso).getTime()) ? "" : formatDate(iso, "time", lng);
}

export function MessageRow({ message, state, showRun }: { message: ManagerChatMessage; state: ChatState; showRun: boolean }) {
  const { t } = useT("manager");
  const lng = useLanguage();
  const plan = message.planId ? state.plans[message.planId] : undefined;
  const run = showRun && message.runId ? state.runs[message.runId] : undefined;
  if (message.role === "SystemEvent") {
    return (
      <div className="mgr-msg mgr-msg--event">
        <Info size={13} aria-hidden="true" className="mgr-msg__event-icon" />
        <div className="mgr-msg__event-body">
          <Markdown source={message.text} className="mgr-md mgr-md--event" />
          {run && <RunCard run={run} />}
          {plan && !message.runId && plan.status === "Proposed" && <PlanCard plan={plan} />}
        </div>
        <time className="mgr-msg__time">{time(message.createdAt, lng)}</time>
      </div>
    );
  }
  const user = message.role === "User";
  return (
    <div className={clsx("mgr-msg", user ? "mgr-msg--user" : "mgr-msg--manager")}>
      <div className="mgr-msg__avatar" aria-hidden="true">
        {user ? <User size={14} /> : <Sparkles size={14} />}
      </div>
      <div className="mgr-msg__body">
        <div className="mgr-msg__meta">
          <strong>{user ? t("you") : t("title")}</strong>
          <time>{time(message.createdAt, lng)}</time>
          {message.model && <span className="mgr-muted">{message.model.model}</span>}
          {message.usage && message.usage.costUsd > 0 && <span className="mgr-muted">{formatUsd(message.usage.costUsd, lng)}</span>}
        </div>
        {user ? <div className="mgr-msg__text">{message.text}</div> : <Markdown source={message.text} className="mgr-md" />}
        {message.toolEvents.length > 0 && (
          <details className="mgr-tools">
            <summary>
              <Wrench size={12} aria-hidden="true" /> {t("message.toolCalls", { count: message.toolEvents.length })}
            </summary>
            <ul>
              {message.toolEvents.map((ev, i) => (
                <li key={i} className={clsx(ev.isError && "mgr-tools__error")}>
                  <code>{ev.name}</code> {managerToolSummary(ev.summary)}
                </li>
              ))}
            </ul>
          </details>
        )}
        {plan && <PlanCard plan={plan} />}
        {message.planId && !plan && <p className="mgr-muted">{t("message.planUnavailable")}</p>}
        {run && <RunCard run={run} />}
      </div>
    </div>
  );
}

export function StreamingRow({ text }: { text: string }) {
  const { t } = useT("manager");
  return (
    <div className="mgr-msg mgr-msg--manager mgr-msg--streaming" aria-busy="true">
      <div className="mgr-msg__avatar" aria-hidden="true">
        <Sparkles size={14} />
      </div>
      <div className="mgr-msg__body">
        <div className="mgr-msg__meta">
          <strong>{t("title")}</strong>
          <Spinner size={12} label={t("message.answering")} />
        </div>
        <Markdown source={text} className="mgr-md" />
        <span className="mgr-caret" aria-hidden="true" />
      </div>
    </div>
  );
}

export function ThinkingRow() {
  const { t } = useT("manager");
  return (
    <div className="mgr-msg mgr-msg--manager mgr-msg--streaming" aria-busy="true">
      <div className="mgr-msg__avatar" aria-hidden="true">
        <Sparkles size={14} />
      </div>
      <div className="mgr-msg__body mgr-thinking">
        <Spinner size={12} label={t("message.working")} /> {t("message.thinking")}
      </div>
    </div>
  );
}

/** Headline of the resume banner, built from the counters (the core's `headline` is English). */
export function resumeHeadline(summary: ManagerResumeSummary): string {
  if (summary.isEmpty) return translate("manager:resume.headlineEmpty");
  const total = summary.tasksTotal - (summary.tasksCancelled ?? 0);
  if (total > 0) {
    return translate("manager:resume.headlineProgress", {
      completed: summary.tasksCompleted,
      total,
      percent: formatPercent(summary.percentComplete / 100),
    });
  }
  const open = summary.tasksInProgress + summary.tasksFailed + summary.pendingApprovals;
  return open > 0 ? translate("manager:resume.label") : translate("manager:resume.headlineIdle");
}

/** Detail lines: per-goal progress and the resumable run. */
export function resumeLines(summary: ManagerResumeSummary): string[] {
  const lines = summary.goals.map((g) =>
    g.total > 0 && g.completed === g.total
      ? translate("manager:resume.goalDone", { goal: g.goal })
      : translate("manager:resume.goalProgress", { goal: g.goal, percent: formatPercent(g.percent / 100), completed: g.completed, total: g.total }),
  );
  const run = summary.activeRun;
  if (run) {
    const status = translate(`logs:enums.runStatus.${run.status}`);
    lines.push(
      run.statusReason
        ? translate("manager:resume.runLineReason", { status, reason: runReason(run.statusReason) })
        : translate("manager:resume.runLine", { status }),
    );
  }
  return lines;
}

/** An issue line of the core (fixed English shapes, see crates/orchestrator/src/resume.rs). */
export function resumeIssue(issue: string): string {
  let m: RegExpMatchArray | null;
  if ((m = issue.match(/^"([\s\S]+)" is (Failed|Interrupted)(?:: ([\s\S]*))?$/))) {
    const status = translate(`common:status.task.${m[2] as "Failed" | "Interrupted"}`);
    return m[3]
      ? translate("manager:resume.issueTaskDetail", { title: m[1], status, detail: m[3] })
      : translate("manager:resume.issueTask", { title: m[1], status });
  }
  if ((m = issue.match(/^(\d+) task\(s\) wait for review$/))) return translate("manager:resume.issueReview", { count: Number(m[1]) });
  if ((m = issue.match(/^the run for "([\s\S]+)" was interrupted — resume it to continue$/))) {
    return translate("manager:resume.issueRunInterrupted", { goal: m[1] });
  }
  return issue;
}

/** "Last session" banner (§57), shown once per project and session. */
export function ResumeBanner({ summary, onResumeRun }: { summary: ManagerResumeSummary; onResumeRun: (runId: string) => void }) {
  const { t } = useT("manager");
  useLanguage();
  const dismiss = useManagerUi((s) => s.dismissResume);
  const run = summary.activeRun;
  const lines = resumeLines(summary);
  return (
    <section className="mgr-resume" aria-label={t("resume.label")}>
      <History size={16} aria-hidden="true" className="mgr-resume__icon" />
      <div className="mgr-resume__body">
        <strong title={resumeHeadline(summary)}>{resumeHeadline(summary)}</strong>
        <div className="mgr-resume__stats">
          {summary.tasksTotal > 0 && (
            <div className="mgr-progress mgr-progress--thin" role="progressbar" aria-label={t("resume.tasksCompleted")} aria-valuemin={0} aria-valuemax={100} aria-valuenow={summary.percentComplete}>
              <div className="mgr-progress__bar" style={{ width: `${summary.percentComplete}%` }} />
            </div>
          )}
          {summary.tasksInProgress > 0 && <Badge tone="violet">{t("resume.inProgress", { count: summary.tasksInProgress })}</Badge>}
          {summary.tasksFailed > 0 && <Badge tone="danger">{t("resume.failed", { count: summary.tasksFailed })}</Badge>}
          {summary.pendingApprovals > 0 && <Badge tone="warning">{t("resume.pendingApprovals", { count: summary.pendingApprovals })}</Badge>}
          {summary.lastSnapshot && <span className="mgr-muted">{t("resume.lastSnapshot", { label: snapshotLabel(summary.lastSnapshot.label) })}</span>}
        </div>
        {lines.length > 0 && (
          <ul className="mgr-resume__lines">
            {lines.slice(0, 5).map((l, i) => (
              <li key={i}>{l}</li>
            ))}
          </ul>
        )}
        {summary.issues.length > 0 && (
          <ul className="mgr-resume__lines mgr-resume__lines--issues">
            {summary.issues.slice(0, 3).map((l, i) => (
              <li key={i}>{resumeIssue(l)}</li>
            ))}
          </ul>
        )}
        {run && (run.status === "Interrupted" || run.status === "Paused") && (
          <Button size="sm" variant="primary" leftIcon={<Play size={13} />} onClick={() => onResumeRun(run.id)}>
            {t("resume.resumeRun")}
          </Button>
        )}
      </div>
      <IconButton size="sm" label={t("resume.dismiss")} icon={<X size={14} />} onClick={() => dismiss(summary.projectId)} />
    </section>
  );
}

/** Current Manager model with a switcher (writes `providers.setManager`). */
export function ModelIndicator({ config, onSaved }: { config: ManagerConfig | null; onSaved: (c: ManagerConfig) => void }) {
  const { t } = useT(["manager", "common"]);
  const { providers } = useProvidersLive();
  const toast = useManagerUi((s) => s.toast);
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState<ModelRef | null>(config?.model ?? null);
  const [saving, setSaving] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const name = (id: string) => {
    const p = providers.find((x) => x.provider.id === id);
    return p ? instanceName(p.provider) : null;
  };

  useEffect(() => {
    if (open) setValue(config?.model ?? null);
  }, [open, config]);

  useEffect(() => {
    if (!open) return undefined;
    const onDown = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const save = async () => {
    if (!config) return;
    setSaving(true);
    try {
      const next = await managerActions.setModel(config, value && value.model.trim() ? value : null);
      onSaved(next);
      setOpen(false);
      toast("success", translate("manager:model.saved", { model: next.model?.model ?? translate("manager:model.auto") }));
    } catch (e) {
      toast("danger", errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="mgr-model" ref={box}>
      <button
        type="button"
        className={clsx("mgr-model__btn", !config?.model && "mgr-model__btn--none")}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={t("model.button", { model: modelLabel(config, name) })}
        disabled={!config}
        onClick={() => setOpen((o) => !o)}
      >
        <Cpu size={13} aria-hidden="true" />
        <span>{modelLabel(config, name)}</span>
        <ChevronDown size={12} aria-hidden="true" />
      </button>
      {open && config && (
        <div className="mgr-model__pop" role="dialog" aria-label={t("model.label")}>
          <ModelPicker label={t("model.label")} value={value} onChange={setValue} providers={providers} allowNone />
          <p className="mgr-muted">{t("model.help")}</p>
          <div className="mgr-model__actions">
            <Button size="sm" variant="primary" loading={saving} onClick={() => void save()}>
              {t("common:actions.save")}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setOpen(false)}>
              {t("common:actions.cancel")}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

export function Toasts() {
  const { t } = useT("common");
  const toasts = useManagerUi((s) => s.toasts);
  const dismiss = useManagerUi((s) => s.dismissToast);
  if (toasts.length === 0) return null;
  return (
    <div className="mgr-toasts" role="status" aria-live="polite">
      {toasts.map((toast) => (
        <div key={toast.id} className={clsx("mgr-toast", `mgr-toast--${toast.tone}`)}>
          <span>{toast.message}</span>
          <button type="button" aria-label={t("actions.close")} onClick={() => dismiss(toast.id)}>
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
