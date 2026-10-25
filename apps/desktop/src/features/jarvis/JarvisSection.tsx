/**
 * Settings → Jarvis (ADR-0049): persona on/off, form of address, the user's
 * name, greeting on start, instant template replies, the conversation model,
 * memory length, project context and «forget the conversation».
 */
import type { JarvisAddress, JarvisConfig } from "@orkestr/protocol";
import { Button, Input } from "@orkestr/ui";
import { Eraser } from "lucide-react";
import { useEffect, useState } from "react";

import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { ModelPicker } from "../providers/ModelPicker";
import { loadProviders, useProvidersStore } from "../providers/store";
import { Segmented, SettingGroup, SettingRow, Switch } from "../settings/controls";
import type { SettingsSectionProps } from "../settings/sections";
import { forgetConversation, loadJarvisConfig, saveJarvisConfig } from "./api";
import { useJarvisStore } from "./store";
import "./jarvis.css";

const ADDRESSES: readonly JarvisAddress[] = ["Sir", "Boss", "Name", "None"];

export function JarvisSection(_props: SettingsSectionProps) {
  const { t } = useT("jarvis");
  const config = useJarvisStore((s) => s.config);
  const bank = useJarvisStore((s) => s.bank);
  const providers = useProvidersStore((s) => s.providers);
  const providersLoaded = useProvidersStore((s) => s.loaded);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const [name, setName] = useState("");
  const [turns, setTurns] = useState("");

  useEffect(() => {
    if (!config) void loadJarvisConfig();
    if (!providersLoaded) void loadProviders();
  }, [config, providersLoaded]);
  useEffect(() => {
    if (config) {
      setName(config.userName);
      setTurns(String(config.memoryTurns));
    }
  }, [config]);

  if (!config) return <p className="jv-set__note">{t("chat.thinking")}</p>;

  const save = async (patch: Partial<JarvisConfig>) => {
    const next: JarvisConfig = { ...config, ...patch };
    if (next.address === "Name" && !next.userName.trim()) return; // waits for the name
    setSaving(true);
    setMsg(null);
    try {
      await saveJarvisConfig(next);
      setMsg({ ok: true, text: t("settings.saved") });
    } catch (e) {
      setMsg({ ok: false, text: describeError(e) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="jv-set">
      <p className="jv-set__intro">{t("settings.intro")}</p>
      <SettingGroup title={t("settings.persona")}>
        <SettingRow title={t("settings.persona")} description={t("settings.personaHint")}>
          <Switch label={t("settings.persona")} checked={config.persona} busy={saving} onChange={(persona) => void save({ persona })} />
        </SettingRow>
        <SettingRow title={t("settings.address")} stacked>
          <Segmented<JarvisAddress>
            label={t("settings.address")}
            value={config.address}
            disabled={!config.persona}
            options={ADDRESSES.map((a) => ({ value: a, label: t(`settings.addressOptions.${a}`) }))}
            onChange={(address) => void save({ address })}
          />
        </SettingRow>
        <SettingRow title={t("settings.userName")}>
          <Input
            aria-label={t("settings.userName")}
            value={name}
            maxLength={40}
            placeholder={t("settings.userNamePlaceholder")}
            onChange={(e) => setName(e.target.value)}
            onBlur={() => {
              if (name.trim() !== config.userName) void save({ userName: name.trim() });
            }}
          />
        </SettingRow>
        <SettingRow title={t("settings.greetOnStart")} description={t("settings.greetHint")}>
          <Switch label={t("settings.greetOnStart")} checked={config.greetOnStart} busy={saving} onChange={(greetOnStart) => void save({ greetOnStart })} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("chat.title")}>
        <SettingRow
          title={t("settings.localTemplates")}
          description={
            <>
              {t("settings.localTemplatesHint")}
              {bank ? ` ${t("settings.bankStats", { intents: bank.intents, questions: bank.questions, answers: bank.answers })}` : ""}
            </>
          }
        >
          <Switch label={t("settings.localTemplates")} checked={config.localTemplates} busy={saving} onChange={(localTemplates) => void save({ localTemplates })} />
        </SettingRow>
        <SettingRow title={t("settings.model")} description={t("settings.modelHint")} stacked>
          <ModelPicker label={t("settings.model")} value={config.model ?? null} providers={providers} allowNone onChange={(model) => void save(model ? { model } : { model: undefined })} />
          {!config.model && <p className="jv-set__note">{t("settings.modelDefault")}</p>}
        </SettingRow>
        <SettingRow title={t("settings.projectContext")} description={t("settings.projectContextHint")}>
          <Switch label={t("settings.projectContext")} checked={config.projectContext} busy={saving} onChange={(projectContext) => void save({ projectContext })} />
        </SettingRow>
        <SettingRow title={t("settings.memoryTurns")} description={t("settings.memoryHint")}>
          <Input
            aria-label={t("settings.memoryTurns")}
            type="number"
            min={2}
            max={60}
            value={turns}
            onChange={(e) => setTurns(e.target.value)}
            onBlur={() => {
              const n = Math.round(Number(turns));
              if (Number.isFinite(n) && n >= 2 && n <= 60 && n !== config.memoryTurns) void save({ memoryTurns: n });
              else setTurns(String(config.memoryTurns));
            }}
          />
        </SettingRow>
        <SettingRow title={t("settings.forget")}>
          <Button
            variant="secondary"
            size="sm"
            leftIcon={<Eraser size={14} />}
            onClick={async () => {
              try {
                await forgetConversation();
                setMsg({ ok: true, text: t("settings.forgotten") });
              } catch (e) {
                setMsg({ ok: false, text: describeError(e) });
              }
            }}
          >
            {t("settings.forget")}
          </Button>
        </SettingRow>
      </SettingGroup>
      {msg && (
        <p className={msg.ok ? "jv-set__note" : "jv-set__note jv-set__note--error"} role="status">
          {msg.text}
        </p>
      )}
    </div>
  );
}
