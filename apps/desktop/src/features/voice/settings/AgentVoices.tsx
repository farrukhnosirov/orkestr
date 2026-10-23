/**
 * Voice per agent (§30, §33): pick engine, voice, language, rate, pitch and
 * style for each agent template. Saved in the voice config (`agentVoices`)
 * because built-in templates are read-only; robots and spoken notifications
 * use it via `speakAsAgent`.
 */
import type { AgentTemplate, VoiceConfig, VoiceProfile } from "@orkestr/protocol";
import { Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { Bot, ChevronDown, ChevronRight, Play, RotateCcw } from "lucide-react";
import { useEffect, useState } from "react";

import { t as tGlobal, useFormat, useT } from "../../../i18n";
import { templateName } from "../../../lib/coreText";
import { loadTemplates } from "../agentVoices";
import { assistantOptions } from "../agentVoices";
import { optionsForProfile, speak, systemVoices } from "../tts/speaker";
import { CLOUD_VOICES, PIPER_VOICES, LANGUAGE_OPTIONS } from "./options";
import { engineFromProvider } from "../engineMeta";
import { Select, Slider } from "./fields";

const DEFAULT_PROFILE: VoiceProfile = { provider: "system", voice: "", language: "auto", style: null };

function profileSummary(p: VoiceProfile | null): string {
  if (!p) return tGlobal("voice:settings.agents.assistantVoice");
  const engine = tGlobal(`voice:settings.tts.engines.${engineFromProvider(p.provider) ?? "System"}`);
  return p.voice ? `${engine} · ${p.voice}` : engine;
}

export function AgentVoices({ config, patch }: { config: VoiceConfig; patch: (fn: (c: VoiceConfig) => VoiceConfig) => void }) {
  const { t } = useT(["voice", "common"]);
  const fmt = useFormat();
  const [templates, setTemplates] = useState<AgentTemplate[] | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [voices, setVoices] = useState<SpeechSynthesisVoice[]>([]);

  useEffect(() => {
    let alive = true;
    void loadTemplates().then((m) => alive && setTemplates([...m.values()].sort((a, b) => a.name.localeCompare(b.name))));
    void systemVoices().then((v) => alive && setVoices(v));
    return () => {
      alive = false;
    };
  }, []);

  if (!templates) return <div className="set-loading"><Spinner size={16} /> {t("common:state.loading")}</div>;
  if (templates.length === 0) return <EmptyState icon={<Bot size={20} />} title={t("settings.agents.empty")} />;

  const cloudAllowed = config.ttsEngine === "OpenAi";
  const setProfile = (id: string, p: VoiceProfile | null) =>
    patch((c) => {
      const next = { ...c.agentVoices };
      if (p) next[id] = p;
      else delete next[id];
      return { ...c, agentVoices: next };
    });

  return (
    <div className="vx-agents">
      {templates.map((tpl) => {
        const override = config.agentVoices[tpl.id] ?? null;
        const effective = override ?? tpl.voice;
        const expanded = open === tpl.id;
        const p = effective ?? DEFAULT_PROFILE;
        const edit = (fn: (x: VoiceProfile) => VoiceProfile) => setProfile(tpl.id, fn({ ...p }));
        const name = templateName(tpl.name);
        const test = () => {
          const base = assistantOptions();
          if (base) void speak(t("settings.agents.sample", { name }), optionsForProfile(effective, base));
        };
        const voiceOptions =
          p.provider === "openai"
            ? CLOUD_VOICES.map((v) => ({ value: v, label: v }))
            : p.provider === "piper"
              ? PIPER_VOICES.map((v) => ({ value: v, label: v }))
            : [{ value: "", label: t("settings.tts.autoVoice") }, ...voices.map((v) => ({ value: v.name, label: `${v.name} (${v.lang})` }))];
        return (
          <div key={tpl.id} className="vx-agent">
            <div className="vx-agent__row">
              <button type="button" className="vx-agent__toggle" aria-expanded={expanded} onClick={() => setOpen(expanded ? null : tpl.id)}>
                {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                <Bot size={14} />
                <span className="vx-agent__name">{name}</span>
                <span className="vx-agent__summary">{profileSummary(effective)}</span>
              </button>
              <IconButton label={t("settings.agents.test", { name })} icon={<Play size={14} />} onClick={test} tooltip="left" />
            </div>
            {expanded && (
              <div className="vx-agent__body">
                <label className="vx-field">
                  <span>{t("settings.agents.engine")}</span>
                  <Select
                    label={t("settings.agents.engine")}
                    value={p.provider}
                    options={[
                      { value: "system", label: t("settings.tts.engines.System") },
                      { value: "piper", label: t("settings.tts.engines.Piper") },
                      { value: "windows", label: t("settings.tts.engines.WindowsOneCore") },
                      { value: "openai", label: t("settings.tts.engines.OpenAi"), disabled: !cloudAllowed },
                    ]}
                    onChange={(v) => edit((x) => ({ ...x, provider: v, voice: v === "openai" ? (CLOUD_VOICES[0] ?? "alloy") : v === "piper" ? (PIPER_VOICES[0] ?? "") : "" }))}
                  />
                </label>
                <label className="vx-field">
                  <span>{t("settings.agents.voice")}</span>
                  <Select label={t("settings.agents.voice")} value={p.voice} options={voiceOptions} onChange={(v) => edit((x) => ({ ...x, voice: v }))} />
                </label>
                <label className="vx-field">
                  <span>{t("settings.general.language")}</span>
                  <Select
                    label={t("settings.general.language")}
                    value={p.language}
                    options={LANGUAGE_OPTIONS.map((l) => ({ value: l, label: t(`settings.languages.${l}`) }))}
                    onChange={(v) => edit((x) => ({ ...x, language: v }))}
                  />
                </label>
                <label className="vx-field">
                  <span>{t("settings.tts.rate")}</span>
                  <Slider value={p.ratePct ?? 100} min={50} max={200} step={5} label={t("settings.tts.rate")} format={(v) => fmt.percent(v / 100)} onCommit={(v) => edit((x) => ({ ...x, ratePct: v }))} />
                </label>
                <label className="vx-field">
                  <span>{t("settings.tts.pitch")}</span>
                  <Slider value={p.pitchPct ?? 100} min={50} max={200} step={5} label={t("settings.tts.pitch")} format={(v) => fmt.percent(v / 100)} onCommit={(v) => edit((x) => ({ ...x, pitchPct: v }))} />
                </label>
                <label className="vx-field">
                  <span>{t("settings.agents.style")}</span>
                  <StyleInput value={p.style ?? ""} disabled={p.provider !== "openai"} onCommit={(v) => edit((x) => ({ ...x, style: v || null }))} />
                </label>
                <div className="vx-agent__actions">
                  <Button variant="ghost" size="sm" leftIcon={<RotateCcw size={13} />} disabled={!override} onClick={() => setProfile(tpl.id, null)}>
                    {t("settings.agents.reset")}
                  </Button>
                  <Button variant="secondary" size="sm" leftIcon={<Play size={13} />} onClick={test}>
                    {t("settings.tts.test")}
                  </Button>
                </div>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function StyleInput({ value, disabled, onCommit }: { value: string; disabled: boolean; onCommit: (v: string) => void }) {
  const { t } = useT("voice");
  const [v, setV] = useState(value);
  useEffect(() => setV(value), [value]);
  return (
    <Input
      value={v}
      disabled={disabled}
      maxLength={200}
      placeholder={t("settings.agents.stylePlaceholder")}
      aria-label={t("settings.agents.style")}
      onChange={(e) => setV(e.target.value)}
      onBlur={() => v !== value && onCommit(v.trim())}
    />
  );
}
