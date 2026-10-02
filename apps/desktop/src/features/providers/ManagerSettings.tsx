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

import { useAppStore } from "../../store/app";
import { Select, Switch, TextArea } from "./controls";
import { moveItem, parseOptionalNumber, refKey, refLabel, ROUTING_PURPOSES, validateManagerConfig } from "./logic";
import { ModelPicker } from "./ModelPicker";
import { errorMessage, getManagerConfig, getRoutingConfig, saveManagerConfig, saveRoutingConfig, useProvidersLive } from "./store";
import "./providers.css";

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
    return <EmptyState icon={<Sparkles size={22} />} title="Connecting to the Orkestr core…" />;
  }
  if (loadError) {
    return (
      <EmptyState
        icon={<X size={22} />}
        title="Could not load Manager settings"
        description={loadError}
        action={
          <Button variant="primary" leftIcon={<RefreshCw size={14} />} onClick={() => void load()}>
            Retry
          </Button>
        }
      />
    );
  }
  if (!draft) {
    return (
      <div className="pv" aria-busy="true">
        <Spinner label="Loading Manager settings" />
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
      setMsg({ ok: false, text: "Every routing rule needs a model." });
      return;
    }
    setSaving(true);
    setMsg(null);
    try {
      const [sm, sr] = await Promise.all([saveManagerConfig(manager), saveRoutingConfig(r)]);
      const d = toDraft(sm, sr);
      setSaved(d);
      setDraft(d);
      setMsg({ ok: true, text: "Saved." });
    } catch (e) {
      setMsg({ ok: false, text: errorMessage(e) });
    } finally {
      setSaving(false);
    }
  };

  const noProviders = providers.length === 0;
  return (
    <div className="pv">
      <p className="pv__intro">
        The AI Manager plans work, spawns agents and reviews results. Pick its model, limits and what happens when a provider is rate-limited or out of
        quota. Model switches are always announced (§7).
      </p>
      {noProviders && <p className="pv-note pv-note--warn">No providers yet — add one under “AI Providers” first.</p>}

      <section className="pv-section" aria-labelledby="pv-mgr-model">
        <h3 id="pv-mgr-model" className="pv-section__title">
          <Sparkles size={16} /> Orchestration model
        </h3>
        <ModelPicker label="Manager" value={m.model} providers={providers} allowNone onChange={(model) => setManager({ model })} />
        <div className="pv-fields">
          <Select label="Reasoning level" value={m.reasoning} onChange={(e) => setManager({ reasoning: e.target.value as ReasoningLevel })}>
            <option value="Low">Low — fastest</option>
            <option value="Medium">Medium</option>
            <option value="High">High — deepest</option>
          </Select>
          <Select label="Context policy" value={m.contextPolicy} onChange={(e) => setManager({ contextPolicy: e.target.value as ContextPolicy })}>
            <option value="Minimal">Minimal</option>
            <option value="Balanced">Balanced</option>
            <option value="Full">Full</option>
          </Select>
          <Select label="Approval policy" value={m.approvalPolicy} onChange={(e) => setManager({ approvalPolicy: e.target.value as ApprovalMode })} hint="Dangerous actions always ask.">
            <option value="Ask">Ask</option>
            <option value="SemiAuto">Semi-auto</option>
            <option value="FullAuto">Full auto</option>
          </Select>
          <Input label="Max parallel agents" inputMode="numeric" value={draft.parallelText} onChange={(e) => setDraft({ ...draft, parallelText: e.target.value })} />
          <Input label="Budget per run (USD)" inputMode="decimal" placeholder="unrestricted" value={draft.budgetText} onChange={(e) => setDraft({ ...draft, budgetText: e.target.value })} />
          <Input label="Time limit (minutes)" inputMode="numeric" placeholder="none" value={draft.timeText} onChange={(e) => setDraft({ ...draft, timeText: e.target.value })} />
        </div>
        <TextArea
          label="System instructions"
          placeholder="Extra guidance for the Manager, e.g. “Prefer small PRs and always add tests.”"
          value={m.systemInstructions}
          maxLength={20000}
          onChange={(e) => setManager({ systemInstructions: e.target.value })}
        />
      </section>

      <section className="pv-section" aria-labelledby="pv-mgr-fallback">
        <h3 id="pv-mgr-fallback" className="pv-section__title">
          <Workflow size={16} /> Fallback chain
        </h3>
        <p className="pv-section__hint">Tried in order when the main model is rate-limited, out of quota or failing, with a structured handoff.</p>
        {m.fallbackChain.length === 0 ? (
          <p className="pv-note">No fallbacks — failures stop the run and ask you.</p>
        ) : (
          <ol className="pv-chain" aria-label="Fallback order">
            {m.fallbackChain.map((ref, i) => (
              <li key={`${refKey(ref)}-${i}`} className="pv-chain__item">
                <span className="pv-chain__pos" aria-hidden="true">
                  {i + 1}
                </span>
                <span className="pv-chain__label">{refLabel(ref, providers)}</span>
                <IconButton label={`Move ${ref.model} up`} size="sm" icon={<ArrowUp size={14} />} disabled={i === 0} onClick={() => setManager({ fallbackChain: moveItem(m.fallbackChain, i, i - 1) })} />
                <IconButton
                  label={`Move ${ref.model} down`}
                  size="sm"
                  icon={<ArrowDown size={14} />}
                  disabled={i === m.fallbackChain.length - 1}
                  onClick={() => setManager({ fallbackChain: moveItem(m.fallbackChain, i, i + 1) })}
                />
                <IconButton label={`Remove ${ref.model}`} size="sm" variant="danger" icon={<X size={14} />} onClick={() => setManager({ fallbackChain: m.fallbackChain.filter((_, j) => j !== i) })} />
              </li>
            ))}
          </ol>
        )}
        <ModelPicker label="Add fallback" value={pendingFallback} providers={providers} allowNone onChange={setPendingFallback} />
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
            Add to chain
          </Button>
        </div>
      </section>

      <section className="pv-section" aria-labelledby="pv-mgr-routing">
        <h3 id="pv-mgr-routing" className="pv-section__title">
          <Route size={16} /> Routing
        </h3>
        <div className="pv-fields">
          <Select label="Mode" value={r.mode} onChange={(e) => setRouting({ mode: e.target.value as RoutingMode })} hint={r.mode === "Auto" ? "Scores capability, cost, latency and limits; rules get a boost." : "Uses the table below exactly."}>
            <option value="Manual">Manual</option>
            <option value="Auto">Automatic</option>
          </Select>
          <Input
            label="Max rate-limit wait (s)"
            inputMode="numeric"
            value={String(r.maxRateLimitWaitSecs)}
            onChange={(e) => {
              const n = Number(e.target.value);
              if (Number.isInteger(n) && n >= 0 && n <= 3600) setRouting({ maxRateLimitWaitSecs: n });
            }}
          />
          <div className="ok-field" style={{ justifyContent: "flex-end" }}>
            <Switch checked={r.preferLocal} label="Prefer local models" onChange={(preferLocal) => setRouting({ preferLocal })} />
          </div>
        </div>
        {r.rules.length === 0 ? (
          <p className="pv-note">No routing rules — every role uses the Manager’s choices{r.mode === "Auto" ? " and automatic scoring" : ""}.</p>
        ) : (
          <table className="pv-table">
            <thead>
              <tr>
                <th scope="col">Role / task</th>
                <th scope="col">Model</th>
                <th scope="col">
                  <span className="pv-sr">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {r.rules.map((rule, i) => (
                <tr key={`${rule.purpose}-${i}`}>
                  <td style={{ width: "28%" }}>
                    <Select
                      label={<span className="pv-sr">Purpose of rule {i + 1}</span>}
                      value={rule.purpose}
                      onChange={(e) => setRouting({ rules: r.rules.map((x, j) => (j === i ? { ...x, purpose: e.target.value } : x)) })}
                    >
                      {[...new Set([rule.purpose, ...ROUTING_PURPOSES])].map((p) => (
                        <option key={p} value={p} disabled={p !== rule.purpose && r.rules.some((x) => x.purpose === p)}>
                          {p}
                        </option>
                      ))}
                    </Select>
                  </td>
                  <td>
                    <ModelPicker
                      label={`Rule ${i + 1}`}
                      value={rule.model}
                      providers={providers}
                      onChange={(model) => model && setRouting({ rules: r.rules.map((x, j) => (j === i ? { ...x, model } : x)) })}
                    />
                  </td>
                  <td style={{ width: 40 }}>
                    <IconButton label={`Remove rule ${rule.purpose}`} size="sm" variant="danger" icon={<X size={14} />} onClick={() => setRouting({ rules: r.rules.filter((_, j) => j !== i) })} />
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
            Add routing rule
          </Button>
        </div>
      </section>

      <div className="pv-savebar">
        {msg && (
          <span className="pv-savebar__msg" role={msg.ok ? "status" : "alert"}>
            {msg.text}
          </span>
        )}
        {!msg && dirty && <span className="pv-savebar__msg">Unsaved changes</span>}
        <Button variant="ghost" leftIcon={<Undo2 size={14} />} disabled={!dirty || saving} onClick={() => saved && setDraft(saved)}>
          Revert
        </Button>
        <Button variant="primary" leftIcon={<Save size={14} />} disabled={!dirty} loading={saving} onClick={() => void save()}>
          Save
        </Button>
      </div>
    </div>
  );
}
