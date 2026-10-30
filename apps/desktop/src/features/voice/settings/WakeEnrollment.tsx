/**
 * Wake-word settings and enrollment (ADR-0017, local only): record the
 * phrase 3–5 times with a live level meter, test detection, tune
 * sensitivity / CPU budget / battery behaviour. Samples (MFCC templates)
 * stay in this device's webview storage — never in the core or the cloud.
 *
 * Several wake phrases (ADR-0056): the phrases are chips («Джарвис»,
 * «Эй, Джарвис», «Jarvis»…); samples are recorded per phrase (a chip
 * selects which one), and the status shows what the listener waits for —
 * including a quiet «Перезапуск слушателя…» while the watchdog recovers it.
 */
import type { VoiceConfig } from "@orkestr/protocol";
import { Badge, Button, Input } from "@orkestr/ui";
import { clsx } from "clsx";
import { Circle, CircleCheck, Mic, Plus, Radar, RotateCcw, ShieldCheck, Square, Trash2, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { useFormat, useT } from "../../../i18n";
import { SettingRow, Switch } from "../../settings/controls";
import { playSound } from "../../sound/engine";
import { acquireMic, micErrorText, syncWakeListener } from "../controller";
import { useVoiceStore } from "../store";
import { Enrollment } from "../wake/enroll";
import { cleanWakePhrases, configWakePhrases, MAX_WAKE_PHRASE_LEN, MAX_WAKE_PHRASES, SUGGESTED_WAKE_PHRASES } from "../wake/phrases";
import { segmentFrames } from "../wake/segmenter";
import { loadTemplateSets, MAX_ENROLL_SAMPLES, MIN_ENROLL_SAMPLES, saveTemplatesFor, type StoredTemplates, templatesFor, WakeDetector } from "../wake/spotter";
import { LevelMeter } from "../ui/LevelMeter";
import { Notice, Slider } from "./fields";

type Mode = "idle" | "recording" | "testing";

const recorded = (sets: readonly StoredTemplates[], phrase: string) => (templatesFor(phrase, sets)?.samples.length ?? 0) >= MIN_ENROLL_SAMPLES;

export function WakeEnrollment({ config, patch }: { config: VoiceConfig; patch: (fn: (c: VoiceConfig) => VoiceConfig) => void }) {
  const { t } = useT(["voice", "common"]);
  const fmt = useFormat();
  const level = useVoiceStore((s) => s.level);
  const wakeStatus = useVoiceStore((s) => s.wakeStatus);
  const wakeError = useVoiceStore((s) => s.wakeError);
  const load = useVoiceStore((s) => s.wakeLoadPct);
  const phrases = useMemo(() => configWakePhrases(config.wake), [config.wake]);
  const [sets, setSets] = useState(() => loadTemplateSets());
  const [target, setTarget] = useState(() => phrases.find((p) => !recorded(loadTemplateSets(), p)) ?? phrases[0] ?? "");
  const [mode, setMode] = useState<Mode>("idle");
  const [count, setCount] = useState(0);
  const [message, setMessage] = useState<{ text: string; tone: "ok" | "warn" } | null>(null);
  const [draft, setDraft] = useState("");
  const [speaking, setSpeaking] = useState(false);
  const stopRef = useRef<(() => void) | null>(null);
  const enrollRef = useRef<Enrollment | null>(null);

  // The enrollment target follows the list (a removed phrase falls back to the main one).
  useEffect(() => {
    if (!phrases.some((p) => p.toLowerCase() === target.toLowerCase())) setTarget(phrases[0] ?? "");
  }, [phrases, target]);
  useEffect(() => () => stopRef.current?.(), []);

  const stored = templatesFor(target, sets);
  const targetRecorded = (stored?.samples.length ?? 0) >= MIN_ENROLL_SAMPLES;
  const anyRecorded = sets.some((s) => s.samples.length >= MIN_ENROLL_SAMPLES);
  const listened = phrases.filter((p) => recorded(sets, p));

  const refresh = () => {
    setSets(loadTemplateSets());
    void syncWakeListener();
  };

  const stop = () => {
    stopRef.current?.();
    stopRef.current = null;
    setMode("idle");
    setSpeaking(false);
  };

  const setPhrases = (next: string[]) => {
    const list = cleanWakePhrases(next, phrases[0]);
    patch((c) => ({ ...c, wake: { ...c.wake, phrase: list[0] ?? c.wake.phrase, phrases: list } }));
  };

  const addPhrase = (raw: string) => {
    const p = raw.trim().replace(/\s+/g, " ").slice(0, MAX_WAKE_PHRASE_LEN);
    setDraft("");
    if (!p || phrases.length >= MAX_WAKE_PHRASES || phrases.some((x) => x.toLowerCase() === p.toLowerCase())) return;
    setPhrases([...phrases, p]);
  };

  const removePhrase = (p: string) => {
    if (phrases.length <= 1) return;
    stop();
    saveTemplatesFor(p, null); // a removed phrase forgets its voice samples
    setPhrases(phrases.filter((x) => x !== p));
    const left = loadTemplateSets();
    setSets(left);
    if (config.wake.enabled && !left.some((s) => s.samples.length >= MIN_ENROLL_SAMPLES)) patch((c) => ({ ...c, wake: { ...c.wake, enabled: false } }));
    void syncWakeListener();
  };

  const startRecording = async () => {
    stop();
    setMessage(null);
    const enrollment = new Enrollment();
    enrollRef.current = enrollment;
    setCount(0);
    try {
      const { mic, release } = await acquireMic("enroll");
      mic.setSegmenting(true);
      const off = mic.subscribe((m) => {
        if (m.type === "level") setSpeaking(m.speaking);
        if (m.type !== "segment") return;
        const r = enrollment.add(m);
        if (r.ok) {
          setCount(r.count);
          playSound("listenEnd", { test: true });
          setMessage({ text: t("settings.wake.sampleOk", { count: r.count }), tone: "ok" });
          if (enrollment.full) finishRecording();
        } else if (r.problem !== "full") {
          setMessage({ text: t(`settings.wake.problems.${r.problem}`), tone: "warn" });
        }
      });
      stopRef.current = () => {
        off();
        release();
      };
      setMode("recording");
    } catch (e) {
      setMessage({ text: micErrorText(e), tone: "warn" });
    }
  };

  function finishRecording() {
    const enrollment = enrollRef.current;
    stop();
    if (!enrollment?.complete) {
      setMessage({ text: t("settings.wake.needMore", { count: MIN_ENROLL_SAMPLES }), tone: "warn" });
      return;
    }
    saveTemplatesFor(
      target,
      enrollment.samples.map((s) => s.map((f) => [...f])),
    );
    setMessage({ text: t("settings.wake.saved"), tone: "ok" });
    playSound("completion", { test: true });
    refresh();
  }

  const startTest = async () => {
    stop();
    const detector = new WakeDetector(loadTemplateSets(), config.wake.sensitivity);
    if (!detector.ready) return;
    setMessage({ text: t("settings.wake.testSay", { phrase: target }), tone: "ok" });
    try {
      const { mic, release } = await acquireMic("enroll");
      mic.setSegmenting(true);
      const off = mic.subscribe((m) => {
        if (m.type === "level") setSpeaking(m.speaking);
        if (m.type !== "segment") return;
        const best = detector.closest(segmentFrames(m), m.early);
        if (!best) return;
        const ratio = fmt.percent(Math.min(9.99, best.ratio));
        if (best.ratio <= 1) {
          playSound("wake", { test: true });
          setMessage({ text: t("settings.wake.testHitPhrase", { phrase: best.phrase, ratio }), tone: "ok" });
        } else setMessage({ text: t("settings.wake.testMiss", { ratio }), tone: "warn" });
      });
      stopRef.current = () => {
        off();
        release();
      };
      setMode("testing");
    } catch (e) {
      setMessage({ text: micErrorText(e), tone: "warn" });
    }
  };

  const remove = () => {
    stop();
    saveTemplatesFor(target, null);
    const left = loadTemplateSets();
    setSets(left);
    setMessage(null);
    if (config.wake.enabled && !left.some((s) => s.samples.length >= MIN_ENROLL_SAMPLES)) patch((c) => ({ ...c, wake: { ...c.wake, enabled: false } }));
    void syncWakeListener();
  };

  const slots = Array.from({ length: MAX_ENROLL_SAMPLES }, (_, i) => i < (mode === "recording" ? count : (stored?.samples.length ?? 0)));
  const statusTone = wakeStatus === "listening" ? "success" : wakeStatus === "error" ? "danger" : wakeStatus === "off" ? "neutral" : "warning";
  const statusText =
    wakeStatus === "listening" && listened.length > 0
      ? t("settings.wake.status.listeningFor", { phrases: listened.map((p) => `«${p}»`).join(", ") })
      : t(`settings.wake.status.${wakeStatus}`);
  const suggestions = SUGGESTED_WAKE_PHRASES.filter((s) => !phrases.some((p) => p.toLowerCase() === s.toLowerCase()));

  return (
    <>
      <Notice tone="ok" icon={<ShieldCheck size={16} />} title={t("settings.wake.privacyTitle")}>
        {t("settings.wake.privacyText")}
      </Notice>

      <SettingRow title={t("settings.wake.enabled")} description={t("settings.wake.enabledDesc")}>
        <span className="vx-row-inline">
          <Badge tone={statusTone}>
            <span role="status" className="vx-wake-status">
              {statusText}
            </span>
          </Badge>
          <Switch
            label={t("settings.wake.enabled")}
            checked={config.wake.enabled}
            disabled={!anyRecorded && !config.wake.enabled}
            onChange={(v) => patch((c) => ({ ...c, wake: { ...c.wake, enabled: v } }))}
          />
        </span>
      </SettingRow>
      {wakeError && <div className="set-alert">{wakeError}</div>}

      <SettingRow title={t("settings.wake.phrases")} description={t("settings.wake.phrasesDesc")} stacked>
        <div className="vx-wake-phrases">
          <div className="vx-wake-phrases__list" role="list" aria-label={t("settings.wake.phrases")}>
            {phrases.map((p, i) => (
              <span key={p} role="listitem" className={clsx("vx-wake-chip", p === target && "vx-wake-chip--target")}>
                <button
                  type="button"
                  className="vx-wake-chip__name"
                  aria-pressed={p === target}
                  title={i === 0 ? t("settings.wake.primary") : undefined}
                  onClick={() => mode === "idle" && setTarget(p)}
                >
                  {recorded(sets, p) && <CircleCheck size={12} aria-label={t("settings.wake.recordedMark")} />}
                  {p}
                </button>
                {phrases.length > 1 && (
                  <button type="button" className="vx-wake-chip__remove" aria-label={t("settings.wake.removePhrase", { phrase: p })} onClick={() => removePhrase(p)}>
                    <X size={12} />
                  </button>
                )}
              </span>
            ))}
          </div>
          {phrases.length < MAX_WAKE_PHRASES && (
            <div className="vx-wake-phrases__add">
              <Input
                className="vx-phrase"
                value={draft}
                maxLength={MAX_WAKE_PHRASE_LEN}
                placeholder={t("settings.wake.newPhrase")}
                aria-label={t("settings.wake.newPhrase")}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") addPhrase(draft);
                }}
              />
              <Button variant="ghost" leftIcon={<Plus size={14} />} disabled={!draft.trim()} onClick={() => addPhrase(draft)}>
                {t("settings.wake.addPhrase")}
              </Button>
              {suggestions.map((s) => (
                <button key={s} type="button" className="vx-chip" aria-label={t("settings.wake.suggest", { phrase: s })} onClick={() => addPhrase(s)}>
                  + {s}
                </button>
              ))}
            </div>
          )}
          <div className="set-muted set-muted--small">{t("settings.wake.phrasesHint")}</div>
        </div>
      </SettingRow>

      <div className={clsx("vx-enroll", mode !== "idle" && "vx-enroll--live")}>
        <div className="vx-enroll__head">
          <div>
            <div className="vx-enroll__title">{t("settings.wake.enrollTitle", { phrase: target })}</div>
            <div className="set-muted set-muted--small">
              {mode === "recording"
                ? t("settings.wake.enrollRecording", { phrase: target })
                : targetRecorded
                  ? t("settings.wake.enrolledCount", { count: stored?.samples.length ?? 0 })
                  : t("settings.wake.enrollHint", { min: MIN_ENROLL_SAMPLES, max: MAX_ENROLL_SAMPLES })}
            </div>
          </div>
          <div className="vx-enroll__slots" aria-label={t("settings.wake.samples")}>
            {slots.map((on, i) => (
              <span key={i} className={clsx("vx-slot", on && "vx-slot--on")}>
                {on ? <CircleCheck size={16} /> : <Circle size={16} />}
              </span>
            ))}
          </div>
        </div>
        <LevelMeter level={mode === "idle" ? 0 : level} active={speaking} label={t("settings.mic.level")} />
        {message && (
          <div className={clsx("vx-enroll__msg", `vx-enroll__msg--${message.tone}`)} role="status">
            {message.text}
          </div>
        )}
        <div className="vx-enroll__actions">
          {mode === "recording" ? (
            <Button variant="primary" leftIcon={<Square size={13} />} onClick={finishRecording}>
              {t("settings.wake.finish")}
            </Button>
          ) : (
            <Button variant={targetRecorded ? "secondary" : "primary"} leftIcon={targetRecorded ? <RotateCcw size={14} /> : <Mic size={14} />} onClick={() => void startRecording()}>
              {targetRecorded ? t("settings.wake.reRecord") : t("settings.wake.record")}
            </Button>
          )}
          {mode === "testing" ? (
            <Button variant="secondary" leftIcon={<Square size={13} />} onClick={stop}>
              {t("settings.wake.stopTest")}
            </Button>
          ) : (
            <Button variant="ghost" leftIcon={<Radar size={14} />} disabled={!anyRecorded || mode !== "idle"} onClick={() => void startTest()}>
              {t("settings.wake.test")}
            </Button>
          )}
          {targetRecorded && mode === "idle" && (
            <Button variant="ghost" leftIcon={<Trash2 size={14} />} onClick={remove}>
              {t("settings.wake.delete")}
            </Button>
          )}
        </div>
      </div>

      <SettingRow title={t("settings.wake.sensitivity")} description={t("settings.wake.sensitivityDesc")}>
        <Slider
          value={config.wake.sensitivity}
          min={0}
          max={100}
          step={5}
          label={t("settings.wake.sensitivity")}
          onCommit={(v) => patch((c) => ({ ...c, wake: { ...c.wake, sensitivity: v } }))}
        />
      </SettingRow>
      <SettingRow
        title={t("settings.wake.cpuBudget")}
        description={wakeStatus === "listening" ? t("settings.wake.cpuBudgetLoad", { load: fmt.percent(load / 100) }) : t("settings.wake.cpuBudgetDesc")}
      >
        <Slider
          value={config.wake.cpuBudgetPct}
          min={1}
          max={10}
          label={t("settings.wake.cpuBudget")}
          format={(v) => fmt.percent(v / 100)}
          onCommit={(v) => patch((c) => ({ ...c, wake: { ...c.wake, cpuBudgetPct: v } }))}
        />
      </SettingRow>
      <SettingRow title={t("settings.wake.pauseOnBattery")} description={t("settings.wake.pauseOnBatteryDesc")}>
        <Switch
          label={t("settings.wake.pauseOnBattery")}
          checked={config.wake.pauseOnBatterySaver}
          onChange={(v) => patch((c) => ({ ...c, wake: { ...c.wake, pauseOnBatterySaver: v } }))}
        />
      </SettingRow>
    </>
  );
}
