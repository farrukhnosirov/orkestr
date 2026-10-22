/**
 * Infrastructure monitor section of the Monitoring panel (ADR-0042, WP5):
 * node overview with sparklines, HTTP/TCP health checks (add / edit / run,
 * status history), alerts with acknowledge, and a log viewer with source /
 * level / text filters and a polling live tail.
 */
import type { Alert, LogLevel, LogLine, MetricSeries, MetricsParams, NodeMetricsSnapshot } from "@orkestr/protocol";
import { Badge, Button, IconButton, Input, Tabs } from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, Pause, Pencil, Play, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { maybeClient } from "../../../core/client";
import { useFormat, useT } from "../../../i18n";
import { errorMessage, useRpcQuery } from "../../../lib/rpcQuery";
import { Sparkline } from "../charts";
import {
  type CheckDraft,
  type CheckRun,
  type CheckStatusView,
  checkTone,
  draftFromCheck,
  draftToParams,
  type DraftError,
  EMPTY_DRAFT,
  lastTs,
  levelTone,
  mergeTail,
  probeSummary,
  type ProbeView,
  seriesValues,
  severityTone,
  snapshotPercents,
  upRatio,
} from "./model";
import "./infra.css";

type Section = "overview" | "checks" | "alerts" | "logs";
const TOPICS = ["global"];
const LEVELS: LogLevel[] = ["trace", "debug", "info", "warn", "error"];

export function InfraMonitor() {
  const { t } = useT("monitorInfra");
  const [section, setSection] = useState<Section>("overview");
  const tabs = (["overview", "checks", "alerts", "logs"] as const).map((id) => ({ id, label: t(`tabs.${id}`) }));
  return (
    <section className="mon-card infra" aria-label={t("title")}>
      <div className="infra-head">
        <strong>{t("title")}</strong>
        <Tabs items={tabs} value={section} onChange={setSection} aria-label={t("tabs.label")} />
      </div>
      {section === "overview" && <Overview />}
      {section === "checks" && <Checks />}
      {section === "alerts" && <Alerts />}
      {section === "logs" && <Logs />}
    </section>
  );
}

// ── Overview ─────────────────────────────────────────────

function Overview() {
  const { t } = useT("monitorInfra");
  const overview = useRpcQuery("monitor.overview", async (c) => c.call("monitor.overview", {}), {
    topics: TOPICS,
    reloadOn: ["HealthCheckChanged", "AlertRaised", "AlertResolved"],
  });
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setTick((n) => n + 1), 10_000);
    return () => clearInterval(id);
  }, []);
  const { reload } = overview;
  useEffect(() => {
    if (tick > 0) reload();
  }, [tick, reload]);
  const data = overview.data;
  if (overview.error && !data) return <p className="mon-error">{overview.error}</p>;
  if (!data) return null;
  return (
    <div className="infra-stack">
      <p className="mon-muted">
        {t("overview.summary", { checks: data.checks.length, alerts: data.openAlerts, sources: data.logSources })}
      </p>
      {data.nodes.length === 0 ? (
        <p className="mon-muted">{t("overview.empty")}</p>
      ) : (
        <div className="infra-nodes">
          {data.nodes.map((n) => (
            <NodeCard key={n.nodeId} node={n} tick={tick} />
          ))}
        </div>
      )}
    </div>
  );
}

