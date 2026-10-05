/**
 * The HUD's voice slot (§34): the main window's microphone state (via
 * `voiceBridge`), red while the mic is open; a click toggles the voice HUD
 * in the main window.
 */
import { clsx } from "clsx";
import { Mic, MicOff } from "lucide-react";

import { useT } from "../../../i18n";
import type { HudVoiceSlotProps } from "../../hud/hudData";
import { useRemoteVoice } from "./voiceBridge";
import "../voice.css";

export function HudVoiceIndicator({ size }: HudVoiceSlotProps) {
  const { t } = useT("voice");
  const { state, toggle } = useRemoteVoice();
  const enabled = state?.enabled ?? false;
  const active = (state?.mic.length ?? 0) > 0;
  const title = active
    ? t("indicator.micOn", { purposes: (state?.mic ?? []).map((p) => t(`indicator.purposes.${p}`)).join(", ") })
    : enabled
      ? t("indicator.open")
      : t("indicator.disabled");
  const icon = Math.max(12, size - 4);
  return (
    <button
      type="button"
      className={clsx("vx-ind", active && "vx-ind--on", !enabled && "vx-ind--off")}
      onClick={toggle}
      aria-label={title}
      title={title}
      style={{ ["--vx-level" as string]: String(state?.level ?? 0) }}
    >
      <span className="vx-ind__icon">{enabled ? <Mic size={icon} /> : <MicOff size={icon} />}</span>
      {active && <span className="vx-ind__dot" aria-hidden="true" />}
    </button>
  );
}
