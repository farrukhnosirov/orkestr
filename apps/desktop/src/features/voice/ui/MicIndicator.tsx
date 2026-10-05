/**
 * Title-bar voice button and microphone indicator (§33 privacy, ADR-0017).
 * Whenever the microphone is open — for any purpose — a red pulsing pill
 * names why ("Listening", "Wake word", …); it can never be hidden while
 * the mic is on. Click toggles the voice HUD.
 */
import { clsx } from "clsx";
import { Mic, MicOff } from "lucide-react";

import { useT } from "../../../i18n";
import { cancelVoice } from "../controller";
import { type MicPurpose, useVoiceStore } from "../store";
import "../voice.css";

const PRIORITY: readonly MicPurpose[] = ["command", "enroll", "meter", "wake"];

export function MicIndicator() {
  const { t } = useT("voice");
  const mic = useVoiceStore((s) => s.mic);
  const level = useVoiceStore((s) => s.level);
  const config = useVoiceStore((s) => s.config);
  const hudOpen = useVoiceStore((s) => s.hudOpen);
  const phase = useVoiceStore((s) => s.phase);
  const wakeStatus = useVoiceStore((s) => s.wakeStatus);
  const setHudOpen = useVoiceStore((s) => s.setHudOpen);

  if (!config) return null;
  const active = mic.length > 0;
  const top = PRIORITY.find((p) => mic.includes(p)) ?? null;
  const label = active && top ? t(`indicator.purposes.${top}`) : null;
  const title = active
    ? t("indicator.micOn", { purposes: mic.map((p) => t(`indicator.purposes.${p}`)).join(", ") })
    : config.enabled
      ? wakeStatus === "pausedBattery"
        ? t("indicator.wakePaused")
        : t("indicator.open")
      : t("indicator.disabled");

  const toggle = () => {
    if (hudOpen || phase !== "idle") {
      cancelVoice();
      setHudOpen(false);
    } else setHudOpen(true);
  };

  return (
    <button
      type="button"
      className={clsx("vx-ind", active && "vx-ind--on", top === "wake" && mic.length === 1 && "vx-ind--wake", !config.enabled && "vx-ind--off")}
      onClick={toggle}
      aria-label={title}
      title={title}
      aria-pressed={hudOpen}
      style={{ ["--vx-level" as string]: String(level) }}
    >
      <span className="vx-ind__icon">{config.enabled ? <Mic size={14} /> : <MicOff size={14} />}</span>
      {active && <span className="vx-ind__dot" aria-hidden="true" />}
      {label && <span className="vx-ind__label">{label}</span>}
    </button>
  );
}