function NodeCard({ node, tick }: { node: NodeMetricsSnapshot; tick: number }) {
  const { t } = useT("monitorInfra");
  const fmt = useFormat();
  const params: MetricsParams & { resolutionSecs?: number } = { nodeId: node.nodeId, metrics: ["cpu", "mem", "disk"], resolutionSecs: 15 };
  const series = useRpcQuery<MetricSeries[]>(`monitor.metrics:${node.nodeId}`, async (c) => (await c.call("monitor.metrics", params)).series);
  const reloadSeries = series.reload;
  useEffect(() => {
    if (tick > 0) reloadSeries();
  }, [tick, reloadSeries]);
  const p = snapshotPercents(node);
  const rows: Array<{ key: "cpu" | "mem" | "disk"; value: number; detail?: string }> = [
    { key: "cpu", value: p.cpu },
    { key: "mem", value: p.mem, detail: t("overview.memValue", { used: fmt.bytes(node.memUsedBytes), total: fmt.bytes(node.memTotalBytes) }) },
    { key: "disk", value: p.disk, detail: t("overview.memValue", { used: fmt.bytes(node.diskUsedBytes), total: fmt.bytes(node.diskTotalBytes) }) },
  ];
  return (
    <article className={clsx("infra-node", !node.online && "infra-node--offline")}>
      <header className="infra-row">
        <strong className="infra-ellipsis">{node.name}</strong>
        {node.local && <Badge tone="accent">{t("overview.local")}</Badge>}
        {!node.online && <Badge tone="danger">{t("overview.offline")}</Badge>}
      </header>
      {rows.map((r) => (
        <div key={r.key} className="infra-metric">
          <span className="infra-metric__label">{t(`overview.${r.key}`)}</span>
          <Sparkline
            values={seriesValues(series.data, r.key)}
            tone={r.value > 90 ? "danger" : r.value > 75 ? "warning" : "accent"}
            width={110}
            height={26}
            label={t("overview.trend", { metric: t(`overview.${r.key}`), node: node.name })}
          />
          <span className="mon-num" title={r.detail}>
            {fmt.percent(r.value / 100)}
          </span>
        </div>
      ))}
      {p.gpu != null && (
        <div className="infra-metric">
          <span className="infra-metric__label">{t("overview.gpu")}</span>
          <span />
          <span className="mon-num">{fmt.percent(p.gpu / 100)}</span>
        </div>
      )}
    </article>
  );
}

// ── Checks ───────────────────────────────────────────────

