/**
 * Settings → Voice (§33): voice commands, microphone (device + level test),
 * speech recognition engines with explicit consent for anything that sends
 * audio off the device, the local wake word, conversation (barge-in and
 * follow-up without the wake word, ADR-0053), the assistant voice (TTS test)
 * and voice per agent.
 */
import type { SttEngine, TtsEngine, VoiceConfig } from "@orkestr/protocol";
import { Badge, Button, EmptyState, Spinner } from "@orkestr/ui";
import { AlertTriangle, CloudUpload, Mic, Play, ShieldAlert, ShieldCheck, Square } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { useFormat, useT } from "../../../i18n";
import { SettingGroup, SettingRow, Switch } from "../../settings/controls";
import type { SettingsSectionProps } from "../../settings/sections";
import { assistantOptions } from "../agentVoices";
import { acquireMic, micErrorText } from "../controller";
import { useVoiceStore } from "../store";
import { STT_ENGINES, TTS_ENGINES, sttConsented, sttNeedsConsent, ttsConsented, ttsIsCloud } from "../engineMeta";
import { webSpeechAvailable } from "../stt/engines";
import { optionsFromConfig, speak, speechLang, systemVoices, ttsSupported } from "../tts/speaker";
import { LevelMeter } from "../ui/LevelMeter";
import { AgentVoices } from "./AgentVoices";
import { ConversationSettings } from "./ConversationSettings";
import { EnginesPanel } from "./EnginesPanel";
import { ModelsPanel } from "./ModelsPanel";
import { Notice, Select, Slider } from "./fields";
import { CLOUD_VOICES, LANGUAGE_OPTIONS, PTT_HOTKEYS } from "./options";
import { WakeEnrollment } from "./WakeEnrollment";
import "../voice.css";

type Consent = { kind: "stt"; engine: SttEngine } | { kind: "tts"; engine: TtsEngine } | null;

