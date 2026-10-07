/**
 * Settings → "Project privacy" (§56): which AI providers may see the active
 * project's code (Cloud allowed / Selected providers only / Local models
 * only) and the project's data classification. Saved with `projects.update`;
 * the core enforces the policy, tightened by the classification, in every
 * provider choice (agents, Manager, reviews, merges, summaries).
 */
import type { AiPolicy, DataClassification, Project } from "@orkestr/protocol";
import { Badge, EmptyState, Spinner } from "@orkestr/ui";
import { Lock } from "lucide-react";
import { useState } from "react";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";
import { instanceName } from "../providers/logic";
import { useProvidersLive } from "../providers/store";
import { Segmented, SettingGroup, SettingRow, Switch } from "../settings/controls";
import type { SettingsSectionProps } from "../settings/sections";
import { AI_MODES, CLASSIFICATIONS, effectiveAiPolicy, policyForMode, toggleProvider, type AiMode } from "./privacy";

export function PrivacySettings({ projectId }: SettingsSectionProps) {
  const { t } = useT("security");
  const project = useAppStore((s) => s.projects.find((p) => p.id === projectId) ?? null);
  const { providers } = useProvidersLive();
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  if (!projectId || !project) return <EmptyState icon={<Lock size={22} />} title={t("privacy.noProject")} />;

  const save = async (key: string, patch: { privacy?: Project["privacy"]; classification?: DataClassification }) => {
    setBusy(key);
    setError(null);
    try {
      const r = await getClient().call("projects.update", { projectId, patch });
      useAppStore.getState().upsertProject(r.project);
    } catch (e) {
      setError(describeError(e));
    } finally {
      setBusy(null);
    }
  };
  const saveAi = (key: string, ai: AiPolicy) => void save(key, { privacy: { ...project.privacy, ai } });

  const ai = project.privacy.ai;
  const effective = effectiveAiPolicy(project.classification, ai);
  const tightened = effective.mode !== ai.mode;
  const selected = ai.mode === "SelectedProvidersOnly" ? ai.providers : [];

  return (
    <div className="sec-privacy">
      <p className="sec-privacy__intro">{t("privacy.intro", { name: project.name })}</p>
      <SettingGroup title={t("privacy.aiGroup")}>
        <SettingRow title={t("privacy.aiTitle")} description={t(`privacy.aiModeHint.${ai.mode}`)} stacked>
          <Segmented<AiMode>
            label={t("privacy.aiTitle")}
            value={ai.mode}
            disabled={busy !== null}
            options={AI_MODES.map((m) => ({ value: m, label: t(`privacy.aiMode.${m}`) }))}
            onChange={(m) => saveAi("ai", policyForMode(m, ai))}
          />
        </SettingRow>
        {ai.mode === "SelectedProvidersOnly" && (
          <SettingRow title={t("privacy.providersTitle")} description={selected.length === 0 ? t("privacy.providersNone") : undefined} stacked>
            <div className="sec-privacy__providers">
              {providers.length === 0 && <span className="sec-privacy__muted">{t("privacy.providersEmpty")}</span>}
              {providers.map((p) => (
                <span key={p.provider.id} className="sec-privacy__provider">
                  <Switch
                    checked={selected.includes(p.provider.id)}
                    label={instanceName(p.provider)}
                    busy={busy === `p:${p.provider.id}`}
                    disabled={busy !== null}
                    onChange={(on) => saveAi(`p:${p.provider.id}`, toggleProvider(ai, p.provider.id, on))}
                  />
                  <span>{instanceName(p.provider)}</span>
                  {p.provider.isLocal && <Badge tone="info">{t("privacy.local")}</Badge>}
                </span>
              ))}
            </div>
          </SettingRow>
        )}
      </SettingGroup>
      <SettingGroup title={t("privacy.classificationGroup")}>
        <SettingRow title={t("privacy.classificationTitle")} description={t(`privacy.classificationHint.${project.classification}`)} stacked>
          <Segmented<DataClassification>
            label={t("privacy.classificationTitle")}
            value={project.classification}
            disabled={busy !== null}
            options={CLASSIFICATIONS.map((c) => ({ value: c, label: t(`privacy.classification.${c}`) }))}
            onChange={(c) => void save("classification", { classification: c })}
          />
        </SettingRow>
      </SettingGroup>
      {tightened && (
        <p className="sec-privacy__effective" role="status">
          {t("privacy.effective", {
            classification: t(`privacy.classification.${project.classification}`),
            mode: t(`privacy.aiMode.${effective.mode}`),
          })}
        </p>
      )}
      {busy !== null && <Spinner size={12} />}
      {error && (
        <p className="sec-privacy__error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
