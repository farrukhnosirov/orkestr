/**
 * "AI Manager" settings (§5, §7): orchestration model, reasoning, system
 * instructions, limits, approval policy, fallback chain and routing table.
 * Persisted by the core (`providers.setManager` / `providers.setRouting`).
 */
import type {
  ApprovalMode,
  ContextPolicy,
  ManagerConfig,
  ModelRef,
  ReasoningLevel,
  RoutingConfig,
  RoutingMode,
} from "@orkestr/protocol";
import { Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { ArrowDown, ArrowUp, Plus, RefreshCw, Route, Save, Sparkles, Undo2, Workflow, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { ClaudeEngineCard } from "../claudeEngine/ClaudeEngineCard";
import { useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { Select, Switch, TextArea } from "./controls";
import { moveItem, parseOptionalNumber, purposeLabel, refKey, refLabel, ROUTING_PURPOSES, validateManagerConfig } from "./logic";
import { ModelPicker } from "./ModelPicker";
import { errorMessage, getManagerConfig, getRoutingConfig, saveManagerConfig, saveRoutingConfig, useProvidersLive } from "./store";
import "./providers.css";

const REASONING_LEVELS: readonly ReasoningLevel[] = ["Low", "Medium", "High"];
const CONTEXT_POLICIES: readonly ContextPolicy[] = ["Minimal", "Balanced", "Full"];
const APPROVAL_MODES: readonly ApprovalMode[] = ["Ask", "SemiAuto", "FullAuto"];
const ROUTING_MODES: readonly RoutingMode[] = ["Manual", "Auto"];

interface Draft {
  manager: ManagerConfig;
  routing: RoutingConfig;
  budgetText: string;
  timeText: string;
  parallelText: string;
}

function toDraft(manager: ManagerConfig, routing: RoutingConfig): Draft {
  return {
    manager,
    routing,
    budgetText: manager.budgetUsd == null ? "" : String(manager.budgetUsd),
    timeText: manager.timeLimitMins == null ? "" : String(manager.timeLimitMins),
    parallelText: String(manager.maxParallelAgents),
  };
}

export function ManagerSettings() {
  const connected = useAppStore((s) => s.connection.state === "open");
  const { providers } = useProvidersLive();
  const [saved, setSaved] = useState<Draft | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [pendingFallback, setPendingFallback] = useState<ModelRef | null>(null);
  const { t } = useT(["providers", "common"]);

  const load = useCallback(async () => {
    setLoadError(null);
    try {
      const [m, r] = await Promise.all([getManagerConfig(), getRoutingConfig()]);
      const d = toDraft(m, r);
      setSaved(d);
      setDraft(d);
    } catch (e) {
      setLoadError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    if (connected && !saved) void load();
  }, [connected, saved, load]);

  const dirty = useMemo(() => saved !== null && draft !== null && JSON.stringify(saved) !== JSON.stringify(draft), [saved, draft]);

  if (!connected && !draft) {
    return <EmptyState icon={<Sparkles size={22} />} title={t("connecting")} />;
  }
  if (loadError) {
    return (
      <EmptyState
        icon={<X size={22} />}
        title={t("manager.loadFailed")}
        description={loadError}
        action={
          <Button variant="primary" leftIcon={<RefreshCw size={14} />} onClick={() => void load()}>
            {t("common:actions.retry")}
          </Button>
        }
      />
    );
  }
  if (!draft) {
    return (
      <div className="pv" aria-busy="true">
        <Spinner label={t("manager.loading")} />
      </div>
    );
  }

  const m = draft.manager;
  const r = draft.routing;
  const setManager = (patch: Partial<ManagerConfig>) => setDraft({ ...draft, manager: { ...m, ...patch } });
  const setRouting = (patch: Partial<RoutingConfig>) => setDraft({ ...draft, routing: { ...r, ...patch } });

  const save = async () => {
    const budgetUsd = parseOptionalNumber(draft.budgetText);
    const timeLimitMins = parseOptionalNumber(draft.timeText);
    const maxParallelAgents = Number(draft.parallelText);
    const manager: ManagerConfig = { ...m, budgetUsd, timeLimitMins, maxParallelAgents };
    const invalid = validateManagerConfig(manager);
    if (invalid) {
      setMsg({ ok: false, text: invalid });
      return;
    }
    if (r.rules.some((rule) => !rule.model.model.trim())) {
      setMsg({ ok: false, text: t("validation.routingModel") });
      return;
    }
    setSaving(true);
    setMsg(null);
    try {
      const [sm, sr] = await Promise.all([saveManagerConfig(manager), saveRoutingConfig(r)]);
      const d = toDraft(sm, sr);
      setSaved(d);
      setDraft(d);
      setMsg({ ok: true, text: t("manager.saved") });
    } catch (e) {
      setMsg({ ok: false, text: errorMessage(e) });
    } finally {
      setSaving(false);
    }
  };

  const noProviders = providers.length === 0;
  return (
    <div className="pv">
      <p className="pv__intro">{t("manager.intro")}</p>
      <ClaudeEngineCard onApplied={() => void load()} />
      {noProviders && <p className="pv-note pv-note--warn">{t("manager.noProviders")}</p>}

      <section className="pv-section" aria-labelledby="pv-mgr-model">
        <h3 id="pv-mgr-model" className="pv-section__title">
          <Sparkles size={16} /> {t("manager.modelTitle")}
        </h3>
        <ModelPicker label={t("manager.managerLabel")} value={m.model} providers={providers} allowNone onChange={(model) => setManager({ model })} />
        <div className="pv-fields">
          <Select label={t("manager.reasoning")} value={m.reasoning} onChange={(e) => setManager({ reasoning: e.target.value as ReasoningLevel })}>
            {REASONING_LEVELS.map((level) => (
              <option key={level} value={level}>
                {t(`manager.reasoningLevels.${level}`)}
              </option>
            ))}
          </Select>
          <Select label={t("manager.contextPolicy")} value={m.contextPolicy} onChange={(e) => setManager({ contextPolicy: e.target.value as ContextPolicy })}>
            {CONTEXT_POLICIES.map((policy) => (
              <option key={policy} value={policy}>
                {t(`manager.contextPolicies.${policy}`)}
              </option>
            ))}
          </Select>
          <Select
            label={t("manager.approvalPolicy")}
            value={m.approvalPolicy}
            onChange={(e) => setManager({ approvalPolicy: e.target.value as ApprovalMode })}
            hint={t("manager.approvalHint")}
          >
            {APPROVAL_MODES.map((mode) => (
              <option key={mode} value={mode}>
                {t(`common:status.approvalMode.${mode}`)}
              </option>
            ))}
          </Select>
          <Input label={t("manager.maxParallel")} inputMode="numeric" value={draft.parallelText} onChange={(e) => setDraft({ ...draft, parallelText: e.target.value })} />
          <Input
            label={t("manager.budget")}
            inputMode="decimal"
            placeholder={t("manager.budgetPlaceholder")}
            value={draft.budgetText}
            onChange={(e) => setDraft({ ...draft, budgetText: e.target.value })}
          />
          <Input
            label={t("manager.timeLimit")}
            inputMode="numeric"
            placeholder={t("manager.timeLimitPlaceholder")}
            value={draft.timeText}
            onChange={(e) => setDraft({ ...draft, timeText: e.target.value })}
          />
        </div>
        <TextArea
          label={t("manager.instructions")}
          placeholder={t("manager.instructionsPlaceholder")}
          value={m.systemInstructions}
          maxLength={20000}
          onChange={(e) => setManager({ systemInstructions: e.target.value })}
        />
      </section>

      <section className="pv-section" aria-labelledby="pv-mgr-fallback">
        <h3 id="pv-mgr-fallback" className="pv-section__title">
          <Workflow size={16} /> {t("manager.fallbackTitle")}
        </h3>
        <p className="pv-section__hint">{t("manager.fallbackHint")}</p>
        {m.fallbackChain.length === 0 ? (
          <p className="pv-note">{t("manager.noFallbacks")}</p>
        ) : (
          <ol className="pv-chain" aria-label={t("manager.fallbackOrder")}>
            {m.fallbackChain.map((ref, i) => (
              <li key={`${refKey(ref)}-${i}`} className="pv-chain__item">
                <span className="pv-chain__pos" aria-hidden="true">
                  {i + 1}
                </span>
                <span className="pv-chain__label">{refLabel(ref, providers)}</span>
                <IconButton label={t("manager.moveUp", { model: ref.model })} size="sm" icon={<ArrowUp size={14} />} disabled={i === 0} onClick={() => setManager({ fallbackChain: moveItem(m.fallbackChain, i, i - 1) })} />
                <IconButton
                  label={t("manager.moveDown", { model: ref.model })}
                  size="sm"
                  icon={<ArrowDown size={14} />}
                  disabled={i === m.fallbackChain.length - 1}
                  onClick={() => setManager({ fallbackChain: moveItem(m.fallbackChain, i, i + 1) })}
                />
                <IconButton label={t("manager.remove", { model: ref.model })} size="sm" variant="danger" icon={<X size={14} />} onClick={() => setManager({ fallbackChain: m.fallbackChain.filter((_, j) => j !== i) })} />
              </li>
            ))}
          </ol>
        )}
        <ModelPicker label={t("manager.addFallback")} value={pendingFallback} providers={providers} allowNone onChange={setPendingFallback} />
        <div>
          <Button
            size="sm"
            leftIcon={<Plus size={14} />}
            disabled={!pendingFallback?.model.trim()}
            onClick={() => {
              if (!pendingFallback) return;
              setManager({ fallbackChain: [...m.fallbackChain, { ...pendingFallback, model: pendingFallback.model.trim() }] });
              setPendingFallback(null);
            }}
          >
            {t("manager.addToChain")}
          </Button>
        </div>
      </section>

      <section className="pv-section" aria-labelledby="pv-mgr-routing">
        <h3 id="pv-mgr-routing" className="pv-section__title">
          <Route size={16} /> {t("manager.routingTitle")}
        </h3>
        <div className="pv-fields">
          <Select
            label={t("manager.routingMode")}
            value={r.mode}
            onChange={(e) => setRouting({ mode: e.target.value as RoutingMode })}
            hint={r.mode === "Auto" ? t("manager.routingAutoHint") : t("manager.routingManualHint")}
          >
            {ROUTING_MODES.map((mode) => (
              <option key={mode} value={mode}>
                {t(`manager.routingModes.${mode}`)}
              </option>
            ))}
          </Select>
          <Input
            label={t("manager.maxWait")}
            inputMode="numeric"
            value={String(r.maxRateLimitWaitSecs)}
            onChange={(e) => {
              const n = Number(e.target.value);
              if (Number.isInteger(n) && n >= 0 && n <= 3600) setRouting({ maxRateLimitWaitSecs: n });
            }}
          />
          <div className="ok-field" style={{ justifyContent: "flex-end" }}>
            <Switch checked={r.preferLocal} label={t("manager.preferLocal")} onChange={(preferLocal) => setRouting({ preferLocal })} />
          </div>
        </div>
        {r.rules.length === 0 ? (
          <p className="pv-note">{r.mode === "Auto" ? t("manager.noRulesAuto") : t("manager.noRules")}</p>
        ) : (
          <table className="pv-table">
            <thead>
              <tr>
                <th scope="col">{t("manager.colPurpose")}</th>
                <th scope="col">{t("manager.colModel")}</th>
                <th scope="col">
                  <span className="pv-sr">{t("manager.colActions")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {r.rules.map((rule, i) => (
                <tr key={`${rule.purpose}-${i}`}>
                  <td style={{ width: "28%" }}>
                    <Select
                      label={<span className="pv-sr">{t("manager.rulePurpose", { index: i + 1 })}</span>}
                      value={rule.purpose}
                      onChange={(e) => setRouting({ rules: r.rules.map((x, j) => (j === i ? { ...x, purpose: e.target.value } : x)) })}
                    >
                      {[...new Set([rule.purpose, ...ROUTING_PURPOSES])].map((p) => (
                        <option key={p} value={p} disabled={p !== rule.purpose && r.rules.some((x) => x.purpose === p)}>
                          {purposeLabel(p)}
                        </option>
                      ))}
                    </Select>
                  </td>
                  <td>
                    <ModelPicker
                      label={t("manager.rule", { index: i + 1 })}
                      value={rule.model}
                      providers={providers}
                      onChange={(model) => model && setRouting({ rules: r.rules.map((x, j) => (j === i ? { ...x, model } : x)) })}
                    />
                  </td>
                  <td style={{ width: 40 }}>
                    <IconButton label={t("manager.removeRule", { purpose: purposeLabel(rule.purpose) })} size="sm" variant="danger" icon={<X size={14} />} onClick={() => setRouting({ rules: r.rules.filter((_, j) => j !== i) })} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <div>
          <Button
            size="sm"
            leftIcon={<Plus size={14} />}
            disabled={noProviders}
            onClick={() => {
              const purpose = ROUTING_PURPOSES.find((p) => !r.rules.some((x) => x.purpose === p)) ?? `Custom ${r.rules.length + 1}`;
              const first = providers.find((p) => p.provider.enabled) ?? providers[0];
              if (!first) return;
              setRouting({ rules: [...r.rules, { purpose, model: { providerId: first.provider.id, model: first.provider.defaultModel ?? "" }, fallback: [] }] });
            }}
          >
            {t("manager.addRule")}
          </Button>
        </div>
      </section>

      <div className="pv-savebar">
        {msg && (
          <span className="pv-savebar__msg" role={msg.ok ? "status" : "alert"}>
            {msg.text}
          </span>
        )}
        {!msg && dirty && <span className="pv-savebar__msg">{t("manager.unsaved")}</span>}
        <Button variant="ghost" leftIcon={<Undo2 size={14} />} disabled={!dirty || saving} onClick={() => saved && setDraft(saved)}>
          {t("manager.revert")}
        </Button>
        <Button variant="primary" leftIcon={<Save size={14} />} disabled={!dirty} loading={saving} onClick={() => void save()}>
          {t("common:actions.save")}
        </Button>
      </div>
    </div>
  );
}
