/**
 * Settings → Voice → Speech engines (ADR-0046): per-language defaults,
 * fallback chains, engine cards (capabilities, readiness, verification),
 * engine options, API keys for paid engines (stored in the secret vault —
 * write-only from the UI) and test buttons (record & transcribe / speak a
 * sample with exactly that engine).
 */
import type { SttEngine, TtsEngine, VoiceConfig, VoiceEngineInfo, VoiceEngineSettings, VoiceEnginesResult, VoiceKeyId } from "@orkestr/protocol";
import { Badge, Button, Spinner } from "@orkestr/ui";
import { ArrowUp, KeyRound, Mic, Play, RefreshCw, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { maybeClient } from "../../../core/client";
import { describeError } from "../../../i18n/errors";
import { useT } from "../../../i18n";
import { SettingRow, Switch } from "../../settings/controls";
import { concatPcm, encodeWav, fromBase64, toBase64 } from "../audio/wav";
import { acquireMic } from "../controller";
import { STT_ENGINES, TTS_ENGINES, sttConsented, ttsConsented } from "../engineMeta";
import { useVoiceStore } from "../store";
import { SAMPLE_RATE } from "../wake/mfcc";
import { Select } from "./fields";

type Patch = (fn: (c: VoiceConfig) => VoiceConfig) => void;
const LANGS = ["ru", "en"] as const;
const RECORD_MS = 4000;

const STATUS_TONE = { Ready: "success", NeedsDownload: "warning", NeedsKey: "warning", Unavailable: "danger", Client: "info" } as const;

/** Which `VoiceEngineSettings` field an engine's option list edits. */
function optionField(e: VoiceEngineInfo): keyof VoiceEngineSettings | null {
  if (e.stt) {
    const m: Partial<Record<SttEngine, keyof VoiceEngineSettings>> = {
      WhisperCpp: "whisperModel",
      Vosk: "voskModel",
      SherpaOnnx: "sherpaModel",
      OpenAiWhisper: "openaiSttModel",
      Deepgram: "deepgramModel",
    };
    return m[e.stt] ?? null;
  }
  if (e.tts) {
    const m: Partial<Record<TtsEngine, keyof VoiceEngineSettings>> = {
      Piper: "piperVoice",
      WindowsOneCore: "windowsVoice",
      Silero: "sileroSpeaker",
      AzureSpeech: "azureVoice",
      GoogleCloud: "googleVoice",
      YandexSpeechKit: "yandexVoice",
    };
    return m[e.tts] ?? null;
  }
  return null;
}

export function useVoiceEngines(): { data: VoiceEnginesResult | null; error: string | null; reload: (refresh?: boolean) => Promise<void>; setData: (d: VoiceEnginesResult) => void } {
  const [data, setData] = useState<VoiceEnginesResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reload = useCallback(async (refresh = false) => {
    const client = maybeClient();
    if (!client) return;
    try {
      setData(await client.call("voice.listEngines", refresh ? { refresh: true } : {}, { timeoutMs: 60_000 }));
      setError(null);
    } catch (e) {
      setError(describeError(e));
    }
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);
  return { data, error, reload, setData };
}

function ChainEditor<E extends string>({ label, chain, all, labelOf, onChange }: { label: string; chain: readonly E[]; all: readonly E[]; labelOf: (e: E) => string; onChange: (next: E[]) => void }) {
  const { t } = useT("voice");
  const rest = all.filter((e) => !chain.includes(e));
  return (
    <div className="vx-chain" aria-label={label}>
      {chain.length === 0 && <span className="set-muted set-muted--small">{t("settings.engines.noFallback")}</span>}
      {chain.map((e, i) => (
        <span key={e} className="vx-chain__item">
          <span>{labelOf(e)}</span>
          {i > 0 && (
            <Button size="sm" variant="ghost" aria-label={t("settings.engines.moveUp")} onClick={() => onChange([...chain.slice(0, i - 1), e, chain[i - 1] as E, ...chain.slice(i + 1)])}>
              <ArrowUp size={12} />
            </Button>
          )}
          <Button size="sm" variant="ghost" aria-label={t("settings.engines.remove")} onClick={() => onChange(chain.filter((x) => x !== e))}>
            <X size={12} />
          </Button>
        </span>
      ))}
      {rest.length > 0 && (
        <Select label={t("settings.engines.addFallback")} value="" options={[{ value: "", label: t("settings.engines.addFallback") }, ...rest.map((e) => ({ value: e, label: labelOf(e) }))]} onChange={(v) => v && onChange([...chain, v as E])} />
      )}
    </div>
  );
}

export function EnginesPanel({ config, patch }: { config: VoiceConfig; patch: Patch }) {
  const { t } = useT(["voice", "common"]);
  const { data, error, reload, setData } = useVoiceEngines();
  const setConsent = useVoiceStore((s) => s.setConsent);
  const sttLabel = (e: SttEngine) => t(`settings.stt.engines.${e}`);
  const ttsLabel = (e: TtsEngine) => t(`settings.tts.engines.${e}`);

  // Chains may only contain consented engines; ask for consent on the way in.
  const addStt = async (next: SttEngine[]) => {
    for (const e of next) if (!sttConsented(config, e) && !(await setConsent("SpeechRecognition", true, e, { keepSelection: true }))) return;
    patch((c) => ({ ...c, sttFallback: next }));
  };
  const addTts = async (next: TtsEngine[]) => {
    for (const e of next) if (!ttsConsented(config, e) && !(await setConsent("CloudSpeech", true, undefined, { ttsEngine: e, keepSelection: true }))) return;
    patch((c) => ({ ...c, ttsFallback: next }));
  };
  const setLangStt = async (lang: string, v: string) => {
    const e = v as SttEngine;
    if (v && !sttConsented(config, e) && !(await setConsent("SpeechRecognition", true, e, { keepSelection: true }))) return;
    patch((c) => {
      const m = { ...c.sttByLanguage };
      if (v) m[lang] = e;
      else delete m[lang];
      return { ...c, sttByLanguage: m };
    });
  };
  const setLangTts = async (lang: string, v: string) => {
    const e = v as TtsEngine;
    if (v && !ttsConsented(config, e) && !(await setConsent("CloudSpeech", true, undefined, { ttsEngine: e, keepSelection: true }))) return;
    patch((c) => {
      const m = { ...c.ttsByLanguage };
      if (v) m[lang] = e;
      else delete m[lang];
      return { ...c, ttsByLanguage: m };
    });
  };

  return (
    <div className="vx-engines">
      <SettingRow title={t("settings.engines.byLanguage")} description={t("settings.engines.byLanguageDesc")} stacked>
        <div className="vx-engines__grid">
          {LANGS.map((lang) => (
            <div key={lang} className="vx-engines__lang">
              <span className="vx-engines__lang-name">{t(`settings.engines.lang.${lang}`)}</span>
              <Select
                label={t("settings.engines.sttFor", { lang: t(`settings.engines.lang.${lang}`) })}
                value={config.sttByLanguage?.[lang] ?? ""}
                options={[{ value: "", label: t("settings.engines.sameAsSelected") }, ...STT_ENGINES.filter((e) => e !== "Disabled" && e !== "WebSpeech").map((e) => ({ value: e, label: sttLabel(e) }))]}
                onChange={(v) => void setLangStt(lang, v)}
              />
              <Select
                label={t("settings.engines.ttsFor", { lang: t(`settings.engines.lang.${lang}`) })}
                value={config.ttsByLanguage?.[lang] ?? ""}
                options={[{ value: "", label: t("settings.engines.sameAsSelected") }, ...TTS_ENGINES.filter((e) => e !== "System").map((e) => ({ value: e, label: ttsLabel(e) }))]}
                onChange={(v) => void setLangTts(lang, v)}
              />
            </div>
          ))}
        </div>
      </SettingRow>
      <SettingRow title={t("settings.engines.fallbackStt")} description={t("settings.engines.fallbackDesc")} stacked>
        <ChainEditor chain={config.sttFallback ?? []} all={STT_ENGINES.filter((e) => e !== "Disabled" && e !== "WebSpeech")} labelOf={sttLabel} label={t("settings.engines.fallbackStt")} onChange={(n) => void addStt(n)} />
      </SettingRow>
      <SettingRow title={t("settings.engines.fallbackTts")} description={t("settings.engines.fallbackTtsDesc")} stacked>
        <ChainEditor chain={config.ttsFallback ?? []} all={TTS_ENGINES.filter((e) => e !== "System")} labelOf={ttsLabel} label={t("settings.engines.fallbackTts")} onChange={(n) => void addTts(n)} />
      </SettingRow>

      <div className="vx-engines__head">
        <span className="set-muted set-muted--small">{t("settings.engines.cardsDesc")}</span>
        <Button size="sm" variant="ghost" onClick={() => void reload(true)}>
          <RefreshCw size={14} /> {t("settings.engines.refresh")}
        </Button>
      </div>
      {error && <div className="set-alert">{error}</div>}
      {!data && !error && <Spinner size={16} />}
      {data && (
        <div className="vx-engines__cards">
          {[...data.stt, ...data.tts].map((e) => (
            <EngineCard key={`${e.stt ?? ""}${e.tts ?? ""}`} engine={e} config={config} patch={patch} onKeys={setData} />
          ))}
        </div>
      )}
    </div>
  );
}

function EngineCard({ engine: e, config, patch, onKeys }: { engine: VoiceEngineInfo; config: VoiceConfig; patch: Patch; onKeys: (d: VoiceEnginesResult) => void }) {
  const { t } = useT(["voice", "common"]);
  const setConsent = useVoiceStore((s) => s.setConsent);
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState<"stt" | "tts" | "key" | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const field = optionField(e);
  const kind = e.stt ? "stt" : "tts";
  const label = e.stt ? t(`settings.stt.engines.${e.stt}`) : t(`settings.tts.engines.${e.tts as TtsEngine}`);
  const runnable = e.status === "Ready" && (e.stt ? e.stt !== "WebSpeech" : e.tts !== "System");

  const ensureConsent = async (): Promise<boolean> => {
    if (!e.needsConsent || e.consented) return true;
    if (e.stt) return setConsent("SpeechRecognition", true, e.stt, { keepSelection: true });
    return setConsent("CloudSpeech", true, undefined, { ttsEngine: e.tts as TtsEngine, keepSelection: true });
  };

  const saveKey = async (value: string | null) => {
    const client = maybeClient();
    if (!client || !e.key) return;
    setBusy("key");
    try {
      onKeys(await client.call("voice.setEngineKey", { key: e.key as VoiceKeyId, ...(value ? { apiKey: value } : {}) }));
      setKey("");
      setResult(value ? t("settings.engines.keySaved") : null);
    } catch (err) {
      setResult(t("settings.engines.testFailed", { error: describeError(err) }));
    } finally {
      setBusy(null);
    }
  };

  const testStt = async () => {
    const client = maybeClient();
    if (!client || !e.stt || !(await ensureConsent())) return;
    setBusy("stt");
    setResult(t("settings.engines.recording"));
    let release: (() => void) | null = null;
    try {
      const m = await acquireMic("meter");
      release = m.release;
      const chunks: Float32Array[] = [];
      const off = m.mic.subscribe((msg) => {
        if (msg.type === "pcm") chunks.push(msg.data);
      });
      m.mic.setRecording(true);
      await new Promise((r) => window.setTimeout(r, RECORD_MS));
      m.mic.setRecording(false);
      off();
      setResult(t("settings.engines.transcribing"));
      const wav = encodeWav(concatPcm(chunks), SAMPLE_RATE);
      const r = await client.call("voice.transcribe", { audioBase64: toBase64(wav), mimeType: "audio/wav", language: config.language === "auto" ? "ru" : config.language, engine: e.stt }, { timeoutMs: 120_000 });
      setResult(r.text ? t("settings.engines.testResult", { text: r.text, ms: r.elapsedMs }) : t("settings.engines.testEmpty"));
    } catch (err) {
      setResult(t("settings.engines.testFailed", { error: describeError(err) }));
    } finally {
      release?.();
      setBusy(null);
    }
  };

  const testTts = async () => {
    const client = maybeClient();
    if (!client || !e.tts || !(await ensureConsent())) return;
    setBusy("tts");
    setResult(null);
    try {
      const r = await client.call("voice.synthesize", { text: t("settings.tts.sample"), engine: e.tts, language: config.language === "auto" ? "ru-RU" : config.language }, { timeoutMs: 120_000 });
      const bytes = fromBase64(r.audioBase64);
      const url = URL.createObjectURL(new Blob([bytes.buffer as ArrayBuffer], { type: r.mimeType }));
      const audio = new Audio(url);
      audio.onended = () => URL.revokeObjectURL(url);
      await audio.play().catch(() => undefined);
      setResult(t("settings.engines.played"));
    } catch (err) {
      setResult(t("settings.engines.testFailed", { error: describeError(err) }));
    } finally {
      setBusy(null);
    }
  };

  const setOption = (v: string) => {
    if (!field) return;
    patch((c) => ({ ...c, engines: { ...c.engines, [field]: field === "windowsVoice" ? v || null : v } }));
  };
  const optionValue = field ? ((config.engines?.[field] as string | null | undefined) ?? "") : "";

  return (
    <article className="vx-engine" data-testid={`engine-${kind}-${e.stt ?? e.tts}`}>
      <header className="vx-engine__head">
        <strong>{label}</strong>
        <Badge tone={STATUS_TONE[e.status]}>{t(`settings.engines.status.${e.status}`)}</Badge>
      </header>
      <div className="vx-engine__chips">
        <Badge tone={e.offline ? "success" : "info"}>{e.offline ? t("settings.engines.offline") : t("settings.engines.cloud")}</Badge>
        {e.russian && <Badge tone="accent">{t("settings.engines.russian")}</Badge>}
        {e.streaming && <Badge>{t("settings.engines.streaming")}</Badge>}
        <Badge>{t("settings.engines.quality", { grade: t(`settings.engines.grade.${e.quality}`) })}</Badge>
        <Badge>{t("settings.engines.speed", { grade: t(`settings.engines.grade.${e.speed}`) })}</Badge>
        <Badge tone="violet">{t(`settings.engines.verification.${e.verification}`)}</Badge>
      </div>
      {e.detail && <div className="set-muted set-muted--small">{e.detail}</div>}
      {field && e.options.length > 0 && (
        <Select
          label={t("settings.engines.option")}
          value={optionValue}
          options={[...(field === "windowsVoice" || field === "googleVoice" ? [{ value: "", label: t("settings.tts.autoVoice") }] : []), ...e.options.map((o) => ({ value: o.id, label: o.available ? o.label : t("settings.engines.notInstalled", { name: o.label }) }))]}
          onChange={setOption}
        />
      )}
      {e.stt === "WhisperCpp" && (
        <label className="vx-engine__row">
          <span>{t("settings.engines.gpu")}</span>
          <Switch label={t("settings.engines.gpu")} checked={config.engines?.whisperGpu ?? false} onChange={(v) => patch((c) => ({ ...c, engines: { ...c.engines, whisperGpu: v } }))} />
        </label>
      )}
      {(e.stt === "AzureSpeech" || e.tts === "AzureSpeech") && (
        <TextField label={t("settings.engines.region")} value={config.engines?.azureRegion ?? ""} placeholder={t("settings.engines.regionPlaceholder")} onCommit={(v) => patch((c) => ({ ...c, engines: { ...c.engines, azureRegion: v.trim().toLowerCase() } }))} />
      )}
      {(e.stt === "YandexSpeechKit" || e.tts === "YandexSpeechKit") && (
        <TextField label={t("settings.engines.folderId")} value={config.engines?.yandexFolderId ?? ""} placeholder={t("settings.engines.folderPlaceholder")} onCommit={(v) => patch((c) => ({ ...c, engines: { ...c.engines, yandexFolderId: v.trim() } }))} />
      )}
      {e.tts === "ElevenLabs" && (
        <TextField label={t("settings.engines.voiceId")} value={config.engines?.elevenlabsVoice ?? ""} onCommit={(v) => v.trim() && patch((c) => ({ ...c, engines: { ...c.engines, elevenlabsVoice: v.trim() } }))} />
      )}
      {e.key && (
        <div className="vx-engine__row">
          <KeyRound size={14} aria-hidden />
          <input
            className="ok-input vx-engine__key"
            type="password"
            autoComplete="off"
            aria-label={t("settings.engines.apiKey")}
            placeholder={e.keyConfigured ? t("settings.engines.keyStored") : t("settings.engines.apiKeyPlaceholder")}
            value={key}
            onChange={(ev) => setKey(ev.target.value)}
          />
          <Button size="sm" disabled={!key.trim() || busy !== null} onClick={() => void saveKey(key.trim())}>
            {t("settings.engines.saveKey")}
          </Button>
          {e.keyConfigured && (
            <Button size="sm" variant="ghost" disabled={busy !== null} onClick={() => void saveKey(null)}>
              {t("settings.engines.clearKey")}
            </Button>
          )}
        </div>
      )}
      <div className="vx-engine__row">
        {e.stt && (
          <Button size="sm" disabled={!runnable || busy !== null} onClick={() => void testStt()}>
            {busy === "stt" ? <Spinner size={12} /> : <Mic size={14} />} {t("settings.engines.testStt")}
          </Button>
        )}
        {e.tts && (
          <Button size="sm" disabled={!runnable || busy !== null} onClick={() => void testTts()}>
            {busy === "tts" ? <Spinner size={12} /> : <Play size={14} />} {t("settings.engines.testTts")}
          </Button>
        )}
        {result && (
          <span className="set-muted set-muted--small" role="status">
            {result}
          </span>
        )}
      </div>
    </article>
  );
}

function TextField({ label, value, placeholder, onCommit }: { label: string; value: string; placeholder?: string; onCommit: (v: string) => void }) {
  const [v, setV] = useState(value);
  useEffect(() => setV(value), [value]);
  return (
    <label className="vx-engine__row">
      <span>{label}</span>
      <input className="ok-input" aria-label={label} value={v} placeholder={placeholder} onChange={(ev) => setV(ev.target.value)} onBlur={() => v !== value && onCommit(v)} />
    </label>
  );
}