export function VoiceSection(_props: SettingsSectionProps) {
  const { t, i18n } = useT(["voice", "common", "settings"]);
  const fmt = useFormat();
  const { config, cloudProviders, loading, error, saving, load, patch, setConsent } = useVoiceStore();
  const [consent, setConsentAsk] = useState<Consent>(null);

  useEffect(() => {
    if (!config && !loading) void load();
  }, [config, loading, load]);

  if (!config) {
    if (loading) return <div className="set-loading"><Spinner size={20} /> {t("common:state.loading")}</div>;
    return (
      <EmptyState
        icon={<AlertTriangle size={22} />}
        title={t("settings.unavailable")}
        description={error ?? undefined}
        action={<Button onClick={() => void load()}>{t("common:actions.retry")}</Button>}
      />
    );
  }

  const up = (fn: (c: VoiceConfig) => VoiceConfig) => void patch(fn);
  const webSpeech = webSpeechAvailable();

  const pickStt = (engine: SttEngine) => {
    if (engine === config.sttEngine) return;
    if (sttNeedsConsent(engine) && !sttConsented(config, engine)) {
      setConsentAsk({ kind: "stt", engine });
      return;
    }
    up((c) => ({ ...c, sttEngine: engine }));
  };
  const pickTts = (engine: TtsEngine) => {
    if (engine === config.ttsEngine) return;
    if (ttsIsCloud(engine) && !ttsConsented(config, engine)) {
      setConsentAsk({ kind: "tts", engine });
      return;
    }
    up((c) => ({ ...c, ttsEngine: engine }));
  };

  const providerOptions = [
    { value: "", label: t("settings.cloud.autoProvider") },
    ...cloudProviders.map((p) => ({ value: p.id, label: p.hasCredential ? p.name : t("settings.cloud.noKey", { name: p.name }) })),
  ];
  const usesCloud = config.sttEngine === "OpenAiWhisper" || config.ttsEngine === "OpenAi";

  return (
    <div className="set-section vx-settings">
      {error && <div className="set-alert">{error}</div>}

      <SettingGroup title={t("settings.groups.commands")}>
        <SettingRow title={t("settings.general.enabled")} description={t("settings.general.enabledDesc")}>
          <Switch label={t("settings.general.enabled")} checked={config.enabled} busy={saving} onChange={(v) => up((c) => ({ ...c, enabled: v }))} />
        </SettingRow>
        <SettingRow title={t("settings.general.hotkey")} description={t("settings.general.hotkeyDesc")}>
          <Select
            label={t("settings.general.hotkey")}
            value={config.pushToTalkHotkey}
            options={(PTT_HOTKEYS.includes(config.pushToTalkHotkey) ? PTT_HOTKEYS : [config.pushToTalkHotkey, ...PTT_HOTKEYS]).map((h) => ({ value: h, label: h }))}
            onChange={(v) => up((c) => ({ ...c, pushToTalkHotkey: v }))}
          />
        </SettingRow>
        <SettingRow title={t("settings.general.language")} description={t("settings.general.languageDesc")}>
          <Select
            label={t("settings.general.language")}
            value={config.language}
            options={LANGUAGE_OPTIONS.map((l) => ({ value: l, label: t(`settings.languages.${l}`) }))}
            onChange={(v) => up((c) => ({ ...c, language: v }))}
          />
        </SettingRow>
        <SettingRow title={t("settings.general.spokenReplies")} description={t("settings.general.spokenRepliesDesc")}>
          <Switch label={t("settings.general.spokenReplies")} checked={config.spokenReplies} onChange={(v) => up((c) => ({ ...c, spokenReplies: v }))} />
        </SettingRow>
        <SettingRow title={t("settings.general.naturalReplies")} description={t("settings.general.naturalRepliesDesc")}>
          <Switch label={t("settings.general.naturalReplies")} checked={config.naturalReplies} onChange={(v) => up((c) => ({ ...c, naturalReplies: v }))} />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("settings.groups.microphone")}>
        <MicrophoneRows config={config} patch={up} />
      </SettingGroup>

      <SettingGroup title={t("settings.groups.stt")}>
        <SettingRow title={t("settings.stt.engine")} description={t("settings.stt.engineDesc")} stacked>
          <Select
            label={t("settings.stt.engine")}
            value={config.sttEngine}
            options={STT_ENGINES.filter((e) => e !== "WebSpeech" || webSpeech || config.sttEngine === "WebSpeech").map((e) => ({ value: e, label: t(`settings.stt.engines.${e}`) }))}
            onChange={(v) => pickStt(v as SttEngine)}
          />
          <div className="set-muted set-muted--small">{t(`settings.stt.engineHints.${config.sttEngine}`)}</div>
        </SettingRow>
        {consent?.kind === "stt" && (
          <Notice
            tone="warning"
            icon={<ShieldAlert size={16} />}
            title={t("settings.consent.sttTitle", { engine: t(`settings.stt.engines.${consent.engine}`) })}
            actions={
              <>
                <Button variant="ghost" onClick={() => setConsentAsk(null)}>
                  {t("common:actions.cancel")}
                </Button>
                <Button
                  variant="primary"
                  onClick={() => {
                    const engine = consent.engine;
                    setConsentAsk(null);
                    void setConsent("SpeechRecognition", true, engine);
                  }}
                >
                  {t("settings.consent.allow")}
                </Button>
              </>
            }
          >
            {consent.engine === "WebSpeech" || consent.engine === "OpenAiWhisper" ? t(`settings.consent.sttText.${consent.engine}`) : t("settings.consent.sttText.Cloud", { engine: t(`settings.stt.engines.${consent.engine}`) })}
          </Notice>
        )}
        {config.sttConsent && (
          <SettingRow
            title={t("settings.consent.given")}
            description={t("settings.consent.givenDesc", {
              engine: t(`settings.stt.engines.${config.sttConsent.engine}`),
              date: fmt.date(config.sttConsent.grantedAt, "dateTime"),
            })}
          >
            <Button variant="ghost" onClick={() => void setConsent("SpeechRecognition", false)}>
              {t("settings.consent.revoke")}
            </Button>
          </SettingRow>
        )}
        {config.sttEngine === "Disabled" && <div className="set-muted set-muted--small">{t("settings.stt.disabledNote")}</div>}
      </SettingGroup>

      <SettingGroup title={t("settings.groups.wake")}>
        <WakeEnrollment config={config} patch={up} />
      </SettingGroup>

      <SettingGroup title={t("settings.groups.conversation")}>
        <ConversationSettings config={config} patch={up} />
      </SettingGroup>

      <SettingGroup title={t("settings.groups.tts")}>
        <SettingRow title={t("settings.tts.engine")} description={t("settings.tts.engineDesc")} stacked>
          <Select
            label={t("settings.tts.engine")}
            value={config.ttsEngine}
            options={TTS_ENGINES.filter((e) => e !== "System" || ttsSupported() || config.ttsEngine === "System").map((e) => ({ value: e, label: t(`settings.tts.engines.${e}`) }))}
            onChange={(v) => pickTts(v as TtsEngine)}
          />
        </SettingRow>
        {consent?.kind === "tts" && (
          <Notice
            tone="warning"
            icon={<CloudUpload size={16} />}
            title={consent.engine === "OpenAi" ? t("settings.consent.ttsTitle") : t("settings.consent.ttsTitleEngine", { engine: t(`settings.tts.engines.${consent.engine}`) })}
            actions={
              <>
                <Button variant="ghost" onClick={() => setConsentAsk(null)}>
                  {t("common:actions.cancel")}
                </Button>
                <Button
                  variant="primary"
                  onClick={() => {
                    const engine = consent.engine;
                    setConsentAsk(null);
                    void setConsent("CloudSpeech", true, undefined, { ttsEngine: engine });
                  }}
                >
                  {t("settings.consent.allow")}
                </Button>
              </>
            }
          >
            {consent.engine === "OpenAi" ? t("settings.consent.ttsText") : t("settings.consent.ttsTextEngine", { engine: t(`settings.tts.engines.${consent.engine}`) })}
          </Notice>
        )}
        {config.ttsConsentAt && (
          <SettingRow title={t("settings.consent.ttsGiven")} description={fmt.date(config.ttsConsentAt, "dateTime")}>
            <Button variant="ghost" onClick={() => void setConsent("CloudSpeech", false)}>
              {t("settings.consent.revoke")}
            </Button>
          </SettingRow>
        )}
        <AssistantVoiceRows config={config} patch={up} uiLanguage={i18n.language} />
      </SettingGroup>

      <SettingGroup title={t("settings.groups.engines")}>
        <EnginesPanel config={config} patch={up} />
      </SettingGroup>

      <SettingGroup title={t("settings.groups.models")}>
        <ModelsPanel />
      </SettingGroup>

      {usesCloud && (
        <SettingGroup title={t("settings.groups.cloud")}>
          <SettingRow title={t("settings.cloud.provider")} description={cloudProviders.length ? t("settings.cloud.providerDesc") : t("settings.cloud.none")}>
            <Select
              label={t("settings.cloud.provider")}
              value={config.cloudProviderId ?? ""}
              options={providerOptions}
              onChange={(v) => up((c) => ({ ...c, cloudProviderId: v || null }))}
            />
          </SettingRow>
        </SettingGroup>
      )}

      <SettingGroup title={t("settings.groups.agents")}>
        <div className="set-muted set-muted--small vx-gap">{t("settings.agents.desc")}</div>
        <AgentVoices config={config} patch={up} />
      </SettingGroup>

      <Notice tone="ok" icon={<ShieldCheck size={16} />} title={t("settings.privacy.title")}>
        {t("settings.privacy.text")}
      </Notice>
    </div>
  );
}

