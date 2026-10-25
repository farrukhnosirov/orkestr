/**
 * Assistant mode surface (ADR-0035): a small centred panel — voice orb and
 * state, last transcript, the reply, confirmation / AI-plan cards, quick
 * command search with live matches, example suggestions, recent commands
 * and the switch back to Orchestra mode.
 */
import type { CommandMatch } from "@orkestr/protocol";
import { Button, IconButton } from "@orkestr/ui";
import { clsx } from "clsx";
import {
  BookOpen,
  Check,
  CircleCheck,
  CircleX,
  CornerDownLeft,
  LayoutDashboard,
  Mic,
  OctagonX,
  Save,
  Search,
  Info,
  ShieldAlert,
  Sparkles,
  Square,
  TriangleAlert,
  Wand2,
  X,
} from "lucide-react";
import { type FormEvent, type KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";

import { getClient } from "../../core/client";
import { currentLanguage, useT } from "../../i18n";
import { CursorPanel } from "../cursor/CursorPanel";
import { useSettingsStore } from "../settings/settingsStore";
import { say, startListening, stopListening, sttReady } from "../voice/controller";
import { useVoiceStore } from "../voice/store";
import {
  type AssistantRun,
  approvePlan,
  confirmPending,
  correctPlan,
  dismissPlan,
  refreshHistory,
  rememberPlan,
  riskText,
  runAssistant,
  runCommand,
  stepText,
  stopAssistant,
} from "./api";
import { type AssistantTone, useAssistantStore } from "./store";
import "./assistant.css";

const EXAMPLES = ["e1", "e2", "e3", "e4", "e5", "e6"] as const;

function speakRun(r: AssistantRun): void {
  if (r.reply) void say(r.reply);
  for (const s of r.speak) void say(s);
}

export function AssistantSurface() {
  const { t } = useT("assistant");
  const s = useAssistantStore();
  const phase = useVoiceStore((v) => v.phase);
  const transcript = useVoiceStore((v) => v.transcript);
  const speaking = useVoiceStore((v) => v.speaking);
  const voiceEnabled = useVoiceStore((v) => v.config?.enabled ?? false);
  const openSettings = useSettingsStore((v) => v.openSettings);
  const [text, setText] = useState("");
  const [matches, setMatches] = useState<CommandMatch[]>([]);
  const [selected, setSelected] = useState(-1);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    void refreshHistory();
    requestAnimationFrame(() => inputRef.current?.focus());
  }, []);

  // quick command search: live matches while typing
  useEffect(() => {
    const q = text.trim();
    if (q.length < 2) {
      setMatches([]);
      return;
    }
    let live = true;
    const h = window.setTimeout(() => {
      getClient()
        .call("computer.match", { text: q, lang: currentLanguage(), limit: 5 })
        .then((r) => live && setMatches(r.matches.filter((m) => m.score >= 0.5)))
        .catch(() => live && setMatches([]));
    }, 120);
    return () => {
      live = false;
      window.clearTimeout(h);
    };
  }, [text]);

  const state = s.busy ? "thinking" : speaking ? "speaking" : phase === "idle" || phase === "result" ? "ready" : phase;
  const heard = phase === "listening" || phase === "transcribing" ? transcript : s.input || transcript;

  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (selected >= 0 && matches[selected]) return pick(matches[selected]);
    const v = text.trim();
    if (!v || s.busy) return;
    setText("");
    setMatches([]);
    speakRun(await runAssistant(v, "text", true));
  };

  const pick = async (m: CommandMatch) => {
    setText("");
    setMatches([]);
    setSelected(-1);
    speakRun(await runCommand(m.commandId, m.setId, m.slots, m.title));
  };

  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown" && matches.length) {
      e.preventDefault();
      setSelected((i) => (i + 1) % matches.length);
    } else if (e.key === "ArrowUp" && matches.length) {
      e.preventDefault();
      setSelected((i) => (i <= 0 ? matches.length - 1 : i - 1));
    } else if (e.key === "Escape") {
      setMatches([]);
      setSelected(-1);
    }
  };

  const talk = {
    onPointerDown: () => void startListening("PushToTalk"),
    onPointerUp: () => void stopListening(),
    onPointerLeave: () => {
      if (useVoiceStore.getState().phase === "listening") void stopListening();
    },
  };

  return (
    <div className="as-surface" data-testid="assistant-surface">
      <section className={clsx("as-card", `as-card--${state}`)} aria-label={t("surface.title")}>
        <header className="as-head">
          <button
            type="button"
            className={clsx("as-orb", `as-orb--${state}`)}
            aria-label={voiceEnabled && sttReady() ? t("surface.holdToTalk") : t("surface.voiceOff")}
            title={voiceEnabled && sttReady() ? t("surface.holdToTalk") : t("surface.voiceOff")}
            {...(voiceEnabled ? talk : { onClick: () => openSettings("voice") })}
          >
            <span className="as-orb__core">
              <Mic size={22} />
            </span>
          </button>
          <div className="as-head__text">
            <div className="as-state" aria-live="polite">
              {t(`state.${state}`, { defaultValue: t("state.ready") })}
            </div>
            <div className="as-heard" title={heard}>
              {heard ? `“${heard}”` : t("surface.prompt")}
            </div>
          </div>
          <IconButton label={t("surface.stop")} tooltip="left" variant="danger" icon={<OctagonX size={16} />} onClick={() => void stopAssistant()} />
        </header>

        {s.reply && (
          <div className={clsx("as-reply", `as-reply--${s.reply.tone}`)} role="status">
            <ReplyIcon tone={s.reply.tone} />
            <span className="as-reply__text">{s.reply.text}</span>
          </div>
        )}

        <CursorPanel />

        {s.pending && (
          <div className="as-block as-confirm" role="alertdialog" aria-label={t("confirm.title")}>
            <div className="as-block__title">
              <ShieldAlert size={15} /> {t("confirm.title")}
            </div>
            <ul className="as-list">
              {s.pending.reasons.map((r, i) => (
                <li key={i}>{riskText(r)}</li>
              ))}
            </ul>
            <div className="as-actions">
              <Button variant="danger" size="sm" leftIcon={<Check size={14} />} loading={s.busy} onClick={async () => speakRun(await confirmPending(true))}>
                {t("confirm.approve")}
              </Button>
              <Button variant="ghost" size="sm" leftIcon={<X size={14} />} onClick={async () => speakRun(await confirmPending(false))}>
                {t("confirm.reject")}
              </Button>
            </div>
          </div>
        )}

        {s.plan && <PlanCard />}

        {s.shown && (
          <div className="as-block">
            <div className="as-block__title">{s.shown.title}</div>
            <ul className="as-list as-list--scroll">
              {s.shown.lines.map((l, i) => (
                <li key={i}>{l}</li>
              ))}
            </ul>
          </div>
        )}

        <form className="as-input" onSubmit={(e) => void submit(e)}>
          <Search size={15} className="as-input__icon" aria-hidden />
          <input
            ref={inputRef}
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setSelected(-1);
            }}
            onKeyDown={onKey}
            placeholder={t("surface.placeholder")}
            aria-label={t("surface.placeholder")}
            aria-autocomplete="list"
            aria-expanded={matches.length > 0}
            disabled={s.busy}
          />
          <IconButton type="submit" label={t("surface.run")} icon={<CornerDownLeft size={15} />} disabled={!text.trim() || s.busy} />
        </form>

        {matches.length > 0 && (
          <ul className="as-matches" role="listbox" aria-label={t("surface.matches")}>
            {matches.map((m, i) => (
              <li key={`${m.setId}/${m.commandId}`} role="option" aria-selected={i === selected}>
                <button type="button" className={clsx("as-match", i === selected && "as-match--sel")} onClick={() => void pick(m)}>
                  <span className="as-match__title">{m.title}</span>
                  <span className="as-match__phrase">{m.phrase}</span>
                  {m.safety !== "Safe" && <ShieldAlert size={13} className="as-match__risk" aria-label={t(`safety.${m.safety}`)} />}
                </button>
              </li>
            ))}
          </ul>
        )}

        {!s.plan && !s.pending && matches.length === 0 && (
          <div className="as-chips" aria-label={t("surface.examples")}>
            {EXAMPLES.map((k) => (
              <button key={k} type="button" className="as-chip" onClick={() => setText(t(`examples.${k}`))}>
                <Sparkles size={12} /> {t(`examples.${k}`)}
              </button>
            ))}
          </div>
        )}

        <Recent />

        <footer className="as-foot">
          <span className="as-foot__meta">
            {s.status ? t("surface.catalogue", { n: s.status.catalogueCommands, user: s.status.userCommands }) : ""}
          </span>
          <Button variant="ghost" size="sm" leftIcon={<BookOpen size={14} />} onClick={() => openSettings("assistant")}>
            {t("surface.commandSets")}
          </Button>
          <Button variant="secondary" size="sm" leftIcon={<LayoutDashboard size={14} />} onClick={() => s.setMode("orchestra")}>
            {t("mode.toOrchestra")}
          </Button>
        </footer>
      </section>
    </div>
  );
}

