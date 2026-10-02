/**
 * Smaller Manager panel parts: message rows, the session resume banner
 * (§57), the model indicator with switcher and toasts.
 */
import type { ManagerChatMessage, ManagerConfig, ManagerResumeSummary, ModelRef } from "@orkestr/protocol";
import { Badge, Button, IconButton, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { ChevronDown, Cpu, History, Info, Play, Sparkles, User, Wrench, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { ModelPicker, useProvidersLive } from "../../features/providers";
import { Markdown } from "../Memory/markdown";
import { errorMessage, managerActions } from "./data";
import { formatUsd, modelLabel } from "./model";
import { PlanCard } from "./PlanCard";
import { RunCard } from "./RunCard";
import type { ChatState } from "./model";
import { useManagerUi } from "./store";

function time(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "" : d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

export function MessageRow({ message, state, showRun }: { message: ManagerChatMessage; state: ChatState; showRun: boolean }) {
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
        <time className="mgr-msg__time">{time(message.createdAt)}</time>
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
          <strong>{user ? "You" : "AI Manager"}</strong>
          <time>{time(message.createdAt)}</time>
          {message.model && <span className="mgr-muted">{message.model.model}</span>}
          {message.usage && message.usage.costUsd > 0 && <span className="mgr-muted">{formatUsd(message.usage.costUsd)}</span>}
        </div>
        {user ? <div className="mgr-msg__text">{message.text}</div> : <Markdown source={message.text} className="mgr-md" />}
        {message.toolEvents.length > 0 && (
          <details className="mgr-tools">
            <summary>
              <Wrench size={12} aria-hidden="true" /> {message.toolEvents.length} tool call{message.toolEvents.length === 1 ? "" : "s"}
            </summary>
            <ul>
              {message.toolEvents.map((t, i) => (
                <li key={i} className={clsx(t.isError && "mgr-tools__error")}>
                  <code>{t.name}</code> {t.summary}
                </li>
              ))}
            </ul>
          </details>
        )}
        {plan && <PlanCard plan={plan} />}
        {message.planId && !plan && <p className="mgr-muted">The plan of this message is no longer available.</p>}
        {run && <RunCard run={run} />}
      </div>
    </div>
  );
}

export function StreamingRow({ text }: { text: string }) {
  return (
    <div className="mgr-msg mgr-msg--manager mgr-msg--streaming" aria-busy="true">
      <div className="mgr-msg__avatar" aria-hidden="true">
        <Sparkles size={14} />
      </div>
      <div className="mgr-msg__body">
        <div className="mgr-msg__meta">
          <strong>AI Manager</strong>
          <Spinner size={12} label="Answering" />
        </div>
        <Markdown source={text} className="mgr-md" />
        <span className="mgr-caret" aria-hidden="true" />
      </div>
    </div>
  );
}

export function ThinkingRow() {
  return (
    <div className="mgr-msg mgr-msg--manager mgr-msg--streaming" aria-busy="true">
      <div className="mgr-msg__avatar" aria-hidden="true">
        <Sparkles size={14} />
      </div>
      <div className="mgr-msg__body mgr-thinking">
        <Spinner size={12} label="Working" /> The Manager is working (reading state, planning)…
      </div>
    </div>
  );
}

/** "Last session" banner (§57), shown once per project and session. */
export function ResumeBanner({ summary, onResumeRun }: { summary: ManagerResumeSummary; onResumeRun: (runId: string) => void }) {
  const dismiss = useManagerUi((s) => s.dismissResume);
  const run = summary.activeRun;
  return (
    <section className="mgr-resume" aria-label="Last session">
      <History size={16} aria-hidden="true" className="mgr-resume__icon" />
      <div className="mgr-resume__body">
        <strong>{summary.headline}</strong>
        <div className="mgr-resume__stats">
          {summary.tasksTotal > 0 && (
            <div className="mgr-progress mgr-progress--thin" role="progressbar" aria-label="Tasks completed" aria-valuemin={0} aria-valuemax={100} aria-valuenow={summary.percentComplete}>
              <div className="mgr-progress__bar" style={{ width: `${summary.percentComplete}%` }} />
            </div>
          )}
          {summary.tasksInProgress > 0 && <Badge tone="violet">{summary.tasksInProgress} in progress</Badge>}
          {summary.tasksFailed > 0 && <Badge tone="danger">{summary.tasksFailed} failed</Badge>}
          {summary.pendingApprovals > 0 && <Badge tone="warning">{summary.pendingApprovals} pending approvals</Badge>}
          {summary.lastSnapshot && <span className="mgr-muted">Last snapshot: {summary.lastSnapshot.label}</span>}
        </div>
        {summary.lines.length > 0 && (
          <ul className="mgr-resume__lines">
            {summary.lines.slice(0, 5).map((l, i) => (
              <li key={i}>{l}</li>
            ))}
          </ul>
        )}
        {summary.issues.length > 0 && (
          <ul className="mgr-resume__lines mgr-resume__lines--issues">
            {summary.issues.slice(0, 3).map((l, i) => (
              <li key={i}>{l}</li>
            ))}
          </ul>
        )}
        {run && (run.status === "Interrupted" || run.status === "Paused") && (
          <Button size="sm" variant="primary" leftIcon={<Play size={13} />} onClick={() => onResumeRun(run.id)}>
            Resume the run
          </Button>
        )}
      </div>
      <IconButton size="sm" label="Dismiss" icon={<X size={14} />} onClick={() => dismiss(summary.projectId)} />
    </section>
  );
}

/** Current Manager model with a switcher (writes `providers.setManager`). */
export function ModelIndicator({ config, onSaved }: { config: ManagerConfig | null; onSaved: (c: ManagerConfig) => void }) {
  const { providers } = useProvidersLive();
  const toast = useManagerUi((s) => s.toast);
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState<ModelRef | null>(config?.model ?? null);
  const [saving, setSaving] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const name = (id: string) => providers.find((p) => p.provider.id === id)?.provider.name ?? null;

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
      toast("success", `Manager model: ${next.model?.model ?? "automatic routing"}`);
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
        aria-label={`Manager model: ${modelLabel(config, name)}. Change`}
        disabled={!config}
        onClick={() => setOpen((o) => !o)}
      >
        <Cpu size={13} aria-hidden="true" />
        <span>{modelLabel(config, name)}</span>
        <ChevronDown size={12} aria-hidden="true" />
      </button>
      {open && config && (
        <div className="mgr-model__pop" role="dialog" aria-label="Manager model">
          <ModelPicker label="Manager model" value={value} onChange={setValue} providers={providers} allowNone />
          <p className="mgr-muted">Applies to new messages and plans. Leave empty to use automatic routing.</p>
          <div className="mgr-model__actions">
            <Button size="sm" variant="primary" loading={saving} onClick={() => void save()}>
              Save
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setOpen(false)}>
              Cancel
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

export function Toasts() {
  const toasts = useManagerUi((s) => s.toasts);
  const dismiss = useManagerUi((s) => s.dismissToast);
  if (toasts.length === 0) return null;
  return (
    <div className="mgr-toasts" role="status" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className={clsx("mgr-toast", `mgr-toast--${t.tone}`)}>
          <span>{t.message}</span>
          <button type="button" aria-label="Dismiss" onClick={() => dismiss(t.id)}>
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
