/**
 * Compact voice card next to the orb: shown when a voice command (wake word,
 * push-to-talk) runs while Orkestr is in the background, so the main window
 * never has to come forward. What was heard, the reply, yes / no for a
 * pending question; the microphone indicator stays visible while it is open
 * (§33 privacy). Closes itself when the command finishes.
 */
import { clsx } from "clsx";
import { Mic, X } from "lucide-react";

import { useT } from "../../i18n";
import type { OrbVoiceAction, OrbVoiceState } from "./channel";

export interface OrbVoicePopupProps {
  voice: OrbVoiceState;
  onAction: (action: OrbVoiceAction) => void;
  onOpenMain: () => void;
}

export function OrbVoicePopup({ voice, onAction, onOpenMain }: OrbVoicePopupProps) {
  const { t } = useT("orb");
  const phase = voice.phase;
  const status =
    phase === "listening" ? t("voice.listening") : phase === "transcribing" ? t("voice.transcribing") : phase === "thinking" ? t("voice.thinking") : voice.speaking ? t("voice.speaking") : t("voice.title");
  const reply = voice.reply ?? null;
  return (
    <section className="orb-voice" role="dialog" aria-label={t("voice.title")} aria-live="polite" data-voice-phase={phase}>
      <header className="orb-voice__head">
        <span className={clsx("orb-voice__status", voice.micOpen && "orb-voice__status--mic")}>
          {voice.micOpen && <Mic size={13} aria-label={t("voice.micOn")} />}
          {status}
        </span>
        <button type="button" className="orb-icon-btn" aria-label={t("voice.close")} title={t("voice.close")} onClick={() => onAction("cancel")}>
          <X size={14} />
        </button>
      </header>
      {voice.transcript ? <p className="orb-voice__heard">«{voice.transcript}»</p> : null}
      {voice.confirmText ? (
        <div className="orb-voice__confirm">
          <p className="orb-voice__reply">{voice.confirmText}</p>
          <div className="orb-voice__actions">
            <button type="button" className="orb-voice__btn orb-voice__btn--ok" onClick={() => onAction("yes")}>
              {t("voice.yes")}
            </button>
            <button type="button" className="orb-voice__btn" onClick={() => onAction("no")}>
              {t("voice.no")}
            </button>
          </div>
        </div>
      ) : reply ? (
        <p className={clsx("orb-voice__reply", `orb-voice__reply--${reply.tone}`)}>{reply.text}</p>
      ) : null}
      {reply && !voice.confirmText ? (
        <div className="orb-voice__actions">
          <button type="button" className="orb-voice__btn" onClick={onOpenMain}>
            {t("hub.openMain")}
          </button>
        </div>
      ) : null}
    </section>
  );
}
