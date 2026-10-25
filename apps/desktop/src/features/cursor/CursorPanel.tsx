/**
 * Cursor capture mode panel (ADR-0050), shown in Assistant mode: start box
 * (task + active model), privacy consent, then the live session — state,
 * plan with the current step, the last screenshot with the target
 * highlighted, the step log, Pause / Resume / Stop, confirmations of risky
 * steps and the model's questions.
 */
import type { CursorSession, CursorShot, CursorStep } from "@orkestr/protocol";
import { Badge, Button, IconButton } from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, MousePointer2, OctagonX, Pause, Play, Send, Settings2, ShieldAlert, X } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";

import { useT } from "../../i18n";
import { useSettingsStore } from "../settings/settingsStore";
import {
  actionText,
  answerCursor,
  confirmCursor,
  declineConsent,
  grantConsentAndStart,
  isActive,
  loadShot,
  pauseCursor,
  refreshCursor,
  resumeCursor,
  startCursor,
  stopCursor,
  useCursorStore,
  watchCursor,
} from "./store";
import "./cursor.css";

function Shot({ shot, rect, label }: { shot?: CursorShot | undefined; rect?: [number, number, number, number] | undefined; label: string }) {
  const url = useCursorStore((s) => (shot ? s.shots[shot.id] : undefined));
  useEffect(() => {
    if (shot && !url) void loadShot(shot.id);
  }, [shot, url]);
  if (!shot) return null;
  return (
    <div className="cu-shot" style={{ aspectRatio: `${shot.width} / ${shot.height}` }}>
      {url ? <img src={url} alt={label} draggable={false} /> : <div className="cu-shot__ph" aria-hidden />}
      {rect && (
        <span
          className="cu-shot__target"
          data-testid="cursor-target"
          style={{
            left: `${(rect[0] / shot.width) * 100}%`,
            top: `${(rect[1] / shot.height) * 100}%`,
            width: `${Math.max(1.5, (rect[2] / shot.width) * 100)}%`,
            height: `${Math.max(1.5, (rect[3] / shot.height) * 100)}%`,
          }}
        />
      )}
    </div>
  );
}

function StepRow({ step }: { step: CursorStep }) {
  const { t } = useT("cursor");
  return (
    <li className={clsx("cu-step", `cu-step--${step.outcome}`)}>
      <span className="cu-step__n">{step.index}</span>
      <span className="cu-step__what">{actionText(step.action)}</span>
      <span className="cu-step__out">{t(`outcome.${step.outcome}`, { defaultValue: step.outcome })}</span>
      {step.thought && <span className="cu-step__thought">{step.thought}</span>}
    </li>
  );
}

function SessionView({ s }: { s: CursorSession }) {
  const { t } = useT("cursor");
  const [answer, setAnswer] = useState("");
  const active = isActive(s);
  const last = s.steps[s.steps.length - 1];
  const done = s.plan.length > 0 ? Math.min(s.steps.filter((x) => x.outcome === "ok").length, s.plan.length) : 0;
  const send = (e: FormEvent) => {
    e.preventDefault();
    if (!answer.trim()) return;
    void answerCursor(answer.trim());
    setAnswer("");
  };
  return (
    <div className={clsx("cu-session", `cu-session--${s.state}`)} data-testid="cursor-session">
      <div className="cu-session__head">
        <span className={clsx("cu-dot", active && "cu-dot--live")} aria-hidden />
        <div className="cu-session__title">
          <div className="cu-state" aria-live="polite">
            {t(`state.${s.state}`)}
            {s.abortReason && !active ? ` · ${t(`abort.${s.abortReason}`, { defaultValue: s.abortReason })}` : ""}
          </div>
          <div className="cu-task" title={s.task}>
            {s.task}
          </div>
        </div>
        <Badge tone="info">{s.model || s.backend}</Badge>
      </div>

      {s.current && active && <div className="cu-current">{s.current}</div>}

      {s.confirmation && (
        <div className="cu-confirm" role="alertdialog" aria-label={t("confirm.title")}>
          <div className="cu-block__title">
            <ShieldAlert size={15} /> {t(`risk.${s.confirmation.code}`, { defaultValue: t("risk.modelFlagged") })}
          </div>
          <div className="cu-confirm__what">
            {actionText(s.confirmation.action)}
            {s.confirmation.detail ? ` — “${s.confirmation.detail}”` : ""}
          </div>
          <Shot shot={s.confirmation.shot} rect={s.confirmation.targetRect} label={t("shot.target")} />
          <div className="cu-actions">
            <Button variant="danger" size="sm" leftIcon={<Check size={14} />} onClick={() => void confirmCursor(true)}>
              {t("confirm.approve")}
            </Button>
            <Button variant="ghost" size="sm" leftIcon={<X size={14} />} onClick={() => void confirmCursor(false)}>
              {t("confirm.reject")}
            </Button>
          </div>
        </div>
      )}

      {s.question && (
        <form className="cu-question" onSubmit={send} aria-label={t("question.title")}>
          <div className="cu-block__title">{s.question}</div>
          <div className="cu-row">
            <input className="cu-input" value={answer} onChange={(e) => setAnswer(e.target.value)} placeholder={t("question.placeholder")} aria-label={t("question.placeholder")} />
            <IconButton type="submit" label={t("question.send")} icon={<Send size={15} />} disabled={!answer.trim()} />
          </div>
        </form>
      )}

      {!s.confirmation && (s.lastShot || last?.shot) && (
        <Shot shot={last?.shot ?? s.lastShot} rect={last?.targetRect} label={t("shot.last")} />
      )}

      {s.plan.length > 0 && (
        <ol className="cu-plan" aria-label={t("plan.title")}>
          {s.plan.map((p, i) => (
            <li key={i} className={clsx(i < done && "cu-plan--done", i === done && active && "cu-plan--now")}>
              {p}
            </li>
          ))}
        </ol>
      )}

      {s.steps.length > 0 && (
        <ul className="cu-steps" aria-label={t("log.title")}>
          {[...s.steps].reverse().map((st) => (
            <StepRow key={st.index} step={st} />
          ))}
        </ul>
      )}

      {s.summary && !active && <div className="cu-summary">{s.summary}</div>}

      <div className="cu-actions">
        {active ? (
          <>
            {s.state === "Paused" ? (
              <Button size="sm" variant="secondary" leftIcon={<Play size={14} />} onClick={() => void resumeCursor()}>
                {t("control.resume")}
              </Button>
            ) : (
              <Button size="sm" variant="secondary" leftIcon={<Pause size={14} />} onClick={() => void pauseCursor()}>
                {t("control.pause")}
              </Button>
            )}
            <Button size="sm" variant="danger" leftIcon={<OctagonX size={14} />} onClick={() => void stopCursor()}>
              {t("control.stop")}
            </Button>
          </>
        ) : (
          <Button size="sm" variant="ghost" onClick={() => useCursorStore.getState().patch({ open: true, dismissed: s.sessionId })}>
            {t("control.new")}
          </Button>
        )}
      </div>
      {active && <div className="cu-hint">{t("hint.abort")}</div>}
    </div>
  );
}