export function MicrophoneRows({ config, patch }: { config: VoiceConfig; patch: (fn: (c: VoiceConfig) => VoiceConfig) => void }) {
  const { t } = useT("voice");
  const level = useVoiceStore((s) => s.level);
  const [devices, setDevices] = useState<{ id: string; label: string }[]>([]);
  const [testing, setTesting] = useState(false);
  const [speaking, setSpeaking] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [deviceLabel, setDeviceLabel] = useState("");
  const stopRef = useRef<(() => void) | null>(null);

  const refresh = async () => {
    try {
      const { listInputDevices } = await import("../audio/mic");
      setDevices(await listInputDevices());
    } catch {
      setDevices([]);
    }
  };

  useEffect(() => {
    void refresh();
    return () => stopRef.current?.();
  }, []);

  const toggleTest = async () => {
    if (testing) {
      stopRef.current?.();
      stopRef.current = null;
      setTesting(false);
      return;
    }
    setErr(null);
    try {
      const { mic, release } = await acquireMic("meter");
      setDeviceLabel(mic.deviceLabel);
      const off = mic.subscribe((m) => {
        if (m.type === "level") setSpeaking(m.speaking);
      });
      stopRef.current = () => {
        off();
        release();
        setSpeaking(false);
      };
      setTesting(true);
      void refresh(); // labels appear after the first permission grant
    } catch (e) {
      setErr(micErrorText(e));
    }
  };

  const labelled = devices.some((d) => d.label);
  return (
    <>
      <SettingRow title={t("settings.mic.device")} description={labelled ? t("settings.mic.deviceDesc") : t("settings.mic.deviceNoLabels")}>
        <Select
          label={t("settings.mic.device")}
          value={config.inputDeviceId ?? ""}
          options={[
            { value: "", label: t("settings.mic.systemDefault") },
            ...devices.map((d, i) => ({ value: d.id, label: d.label || t("settings.mic.unnamed", { n: i + 1 }) })),
          ]}
          onChange={(v) => patch((c) => ({ ...c, inputDeviceId: v || null }))}
        />
      </SettingRow>
      <SettingRow title={t("settings.mic.test")} description={testing && deviceLabel ? t("settings.mic.testing", { device: deviceLabel }) : t("settings.mic.testDesc")} stacked>
        <div className="vx-mictest">
          <Button variant={testing ? "secondary" : "primary"} leftIcon={testing ? <Square size={13} /> : <Mic size={14} />} onClick={() => void toggleTest()}>
            {testing ? t("settings.mic.stopTest") : t("settings.mic.startTest")}
          </Button>
          <LevelMeter level={testing ? level : 0} active={speaking} label={t("settings.mic.level")} />
          {testing && <Badge tone={speaking ? "success" : "neutral"}>{speaking ? t("settings.mic.speech") : t("settings.mic.silence")}</Badge>}
        </div>
      </SettingRow>
      {err && <div className="set-alert">{err}</div>}
    </>
  );
}