/** Small status mark of the reply card (the text itself is natural speech). */
function ReplyIcon({ tone }: { tone: AssistantTone }) {
  const Icon = tone === "ok" ? CircleCheck : tone === "error" ? CircleX : tone === "warn" ? TriangleAlert : Info;
  return <Icon size={15} className="as-reply__icon" aria-hidden />;
}

function PlanCard() {
  const { t } = useT("assistant");
  const plan = useAssistantStore((s) => s.plan);
  const ranPlanId = useAssistantStore((s) => s.ranPlanId);
  const busy = useAssistantStore((s) => s.busy);
  const [correction, setCorrection] = useState("");
  const [phrase, setPhrase] = useState("");
  const ran = !!plan && ranPlanId === plan.planId;
  useEffect(() => setPhrase(plan?.request ?? ""), [plan?.request]);
  const steps = useMemo(() => plan?.steps ?? [], [plan]);
  if (!plan) return null;
  return (
    <div className="as-block as-plan" aria-label={t("plan.title")}>
      <div className="as-block__title">
        <Wand2 size={15} /> {t("plan.title")}
        <span className="as-block__meta">{plan.model}</span>
      </div>
      <p className="as-plan__why">{plan.explanation}</p>
      <ol className="as-steps">
        {steps.map((st, i) => (
          <li key={i} className={clsx(plan.risks.some((r) => r.step === i) && "as-steps__risky")}>
            {stepText(st)}
          </li>
        ))}
      </ol>
      {plan.risks.length > 0 && <div className="as-plan__risk">{t("plan.risky", { reasons: plan.risks.map(riskText).join("; ") })}</div>}
      {!ran ? (
        <>
          <div className="as-actions">
            <Button variant="primary" size="sm" leftIcon={<Check size={14} />} loading={busy} onClick={async () => speakRun(await approvePlan())}>
              {t("plan.run")}
            </Button>
            <Button variant="ghost" size="sm" leftIcon={<Square size={13} />} onClick={() => dismissPlan()}>
              {t("plan.dismiss")}
            </Button>
          </div>
          <form
            className="as-inline"
            onSubmit={async (e) => {
              e.preventDefault();
              const c = correction.trim();
              if (!c) return;
              setCorrection("");
              speakRun(await correctPlan(c));
            }}
          >
            <input value={correction} onChange={(e) => setCorrection(e.target.value)} placeholder={t("plan.correctPlaceholder")} aria-label={t("plan.correct")} />
            <Button type="submit" size="sm" disabled={!correction.trim() || busy}>
              {t("plan.correct")}
            </Button>
          </form>
        </>
      ) : (
        <form
          className="as-inline"
          onSubmit={async (e) => {
            e.preventDefault();
            speakRun(await rememberPlan(phrase));
          }}
        >
          <input value={phrase} onChange={(e) => setPhrase(e.target.value)} placeholder={t("plan.rememberPlaceholder")} aria-label={t("plan.rememberAs")} />
          <Button type="submit" variant="primary" size="sm" leftIcon={<Save size={14} />} disabled={busy}>
            {t("plan.remember")}
          </Button>
        </form>
      )}
    </div>
  );
}

function Recent() {
  const { t } = useT("assistant");
  const history = useAssistantStore((s) => s.history);
  if (history.length === 0) return null;
  return (
    <div className="as-recent">
      <div className="as-recent__title">{t("surface.recent")}</div>
      <ul>
        {history.slice(0, 5).map((h, i) => (
          <li key={`${h.at}-${i}`}>
            <button type="button" className="as-recent__item" title={h.text} onClick={async () => speakRun(await runAssistant(h.text, "text", true))}>
              <span className={clsx("as-dot", `as-dot--${h.outcome}`)} aria-hidden />
              <span className="as-recent__text">{h.title || h.text}</span>
              <span className="as-recent__outcome">{t(`outcome.${h.outcome}`, { defaultValue: h.outcome })}</span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
