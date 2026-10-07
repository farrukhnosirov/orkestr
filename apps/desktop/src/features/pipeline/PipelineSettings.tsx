/**
 * Settings → Quality gates → "Quality pipeline" (per project,
 * docs/architecture/quality-pipeline.md, §17): run the pipeline for Manager
 * tasks or not, its stages (also per task kind), the Self-Healing limits
 * (attempts, time, spend) and what the Merge stage does. Loaded with
 * `pipeline.getConfig`, saved with `pipeline.setConfig`.
 */
import type { PipelineConfig, PipelineKindOverride, PipelineMergeMode, PipelineStageKind, SetPipelineConfigParams } from "@orkestr/protocol";
import { Badge, Button, IconButton, Input, Spinner } from "@orkestr/ui";
import { Plus, RotateCcw, Save, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { Segmented, SettingGroup, SettingRow, Switch } from "../settings/controls";
import { addOverride, healingForm, type HealingForm, MAX_ATTEMPTS, MAX_WALL_MINS, parseHealing, TOGGLE_STAGES, toggleStage } from "./configLogic";
import "./pipeline.css";

const MERGE_MODES: readonly PipelineMergeMode[] = ["Off", "Propose", "PerPolicy"];

type Patch = Omit<SetPipelineConfigParams, "projectId">;

export function PipelineSettings({ projectId }: { projectId: string }) {
  const { t } = useT(["pipeline", "common"]);
  const [cfg, setCfg] = useState<PipelineConfig | null>(null);
  const [form, setForm] = useState<HealingForm | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const [newKind, setNewKind] = useState("");
  const [kindError, setKindError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const adopt = (c: PipelineConfig) => {
    setCfg(c);
    setForm(healingForm(c.healing));
    setFormError(null);
  };

  const load = useCallback(async () => {
    setError(null);
    try {
      adopt(await getClient().call("pipeline.getConfig", { projectId }));
    } catch (e) {
      setError(describeError(e));
    }
  }, [projectId]);

  useEffect(() => {
    setCfg(null);
    void load();
  }, [load]);

  const save = async (key: string, patch: Patch) => {
    setBusy(key);
    setError(null);
    try {
      adopt(await getClient().call("pipeline.setConfig", { projectId, ...patch }));
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(null);
    }
  };

  if (!cfg || !form) {
    return (
      <SettingGroup title={t("settings.title")}>
        {error ? (
          <div className="set-alert" role="alert">
            {error}{" "}
            <Button size="sm" variant="ghost" onClick={() => void load()}>
              {t("common:actions.retry")}
            </Button>
          </div>
        ) : (
          <div className="q-muted">
            <Spinner size={14} /> {t("loading")}
          </div>
        )}
      </SettingGroup>
    );
  }

  const disabled = busy !== null;
  const stageName = (s: PipelineStageKind) => t(`stage.${s}`);
  const setOverrides = (key: string, overrides: PipelineKindOverride[]) => void save(key, { overrides });

  const saveHealing = () => {
    const parsed = parseHealing(form);
    if ("error" in parsed) {
      setFormError(t(`settings.errors.${parsed.error}`, { maxAttempts: MAX_ATTEMPTS, maxMins: MAX_WALL_MINS }));
      return;
    }
    setFormError(null);
    void save("healing", { healing: parsed.limits });
  };
  const healingDirty = JSON.stringify(form) !== JSON.stringify(healingForm(cfg.healing));

  const add = () => {
    const r = addOverride(cfg.overrides, newKind, cfg.stages);
    if ("error" in r) {
      setKindError(t(`settings.errors.${r.error}`));
      return;
    }
    setKindError(null);
    setNewKind("");
    setOverrides("override:add", r.overrides);
  };

  return (
    <div className="pl-settings">
      <SettingGroup title={t("settings.title")}>
        <SettingRow title={t("settings.enabled")} description={t("settings.enabledDesc")}>
          <Switch label={t("settings.enabled")} checked={cfg.enabled} busy={busy === "enabled"} disabled={disabled} onChange={(v) => void save("enabled", { enabled: v })} />
        </SettingRow>
        {TOGGLE_STAGES.map((s) => (
          <SettingRow key={s} title={stageName(s)} description={t(`settings.stageDesc.${s as "Review"}`)}>
            <Switch
              label={t("settings.stageToggle", { stage: stageName(s) })}
              checked={cfg.stages.includes(s)}
              busy={busy === `stage:${s}`}
              disabled={disabled || !cfg.enabled}
              onChange={(on) => void save(`stage:${s}`, { stages: toggleStage(cfg.stages, s, on) })}
            />
          </SettingRow>
        ))}
        <SettingRow title={t("settings.merge")} description={t(`settings.mergeDesc.${cfg.merge}`)} stacked>
          <Segmented
            label={t("settings.merge")}
            value={cfg.merge}
            disabled={disabled || !cfg.enabled}
            options={MERGE_MODES.map((m) => ({ value: m, label: t(`settings.mergeMode.${m}`) }))}
            onChange={(merge) => void save("merge", { merge, stages: toggleStage(cfg.stages, "Merge", merge !== "Off") })}
          />
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t("settings.healingGroup")}>
        <SettingRow title={t("settings.maxAttempts")} description={t("settings.maxAttemptsDesc", { max: MAX_ATTEMPTS })}>
          <Input
            className="pl-settings__num"
            inputMode="numeric"
            aria-label={t("settings.maxAttempts")}
            value={form.maxAttempts}
            onChange={(e) => setForm({ ...form, maxAttempts: e.target.value })}
          />
        </SettingRow>
        <SettingRow title={t("settings.maxWallMins")} description={t("settings.maxWallMinsDesc", { max: MAX_WALL_MINS })}>
          <Input
            className="pl-settings__num"
            inputMode="numeric"
            aria-label={t("settings.maxWallMins")}
            value={form.maxWallMins}
            onChange={(e) => setForm({ ...form, maxWallMins: e.target.value })}
          />
        </SettingRow>
        <SettingRow title={t("settings.maxBudget")} description={t("settings.maxBudgetDesc")}>
          <Input
            className="pl-settings__num"
            inputMode="decimal"
            aria-label={t("settings.maxBudget")}
            placeholder={t("settings.noLimit")}
            value={form.maxBudgetUsd}
            onChange={(e) => setForm({ ...form, maxBudgetUsd: e.target.value })}
          />
        </SettingRow>
        <SettingRow title={t("settings.stopOnRepeat")} description={t("settings.stopOnRepeatDesc")}>
          <Switch label={t("settings.stopOnRepeat")} checked={form.stopOnRepeat} disabled={disabled} onChange={(v) => setForm({ ...form, stopOnRepeat: v })} />
        </SettingRow>
        <SettingRow title={t("settings.escalateModel")} description={t("settings.escalateModelDesc")}>
          <Switch label={t("settings.escalateModel")} checked={form.escalateModel} disabled={disabled} onChange={(v) => setForm({ ...form, escalateModel: v })} />
        </SettingRow>
        {formError && (
          <div className="set-alert" role="alert">
            {formError}
          </div>
        )}
        <div className="q-actions">
          <Button size="sm" variant="primary" leftIcon={<Save size={12} />} loading={busy === "healing"} disabled={disabled || !healingDirty} onClick={saveHealing}>
            {t("settings.saveHealing")}
          </Button>
          <Button size="sm" variant="ghost" disabled={disabled || !healingDirty} onClick={() => adopt(cfg)}>
            {t("settings.discard")}
          </Button>
        </div>
      </SettingGroup>

      <SettingGroup title={t("settings.overridesGroup")}>
        <p className="q-muted">{t("settings.overridesHint")}</p>
        {cfg.overrides.map((o, i) => (
          <SettingRow key={o.kind} title={o.kind} stacked>
            <span className="q-switches">
              {TOGGLE_STAGES.map((s) => (
                <span key={s} className="q-switch-label">
                  <Switch
                    label={t("settings.overrideStage", { kind: o.kind, stage: stageName(s) })}
                    checked={o.stages.includes(s)}
                    disabled={disabled}
                    onChange={(on) =>
                      setOverrides(
                        `override:${o.kind}`,
                        cfg.overrides.map((x, j) => (j === i ? { ...x, stages: toggleStage(x.stages, s, on) } : x)),
                      )
                    }
                  />{" "}
                  {stageName(s)}
                </span>
              ))}
              <IconButton
                label={t("settings.removeOverride", { kind: o.kind })}
                icon={<Trash2 size={13} />}
                disabled={disabled}
                onClick={() => setOverrides(`override:${o.kind}`, cfg.overrides.filter((_, j) => j !== i))}
              />
            </span>
          </SettingRow>
        ))}
        <div className="q-actions">
          <Input
            aria-label={t("settings.kindLabel")}
            placeholder={t("settings.kindPlaceholder")}
            value={newKind}
            error={kindError ?? undefined}
            onChange={(e) => setNewKind(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && add()}
          />
          <Button size="sm" leftIcon={<Plus size={12} />} disabled={disabled} onClick={add}>
            {t("settings.addOverride")}
          </Button>
        </div>
      </SettingGroup>

      {error && (
        <div className="set-alert" role="alert">
          {error}
        </div>
      )}
      <div className="q-actions">
        {cfg.customized && <Badge tone="info">{t("settings.customized")}</Badge>}
        <Button size="sm" variant="ghost" leftIcon={<RotateCcw size={12} />} disabled={disabled || !cfg.customized} onClick={() => void save("reset", { reset: true })}>
          {t("settings.reset")}
        </Button>
      </div>
    </div>
  );
}
