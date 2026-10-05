/**
 * Voice HUD overlay (§33, §34): listening orb + waveform, live transcript,
 * the result / confirmation card and a typed-command field. Floats above the
 * workspace (bottom centre), glass + glow within the visual budget. Esc
 * cancels; it closes itself after a result unless pinned open.
 */
import { Button, IconButton, Kbd } from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, CornerDownLeft, Lock, Mic, MicOff, Settings2, ShieldAlert, Square, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type FormEvent, useEffect, useRef, useState } from "react";

import { useT } from "../../../i18n";
import { useSettingsStore } from "../../settings/settingsStore";
import { answerConfirmation, cancelVoice, currentAnalyser, startListening, stopListening, submitTyped, sttReady } from "../controller";
import { hotkeyKeys } from "../hotkey";
import { useVoiceStore } from "../store";
import { useVisualBudget } from "./visual";
import { Waveform } from "./Waveform";
import "../voice.css";

const EXAMPLES = ["ex1", "ex2", "ex3", "ex4"] as const;

export function VoiceHud() {
  const { t } = useT(["voice", "common"]);
  const hudOpen = useVoiceStore((s) => s.hudOpen);
  const phase = useVoiceStore((s) => s.phase);
  const transcript = useVoiceStore((s) => s.transcript);
  const reply = useVoiceStore((s) => s.reply);
  const confirmText = useVoiceStore((s) => s.confirmText);
  const level = useVoiceStore((s) => s.level);
  const speaking = useVoiceStore((s) => s.speaking);
  const config = useVoiceStore((s) => s.config);
  const setHudOpen = useVoiceStore((s) => s.setHudOpen);
  const openSettings = useSettingsStore((s) => s.openSettings);
  const budget = useVisualBudget();
  const [text, setText] = useState("");
  const inputRef = useRef<HTMLInputElement | null>(null);

  const visible = hudOpen || phase !== "idle";
  const canTalk = sttReady();
  const cloud = config?.sttEngine === "OpenAiWhisper" || config?.sttEngine === "WebSpeech";

  useEffect(() => {
    if (visible && !canTalk) requestAnimationFrame(() => inputRef.current?.focus());
  }, [visible, canTalk]);

  const close = () => {
    cancelVoice();
    setHudOpen(false);
    setText("");
  };

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    const v = text.trim();
    if (!v) return;
    setText("");
    void submitTyped(v);
  };

  const phaseLabel = speaking ? t("hud.speaking") : t(`hud.phases.${phase}`);
  const keys = hotkeyKeys(config?.pushToTalkHotkey ?? "Ctrl+Space");
  const analyser = phase === "listening" ? currentAnalyser() : null;

  return (
    <AnimatePresence>
      {visible && (
        <motion.div
          key="vx-hud"
          className={clsx("vx-hud", budget.effects && "vx-hud--fx", `vx-hud--${phase}`)}
          role="dialog"
          aria-modal="false"
          aria-label={t("hud.title")}
          initial={budget.motion ? { opacity: 0, y: 24, scale: 0.96 } : false}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={budget.motion ? { opacity: 0, y: 16, scale: 0.97 } : { opacity: 0 }}
          transition={{ type: "spring", stiffness: 420, damping: 34 }}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.stopPropagation();
              close();
            }
          }}
        >
          <div className="vx-hud__glow" aria-hidden="true" />
          <div className="vx-hud__top">
            <div className="vx-hud__orb">
              <Waveform analyser={analyser} phase={phase} level={level} fps={budget.fps} />
              <span className="vx-hud__orb-icon" aria-hidden="true">
                {phase === "listening" ? <Mic size={20} /> : phase === "error" ? <MicOff size={20} /> : <Mic size={18} />}
              </span>
            </div>
            <div className="vx-hud__main">
              <div className="vx-hud__phase" aria-live="polite">
                <span className={clsx("vx-dot", `vx-dot--${phase}`)} />
                {phaseLabel}
              </div>
              <div className={clsx("vx-hud__transcript", !transcript && "vx-hud__transcript--empty")}>
                {transcript || (phase === "listening" ? t("hud.speakNow") : t("hud.prompt"))}
              </div>
              <div className="vx-hud__privacy">
                <Lock size={11} />
                {cloud && canTalk ? t("hud.privacyCloud", { engine: t(`settings.stt.engines.${config?.sttEngine ?? "Disabled"}`) }) : t("hud.privacyLocal")}
              </div>
            </div>
            <div className="vx-hud__actions">
              <IconButton label={t("hud.settings")} icon={<Settings2 size={15} />} onClick={() => openSettings("voice")} />
              <IconButton label={t("common:actions.close")} icon={<X size={15} />} onClick={close} />
            </div>
          </div>

          <AnimatePresence mode="popLayout">
            {phase === "confirm" && confirmText && (
              <motion.div
                key="confirm"
                className="vx-card vx-card--confirm"
                initial={budget.motion ? { opacity: 0, y: 8 } : false}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0 }}
              >
                <ShieldAlert size={16} className="vx-card__icon" />
                <div className="vx-card__text">
                  {confirmText}
                  <div className="vx-card__hint">{t("hud.confirmHint")}</div>
                </div>
                <div className="vx-card__buttons">
                  <Button variant="ghost" onClick={() => void answerConfirmation(false)}>
                    {t("hud.no")}
                  </Button>
                  <Button variant="primary" leftIcon={<Check size={14} />} onClick={() => void answerConfirmation(true)} autoFocus>
                    {t("hud.yes")}
                  </Button>
                </div>
              </motion.div>
            )}
            {reply && phase !== "confirm" && (
              <motion.div
                key={`reply-${reply.text}`}
                className={clsx("vx-card", `vx-card--${reply.tone}`)}
                initial={budget.motion ? { opacity: 0, y: 8 } : false}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0 }}
                role="status"
              >
                <div className="vx-card__text">{reply.text}</div>
              </motion.div>
            )}
          </AnimatePresence>

          {!canTalk && <div className="vx-hud__note">{t("hud.typedOnly")}</div>}

          <form className="vx-hud__form" onSubmit={onSubmit}>
            {canTalk && (
              <button
                type="button"
                className={clsx("vx-talk", phase === "listening" && "vx-talk--on")}
                aria-pressed={phase === "listening"}
                onClick={() => void (phase === "listening" ? stopListening() : startListening("PushToTalk"))}
              >
                {phase === "listening" ? <Square size={13} /> : <Mic size={15} />}
                {phase === "listening" ? t("hud.stop") : t("hud.talk")}
              </button>
            )}
            <input
              ref={inputRef}
              className="vx-hud__input"
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder={t("hud.placeholder")}
              aria-label={t("hud.placeholder")}
              maxLength={500}
            />
            <IconButton label={t("hud.send")} icon={<CornerDownLeft size={15} />} type="submit" disabled={!text.trim()} />
          </form>

          <div className="vx-hud__foot">
            {canTalk ? (
              <span className="vx-hud__keys">
                {t("hud.hold")}
                {keys.map((k) => (
                  <Kbd key={k}>{k}</Kbd>
                ))}
                {config?.wake.enabled && <span className="vx-hud__wake">{t("hud.sayWake", { phrase: config.wake.phrase })}</span>}
              </span>
            ) : (
              <span />
            )}
            <span className="vx-hud__examples">
              {EXAMPLES.map((k) => (
                <button key={k} type="button" className="vx-chip" onClick={() => void submitTyped(t(`hud.examples.${k}`))}>
                  {t(`hud.examples.${k}`)}
                </button>
              ))}
            </span>
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
