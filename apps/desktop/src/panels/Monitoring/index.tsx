import type { OrkestrProcess } from "@orkestr/protocol";
import { Badge, IconButton, type Status, StatusDot, Tabs, type Tone } from "@orkestr/ui";
import { clsx } from "clsx";
import { Activity, AlertOctagon, AlertTriangle, Cpu, Database, HardDrive, LayoutGrid, MemoryStick, RefreshCw, Server, Trash2 } from "lucide-react";
import { useMemo, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { useRpcQuery } from "../../lib/rpcQuery";
import { useAppStore } from "../../store/app";
import { useNow } from "../MissionControl/widgets/agents";
import { useMissionControlStore } from "../MissionControl/store";
import { useVisualBudget } from "../MissionControl/visual";
import type { PanelProps } from "../types";
import { type ChartTone, loadTone, Meter, TimeSeriesChart } from "./charts";
import {
  type CheckLevel,
  latencyStats,
  levelOf,
  type ProcessSort,
  sortProcesses,
  THRESHOLDS,
  useCoreHealth,
  useLogTail,
  useLogTailFeed,
  worstLevel,
} from "./health";
import "./monitoring.css";
import { ratio, seriesStats, type TelemetryPoint, useTelemetryFeed, useTelemetryStore, windowed } from "./telemetry";

type WindowId = "2m" | "5m" | "10m";
const WINDOW_MS: Record<WindowId, number> = { "2m": 120_000, "5m": 300_000, "10m": 600_000 };

const LEVEL_DOT: Record<CheckLevel, Status> = { ok: "online", warn: "warning", fail: "error", unknown: "idle" };
const LEVEL_TONE: Record<CheckLevel, Tone> = { ok: "success", warn: "warning", fail: "danger", unknown: "neutral" };

interface SeriesDef {
  id: string;
  title: string;
  icon: typeof Cpu;
  /** Value of a point (null = not measured). */
  value: (p: TelemetryPoint) => number | null;
  max: number;
  tone: ChartTone;
  format: (v: number) => string;
}

function ResourceChart({ def, points, rich }: { def: SeriesDef; points: TelemetryPoint[]; rich: boolean }) {
  const { t } = useT("monitoring");
  const fmt = useFormat();
  const series = useMemo(
    () => points.flatMap((p) => {
      const v = def.value(p);
      return v === null ? [] : [{ ts: p.ts, value: v }];
    }),
    [points, def],
  );
  const stats = seriesStats(series.map((s) => s.value));
  const Icon = def.icon;
  const tone = def.max === 100 && stats ? loadTone(stats.last / 100, def.tone) : def.tone;
  return (
    <section className="mon-card mon-chart" aria-label={def.title}>
      <header className="mon-chart__head">
        <Icon size={14} aria-hidden="true" />
        <h3>{def.title}</h3>
        <span className="mon-chart__now">{stats ? def.format(stats.last) : "—"}</span>
      </header>
      <TimeSeriesChart
        points={series}
        max={def.max}
        tone={tone}
        rich={rich}
        height={120}
        label={stats ? t("charts.label", { title: def.title, value: def.format(stats.last) }) : def.title}
        format={def.format}
        formatTime={(ts) => fmt.date(ts, "timeSeconds")}
        emptyText={t("charts.collecting")}
      />
      {stats && (
        <footer className="mon-chart__stats">
          <span>{t("charts.min", { value: def.format(stats.min) })}</span>
          <span>{t("charts.avg", { value: def.format(stats.avg) })}</span>
          <span>{t("charts.max", { value: def.format(stats.max) })}</span>
        </footer>
      )}
    </section>
  );
}

function PerCore({ cores }: { cores: number[] }) {
  const { t } = useT("monitoring");
  const fmt = useFormat();
  if (cores.length === 0) return null;
  return (
    <section className="mon-card mon-cores" aria-label={t("cores.title")}>
      <header className="mon-section__head">
        <Cpu size={14} aria-hidden="true" />
        <h3>{t("cores.title")}</h3>
        <span className="mon-muted">{t("cores.count", { count: cores.length })}</span>
      </header>
      <div className="mon-cores__grid">
        {cores.map((v, i) => (
          <div key={i} className="mon-core" title={t("cores.core", { index: i + 1, value: fmt.percent(v / 100) })}>
            <span className="mon-core__bar">
              <span className={clsx("mon-core__fill", `mon-core__fill--${loadTone(v / 100, "accent")}`)} style={{ height: `${Math.max(2, v)}%` }} />
            </span>
            <span className="mon-core__label">{i + 1}</span>
          </div>
        ))}
      </div>
    </section>
  );
}

interface Check {
  id: string;
  title: string;
  value: string;
  hint?: string | undefined;
  level: CheckLevel;
}

function Services({ checks, uptime, version }: { checks: Check[]; uptime: string | null; version: string | null }) {
  const { t } = useT("monitoring");
  const overall = worstLevel(checks.map((c) => c.level));
  return (
    <section className="mon-card mon-services" aria-label={t("services.title")}>
      <header className="mon-section__head">
        <Activity size={14} aria-hidden="true" />
        <h3>{t("services.title")}</h3>
        <Badge tone={LEVEL_TONE[overall]}>{t(`level.${overall}`)}</Badge>
        {version && <span className="mon-muted">{t("services.version", { version })}</span>}
        {uptime && <span className="mon-muted">{t("services.uptime", { time: uptime })}</span>}
      </header>
      <ul className="mon-checks">
        {checks.map((c) => (
          <li key={c.id} className={clsx("mon-check", `mon-check--${c.level}`)}>
            <StatusDot status={LEVEL_DOT[c.level]} label={t(`level.${c.level}`)} />
            <span className="mon-check__title">{c.title}</span>
            <span className="mon-check__value">{c.value}</span>
            {c.hint && <span className="mon-check__hint">{c.hint}</span>}
          </li>
        ))}
      </ul>
    </section>
  );
}

function ProcessTable({ rows, coreRow }: { rows: OrkestrProcess[]; coreRow: OrkestrProcess | null }) {
  const { t } = useT("monitoring");
  const fmt = useFormat();
  const projects = useAppStore((s) => s.projects);
  const [sort, setSort] = useState<ProcessSort>("cpu");
  const sorted = useMemo(() => sortProcesses(rows, sort), [rows, sort]);
  const all = coreRow ? [coreRow, ...sorted] : sorted;
  const head = (id: ProcessSort, label: string, numeric = false) => (
    <th scope="col" aria-sort={sort === id ? (id === "name" ? "ascending" : "descending") : "none"} className={clsx(numeric && "mon-num")}>
      <button type="button" className={clsx("mon-sort", sort === id && "mon-sort--active")} onClick={() => setSort(id)}>
        {label}
      </button>
    </th>
  );
  return (
    <section className="mon-card mon-procs" aria-label={t("processes.title")}>
      <header className="mon-section__head">
        <Server size={14} aria-hidden="true" />
        <h3>{t("processes.title")}</h3>
        <span className="mon-muted">{t("processes.count", { count: all.length })}</span>
      </header>
      {all.length === 0 ? (
        <p className="mon-empty">{t("processes.empty")}</p>
      ) : (
        <div className="mon-table-wrap">
          <table className="mon-table">
            <thead>
              <tr>
                {head("name", t("processes.name"))}
                <th scope="col">{t("processes.kind")}</th>
                <th scope="col">{t("processes.scope")}</th>
                {head("cpu", t("processes.cpu"), true)}
                {head("memory", t("processes.memory"), true)}
                <th scope="col" className="mon-num">{t("processes.pid")}</th>
              </tr>
            </thead>
            <tbody>
              {all.map((p) => {
                const project = p.projectId ? projects.find((x) => x.id === p.projectId)?.name : null;
                return (
                  <tr key={p.id} className={clsx(!p.running && "mon-row--stopped")}>
                    <td className="mon-proc__name" title={p.label}>
                      <StatusDot status={p.running ? "online" : "offline"} label={p.running ? t("processes.running") : t("processes.stopped")} />
                      {p.label}
                      {p.processCount > 1 && <span className="mon-muted">{t("processes.subtree", { count: p.processCount })}</span>}
                    </td>
                    <td>
                      <Badge tone={p.kind === "Core" ? "accent" : p.kind === "AgentTerminal" ? "violet" : "neutral"}>{t(`processes.kinds.${p.kind}`)}</Badge>
                    </td>
                    <td className="mon-muted">{project ?? (p.agentId ? t("processes.agent") : "—")}</td>
                    <td className="mon-num">
                      {p.cpuPercent === null ? "—" : (
                        <span className="mon-cpu">
                          <Meter value={p.cpuPercent / 100} tone={loadTone(p.cpuPercent / 100)} label={t("processes.cpu")} />
                          {fmt.percent(p.cpuPercent / 100, 1)}
                        </span>
                      )}
                    </td>
                    <td className="mon-num">{p.memoryBytes === null ? "—" : fmt.bytes(p.memoryBytes)}</td>
                    <td className="mon-num mon-mono">{p.pid ?? "—"}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function LogTail() {
  const { t } = useT("monitoring");
  const fmt = useFormat();
  useLogTailFeed();
  const lines = useLogTail((s) => s.lines);
  const loaded = useLogTail((s) => s.loaded);
  return (
    <section className="mon-card mon-log" aria-label={t("log.title")}>
      <header className="mon-section__head">
        <AlertOctagon size={14} aria-hidden="true" />
        <h3>{t("log.title")}</h3>
        <span className="mon-muted">{t("log.count", { count: lines.length })}</span>
        <IconButton size="sm" label={t("log.clear")} icon={<Trash2 size={13} />} onClick={() => useLogTail.getState().clear()} disabled={lines.length === 0} />
      </header>
      {lines.length === 0 ? (
        <p className="mon-empty">{loaded ? t("log.empty") : t("log.loading")}</p>
      ) : (
        <ol className="mon-log__lines" aria-live="polite">
          {lines.slice(0, 80).map((l) => (
            <li key={l.id} className={clsx("mon-log__line", `mon-log__line--${l.level}`)}>
              {l.level === "error" ? <AlertOctagon size={12} aria-hidden="true" /> : <AlertTriangle size={12} aria-hidden="true" />}
              <time dateTime={l.ts}>{fmt.date(l.ts, "timeSeconds")}</time>
              <span className="mon-log__level">{t(`log.levels.${l.level}`)}</span>
              <span className="mon-log__target">{l.target}</span>
              <span className="mon-log__msg">{l.message}</span>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

/** Monitoring: node resources over time, Orkestr processes, core health and the error tail. */
export default function MonitoringPanel(_props: PanelProps) {
  const { t } = useT("monitoring");
  const fmt = useFormat();
  const budget = useVisualBudget();
  const [win, setWin] = useState<WindowId>("5m");
  useTelemetryFeed();
  const points = useTelemetryStore((s) => s.points);
  const latest = useTelemetryStore((s) => s.latest);
  const eventLag = useTelemetryStore((s) => s.eventLagMs);
  const intervalMs = useTelemetryStore((s) => s.intervalMs);
  const telemetryError = useTelemetryStore((s) => s.error);
  const nodes = useRpcQuery("nodes.list", async (c) => (await c.call("nodes.list", {})).nodes);
  const health = useCoreHealth();
  const now = useNow(2000);
  const node = nodes.data?.[0];

  const shown = useMemo(() => {
    const w = windowed(points, WINDOW_MS[win], now);
    // Down-sample to the visual budget (cheap SVG): keep every n-th point and the last one.
    const step = Math.max(1, Math.ceil(w.length / budget.chartPoints));
    return step === 1 ? w : w.filter((_, i) => i % step === 0 || i === w.length - 1);
  }, [points, win, now, budget.chartPoints]);

  const pct = (v: number) => fmt.percent(v / 100);
  const series: SeriesDef[] = useMemo(
    () => [
      { id: "cpu", title: t("charts.cpu"), icon: Cpu, value: (p) => p.cpu, max: 100, tone: "accent", format: pct },
      { id: "ram", title: t("charts.ram"), icon: MemoryStick, value: (p) => ratio(p.memUsed, p.memTotal) * 100, max: 100, tone: "violet", format: pct },
      { id: "disk", title: t("charts.disk"), icon: HardDrive, value: (p) => ratio(p.diskUsed, p.diskTotal) * 100, max: 100, tone: "success", format: pct },
      { id: "swap", title: t("charts.swap"), icon: MemoryStick, value: (p) => (p.swapTotal > 0 ? ratio(p.swapUsed, p.swapTotal) * 100 : null), max: 100, tone: "info", format: pct },
      { id: "coreCpu", title: t("charts.coreCpu"), icon: Activity, value: (p) => p.coreCpu, max: 100, tone: "accent", format: pct },
      {
        id: "coreMem",
        title: t("charts.coreMem"),
        icon: Database,
        value: (p) => p.coreMem,
        max: Math.max(64 * 1024 ** 2, ...shown.map((p) => (p.coreMem ?? 0) * 1.25)),
        tone: "violet",
        format: (v) => fmt.bytes(v),
      },
    ],
    [t, fmt, shown],
  );

  const h = health.health;
  const lat = latencyStats(health.latency);
  const checks: Check[] = [
    {
      id: "rpc",
      title: t("services.rpc"),
      value: lat ? t("services.ms", { ms: fmt.integer(Math.round(lat.last)) }) : "—",
      hint: lat ? t("services.rpcHint", { avg: fmt.integer(Math.round(lat.avg)), p95: fmt.integer(Math.round(lat.p95)) }) : undefined,
      level: health.error ? "fail" : levelOf(lat?.p95, THRESHOLDS.rpcWarnMs, THRESHOLDS.rpcFailMs),
    },
    {
      id: "events",
      title: t("services.eventLag"),
      value: eventLag === null ? "—" : t("services.ms", { ms: fmt.integer(Math.round(eventLag)) }),
      hint: h ? t("services.lastSeq", { seq: fmt.integer(h.lastEventSeq) }) : undefined,
      level: telemetryError ? "fail" : levelOf(eventLag, THRESHOLDS.lagWarnMs, THRESHOLDS.lagFailMs),
    },
    {
      id: "db",
      title: t("services.db"),
      value: h ? fmt.bytes(h.storage.dbBytes) : "—",
      hint: h ? t("services.wal", { size: fmt.bytes(h.storage.dbWalBytes) }) : undefined,
      level: h ? worstLevel([levelOf(h.storage.dbBytes, THRESHOLDS.dbWarnBytes), levelOf(h.storage.dbWalBytes, THRESHOLDS.walWarnBytes)]) : "unknown",
    },
    {
      id: "objects",
      title: t("services.objects"),
      value: h ? fmt.bytes(h.storage.objectStoreBytes) : "—",
      hint: h
        ? h.storage.objectScanTruncated
          ? t("services.objectsTruncated", { count: h.storage.objectCount })
          : t("services.objectsCount", { count: h.storage.objectCount })
        : undefined,
      level: h ? levelOf(h.storage.objectStoreBytes, THRESHOLDS.objectsWarnBytes) : "unknown",
    },
    {
      id: "rpcMethods",
      title: t("services.methods"),
      value: h ? fmt.integer(h.rpcMethods) : "—",
      hint: h ? t("services.probe", { ms: fmt.integer(h.tookMs) }) : undefined,
      level: h ? "ok" : "unknown",
    },
  ];

  const coreRow: OrkestrProcess | null = h?.coreProcess
    ? {
        kind: "Core",
        id: "core",
        label: h.coreProcess.name,
        projectId: null,
        agentId: null,
        pid: h.coreProcess.pid,
        running: true,
        cpuPercent: h.coreProcess.cpuPercent,
        memoryBytes: h.coreProcess.memoryBytes,
        processCount: 1,
      }
    : null;

  return (
    <div className={clsx("mon", budget.rich && "mon--rich")}>
      <div className="mon-toolbar">
        <Server size={15} aria-hidden="true" />
        <span className="mon-toolbar__node">{node?.name ?? node?.hostname ?? t("node.thisNode")}</span>
        {node && <span className="mon-muted">{`${node.os} · ${node.arch}`}</span>}
        {latest && <span className="mon-muted">{t("node.interval", { ms: fmt.integer(intervalMs) })}</span>}
        <span className="mon-toolbar__spacer" />
        <Tabs<WindowId>
          aria-label={t("window.label")}
          value={win}
          onChange={setWin}
          items={(["2m", "5m", "10m"] as const).map((id) => ({ id, label: t(`window.${id}`) }))}
        />
        <IconButton label={t("toolbar.refresh")} icon={<RefreshCw size={14} />} onClick={health.reload} />
        <IconButton
          label={t("toolbar.missionControl")}
          tooltip
          icon={<LayoutGrid size={14} />}
          onClick={() => useMissionControlStore.getState().setFullscreen(true)}
        />
      </div>
      <div className="mon-scroll">
        {telemetryError && (
          <div className="mon-error" role="alert">
            {t("errors.telemetry", { error: telemetryError })}
          </div>
        )}
        <div className="mon-charts">
          {series.map((s) => (
            <ResourceChart key={s.id} def={s} points={shown} rich={budget.rich} />
          ))}
        </div>
        <div className="mon-row">
          <PerCore cores={latest?.perCorePercent ?? []} />
          <Services
            checks={checks}
            uptime={h ? fmt.duration(h.uptimeMs) : null}
            version={h?.coreVersion ?? null}
          />
        </div>
        {health.error && (
          <div className="mon-error" role="alert">
            {t("errors.health", { error: health.error })}
          </div>
        )}
        <ProcessTable rows={h?.processes ?? []} coreRow={coreRow} />
        <LogTail />
      </div>
    </div>
  );
}
