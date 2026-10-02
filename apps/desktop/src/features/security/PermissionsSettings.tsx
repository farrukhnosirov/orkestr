import type {
  ApprovalMode,
  Capability,
  CommandShell,
  DecisionView,
  ModeResult,
  PermissionEffect,
  PermissionRuleRecord,
  PermissionScope,
  ParamsOf,
} from "@orkestr/protocol";
import { Badge, Button, EmptyState, Input, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { FlaskConical, Plus, ShieldAlert, Trash2 } from "lucide-react";
import { type FormEvent, useCallback, useEffect, useState } from "react";
import { Trans } from "react-i18next";

import { maybeClient } from "../../core/client";
import { useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import type { SettingsSectionProps } from "../settings/sections";
import {
  APPROVAL_MODE_IDS,
  approvalModeDescription,
  approvalModeLabel,
  CAPABILITIES,
  capabilityLabel,
  dangerLabel,
  EFFECTS,
  effectLabel,
  effectTone,
  riskLabel,
  riskTone,
  scopeLabel,
  sortRules,
} from "./logic";
import { errorMessage, refreshFlags, useSecurityStore } from "./store";

function useClientReady(): boolean {
  return useAppStore((s) => s.connection.state === "open");
}

function ModePicker({ projectId }: { projectId: string }) {
  const ready = useClientReady();
  const [mode, setMode] = useState<ModeResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<ApprovalMode | null>(null);
  const { t } = useT("security");

  const load = useCallback(async () => {
    const c = maybeClient();
    if (!c) return;
    try {
      setMode(await c.call("permissions.getMode", { projectId }));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [projectId]);

  useEffect(() => {
    if (ready) void load();
  }, [ready, load]);

  const choose = async (m: ApprovalMode) => {
    const c = maybeClient();
    if (!c) return;
    setBusy(m);
    setError(null);
    try {
      setMode(await c.call("permissions.setMode", { projectId, mode: m }));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  if (!mode && !error) return <Spinner label={t("permissions.modeLoading")} />;
  return (
    <div className="sec-modes">
      <div role="radiogroup" aria-label={t("permissions.modeTitle")} className="sec-modes__group">
        {APPROVAL_MODE_IDS.map((id) => {
          const selected = mode?.projectMode === id;
          return (
            <button
              key={id}
              type="button"
              role="radio"
              aria-checked={selected}
              className={clsx("sec-mode", selected && "sec-mode--selected")}
              disabled={busy !== null}
              onClick={() => void choose(id)}
            >
              <span className="sec-mode__label">
                {approvalModeLabel(id)} {busy === id && <Spinner size={12} />}
              </span>
              <span className="sec-mode__desc">{approvalModeDescription(id)}</span>
            </button>
          );
        })}
      </div>
      {mode && mode.effectiveMode !== mode.projectMode && (
        <p className="sec-hint">
          <ShieldAlert size={13} />{" "}
          <Trans
            t={t}
            i18nKey="permissions.effectiveMode"
            values={{ mode: approvalModeLabel(mode.effectiveMode) }}
            components={{ strong: <strong /> }}
          />
        </p>
      )}
      {error && (
        <p className="sec-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

function EmergencyToggles() {
  const flags = useSecurityStore((s) => s.flags);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { t } = useT("security");
  const toggle = async (which: "emergency.disableFullAuto" | "emergency.disableNetworkTools", disabled: boolean) => {
    const c = maybeClient();
    if (!c) return;
    setBusy(which);
    setError(null);
    try {
      const r = await c.call(which, { disabled });
      useSecurityStore.getState().setFlags(r.flags);
    } catch (e) {
      setError(errorMessage(e));
      void refreshFlags();
    } finally {
      setBusy(null);
    }
  };
  return (
    <div className="sec-toggles">
      <label className="sec-toggle">
        <input
          type="checkbox"
          checked={flags.fullAutoDisabled}
          disabled={busy !== null}
          onChange={(e) => void toggle("emergency.disableFullAuto", e.target.checked)}
        />
        <span>
          <strong>{t("permissions.disableFullAuto")}</strong>
          <small>{t("permissions.disableFullAutoHint")}</small>
        </span>
      </label>
      <label className="sec-toggle">
        <input
          type="checkbox"
          checked={flags.networkToolsDisabled}
          disabled={busy !== null}
          onChange={(e) => void toggle("emergency.disableNetworkTools", e.target.checked)}
        />
        <span>
          <strong>{t("permissions.disableNetwork")}</strong>
          <small>{t("permissions.disableNetworkHint")}</small>
        </span>
      </label>
      {error && (
        <p className="sec-error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

function RuleForm({ projectId, onSaved }: { projectId: string | null; onSaved: () => void }) {
  const [capability, setCapability] = useState<Capability>("Terminal");
  const [effect, setEffect] = useState<PermissionEffect>("Allow");
  const [scopeKind, setScopeKind] = useState<"Global" | "Project">(projectId ? "Project" : "Global");
  const [resource, setResource] = useState("");
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { t } = useT("security");

  useEffect(() => {
    if (!projectId) setScopeKind("Global");
  }, [projectId]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const c = maybeClient();
    if (!c) return;
    const scope: PermissionScope = scopeKind === "Project" && projectId ? { scope: "Project", projectId } : { scope: "Global" };
    setBusy(true);
    setError(null);
    try {
      const params: { rule: { capability: Capability; effect: PermissionEffect; resource: string | null; scope: PermissionScope }; note?: string } = {
        rule: { capability, effect, resource: resource.trim() || null, scope },
      };
      if (note.trim()) params.note = note.trim();
      await c.call("permissions.set", params);
      setResource("");
      setNote("");
      onSaved();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="sec-rule-form" onSubmit={(e) => void submit(e)} aria-label={t("permissions.addRuleAria")}>
      <label className="sec-select">
        <span>{t("permissions.capability")}</span>
        <select value={capability} onChange={(e) => setCapability(e.target.value as Capability)}>
          {CAPABILITIES.map((c) => (
            <option key={c} value={c}>
              {capabilityLabel(c)}
            </option>
          ))}
        </select>
      </label>
      <label className="sec-select">
        <span>{t("permissions.effect")}</span>
        <select value={effect} onChange={(e) => setEffect(e.target.value as PermissionEffect)}>
          {EFFECTS.map((x) => (
            <option key={x} value={x}>
              {effectLabel(x)}
            </option>
          ))}
        </select>
      </label>
      <label className="sec-select">
        <span>{t("permissions.scope")}</span>
        <select value={scopeKind} onChange={(e) => setScopeKind(e.target.value as "Global" | "Project")}>
          <option value="Global">{t("scope.global")}</option>
          <option value="Project" disabled={!projectId}>
            {t("scope.thisProject")}
          </option>
        </select>
      </label>
      <Input
        className="sec-rule-form__grow"
        label={t("permissions.resourcePattern")}
        placeholder={t("permissions.resourcePlaceholder")}
        value={resource}
        maxLength={1024}
        onChange={(e) => setResource(e.target.value)}
        hint={t("permissions.resourceHint")}
      />
      <Input className="sec-rule-form__grow" label={t("permissions.note")} value={note} maxLength={500} onChange={(e) => setNote(e.target.value)} />
      <Button type="submit" variant="primary" leftIcon={<Plus size={14} />} loading={busy}>
        {t("permissions.addRule")}
      </Button>
      {error && (
        <p className="sec-error sec-rule-form__full" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

function RulesTable({ projectId }: { projectId: string | null }) {
  const ready = useClientReady();
  const projects = useAppStore((s) => s.projects);
  const [rules, setRules] = useState<PermissionRuleRecord[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const { t } = useT(["security", "common"]);

  const load = useCallback(async () => {
    const c = maybeClient();
    if (!c) return;
    try {
      const r = await c.call("permissions.list", projectId ? { projectId } : {});
      setRules(sortRules(r.rules));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [projectId]);

  useEffect(() => {
    if (ready) void load();
  }, [ready, load]);

  const remove = async (id: string) => {
    const c = maybeClient();
    if (!c) return;
    setDeleting(id);
    try {
      await c.call("permissions.delete", { id });
      await load();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setDeleting(null);
    }
  };

  const projectName = (id: string) => projects.find((p) => p.id === id)?.name;

  return (
    <div className="sec-rules">
      <RuleForm projectId={projectId} onSaved={() => void load()} />
      {error && (
        <p className="sec-error" role="alert">
          {error}
        </p>
      )}
      {rules === null && !error ? (
        <Spinner label={t("permissions.rulesLoading")} />
      ) : rules && rules.length === 0 ? (
        <EmptyState
          title={t("permissions.noRulesTitle")}
          description={t("permissions.noRulesDescription")}
        />
      ) : rules ? (
        <table className="sec-table">
          <thead>
            <tr>
              <th scope="col">{t("permissions.scope")}</th>
              <th scope="col">{t("permissions.capability")}</th>
              <th scope="col">{t("permissions.effect")}</th>
              <th scope="col">{t("permissions.resource")}</th>
              <th scope="col">{t("permissions.note")}</th>
              <th scope="col">
                <span className="sr-only">{t("permissions.actions")}</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {rules.map((r) => (
              <tr key={r.id}>
                <td>{scopeLabel(r.rule.scope, projectName)}</td>
                <td>{capabilityLabel(r.rule.capability)}</td>
                <td>
                  <Badge tone={effectTone(r.rule.effect)}>{effectLabel(r.rule.effect)}</Badge>
                </td>
                <td>
                  <code>{r.rule.resource ?? "*"}</code>
                </td>
                <td className="sec-muted">{r.note ?? ""}</td>
                <td>
                  <Button
                    variant="ghost"
                    size="sm"
                    aria-label={t("permissions.deleteRule", {
                      capability: capabilityLabel(r.rule.capability),
                      effect: effectLabel(r.rule.effect),
                    })}
                    loading={deleting === r.id}
                    leftIcon={<Trash2 size={13} />}
                    onClick={() => void remove(r.id)}
                  >
                    {t("common:actions.delete")}
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
    </div>
  );
}

function CheckTool({ projectId }: { projectId: string | null }) {
  const [command, setCommand] = useState("");
  const [shell, setShell] = useState<CommandShell>("PowerShell");
  const [result, setResult] = useState<DecisionView | null>(null);
  const { t } = useT("security");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (e: FormEvent) => {
    e.preventDefault();
    const c = maybeClient();
    if (!c || !command.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const params: ParamsOf<"permissions.check"> = {
        capability: "Terminal",
        resource: { type: "command", command, shell, cwd: null },
      };
      if (projectId) params.projectId = projectId;
      setResult(await c.call("permissions.check", params));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="sec-check" onSubmit={(e) => void run(e)} aria-label={t("permissions.checkAria")}>
      <Input
        className="sec-rule-form__grow"
        label={t("permissions.checkLabel")}
        placeholder={t("permissions.checkPlaceholder")}
        value={command}
        onChange={(e) => setCommand(e.target.value)}
      />
      <label className="sec-select">
        <span>{t("permissions.shell")}</span>
        <select value={shell} onChange={(e) => setShell(e.target.value as CommandShell)}>
          {(["PowerShell", "Cmd", "Bash", "Sh"] as const).map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
      </label>
      <Button type="submit" leftIcon={<FlaskConical size={14} />} loading={busy} disabled={!command.trim()}>
        {t("permissions.check")}
      </Button>
      {result && (
        <div className="sec-check__result sec-rule-form__full" role="status">
          <Badge tone={result.decision === "Allow" ? "success" : result.decision === "Ask" ? "warning" : "danger"}>
            {t(`decision.${result.decision}`)}
          </Badge>
          {result.risk && <Badge tone={riskTone(result.risk)}>{riskLabel(result.risk)}</Badge>}
          {result.danger && <Badge tone="danger">{dangerLabel(result.danger)}</Badge>}
          <span className="sec-muted">
            {t("permissions.checkResult", { message: result.message, mode: approvalModeLabel(result.effectiveMode) })}
          </span>
        </div>
      )}
      {error && (
        <p className="sec-error sec-rule-form__full" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

/** Settings section "Security & Permissions". */
export function PermissionsSettings({ projectId }: SettingsSectionProps) {
  const project = useAppStore((s) => s.projects.find((p) => p.id === projectId));
  const { t } = useT("security");
  return (
    <div className="sec-section">
      <section aria-labelledby="sec-mode-h">
        <h3 id="sec-mode-h" className="sec-h">
          {t("permissions.modeTitle")} {project ? <span className="sec-muted">· {project.name}</span> : null}
        </h3>
        {projectId ? <ModePicker projectId={projectId} /> : <p className="sec-muted">{t("permissions.openProject")}</p>}
      </section>
      <section aria-labelledby="sec-em-h">
        <h3 id="sec-em-h" className="sec-h">
          {t("permissions.emergencyTitle")}
        </h3>
        <EmergencyToggles />
      </section>
      <section aria-labelledby="sec-rules-h">
        <h3 id="sec-rules-h" className="sec-h">
          {t("permissions.rulesTitle")}
        </h3>
        <p className="sec-muted">{t("permissions.rulesHint")}</p>
        <RulesTable projectId={projectId} />
      </section>
      <section aria-labelledby="sec-check-h">
        <h3 id="sec-check-h" className="sec-h">
          {t("permissions.guardTitle")}
        </h3>
        <CheckTool projectId={projectId} />
      </section>
    </div>
  );
}
