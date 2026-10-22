/**
 * Alert rules editor and log sources manager of the infrastructure monitor
 * (ADR-0042): `monitor.rules` / `ruleUpsert` / `ruleDelete` and
 * `monitor.logSources` / `sourceAdd` / `sourceRemove`. Typed forms per rule
 * kind; ingest streams and deployment logs are listed read-only.
 */
import type { AlertRule, LogLevel, LogSourceInfo } from "@orkestr/protocol";
import { Badge, Button, IconButton, Input } from "@orkestr/ui";
import { clsx } from "clsx";
import { Pencil, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useState } from "react";

import { PathInput } from "../../../components/PathInput";
import { maybeClient } from "../../../core/client";
import { useFormat, useT } from "../../../i18n";
import { errorMessage, useRpcQuery } from "../../../lib/rpcQuery";
import { severityTone } from "./model";
import {
  draftFromRule,
  EMPTY_RULE,
  isRemovableSource,
  RULE_KINDS,
  RULE_METRICS,
  type RuleDraft,
  type RuleDraftError,
  ruleDraftToRule,
  SEVERITIES,
  type SourceDraftError,
  sourceDetail,
  sourceDraftToParams,
} from "./rulesModel";

const LEVELS: LogLevel[] = ["trace", "debug", "info", "warn", "error"];

function useAct(reload: () => void) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const act = async (key: string, f: () => Promise<unknown>) => {
    setBusy(key);
    setError(null);
    try {
      await f();
      reload();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };
  return { busy, error, act };
}

// ── Alert rules ──────────────────────────────────────────

export function RulesEditor() {
  const { t } = useT("monitorInfra");
  const list = useRpcQuery("monitor.rules", async (c) => (await c.call("monitor.rules", {})).rules);
  const [draft, setDraft] = useState<RuleDraft | null>(null);
  const { busy, error, act } = useAct(list.reload);

  const remove = (r: AlertRule) => {
    if (!globalThis.confirm(t("rules.deleteConfirm", { name: r.name }))) return;
    const c = maybeClient();
    if (c) void act(`del:${r.id}`, () => c.call("monitor.ruleDelete", { ruleId: r.id }));
  };
  const toggle = (r: AlertRule) => {
    const c = maybeClient();
    if (c) void act(`tog:${r.id}`, () => c.call("monitor.ruleUpsert", { rule: { ...r, enabled: !r.enabled } }));
  };

  return (
    <div className="infra-stack">
      <div className="infra-row">
        <Button size="sm" leftIcon={<Plus size={14} />} onClick={() => setDraft({ ...EMPTY_RULE })}>
          {t("rules.add")}
        </Button>
        <IconButton size="sm" label={t("rules.refresh")} tooltip onClick={list.reload} icon={<RefreshCw size={14} />} />
      </div>
      <p className="mon-muted">{t("rules.intro")}</p>
      {draft && (
        <RuleForm
          draft={draft}
          onCancel={() => setDraft(null)}
          onSaved={() => {
            setDraft(null);
            list.reload();
          }}
        />
      )}
      {(error ?? list.error) && <p className="mon-error">{error ?? list.error}</p>}
      {list.data?.length === 0 && <p className="mon-muted">{t("rules.empty")}</p>}
      <ul className="infra-list" aria-label={t("rules.title")}>
        {list.data?.map((r) => (
          <li key={r.id} className={clsx("infra-item", !r.enabled && "infra-item--muted")} data-testid={`rule-${r.id}`}>
            <div className="infra-row">
              <Badge tone={severityTone(r.severity)}>{t(`alerts.severity.${r.severity}`)}</Badge>
              <strong className="infra-ellipsis">{r.name}</strong>
              <span className="mon-muted mon-mono">{r.id}</span>
              <span className="infra-spacer" />
              <label className="infra-check">
                <input type="checkbox" checked={r.enabled} disabled={busy !== null} onChange={() => toggle(r)} />
                <span>{t("rules.enabled")}</span>
              </label>
              <IconButton size="sm" label={t("rules.edit")} tooltip onClick={() => setDraft(draftFromRule(r))} icon={<Pencil size={14} />} />
              <IconButton size="sm" label={t("rules.delete")} tooltip disabled={busy !== null} onClick={() => remove(r)} icon={<Trash2 size={14} />} />
            </div>
            <RuleSummary rule={r} />
          </li>
        ))}
      </ul>
    </div>
  );
}

