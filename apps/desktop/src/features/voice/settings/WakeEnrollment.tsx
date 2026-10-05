/**
 * Wake-word settings and enrollment (ADR-0017, local only): record the
 * phrase 3–5 times with a live level meter, test detection, tune
 * sensitivity / CPU budget / battery behaviour. Samples (MFCC templates)
 * stay in this device's webview storage — never in the core or the cloud.
 */
import type { VoiceConfig } from "@orkestr/protocol";
import { Badge, Button, Input } from "@orkestr/ui";
import { clsx } from "clsx";
import { Circle, CircleCheck, Mic, Radar, RotateCcw, ShieldCheck, Square, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { useFormat, useT } from "../../../i18n";
import { SettingRow, Switch } from "../../settings/controls";
import { playSound } from "../../sound/engine";
import { acquireMic, micErrorText, syncWakeListener } from "../controller";
import { useVoiceStore } from "../store";
import { Enrollment } from "../wake/enroll";
import { segmentFrames } from "../wake/segmenter";
import { loadTemplates, MAX_ENROLL_SAMPLES, MIN_ENROLL_SAMPLES, saveTemplates, WakeSpotter } from "../wake/spotter";
import { LevelMeter } from "../ui/LevelMeter";
import { Notice, Slider } from "./fields";

type Mode = "idle" | "recording" | "testing";

export function WakeEnrollment({ config, patch }: { config: VoiceConfig; patch: (fn: (c: VoiceConfig) => VoiceConfig) => void }) {
  const { t } = useT(["voice", "common"]);
  const fmt = useFormat();
  const level = useVoiceStore((s) => s.level);
  const wakeStatus = useVoiceStore((s) => s.wakeStatus);
  const wakeError = useVoiceStore((s) => s.wakeError);
  const load = useVoiceStore((s) => s.wakeLoadPct);
  const [stored, setStored] = useState(() => loadTemplates());
  const [mode, setMode] = useState<Mode>("idle");
  const [count, setCount] = useState(0);
  const [message, setMessage] = useState<{ text: string; tone: "ok" | "warn" } | null>(null);
  const [phrase, setPhrase] = useState(config.wake.phrase);
  const [speaking, setSpeaking] = useState(false);
  const stopRef = useRef<(() => void) | null>(null);
  const enrollRef = useRef<Enrollment | null>(null);

  useEffect(() => setPhrase(config.wake.phrase), [config.wake.phrase]);
  useEffect(() => () => stopRef.current?.(), []);

  const enrolled = (stored?.samples.length ?? 0) >= MIN_ENROLL_SAMPLES;
  const phraseMismatch = enrolled && stored?.phrase !== config.wake.phrase;

  const stop = () => {
    stopRef.current?.();
    stopRef.current = null;
    setMode("idle");
    setSpeaking(false);
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
    const next = { phrase: config.wake.phrase, samples: enrollment.samples.map((s) => s.map((f) => [...f])) };
    saveTemplates(next);
    setStored(loadTemplates() ?? next);
    setMessage({ text: t("settings.wake.saved"), tone: "ok" });
    playSound("completion", { test: true });
    void syncWakeListener();
  }

  const startTest = async () => {
    stop();
    const s = loadTemplates();
    if (!s) return;
    const spotter = new WakeSpotter(s.samples, config.wake.sensitivity);
    setMessage({ text: t("settings.wake.testSay", { phrase: config.wake.phrase }), tone: "ok" });
    try {
      const { mic, release } = await acquireMic("enroll");
      mic.setSegmenting(true);
      const off = mic.subscribe((m) => {
        if (m.type === "level") setSpeaking(m.speaking);
        if (m.type !== "segment") return;
        const frames = segmentFrames(m);
        const d = spotter.distance(frames, m.early);
        const ok = d <= spotter.acceptThreshold;
        if (ok) playSound("wake", { test: true });
        const ratio = fmt.percent(Math.min(9.99, d / spotter.acceptThreshold));
        setMessage({ text: ok ? t("settings.wake.testHit", { ratio }) : t("settings.wake.testMiss", { ratio }), tone: ok ? "ok" : "warn" });
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
    saveTemplates(null);
    setStored(null);
    setMessage(null);
    if (config.wake.enabled) patch((c) => ({ ...c, wake: { ...c.wake, enabled: false } }));
    void syncWakeListener();
  };

  const commitPhrase = () => {
    const p = phrase.trim();
    if (p && p !== config.wake.phrase) patch((c) => ({ ...c, wake: { ...c.wake, phrase: p.slice(0, 40) } }));
    else setPhrase(config.wake.phrase);
  };

  const slots = Array.from({ length: MAX_ENROLL_SAMPLES }, (_, i) => i < (mode === "recording" ? count : (stored?.samples.length ?? 0)));
  const statusTone = wakeStatus === "listening" ? "success" : wakeStatus === "error" ? "danger" : wakeStatus === "off" ? "neutral" : "warning";

  return (
    <>
      <Notice tone="ok" icon={<ShieldCheck size={16} />} title={t("settings.wake.privacyTitle")}>
        {t("settings.wake.privacyText")}
      </Notice>

      <SettingRow title={t("settings.wake.enabled")} description={t("settings.wake.enabledDesc")}>
        <span className="vx-row-inline">
          <Badge tone={statusTone}>{t(`settings.wake.status.${wakeStatus}`)}</Badge>
          <Switch
            label={t("settings.wake.enabled")}
            checked={config.wake.enabled}
            disabled={!enrolled && !config.wake.enabled}
            onChange={(v) => patch((c) => ({ ...c, wake: { ...c.wake, enabled: v } }))}
          />
        </span>
      </SettingRow>
      {wakeError && <div className="set-alert">{wakeError}</div>}

      <SettingRow title={t("settings.wake.phrase")} description={t("settings.wake.phraseDesc")}>
        <Input
          className="vx-phrase"
          value={phrase}
          maxLength={40}
          aria-label={t("settings.wake.phrase")}
          onChange={(e) => setPhrase(e.target.value)}
          onBlur={commitPhrase}
          onKeyDown={(e) => {
            if (e.key === "Enter") commitPhrase();
          }}
        />
      </SettingRow>

      <div className={clsx("vx-enroll", mode !== "idle" && "vx-enroll--live")}>
        <div className="vx-enroll__head">
          <div>
            <div className="vx-enroll__title">{t("settings.wake.enrollTitle", { phrase: config.wake.phrase })}</div>
            <div className="set-muted set-muted--small">
              {mode === "recording"
                ? t("settings.wake.enrollRecording", { phrase: config.wake.phrase })
                : enrolled
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
        {message && <div className={clsx("vx-enroll__msg", `vx-enroll__msg--${message.tone}`)} role="status">{message.text}</div>}
        {phraseMismatch && mode === "idle" && <div className="vx-enroll__msg vx-enroll__msg--warn">{t("settings.wake.phraseChanged")}</div>}
        <div className="vx-enroll__actions">
          {mode === "recording" ? (
            <Button variant="primary" leftIcon={<Square size={13} />} onClick={finishRecording}>
              {t("settings.wake.finish")}
            </Button>
          ) : (
            <Button variant={enrolled ? "secondary" : "primary"} leftIcon={enrolled ? <RotateCcw size={14} /> : <Mic size={14} />} onClick={() => void startRecording()}>
              {enrolled ? t("settings.wake.reRecord") : t("settings.wake.record")}
            </Button>
          )}
          {mode === "testing" ? (
            <Button variant="secondary" leftIcon={<Square size={13} />} onClick={stop}>
              {t("settings.wake.stopTest")}
            </Button>
          ) : (
            <Button variant="ghost" leftIcon={<Radar size={14} />} disabled={!enrolled || mode !== "idle"} onClick={() => void startTest()}>
              {t("settings.wake.test")}
            </Button>
          )}
          {enrolled && mode === "idle" && (
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
