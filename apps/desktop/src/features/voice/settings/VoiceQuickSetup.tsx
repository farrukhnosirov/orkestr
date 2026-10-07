/**
 * Voice in the setup wizard (§55 step 9): the essentials of Settings → Voice
 * — turn voice commands on, pick the push-to-talk keys, check the
 * microphone. Everything else stays in Settings → Voice.
 */
import type { VoiceConfig } from "@orkestr/protocol";
import { Button, Spinner } from "@orkestr/ui";
import { useEffect } from "react";

import { useT } from "../../../i18n";
import { SettingGroup, SettingRow, Switch } from "../../settings/controls";
import { useVoiceStore } from "../store";
import { Select } from "./fields";
import { PTT_HOTKEYS } from "./options";
import { MicrophoneRows } from "./VoiceSection";
import "../voice.css";

export function VoiceQuickSetup() {
  const { t } = useT(["voice", "common", "settings"]);
  const { config, loading, error, saving, load, patch } = useVoiceStore();

  useEffect(() => {
    if (!config && !loading) void load();
  }, [config, loading, load]);

  if (!config) {
    if (loading) return <div className="set-loading"><Spinner size={16} /> {t("common:state.loading")}</div>;
    return (
      <div className="set-alert" role="alert">
        {error ?? t("voice:settings.unavailable")}{" "}
        <Button size="sm" variant="ghost" onClick={() => void load()}>
          {t("common:actions.retry")}
        </Button>
      </div>
    );
  }

  const up = (fn: (c: VoiceConfig) => VoiceConfig) => void patch(fn);
  return (
    <div className="vx-settings">
      {error && <div className="set-alert">{error}</div>}
      <SettingGroup title={t("voice:settings.groups.commands")}>
        <SettingRow title={t("voice:settings.general.enabled")} description={t("voice:settings.general.enabledDesc")}>
          <Switch label={t("voice:settings.general.enabled")} checked={config.enabled} busy={saving} onChange={(v) => up((c) => ({ ...c, enabled: v }))} />
        </SettingRow>
        <SettingRow title={t("voice:settings.general.hotkey")} description={t("voice:settings.general.hotkeyDesc")}>
          <Select
            label={t("voice:settings.general.hotkey")}
            value={config.pushToTalkHotkey}
            options={(PTT_HOTKEYS.includes(config.pushToTalkHotkey) ? PTT_HOTKEYS : [config.pushToTalkHotkey, ...PTT_HOTKEYS]).map((h) => ({ value: h, label: h }))}
            onChange={(v) => up((c) => ({ ...c, pushToTalkHotkey: v }))}
          />
        </SettingRow>
      </SettingGroup>
      <SettingGroup title={t("voice:settings.groups.microphone")}>
        <MicrophoneRows config={config} patch={up} />
      </SettingGroup>
    </div>
  );
}