function AssistantVoiceRows({
  config,
  patch,
  uiLanguage,
}: {
  config: VoiceConfig;
  patch: (fn: (c: VoiceConfig) => VoiceConfig) => void;
  uiLanguage: string;
}) {
  const { t } = useT("voice");
  const fmt = useFormat();
  const [voices, setVoices] = useState<SpeechSynthesisVoice[]>([]);
  useEffect(() => {
    let alive = true;
    void systemVoices().then((v) => alive && setVoices(v));
    return () => {
      alive = false;
    };
  }, []);
  const lang = speechLang(config.language, uiLanguage).split("-")[0] ?? "en";
  const matching = voices.filter((v) => v.lang.toLowerCase().startsWith(lang));
  const list = matching.length ? matching : voices;
  const test = () => {
    const base = assistantOptions() ?? optionsFromConfig(config, uiLanguage);
    void speak(t("settings.tts.sample"), base);
  };
  return (
    <>
      {config.ttsEngine === "System" ? (
        <SettingRow title={t("settings.tts.voice")} description={voices.length ? t("settings.tts.voiceDesc", { count: matching.length }) : t("settings.tts.noVoices")}>
          <Select
            label={t("settings.tts.voice")}
            value={config.systemVoice ?? ""}
            options={[{ value: "", label: t("settings.tts.autoVoice") }, ...list.map((v) => ({ value: v.name, label: `${v.name} (${v.lang})` }))]}
            onChange={(v) => patch((c) => ({ ...c, systemVoice: v || null }))}
          />
        </SettingRow>
      ) : config.ttsEngine === "OpenAi" ? (
        <SettingRow title={t("settings.tts.cloudVoice")} description={t("settings.tts.cloudVoiceDesc")}>
          <Select
            label={t("settings.tts.cloudVoice")}
            value={config.cloudVoice}
            options={CLOUD_VOICES.map((v) => ({ value: v, label: v }))}
            onChange={(v) => patch((c) => ({ ...c, cloudVoice: v }))}
          />
        </SettingRow>
      ) : (
        <div className="set-muted set-muted--small">{t("settings.tts.coreVoiceNote")}</div>
      )}
      <SettingRow title={t("settings.tts.rate")}>
        <Slider value={config.ratePct} min={50} max={200} step={5} label={t("settings.tts.rate")} format={(v) => fmt.percent(v / 100)} onCommit={(v) => patch((c) => ({ ...c, ratePct: v }))} />
      </SettingRow>
      <SettingRow title={t("settings.tts.pitch")}>
        <Slider value={config.pitchPct} min={50} max={200} step={5} label={t("settings.tts.pitch")} format={(v) => fmt.percent(v / 100)} onCommit={(v) => patch((c) => ({ ...c, pitchPct: v }))} />
      </SettingRow>
      <SettingRow title={t("settings.tts.test")} description={t("settings.tts.testDesc")}>
        <Button variant="secondary" leftIcon={<Play size={14} />} onClick={test}>
          {t("settings.tts.testButton")}
        </Button>
      </SettingRow>
    </>
  );
}
