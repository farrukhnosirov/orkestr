/**
 * Settings → Interface → «Запуск» (ADR-0053): startup animation, startup
 * sound and the spoken greeting. Device preferences (local storage): the
 * boot sequence reads them before the core is connected.
 */
import { Button } from "@orkestr/ui";
import { Play } from "lucide-react";

import { useT } from "../../i18n";
import { Segmented, SettingGroup, SettingRow, Switch } from "../settings/controls";
import type { BootWhen } from "./prefs";
import { useBootStore } from "./store";

const WHEN: readonly BootWhen[] = ["always", "firstLaunch", "off"];

export function BootSettings({ reduced }: { reduced: boolean }) {
  const { t } = useT("boot");
  const prefs = useBootStore((s) => s.prefs);
  const setPrefs = useBootStore((s) => s.setPrefs);
  const preview = useBootStore((s) => s.preview);
  return (
    <SettingGroup title={t("settings.group")}>
      <SettingRow title={t("settings.animation")} description={t("settings.animationDesc")} stacked>
        <Segmented
          label={t("settings.animation")}
          value={prefs.animation}
          options={WHEN.map((w) => ({ value: w, label: t(`settings.when.${w}`) }))}
          onChange={(animation) => setPrefs({ animation })}
        />
      </SettingRow>
      {reduced && <p className="perf-note perf-note--muted">{t("settings.reducedNote")}</p>}
      <SettingRow title={t("settings.sound")} description={t("settings.soundDesc")} stacked>
        <Segmented
          label={t("settings.sound")}
          value={prefs.sound}
          options={WHEN.map((w) => ({ value: w, label: t(`settings.whenSound.${w}`) }))}
          onChange={(sound) => setPrefs({ sound })}
        />
      </SettingRow>
      <SettingRow title={t("settings.voice")} description={t("settings.voiceDesc")}>
        <Switch label={t("settings.voice")} checked={prefs.voice} onChange={(voice) => setPrefs({ voice })} />
      </SettingRow>
      <div className="boot-settings__preview">
        <Button size="sm" leftIcon={<Play size={13} />} disabled={preview !== null} onClick={() => useBootStore.getState().setPreview("full")}>
          {t("settings.preview")}
        </Button>
      </div>
    </SettingGroup>
  );
}
