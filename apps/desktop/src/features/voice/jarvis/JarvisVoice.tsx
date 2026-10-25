/**
 * Jarvis voice card: the reactor circle with a glass transcript panel
 * attached — live partial transcript (words fade in), the final transcript,
 * the assistant reply typed out in step with the voice, a status line
 * (state, mode, engine, microphone) and quick actions (repeat, cancel, open
 * in Orkestr, yes / no for a pending question).
 *
 * Generic on purpose (wave 9 packages plug in): hosts pass
 * `{ state, level, partialText, finalText, replyText, mode }` plus optional
 * callbacks; it never reads the voice store itself. Never takes the focus:
 * nothing autofocuses, buttons only act on click.
 */
import { clsx } from "clsx";
import { Check, ExternalLink, Mic, RotateCcw, X } from "lucide-react";
import { type PointerEvent as ReactPointerEvent, useEffect, useRef, useState } from "react";

import { useT } from "../../../i18n";
import { JarvisReactor } from "./JarvisReactor";
import { BANDS, type JarvisMode, type JarvisSignal, type JarvisState, snapToWord, splitWords, statusKey, typedChars } from "./model";
import "./jarvis.css";

export interface JarvisVoiceProps {
  state: JarvisState;
  /** Level 0..1 (fallback when `signal` has nothing live). */
  level: number;
  /** Live (interim) transcript while listening. */
  partialText: string;
  /** Final transcript (what was heard). */
  finalText: string;
  /** Assistant reply (typed out). */
  replyText: string;
  mode?: JarvisMode;
  /** Pipeline phase, refines the status line (`transcribing`, `confirm`). */
  phase?: string;
  replyTone?: string;
  /** A pending yes / no question. */
  confirmText?: string | null;
  /** Engine label for the status line ("Vosk (offline)"). */
  engine?: string | null;
  /** The microphone is open (privacy indicator). */
  micOpen?: boolean;
  /** The reply is being spoken (typing follows the voice). */
  speaking?: boolean;
  /** Reactor diameter, CSS px (120–180 on the desktop card). */
  size?: number;
  /** Reactor frame cap; 0 = still frame. */
  fps: number;
  /** Text animations (word fade-in, typing) allowed. */
  motion?: boolean;
  signal?: JarvisSignal | null;
  themeKey?: string;
  accent?: string | null;
  /** Where the reactor sits relative to the panel (next to the orb). */
  reactorSide?: "start" | "end";
  /** Compact in-app variant (no own panel chrome; the host frames it). */
  variant?: "card" | "inline";
  /**
   * Render the reactor here (default). The desktop orb draws the reactor
   * itself (it *is* the orb) and shows only the panel next to it.
   */
  showReactor?: boolean;
  /** Closing: collapse back into the orb. */
  leaving?: boolean;
  onRepeat?: () => void;
  onCancel?: () => void;
  onOpen?: () => void;
  onConfirm?: (yes: boolean) => void;
  /** Pointer down on the reactor (window drag). */
  onReactorPointerDown?: (e: ReactPointerEvent) => void;
}

/** Words that fade in as they arrive. */
export function JarvisWords({ text, className, motion = true }: { text: string; className?: string; motion?: boolean }) {
  const words = splitWords(text);
  return (
    <span className={clsx("jv-words", motion && "jv-words--motion", className)}>
      {words.map((w, i) => (
        <span key={i} className="jv-word">
          {w}
        </span>
      ))}
    </span>
  );
}

/** The reply typed out, following the TTS progress when it is spoken. */
function useTyped(text: string, speaking: boolean, motion: boolean, signal: JarvisSignal | null): number {
  const [shown, setShown] = useState(motion ? 0 : text.length);
  const startedAt = useRef(0);
  const live = useRef({ speaking, signal });
  live.current = { speaking, signal };
  useEffect(() => {
    if (!motion || !text) {
      setShown(text.length);
      return undefined;
    }
    startedAt.current = performance.now();
    setShown(0);
    const scratch = new Float32Array(BANDS);
    let done = false;
    const timer = window.setInterval(() => {
      const now = performance.now();
      const s = live.current.signal?.sample(scratch, now) ?? null;
      const n = typedChars({ length: text.length, elapsedMs: now - startedAt.current, progress: s?.progress ?? null, speaking: live.current.speaking, animate: true });
      const snapped = snapToWord(text, n);
      setShown((prev) => Math.max(prev, snapped));
      if (snapped >= text.length && !done) {
        done = true;
        window.clearInterval(timer);
      }
    }, 45);
    return () => window.clearInterval(timer);
  }, [text, motion]);
  return Math.min(shown, text.length);
}