function RuleSummary({ rule }: { rule: AlertRule }) {
  const { t } = useT("monitorInfra");
  const fmt = useFormat();
  const k = rule.kind;
  let text: string;
  switch (k.type) {
    case "CheckDown":
      text = k.checkId ? t("rules.summary.checkOne", { id: k.checkId.slice(0, 8) }) : t("rules.summary.checkAny");
      break;
    case "MetricThreshold":
      text = t("rules.summary.metric", { metric: k.metric, above: fmt.percent(k.above / 100), samples: k.samples });
      break;
    case "LogRate":
      text = t("rules.summary.log", {
        count: k.count,
        window: k.windowSecs,
        level: k.minLevel ? t(`logs.levels.${k.minLevel}`) : t("logs.anyLevel"),
        source: k.sourceId ?? t("logs.allSources"),
      });
      if (k.pattern) text = `${text} · ${t("rules.summary.pattern", { pattern: k.pattern })}`;
      break;
  }
  return <span className="mon-muted">{text}</span>;
}

function metricLabel(m: string, t: ReturnType<typeof useT<"monitorInfra">>["t"]): string {
  if (m === "cpu" || m === "mem" || m === "disk") return t(`rules.metric.${m}`);
  return t("rules.metric.gpu", { n: m.slice(3) });
}

