/**
 * Settings → Voice → Conversation (ADR-0054): voice barge-in (stop phrases
 * while the assistant speaks or works, what «окей» does, sensitivity) and
 * continuous conversation (follow-up without the wake word, window length,
 * turns in a row — ADR-0056).
 */
import type { VoiceConfig } from "@orkestr/protocol";
import { useEffect, useState } from "react";

import { useT } from "../../../i18n";
import { SettingRow, Switch } from "../../settings/controls";
import { Select, Slider } from "./fields";

/** Phrases typed one per line or comma-separated → the stored list (trimmed, unique, ≤ 64 × 40 chars). */
export function parsePhrases(text: string): string[] {
  const out: string[] = [];
  for (const raw of text.split(/[\n,;]+/)) {
    const p = raw.trim().replace(/\s+/g, " ").slice(0, 40);
    if (p && !out.some((x) => x.toLowerCase() === p.toLowerCase())) out.push(p);
  }
  return out.slice(0, 64);
}

function PhraseList({ label, value, onCommit }: { label: string; value: readonly string[]; onCommit: (v: string[]) => void }) {
  const [text, setText] = useState(value.join(", "));
  useEffect(() => setText(value.join(", ")), [value]);
  return (
    <textarea
      className="vx-phrases"
      aria-label={label}
      value={text}
      rows={3}
      onChange={(e) => setText(e.target.value)}
      onBlur={() => {
        const next = parsePhrases(text);
        if (next.join("\n") !== value.join("\n")) onCommit(next);
        else setText(value.join(", "));
      }}
    />
  );
}

export function ConversationSettings({ config, patch }: { config: VoiceConfig; patch: (fn: (c: VoiceConfig) => VoiceConfig) => void }) {
  const { t } = useT("voice");
  const b = config.barge;
  const f = config.followUp;
  const setBarge = (p: Partial<VoiceConfig["barge"]>) => patch((c) => ({ ...c, barge: { ...c.barge, ...p } }));
  const setFollow = (p: Partial<VoiceConfig["followUp"]>) => patch((c) => ({ ...c, followUp: { ...c.followUp, ...p } }));
  return (
    <>
      <SettingRow title={t("settings.barge.enabled")} description={t("settings.barge.enabledDesc")}>
        <Switch label={t("settings.barge.enabled")} checked={b.enabled} onChange={(v) => setBarge({ enabled: v })} />
      </SettingRow>
      {b.enabled && (
        <>
          <SettingRow title={t("settings.barge.hard")} description={t("settings.barge.hardDesc")} stacked>
            <PhraseList label={t("settings.barge.hard")} value={b.hardPhrases} onCommit={(v) => setBarge({ hardPhrases: v })} />
          </SettingRow>
          <SettingRow title={t("settings.barge.soft")} description={t("settings.barge.softDesc")} stacked>
            <PhraseList label={t("settings.barge.soft")} value={b.softPhrases} onCommit={(v) => setBarge({ softPhrases: v })} />
          </SettingRow>
          <SettingRow title={t("settings.barge.softMode")} description={t("settings.barge.softModeDesc")}>
            <Select
              label={t("settings.barge.softMode")}
              value={b.softCancelsActions ? "cancel" : "silence"}
              options={[
                { value: "silence", label: t("settings.barge.softModes.silence") },
                { value: "cancel", label: t("settings.barge.softModes.cancel") },
              ]}
              onChange={(v) => setBarge({ softCancelsActions: v === "cancel" })}
            />
          </SettingRow>
          <SettingRow title={t("settings.barge.sensitivity")} description={t("settings.barge.sensitivityDesc")}>
            <Slider value={b.sensitivity} min={0} max={100} step={5} label={t("settings.barge.sensitivity")} onCommit={(v) => setBarge({ sensitivity: v })} />
          </SettingRow>
        </>
      )}
      <SettingRow title={t("settings.followUp.enabled")} description={t("settings.followUp.enabledDesc")}>
        <Switch label={t("settings.followUp.enabled")} checked={f.enabled} onChange={(v) => setFollow({ enabled: v })} />
      </SettingRow>
      {f.enabled && (
        <>
          <SettingRow title={t("settings.followUp.window")} description={t("settings.followUp.windowDesc")}>
            <Slider value={f.windowSecs} min={3} max={10} step={1} label={t("settings.followUp.window")} format={(v) => t("settings.followUp.seconds", { count: v })} onCommit={(v) => setFollow({ windowSecs: v })} />
          </SettingRow>
          <SettingRow title={t("settings.followUp.maxTurns")} description={t("settings.followUp.maxTurnsDesc")}>
            <Slider value={f.maxTurns} min={1} max={10} step={1} label={t("settings.followUp.maxTurns")} onCommit={(v) => setFollow({ maxTurns: v })} />
          </SettingRow>
          <SettingRow title={t("settings.followUp.chime")} description={t("settings.followUp.chimeDesc")}>
            <Switch label={t("settings.followUp.chime")} checked={f.chimeOnClose} onChange={(v) => setFollow({ chimeOnClose: v })} />
          </SettingRow>
        </>
      )}
    </>
  );
}
