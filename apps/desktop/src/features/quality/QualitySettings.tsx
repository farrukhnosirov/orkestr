/**
 * Settings → "Quality gates" (per project, §17, §58): toggle gates and their
 * "required" flag, the Definition of Done checklist, and the check commands
 * (detected from manifests; user overrides persisted by `quality.setProfile`).
 */
import type { CheckDef, CheckKind, DodSetting, GateConfig, GateSetting, QualityProfile } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { Plus, RotateCcw, Save, ShieldCheck, Trash2, Undo2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { PathInput } from "../../components/PathInput";
import { useAppStore } from "../../store/app";
import { PipelineSettings } from "../pipeline/PipelineSettings";
import { SettingGroup, SettingRow, Switch } from "../settings/controls";
import type { SettingsSectionProps } from "../settings/sections";
import { relativeTo } from "../workbench/hostPaths";
import { blankCheck, CHECK_KINDS, validateProfile } from "./logic";
import { qualityApi } from "./store";
import "./quality.css";

function GatesEditor({ projectId, config, onChange }: { projectId: string; config: GateConfig; onChange: (c: GateConfig) => void }) {
  const { t } = useT("quality");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const save = async (key: string, patch: { gates?: GateSetting[]; dod?: DodSetting[]; reset?: boolean }) => {
    setBusy(key);
    setError(null);
    try {
      onChange(await qualityApi.setGates(projectId, patch));
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <>
      <SettingGroup title={t("settings.gatesGroup")}>
        {config.gates.map((g) => (
          <SettingRow key={g.gate} title={t(`gate.${g.gate}`)} description={g.enabled ? t("settings.requiredHint") : undefined}>
            <span className="q-switches">
              <span className="q-switch-label">{t("settings.enabled")}</span>
              <Switch
                checked={g.enabled}
                label={`${t(`gate.${g.gate}`)} — ${t("settings.enabled")}`}
                busy={busy === `g:${g.gate}`}
                disabled={busy !== null}
                onChange={(enabled) => void save(`g:${g.gate}`, { gates: [{ ...g, enabled }] })}
              />
              <span className="q-switch-label">{t("settings.required")}</span>
              <Switch
                checked={g.required}
                label={`${t(`gate.${g.gate}`)} — ${t("settings.required")}`}
                busy={busy === `r:${g.gate}`}
                disabled={busy !== null || !g.enabled}
                onChange={(required) => void save(`r:${g.gate}`, { gates: [{ ...g, required }] })}
              />
            </span>
          </SettingRow>
        ))}
      </SettingGroup>
      <SettingGroup title={t("settings.dodGroup")}>
        {config.dod.map((d) => (
          <SettingRow key={d.item} title={t(`dod.${d.item}`)}>
            <Switch
              checked={d.enabled}
              label={t(`dod.${d.item}`)}
              busy={busy === `d:${d.item}`}
              disabled={busy !== null}
              onChange={(enabled) => void save(`d:${d.item}`, { dod: [{ ...d, enabled }] })}
            />
          </SettingRow>
        ))}
      </SettingGroup>
      <div className="q-actions">
        {config.customized && <Badge tone="info">{t("settings.customized")}</Badge>}
        <Button size="sm" variant="ghost" leftIcon={<RotateCcw size={12} />} disabled={busy !== null || !config.customized} onClick={() => void save("reset", { reset: true })}>
          {t("settings.resetGates")}
        </Button>
      </div>
      {error && <p className="q-error" role="alert">{error}</p>}
    </>
  );
}

function ChecksEditor({ projectId, profile, onChange }: { projectId: string; profile: QualityProfile; onChange: (p: QualityProfile) => void }) {
  const { t } = useT("quality");
  const [draft, setDraft] = useState<CheckDef[]>(profile.checks);
  const rootPath = useAppStore((s) => s.projects.find((p) => p.id === projectId)?.rootPath ?? null);
  const [busy, setBusy] = useState<"save" | "reset" | null>(null);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => setDraft(profile.checks), [profile]);

  const dirty = useMemo(() => JSON.stringify(draft) !== JSON.stringify(profile.checks), [draft, profile.checks]);
  const problems = useMemo(() => validateProfile(draft), [draft]);
  const problemAt = (index: number) => problems.find((p) => p.index === index);

  const patch = (index: number, change: Partial<CheckDef>) =>
    setDraft((list) => list.map((c, i) => (i === index ? { ...c, ...change, source: "User" } : c)));

  const save = async (reset: boolean) => {
    setBusy(reset ? "reset" : "save");
    setMessage(null);
    try {
      onChange(await qualityApi.setProfile(projectId, reset ? [] : draft, reset));
      setMessage({ ok: true, text: t("settings.saved") });
    } catch (e) {
      setMessage({ ok: false, text: describeError(e) });
    } finally {
      setBusy(null);
    }
  };

  return (
    <SettingGroup title={t("settings.checksGroup")}>
      {profile.roots.length > 0 && (
        <p className="q-muted">
          {t("settings.detectedRoots", {
            roots: profile.roots.map((r) => `${r.path === "." ? "/" : r.path} (${r.toolchains.join(", ")})`).join("; "),
          })}
        </p>
      )}
      {draft.length === 0 && <p className="q-muted">{t("settings.noChecks")}</p>}
      <ul className="q-check-list">
        {draft.map((c, i) => {
          const problem = problemAt(i);
          const name = c.label || c.command || t(`kind.${c.kind}`);
          return (
            <li key={c.id} className="q-check-edit">
              <div className="q-check-edit__head">
                <Switch checked={c.enabled} label={t("settings.checkEnabled", { label: name })} onChange={(enabled) => patch(i, { enabled })} />
                <select
                  className="q-select"
                  value={c.kind}
                  aria-label={t("settings.kindLabel")}
                  onChange={(e) => patch(i, { kind: e.target.value as CheckKind })}
                >
                  {CHECK_KINDS.map((k) => (
                    <option key={k} value={k}>
                      {t(`kind.${k}`)}
                    </option>
                  ))}
                </select>
                <Badge tone={c.source === "Detected" ? "neutral" : "violet"}>
                  {c.source === "Detected" ? t("settings.detected") : t("settings.toolchainUser")}
                </Badge>
                <span className="q-spacer" />
                <IconButton
                  size="sm"
                  label={t("settings.removeCheck", { label: name })}
                  icon={<Trash2 size={12} />}
                  onClick={() => setDraft((list) => list.filter((_, j) => j !== i))}
                />
              </div>
              <div className="q-check-edit__grid">
                <Input label={t("settings.labelLabel")} value={c.label} maxLength={200} onChange={(e) => patch(i, { label: e.target.value })} />
                <Input
                  className="q-check-edit__command"
                  label={t("settings.commandLabel")}
                  value={c.command}
                  spellCheck={false}
                  error={problem && problem.key !== "cwdOutside" && problem.key !== "timeout" ? t(`settings.errors.${problem.key}`) : undefined}
                  onChange={(e) => patch(i, { command: e.target.value })}
                />
                <PathInput
                  label={t("settings.cwdLabel")}
                  value={c.cwd}
                  error={problem?.key === "cwdOutside" ? t("settings.errors.cwdOutside") : undefined}
                  defaultPath={rootPath ?? undefined}
                  // The folder is stored relative to the project; a pick outside it stays absolute and is flagged.
                  mapPicked={(folder) => (rootPath ? (relativeTo(rootPath, folder) ?? folder) || "." : folder)}
                  onPathChange={(cwd) => patch(i, { cwd })}
                />
                <Input
                  label={t("settings.timeoutLabel")}
                  inputMode="numeric"
                  value={c.timeoutSecs == null ? "" : String(c.timeoutSecs)}
                  placeholder={t("settings.timeoutPlaceholder")}
                  error={problem?.key === "timeout" ? t("settings.errors.timeout") : undefined}
                  onChange={(e) => {
                    const v = e.target.value.trim();
                    patch(i, { timeoutSecs: v === "" ? undefined : Number(v) });
                  }}
                />
              </div>
            </li>
          );
        })}
      </ul>
      <div className="q-actions">
        <Button size="sm" variant="secondary" leftIcon={<Plus size={12} />} onClick={() => setDraft((list) => [...list, blankCheck(list)])}>
          {t("settings.addCheck")}
        </Button>
        <span className="q-spacer" />
        <Button size="sm" variant="ghost" leftIcon={<Undo2 size={12} />} disabled={!dirty || busy !== null} onClick={() => setDraft(profile.checks)}>
          {t("settings.discard")}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          leftIcon={<RotateCcw size={12} />}
          disabled={!profile.customized || busy !== null}
          loading={busy === "reset"}
          onClick={() => void save(true)}
        >
          {t("settings.resetProfile")}
        </Button>
        <Button
          size="sm"
          variant="primary"
          leftIcon={<Save size={12} />}
          disabled={!dirty || problems.length > 0 || busy !== null}
          loading={busy === "save"}
          onClick={() => void save(false)}
        >
          {t("settings.save")}
        </Button>
      </div>
      {message && (
        <p className={message.ok ? "q-ok" : "q-error"} role={message.ok ? "status" : "alert"}>
          {message.text}
        </p>
      )}
    </SettingGroup>
  );
}

export function QualitySettings({ projectId }: SettingsSectionProps) {
  const { t } = useT(["quality", "common"]);
  const connected = useAppStore((s) => s.connection.state === "open");
  const [gates, setGates] = useState<GateConfig | null>(null);
  const [profile, setProfile] = useState<QualityProfile | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async (id: string) => {
    setError(null);
    try {
      const [g, p] = await Promise.all([qualityApi.gates(id), qualityApi.getProfile(id)]);
      setGates(g);
      setProfile(p);
    } catch (e) {
      setError(describeError(e));
    }
  }, []);

  useEffect(() => {
    setGates(null);
    setProfile(null);
    if (projectId && connected) void load(projectId);
  }, [projectId, connected, load]);

  if (!projectId) return <EmptyState icon={<ShieldCheck size={22} />} title={t("settings.noProject")} />;
  if (error) {
    return (
      <EmptyState
        icon={<ShieldCheck size={22} />}
        title={t("settings.loadFailed")}
        description={error}
        action={
          <Button size="sm" onClick={() => void load(projectId)}>
            {t("common:actions.retry")}
          </Button>
        }
      />
    );
  }
  if (!gates || !profile) {
    return (
      <div className="q-muted">
        <Spinner size={14} /> {t("settings.loading")}
      </div>
    );
  }
  return (
    <div className="q-settings">
      <p className="q-muted">{t("settings.intro")}</p>
      <GatesEditor projectId={projectId} config={gates} onChange={setGates} />
      <ChecksEditor projectId={projectId} profile={profile} onChange={setProfile} />
      <PipelineSettings key={projectId} projectId={projectId} />
    </div>
  );
}