export function JarvisVoice(props: JarvisVoiceProps) {
  const { t } = useT("jarvis");
  const {
    state,
    level,
    partialText,
    finalText,
    replyText,
    mode = "command",
    phase,
    replyTone = "info",
    confirmText = null,
    engine = null,
    micOpen = false,
    speaking = false,
    size = 150,
    fps,
    motion = true,
    signal = null,
    themeKey,
    accent = null,
    reactorSide = "start",
    variant = "card",
    leaving = false,
    showReactor = true,
  } = props;
  const typed = useTyped(replyText, speaking, motion, signal);
  const bodyRef = useRef<HTMLDivElement | null>(null);

  // Keep the newest line in view.
  useEffect(() => {
    const el = bodyRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [partialText, finalText, typed, confirmText]);

  const listening = state === "listening" || state === "wake";
  const heard = listening ? partialText : finalText || partialText;
  const status = t(statusKey(state, phase));
  const replyShown = replyText.slice(0, typed);
  const typing = typed < replyText.length;

  const reactor = (
    <div className="jv__reactor" style={{ width: size, height: size }} onPointerDown={props.onReactorPointerDown}>
      <JarvisReactor state={state} level={level} size={size} fps={fps} signal={signal} themeKey={themeKey ?? ""} accent={accent} />
    </div>
  );

  return (
    <section
      className={clsx("jv", `jv--${variant}`, `jv--${state}`, `jv--mode-${mode}`, reactorSide === "end" && "jv--reverse", !showReactor && "jv--panel-only", leaving && "jv--leaving", motion && "jv--motion")}
      role={variant === "card" ? "dialog" : "group"}
      aria-label={t("title")}
      data-jarvis-state={state}
      data-jarvis-mode={mode}
      style={{ ["--jv-size" as string]: `${size}px` }}
    >
      {showReactor && reactor}
      <div className="jv__panel">
        <header className="jv__status" aria-live="polite">
          <span className={clsx("jv__dot", `jv__dot--${state}`)} aria-hidden="true" />
          <span className="jv__state">{status}</span>
          <span className="jv__mode">{t(`mode.${mode}`)}</span>
          <span className="jv__spacer" />
          {micOpen && (
            <span className="jv__mic" title={t("micOn")}>
              <Mic size={11} aria-label={t("micOn")} />
            </span>
          )}
          {engine && <span className="jv__engine">{engine}</span>}
          {props.onCancel && (
            <button type="button" className="jv__icon" aria-label={t("actions.cancel")} title={t("actions.cancel")} onClick={props.onCancel}>
              <X size={13} />
            </button>
          )}
        </header>
        <div className="jv__body" ref={bodyRef}>
          {heard ? (
            <p className={clsx("jv__heard", listening ? "jv__heard--live" : "jv__heard--final")}>
              <JarvisWords text={heard} motion={motion} />
              {listening && <span className="jv__caret" aria-hidden="true" />}
            </p>
          ) : (
            <p className="jv__hint">{listening ? t("hint.listening") : state === "thinking" ? t("hint.thinking") : t("hint.idle")}</p>
          )}
          {confirmText ? (
            <p className="jv__reply jv__reply--confirm">{confirmText}</p>
          ) : replyText ? (
            <p className={clsx("jv__reply", `jv__reply--${replyTone}`)} role="status" aria-label={replyText}>
              <span aria-hidden="true">{replyShown}</span>
              {typing && <span className="jv__caret jv__caret--reply" aria-hidden="true" />}
            </p>
          ) : null}
        </div>
        {(confirmText && props.onConfirm) || (replyText && (props.onRepeat || props.onOpen)) || (!replyText && props.onOpen && variant === "card") ? (
          <footer className="jv__actions">
            {confirmText && props.onConfirm ? (
              <>
                <button type="button" className="jv__btn jv__btn--primary" onClick={() => props.onConfirm?.(true)}>
                  <Check size={13} />
                  {t("actions.yes")}
                </button>
                <button type="button" className="jv__btn" onClick={() => props.onConfirm?.(false)}>
                  {t("actions.no")}
                </button>
              </>
            ) : (
              <>
                {replyText && props.onRepeat && (
                  <button type="button" className="jv__btn" title={t("actions.repeatHint")} onClick={props.onRepeat}>
                    <RotateCcw size={12} />
                    {t("actions.repeat")}
                  </button>
                )}
                {props.onOpen && (
                  <button type="button" className="jv__btn" onClick={props.onOpen}>
                    <ExternalLink size={12} />
                    {t("actions.open")}
                  </button>
                )}
              </>
            )}
          </footer>
        ) : null}
      </div>
    </section>
  );
}