function RuleForm({ draft, onCancel, onSaved }: { draft: RuleDraft; onCancel: () => void; onSaved: () => void }) {
  const { t } = useT("monitorInfra");
  const [d, setD] = useState<RuleDraft>(draft);
  const [invalid, setInvalid] = useState<RuleDraftError | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const set = (patch: Partial<RuleDraft>) => setD((prev) => ({ ...prev, ...patch }));
  const checks = useRpcQuery("monitor.checksList", async (c) => (await c.call("monitor.checksList", {})).checks);
  const sources = useRpcQuery("monitor.logSources", async (c) => (await c.call("monitor.logSources", {})).sources);
  const err = (k: RuleDraftError) => (invalid === k ? t(`rules.error.${k}`) : undefined);

  const save = async () => {
    const res = ruleDraftToRule(d);
    if ("error" in res) {
      setInvalid(res.error);
      return;
    }
    const c = maybeClient();
    if (!c) return;
    setInvalid(null);
    setSaving(true);
    try {
      await c.call("monitor.ruleUpsert", res);
      onSaved();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const title = d.editing ? t("rules.form.titleEdit") : t("rules.form.titleNew");
  return (
    <form
      className="infra-form"
      aria-label={title}
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <strong>{title}</strong>
      <div className="infra-grid">
        <Input label={t("rules.form.name")} value={d.name} onChange={(e) => set({ name: e.target.value })} error={err("name")} />
        <Input
          label={t("rules.form.id")}
          hint={d.editing ? undefined : t("rules.form.idHint")}
          value={d.id}
          disabled={d.editing !== null}
          onChange={(e) => set({ id: e.target.value })}
          error={err("id")}
        />
        <label className="infra-field">
          <span>{t("rules.form.severity")}</span>
          <select className="infra-select" value={d.severity} onChange={(e) => set({ severity: e.target.value as RuleDraft["severity"] })}>
            {SEVERITIES.map((s) => (
              <option key={s} value={s}>
                {t(`alerts.severity.${s}`)}
              </option>
            ))}
          </select>
        </label>
        <label className="infra-field">
          <span>{t("rules.form.kind")}</span>
          <select className="infra-select" value={d.kind} onChange={(e) => set({ kind: e.target.value as RuleDraft["kind"] })}>
            {RULE_KINDS.map((k) => (
              <option key={k} value={k}>
                {t(`rules.kind.${k}`)}
              </option>
            ))}
          </select>
        </label>
        {d.kind === "CheckDown" && (
          <label className="infra-field">
            <span>{t("rules.form.check")}</span>
            <select className="infra-select" value={d.checkId} onChange={(e) => set({ checkId: e.target.value })}>
              <option value="">{t("rules.form.anyCheck")}</option>
              {checks.data?.map((c) => (
                <option key={c.check.id} value={c.check.id}>
                  {c.check.name}
                </option>
              ))}
            </select>
          </label>
        )}
        {d.kind === "MetricThreshold" && (
          <>
            <label className="infra-field">
              <span>{t("rules.form.metric")}</span>
              <select className="infra-select" value={d.metric} onChange={(e) => set({ metric: e.target.value })}>
                {(RULE_METRICS as readonly string[]).concat(RULE_METRICS.includes(d.metric as never) ? [] : [d.metric]).map((m) => (
                  <option key={m} value={m}>
                    {metricLabel(m, t)}
                  </option>
                ))}
              </select>
            </label>
            <Input label={t("rules.form.above")} hint={t("rules.form.aboveHint")} inputMode="decimal" value={d.above} onChange={(e) => set({ above: e.target.value })} error={err("above")} />
            <Input label={t("rules.form.samples")} hint={t("rules.form.samplesHint")} inputMode="numeric" value={d.samples} onChange={(e) => set({ samples: e.target.value })} error={err("samples")} />
            {invalid === "metric" && <p className="mon-error">{t("rules.error.metric")}</p>}
          </>
        )}
        {d.kind === "LogRate" && (
          <>
            <label className="infra-field">
              <span>{t("rules.form.source")}</span>
              <select className="infra-select" value={d.sourceId} onChange={(e) => set({ sourceId: e.target.value })}>
                <option value="">{t("logs.allSources")}</option>
                {sources.data?.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name}
                  </option>
                ))}
                {d.sourceId && !sources.data?.some((s) => s.id === d.sourceId) && <option value={d.sourceId}>{d.sourceId}</option>}
              </select>
            </label>
            <label className="infra-field">
              <span>{t("rules.form.minLevel")}</span>
              <select className="infra-select" value={d.minLevel} onChange={(e) => set({ minLevel: e.target.value as RuleDraft["minLevel"] })}>
                <option value="">{t("logs.anyLevel")}</option>
                {LEVELS.map((l) => (
                  <option key={l} value={l}>
                    {t(`logs.levels.${l}`)}
                  </option>
                ))}
              </select>
            </label>
            <Input label={t("rules.form.pattern")} hint={t("rules.form.patternHint")} value={d.pattern} onChange={(e) => set({ pattern: e.target.value })} error={err("pattern")} />
            <Input label={t("rules.form.count")} inputMode="numeric" value={d.count} onChange={(e) => set({ count: e.target.value })} error={err("count")} />
            <Input label={t("rules.form.window")} inputMode="numeric" value={d.windowSecs} onChange={(e) => set({ windowSecs: e.target.value })} error={err("window")} />
          </>
        )}
        <label className="infra-check">
          <input type="checkbox" checked={d.enabled} onChange={(e) => set({ enabled: e.target.checked })} />
          <span>{t("rules.enabled")}</span>
        </label>
      </div>
      {error && <p className="mon-error">{error}</p>}
      <div className="infra-row">
        <Button type="submit" variant="primary" size="sm" loading={saving}>
          {t("form.save")}
        </Button>
        <Button type="button" variant="ghost" size="sm" onClick={onCancel}>
          {t("form.cancel")}
        </Button>
      </div>
    </form>
  );
}

