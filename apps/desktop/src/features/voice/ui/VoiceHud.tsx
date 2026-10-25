/**
 * Voice HUD overlay (§33, §34): listening orb + waveform, live transcript,
 * the result / confirmation card and a typed-command field. Floats above the
 * workspace (bottom centre), glass + glow within the visual budget. Esc
 * cancels; it closes itself after a result unless pinned open.
 */
import { Button, IconButton, Kbd } from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, CornerDownLeft, Lock, Mic, Settings2, ShieldAlert, Square, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type FormEvent, useEffect, useRef, useState } from "react";

import { useT } from "../../../i18n";
import { useSettingsStore } from "../../settings/settingsStore";
import { useUiPrefs } from "../../aiui/aiuiStore";
import { orbPrefsOf } from "../../orb/model";
import { orbShell } from "../../orb/shell";
import { usePerfStore } from "../../performance/perfStore";
import { answerConfirmation, cancelVoice, repeatReply, startListening, stopListening, submitTyped, sttReady } from "../controller";
import { JarvisVoice } from "../jarvis/JarvisVoice";
import { reactorFps } from "../jarvis/model";
import { liveVoiceSignal } from "../jarvis/signal";
import { useJarvisDebug, useJarvisState } from "../jarvis/useJarvisState";
import { hotkeyKeys } from "../hotkey";
import { useVoiceStore } from "../store";
import { useVisualBudget } from "./visual";
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
  const source = useVoiceStore((s) => s.source);
  const mic = useVoiceStore((s) => s.mic);
  const assistantMode = useVoiceStore((s) => s.assistantMode);
  const budget = useVisualBudget();
  const perfMode = usePerfStore((s) => s.mode);
  const reduced = usePerfStore((s) => s.reducedMotion);
  const paused = usePerfStore((s) => s.policy.paused || s.policy.idle);
  const jvState = useJarvisState(phase, speaking, source);
  const forced = useJarvisDebug();
  const [text, setText] = useState("");
  const inputRef = useRef<HTMLInputElement | null>(null);

  // One voice circle: with the desktop orb shown, the orb grows into it, so
  // this HUD opens only on request (typed commands) and shows no reactor.
  const orbCarriesVoice = orbShell.available() && orbPrefsOf(useUiPrefs().orb).enabled;
  const visible = hudOpen || forced !== null || (!orbCarriesVoice && phase !== "idle");
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

  const keys = hotkeyKeys(config?.pushToTalkHotkey ?? "Ctrl+Space");
  const listening = phase === "listening";
  const engineId = speaking ? config?.ttsEngine : config?.sttEngine;
  const engine = !canTalk
    ? null
    : speaking && config?.ttsEngine
      ? t(`settings.tts.engines.${config.ttsEngine}`)
      : config?.sttEngine
        ? t(`settings.stt.engines.${config.sttEngine}`)
        : null;
  const fx = forced;

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
            <div className="vx-hud__main">
              <JarvisVoice
                key={orbCarriesVoice ? "panel" : "reactor"}
                variant="inline"
                showReactor={!orbCarriesVoice}
                size={112}
                state={fx?.state ?? jvState}
                level={fx?.level ?? level}
                partialText={fx ? (fx.partialText ?? "") : listening ? transcript : ""}
                finalText={fx ? (fx.finalText ?? "") : listening ? "" : transcript}
                replyText={fx ? (fx.replyText ?? "") : phase !== "confirm" ? (reply?.text ?? "") : ""}
                replyTone={reply?.tone ?? "info"}
                mode={fx?.mode ?? assistantMode}
                phase={fx?.phase ?? phase}
                engine={fx?.engine ?? (engineId ? engine : null)}
                micOpen={fx?.micOpen ?? mic.includes("command")}
                speaking={fx?.speaking ?? speaking}
                fps={reactorFps(perfMode, reduced, paused)}
                motion={budget.motion}
                signal={fx ? null : liveVoiceSignal}
                onRepeat={repeatReply}
              />
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