function ConsentCard() {
  const { t } = useT("cursor");
  const c = useCursorStore((s) => s.consent);
  if (!c) return null;
  return (
    <div className="cu-consent" role="dialog" aria-label={t("consent.title")}>
      <div className="cu-block__title">
        <ShieldAlert size={15} /> {t("consent.title")}
      </div>
      <p className="cu-note">{t("consent.body", { provider: c.backend.label })}</p>
      <div className="cu-actions">
        <Button size="sm" variant="primary" leftIcon={<Check size={14} />} onClick={() => void grantConsentAndStart()}>
          {t("consent.allow")}
        </Button>
        <Button size="sm" variant="ghost" onClick={declineConsent}>
          {t("consent.cancel")}
        </Button>
      </div>
    </div>
  );
}

/** The cursor-mode block of the assistant surface. */
export function CursorPanel({ initialTask = "" }: { initialTask?: string }) {
  const { t } = useT("cursor");
  const st = useCursorStore();
  const openSettings = useSettingsStore((v) => v.openSettings);
  const [task, setTask] = useState(initialTask);
  useEffect(() => {
    watchCursor();
    void refreshCursor();
  }, []);
  const session = st.status?.session;
  const active = st.status?.backends?.find((b) => b.id === st.status?.activeBackend);

  if (session && (isActive(session) || session.sessionId !== st.dismissed)) {
    return (
      <section className="cu-panel" aria-label={t("title")}>
        <SessionView s={session} />
      </section>
    );
  }
  if (!st.open) {
    return (
      <button type="button" className="cu-launch" onClick={() => st.patch({ open: true })} data-testid="cursor-launch">
        <MousePointer2 size={15} /> {t("launch")}
      </button>
    );
  }
  const submit = (e: FormEvent) => {
    e.preventDefault();
    void startCursor(task, "ui");
  };
  return (
    <section className="cu-panel" aria-label={t("title")}>
      <form className="cu-start" onSubmit={submit}>
        <div className="cu-block__title">
          <MousePointer2 size={15} /> {t("title")}
          <span className="cu-spacer" />
          <IconButton label={t("openSettings")} icon={<Settings2 size={14} />} onClick={() => openSettings("cursor")} />
          <IconButton label={t("close")} icon={<X size={14} />} onClick={() => st.patch({ open: false, error: null })} />
        </div>
        <textarea
          className="cu-input cu-input--task"
          value={task}
          rows={2}
          onChange={(e) => setTask(e.target.value)}
          placeholder={t("placeholder")}
          aria-label={t("placeholder")}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) submit(e);
          }}
        />
        <div className="cu-row">
          <span className="cu-model">
            {active ? t("model.active", { name: `${active.label}${active.model ? ` · ${active.model}` : ""}` }) : t("model.none")}
          </span>
          <Button size="sm" variant="primary" type="submit" loading={st.busy} disabled={!task.trim() || st.status?.supported === false}>
            {t("start")}
          </Button>
        </div>
        {st.status?.supported === false && <div className="cu-error">{st.status.unsupportedReason}</div>}
        {st.error && <div className="cu-error">{st.error}</div>}
        <p className="cu-note">{t("privacy")}</p>
      </form>
      <ConsentCard />
    </section>
  );
}