function Checks() {
  const { t } = useT("monitorInfra");
  const fmt = useFormat();
  const list = useRpcQuery("monitor.checksList", async (c) => (await c.call("monitor.checksList", {})).checks as CheckStatusView[], {
    topics: TOPICS,
    reloadOn: ["HealthCheckChanged"],
  });
  const [draft, setDraft] = useState<CheckDraft | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const act = async (key: string, f: () => Promise<unknown>) => {
    const c = maybeClient();
    if (!c) return;
    setBusy(key);
    setError(null);
    try {
      await f();
      list.reload();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="infra-stack">
      <div className="infra-row">
        <Button size="sm" leftIcon={<Plus size={14} />} onClick={() => setDraft({ ...EMPTY_DRAFT })}>
          {t("checks.add")}
        </Button>
        <IconButton size="sm" label={t("checks.refresh")} tooltip onClick={list.reload} icon={<RefreshCw size={14} />} />
      </div>
      {draft && (
        <CheckForm
          draft={draft}
          onCancel={() => setDraft(null)}
          onSaved={() => {
            setDraft(null);
            list.reload();
          }}
        />
      )}
      {(error ?? list.error) && <p className="mon-error">{error ?? list.error}</p>}
      {list.data?.length === 0 && <p className="mon-muted">{t("checks.empty")}</p>}
      <ul className="infra-list">
        {list.data?.map((c) => {
          const ratio = upRatio(c.history);
          return (
            <li key={c.check.id} className="infra-item">
              <div className="infra-row">
                <Badge tone={checkTone(c.state)}>{t(`checks.state.${c.state}`)}</Badge>
                <strong className="infra-ellipsis">{c.check.name}</strong>
                {!c.check.enabled && <Badge>{t("checks.disabled")}</Badge>}
                <span className="infra-spacer" />
                {c.lastLatencyMs != null && <span className="mon-num mon-muted">{t("checks.latency", { ms: c.lastLatencyMs })}</span>}
                <IconButton
                  size="sm"
                  label={t("checks.run")}
                  tooltip
                  disabled={busy !== null}
                  onClick={() => void act(`run:${c.check.id}`, () => maybeClient()!.call("monitor.checkRun", { checkId: c.check.id }))}
                  icon={<Play size={14} />}
                />
                <IconButton size="sm" label={t("checks.edit")} tooltip onClick={() => setDraft(draftFromCheck(c))} icon={<Pencil size={14} />} />
                <IconButton
                  size="sm"
                  label={t("checks.delete")}
                  tooltip
                  disabled={busy !== null}
                  onClick={() => {
                    if (!globalThis.confirm(t("checks.deleteConfirm", { name: c.check.name }))) return;
                    void act(`del:${c.check.id}`, () => maybeClient()!.call("monitor.checkDelete", { checkId: c.check.id }));
                  }}
                  icon={<Trash2 size={14} />}
                />
              </div>
              <div className="infra-row mon-muted">
                <span className="mon-mono infra-ellipsis">{probeSummary(c.check.probe as ProbeView)}</span>
                {c.since && <span>{t("checks.since", { time: fmt.relative(c.since) })}</span>}
                {ratio != null && <span>{t("checks.uptime", { ratio: fmt.percent(ratio) })}</span>}
              </div>
              {c.history && c.history.length > 0 && <RunHistory runs={c.history} />}
              {c.state === "Down" && c.lastError && <p className="mon-error">{t("checks.lastError", { error: c.lastError })}</p>}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function RunHistory({ runs }: { runs: readonly CheckRun[] }) {
  const { t } = useT("monitorInfra");
  const fmt = useFormat();
  return (
    <div className="infra-history" role="list" aria-label={t("checks.history")}>
      {runs.slice(-40).map((r) => (
        <span
          key={r.ts}
          role="listitem"
          className={clsx("infra-history__bar", r.ok ? "infra-history__bar--ok" : "infra-history__bar--fail")}
          title={r.ok ? t("checks.runOk", { time: fmt.date(r.ts, "time") }) : t("checks.runFail", { time: fmt.date(r.ts, "time"), error: r.error ?? "" })}
        />
      ))}
    </div>
  );
}

function CheckForm({ draft, onCancel, onSaved }: { draft: CheckDraft; onCancel: () => void; onSaved: () => void }) {
  const { t } = useT("monitorInfra");
  const [d, setD] = useState<CheckDraft>(draft);
  const [invalid, setInvalid] = useState<DraftError | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const set = (patch: Partial<CheckDraft>) => setD((prev) => ({ ...prev, ...patch }));

  const save = async () => {
    const res = draftToParams(d);
    if ("error" in res) {
      setInvalid(res.error);
      return;
    }
    const c = maybeClient();
    if (!c) return;
    setInvalid(null);
    setSaving(true);
    try {
      await c.call("monitor.checkUpsert", res.params);
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
      aria-label={d.id ? t("form.titleEdit") : t("form.titleNew")}
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <strong>{d.id ? t("form.titleEdit") : t("form.titleNew")}</strong>
      <div className="infra-grid">
        <Input label={t("form.name")} value={d.name} onChange={(e) => set({ name: e.target.value })} error={invalid === "name" ? t("form.error.name") : undefined} />
        <label className="infra-field">
          <span>{t("form.kind")}</span>
          <select className="infra-select" value={d.kind} onChange={(e) => set({ kind: e.target.value as CheckDraft["kind"] })}>
            <option value="Http">{t("form.http")}</option>
            <option value="Tcp">{t("form.tcp")}</option>
          </select>
        </label>
        {d.kind === "Http" ? (
          <>
            <Input label={t("form.url")} value={d.url} onChange={(e) => set({ url: e.target.value })} error={invalid === "url" ? t("form.error.url") : undefined} />
            <label className="infra-field">
              <span>{t("form.method")}</span>
              <select className="infra-select" value={d.method} onChange={(e) => set({ method: e.target.value })}>
                {["GET", "HEAD", "POST", "PUT", "OPTIONS"].map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            </label>
            <Input
              label={t("form.expectStatus")}
              hint={t("form.expectStatusHint")}
              inputMode="numeric"
              value={d.expectStatus}
              onChange={(e) => set({ expectStatus: e.target.value })}
              error={invalid === "status" ? t("form.error.status") : undefined}
            />
            <Input label={t("form.contains")} value={d.contains} onChange={(e) => set({ contains: e.target.value })} />
          </>
        ) : (
          <>
            <Input label={t("form.host")} value={d.host} onChange={(e) => set({ host: e.target.value })} error={invalid === "host" ? t("form.error.host") : undefined} />
            <Input label={t("form.port")} inputMode="numeric" value={d.port} onChange={(e) => set({ port: e.target.value })} error={invalid === "port" ? t("form.error.port") : undefined} />
          </>
        )}
        <Input label={t("form.interval")} inputMode="numeric" value={d.intervalSecs} onChange={(e) => set({ intervalSecs: e.target.value })} />
        <Input label={t("form.timeout")} inputMode="numeric" value={d.timeoutMs} onChange={(e) => set({ timeoutMs: e.target.value })} />
        <Input
          label={t("form.threshold")}
          hint={t("form.thresholdHint")}
          inputMode="numeric"
          value={d.failureThreshold}
          onChange={(e) => set({ failureThreshold: e.target.value })}
          error={invalid === "numbers" ? t("form.error.numbers") : undefined}
        />
        <label className="infra-check">
          <input type="checkbox" checked={d.enabled} onChange={(e) => set({ enabled: e.target.checked })} />
          <span>{t("form.enabled")}</span>
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

// ── Alerts ───────────────────────────────────────────────

function Alerts() {
  const { t } = useT("monitorInfra");
  const fmt = useFormat();
  const [showResolved, setShowResolved] = useState(false);
  const list = useRpcQuery(`monitor.alerts:${showResolved}`, async (c) => (await c.call("monitor.alerts", { includeResolved: showResolved })).alerts, {
    topics: TOPICS,
    reloadOn: ["AlertRaised", "AlertResolved"],
  });
  const [error, setError] = useState<string | null>(null);
  const ack = async (a: Alert) => {
    const c = maybeClient();
    if (!c) return;
    try {
      await c.call("monitor.ackAlert", { alertId: a.id });
      list.reload();
    } catch (e) {
      setError(errorMessage(e));
    }
  };
  return (
    <div className="infra-stack">
      <label className="infra-check">
        <input type="checkbox" checked={showResolved} onChange={(e) => setShowResolved(e.target.checked)} />
        <span>{t("alerts.showResolved")}</span>
      </label>
      {(error ?? list.error) && <p className="mon-error">{error ?? list.error}</p>}
      {list.data?.length === 0 && <p className="mon-muted">{t("alerts.empty")}</p>}
      <ul className="infra-list">
        {list.data?.map((a) => (
          <li key={a.id} className={clsx("infra-item", a.state === "Resolved" && "infra-item--muted")}>
            <div className="infra-row">
              <Badge tone={severityTone(a.severity)}>{t(`alerts.severity.${a.severity}`)}</Badge>
              <strong className="infra-ellipsis">{a.title}</strong>
              {a.count > 1 && <span className="mon-num mon-muted">{t("alerts.count", { count: a.count })}</span>}
              <span className="infra-spacer" />
              <Badge tone={a.state === "Open" ? "danger" : a.state === "Acknowledged" ? "warning" : "success"}>{t(`alerts.state.${a.state}`)}</Badge>
              {a.state === "Open" && (
                <Button size="sm" variant="ghost" leftIcon={<Check size={14} />} onClick={() => void ack(a)}>
                  {t("alerts.ack")}
                </Button>
              )}
            </div>
            <p className="mon-mono infra-wrap">{a.message}</p>
            <span className="mon-muted">{t("alerts.opened", { time: fmt.relative(a.openedAt) })}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

// ── Logs ─────────────────────────────────────────────────

function Logs() {
  const { t } = useT("monitorInfra");
  const fmt = useFormat();
  const sources = useRpcQuery("monitor.logSources", async (c) => (await c.call("monitor.logSources", {})).sources);
  const [sourceId, setSourceId] = useState("");
  const [minLevel, setMinLevel] = useState<LogLevel | "">("");
  const [text, setText] = useState("");
  const [filter, setFilter] = useState("");
  useEffect(() => {
    const id = setTimeout(() => setFilter(text.trim()), 300);
    return () => clearTimeout(id);
  }, [text]);
  const [live, setLive] = useState(true);
  const [lines, setLines] = useState<LogLine[]>([]);
  const [truncated, setTruncated] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const linesRef = useRef<LogLine[]>([]);
  linesRef.current = lines;

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const base = {
      ...(sourceId ? { sourceId } : {}),
      ...(minLevel ? { minLevel } : {}),
      ...(filter ? { contains: filter } : {}),
    };
    const poll = async (first: boolean) => {
      const c = maybeClient();
      if (c) {
        try {
          const since = first ? undefined : lastTs(linesRef.current);
          const res = await c.call("monitor.logs", { ...base, limit: 500, ...(since ? { since } : {}) });
          if (cancelled) return;
          setError(null);
          if (first) {
            setLines(res.lines);
            setTruncated(res.truncated);
          } else {
            setLines((prev) => mergeTail(prev, res.lines));
          }
        } catch (e) {
          if (!cancelled) setError(errorMessage(e));
        }
      }
      if (!cancelled && live) timer = setTimeout(() => void poll(false), 2000);
    };
    void poll(true);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [sourceId, minLevel, filter, live]);

  return (
    <div className="infra-stack">
      <div className="infra-row infra-filters">
        <label className="infra-field">
          <span>{t("logs.source")}</span>
          <select className="infra-select" value={sourceId} onChange={(e) => setSourceId(e.target.value)}>
            <option value="">{t("logs.allSources")}</option>
            {sources.data?.map((s) => (
              <option key={s.id} value={s.id}>
                {s.name}
              </option>
            ))}
          </select>
        </label>
        <label className="infra-field">
          <span>{t("logs.level")}</span>
          <select className="infra-select" value={minLevel} onChange={(e) => setMinLevel(e.target.value as LogLevel | "")}>
            <option value="">{t("logs.anyLevel")}</option>
            {LEVELS.map((l) => (
              <option key={l} value={l}>
                {t(`logs.levels.${l}`)}
              </option>
            ))}
          </select>
        </label>
        <Input label={t("logs.search")} value={text} onChange={(e) => setText(e.target.value)} />
        <Button size="sm" variant={live ? "secondary" : "primary"} leftIcon={live ? <Pause size={14} /> : <Play size={14} />} onClick={() => setLive((v) => !v)}>
          {live ? t("logs.pause") : t("logs.live")}
        </Button>
      </div>
      {error && <p className="mon-error">{error}</p>}
      {sources.data?.length === 0 ? (
        <p className="mon-muted">{t("logs.noSources")}</p>
      ) : lines.length === 0 ? (
        <p className="mon-muted">{t("logs.empty")}</p>
      ) : (
        <div className="infra-log mon-scroll" role="log" aria-live={live ? "polite" : "off"}>
          {lines.map((l, i) => (
            <div key={`${l.ts}-${i}`} className="infra-log__line">
              <span className="mon-muted mon-num">{fmt.date(l.ts, "time")}</span>
              <Badge tone={levelTone(l.level)}>{l.level ? t(`logs.levels.${l.level}`) : "·"}</Badge>
              <span className="mon-muted infra-ellipsis infra-log__src">{l.sourceId}</span>
              <span className="mon-mono infra-wrap">{l.message}</span>
            </div>
          ))}
        </div>
      )}
      <p className="mon-muted">
        {truncated ? `${t("logs.truncated")} ` : ""}
        {t("logs.redactedNote")}
      </p>
    </div>
  );
}
