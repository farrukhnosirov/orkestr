/**
 * Jarvis voice card next to the orb: shown when a voice command (wake word,
 * push-to-talk) runs while Orkestr is in the background, so the main window
 * never has to come forward (and the user's app keeps the focus). The
 * reactor reacts to the live mic / TTS signal sent by the main window; the
 * glass panel shows what was heard, the reply typed out, yes / no for a
 * pending question; the microphone indicator stays visible while it is open
 * (§33 privacy). Collapses back into the orb when the command finishes.
 */
import type { PointerEvent as ReactPointerEvent } from "react";

import { useT } from "../../i18n";
import { JarvisVoice } from "../voice/jarvis/JarvisVoice";
import type { JarvisMode, JarvisSignal, JarvisState } from "../voice/jarvis/model";
import type { JarvisDebugOverride } from "../voice/jarvis/useJarvisState";
import type { OrbVoiceAction, OrbVoiceState } from "./channel";

export interface OrbVoicePopupProps {
  voice: OrbVoiceState;
  /** Visual state (derived by the host: wake bloom timing). */
  state: JarvisState;
  onAction: (action: OrbVoiceAction) => void;
  onOpenMain: () => void;
  size: number;
  fps: number;
  motion: boolean;
  signal: JarvisSignal | null;
  themeKey: string;
  accent: string | null;
  /** The card opens to this side of the orb. */
  side: "left" | "right";
  leaving: boolean;
  onReactorPointerDown?: (e: ReactPointerEvent) => void;
  /** Debug override (screenshots). */
  forced?: JarvisDebugOverride | null;
}

const MODES: readonly JarvisMode[] = ["command", "chat", "cursor"];

export function OrbVoicePopup(p: OrbVoicePopupProps) {
  const { t } = useT("voice");
  const v = p.voice;
  const f = p.forced ?? null;
  const phase = f?.phase ?? v.phase;
  const listening = phase === "listening";
  const transcript = v.transcript ?? "";
  const speaking = f?.speaking ?? v.speaking;
  const sttEngine = v.sttEngine ?? null;
  const ttsEngine = v.ttsEngine ?? null;
  const engine =
    f?.engine ??
    (speaking && ttsEngine ? t(`settings.tts.engines.${ttsEngine}` as "settings.tts.engines.System") : sttEngine && sttEngine !== "Disabled" ? t(`settings.stt.engines.${sttEngine}` as "settings.stt.engines.Vosk") : null);
  const mode = (f?.mode ?? (MODES.includes(v.mode as JarvisMode) ? (v.mode as JarvisMode) : "command")) satisfies JarvisMode;
  return (
    <JarvisVoice
      state={f?.state ?? p.state}
      level={f?.level ?? v.level}
      partialText={f ? (f.partialText ?? "") : listening ? transcript : ""}
      finalText={f ? (f.finalText ?? "") : listening ? "" : transcript}
      replyText={f ? (f.replyText ?? "") : (v.reply?.text ?? "")}
      replyTone={v.reply?.tone ?? "info"}
      confirmText={f ? (f.confirmText ?? null) : (v.confirmText ?? null)}
      mode={mode}
      phase={phase}
      engine={engine}
      micOpen={f?.micOpen ?? !!v.micOpen}
      speaking={speaking}
      size={p.size}
      fps={p.fps}
      motion={p.motion}
      signal={f ? null : p.signal}
      themeKey={p.themeKey}
      accent={p.accent}
      reactorSide={p.side === "right" ? "start" : "end"}
      variant="card"
      showReactor={false}
      leaving={p.leaving}
      onReactorPointerDown={p.onReactorPointerDown}
      onCancel={() => p.onAction("cancel")}
      onRepeat={() => p.onAction("repeat")}
      onConfirm={(yes) => p.onAction(yes ? "yes" : "no")}
      onOpen={p.onOpenMain}
    />
  );
}