// ── Log sources ──────────────────────────────────────────

export function SourcesManager() {
  const { t } = useT("monitorInfra");
  const list = useRpcQuery("monitor.logSources", async (c) => (await c.call("monitor.logSources", {})).sources);
  const { busy, error, act } = useAct(list.reload);
  const [adding, setAdding] = useState(false);

  const remove = (s: LogSourceInfo) => {
    if (!globalThis.confirm(t("sources.deleteConfirm", { name: s.name }))) return;
    const c = maybeClient();
    if (c) void act(`del:${s.id}`, () => c.call("monitor.sourceRemove", { sourceId: s.id }));
  };

  return (
    <div className="infra-stack">
      <div className="infra-row">
        <Button size="sm" leftIcon={<Plus size={14} />} onClick={() => setAdding(true)}>
          {t("sources.add")}
        </Button>
        <IconButton size="sm" label={t("sources.refresh")} tooltip onClick={list.reload} icon={<RefreshCw size={14} />} />
      </div>
      {adding && (
        <SourceForm
          existing={list.data ?? []}
          onCancel={() => setAdding(false)}
          onSaved={() => {
            setAdding(false);
            list.reload();
          }}
        />
      )}
      {(error ?? list.error) && <p className="mon-error">{error ?? list.error}</p>}
      {list.data?.length === 0 && <p className="mon-muted">{t("logs.noSources")}</p>}
      <ul className="infra-list" aria-label={t("sources.title")}>
        {list.data?.map((s) => (
          <li key={s.id} className="infra-item" data-testid={`source-${s.id}`}>
            <div className="infra-row">
              <Badge tone={isRemovableSource(s) ? "info" : "neutral"}>{t(`sources.kind.${s.kind.type}`)}</Badge>
              <strong className="infra-ellipsis">{s.name}</strong>
              <span className="infra-spacer" />
              {isRemovableSource(s) ? (
                <IconButton size="sm" label={t("sources.remove")} tooltip disabled={busy !== null} onClick={() => remove(s)} icon={<Trash2 size={14} />} />
              ) : (
                <span className="mon-muted">{t("sources.builtIn")}</span>
              )}
            </div>
            <span className="mon-mono mon-muted infra-ellipsis">{sourceDetail(s)}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function SourceForm({ existing, onCancel, onSaved }: { existing: readonly LogSourceInfo[]; onCancel: () => void; onSaved: () => void }) {
  const { t } = useT("monitorInfra");
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [invalid, setInvalid] = useState<SourceDraftError | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const save = async () => {
    const res = sourceDraftToParams(name, path, existing);
    if ("error" in res) {
      setInvalid(res.error);
      return;
    }
    const c = maybeClient();
    if (!c) return;
    setInvalid(null);
    setSaving(true);
    try {
      await c.call("monitor.sourceAdd", res.params);
      onSaved();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <form
      className="infra-form"
      aria-label={t("sources.form.title")}
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <strong>{t("sources.form.title")}</strong>
      <div className="infra-grid">
        <PathInput
          label={t("sources.form.path")}
          hint={t("sources.form.pathHint")}
          kind="file"
          dialogTitle={t("sources.form.pick")}
          value={path}
          onPathChange={setPath}
          error={invalid === "path" ? t("sources.error.path") : invalid === "duplicate" ? t("sources.error.duplicate") : undefined}
        />
        <Input label={t("sources.form.name")} hint={t("sources.form.nameHint")} value={name} onChange={(e) => setName(e.target.value)} error={invalid === "name" ? t("sources.error.name") : undefined} />
      </div>
      {error && <p className="mon-error">{error}</p>}
      <div className="infra-row">
        <Button type="submit" variant="primary" size="sm" loading={saving}>
          {t("sources.form.add")}
        </Button>
        <Button type="button" variant="ghost" size="sm" onClick={onCancel}>
          {t("form.cancel")}
        </Button>
      </div>
    </form>
  );
}
