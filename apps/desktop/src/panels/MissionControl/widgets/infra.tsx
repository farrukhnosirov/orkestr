import type { ProviderHealth, ProviderHealthRow } from "@orkestr/protocol";
import { Badge, type Status, StatusDot } from "@orkestr/ui";
import { Cpu, HardDrive, MemoryStick, MonitorCog, Server, Zap } from "lucide-react";
import type { ReactNode } from "react";

import { workspaceActions } from "../../../components/workspaceActions";
import { useFormat, useT } from "../../../i18n";
import { useRpcQuery } from "../../../lib/rpcQuery";
import { type ChartTone, Sparkline, loadTone } from "../../Monitoring/charts";
import { ratio, useTelemetryFeed, useTelemetryStore } from "../../Monitoring/telemetry";
import { type WidgetProps, useMission } from "../context";
import { WidgetEmpty, WidgetStatus } from "./common";

function ResourceTile({
  icon,
  title,
  value,
  sub,
  values,
  tone,
  label,
  rich,
}: {
  icon: ReactNode;
  title: string;
  value: string;
  sub: string;
  values: number[];
  tone: ChartTone;
  label: string;
  rich: boolean;
}) {
  return (
    <div className="mc-res">
      <div className="mc-res__head">
        {icon}
        <span className="mc-res__title">{title}</span>
        <span className="mc-res__value">{value}</span>
      </div>
      <Sparkline values={values} tone={tone} rich={rich} label={label} height={30} />
      <span className="mc-res__sub">{sub}</span>
    </div>
  );
}

/** This node (Phase 6 adds remote nodes) with live CPU/RAM/GPU/disk sparklines. */
export function NodesWidget(_props: WidgetProps) {
  const { t } = useT("missionControl");
  const fmt = useFormat();
  const { budget } = useMission();
  useTelemetryFeed();
  const points = useTelemetryStore((s) => s.points);
  const error = useTelemetryStore((s) => s.error);
  const nodes = useRpcQuery("nodes.list", async (c) => (await c.call("nodes.list", {})).nodes);
  const node = nodes.data?.[0];
  const recent = points.slice(-budget.chartPoints);
  const last = recent[recent.length - 1];
  if (!last) return <WidgetStatus loading={!error} error={error} />;
  const memFrac = ratio(last.memUsed, last.memTotal);
  const diskFrac = ratio(last.diskUsed, last.diskTotal);
  return (
    <div className="mc-node">
      <div className="mc-node__head">
        <Server size={15} aria-hidden="true" />
        <span className="mc-node__name">{node?.name ?? node?.hostname ?? t("nodes.thisNode")}</span>
        <Badge tone="success">{t("nodes.local")}</Badge>
        <span className="mc-node__os">{node ? `${node.os} · ${node.arch}` : ""}</span>
        <button type="button" className="mc-link" onClick={() => workspaceActions.openPanel("monitoring")}>
          {t("nodes.details")}
        </button>
      </div>
      <div className="mc-node__tiles">
        <ResourceTile
          icon={<Cpu size={14} aria-hidden="true" />}
          title={t("nodes.cpu")}
          value={fmt.percent(last.cpu / 100)}
          sub={t("nodes.cores", { count: last.perCore.length })}
          values={recent.map((p) => p.cpu)}
          tone={loadTone(last.cpu / 100, "accent")}
          label={t("nodes.cpuLabel", { value: fmt.percent(last.cpu / 100) })}
          rich={budget.rich}
        />
        <ResourceTile
          icon={<MemoryStick size={14} aria-hidden="true" />}
          title={t("nodes.ram")}
          value={fmt.percent(memFrac)}
          sub={t("nodes.usedOf", { used: fmt.bytes(last.memUsed), total: fmt.bytes(last.memTotal) })}
          values={recent.map((p) => ratio(p.memUsed, p.memTotal) * 100)}
          tone={loadTone(memFrac, "violet")}
          label={t("nodes.ramLabel", { value: fmt.percent(memFrac) })}
          rich={budget.rich}
        />
        <ResourceTile
          icon={<MonitorCog size={14} aria-hidden="true" />}
          title={t("nodes.gpu")}
          value={last.gpu === null ? "—" : fmt.percent(last.gpu / 100)}
          sub={last.gpu === null ? t("nodes.gpuUnavailable") : ""}
          values={recent.map((p) => p.gpu ?? 0)}
          tone="info"
          label={t("nodes.gpuLabel")}
          rich={budget.rich}
        />
        <ResourceTile
          icon={<HardDrive size={14} aria-hidden="true" />}
          title={t("nodes.disk")}
          value={fmt.percent(diskFrac)}
          sub={t("nodes.freeOf", { free: fmt.bytes(last.diskTotal - last.diskUsed), total: fmt.bytes(last.diskTotal) })}
          values={recent.map((p) => ratio(p.diskUsed, p.diskTotal) * 100)}
          tone={loadTone(diskFrac, "success")}
          label={t("nodes.diskLabel", { value: fmt.percent(diskFrac) })}
          rich={budget.rich}
        />
      </div>
      <p className="mc-node__more">{t("nodes.moreNodes")}</p>
    </div>
  );
}

const HEALTH_DOT: Record<ProviderHealth, Status> = {
  Unknown: "idle",
  Healthy: "online",
  Degraded: "warning",
  RateLimited: "busy",
  Down: "error",
};

/** Seconds until an ISO time, or null when past/absent. */
export function secondsUntil(iso: string | null, now: number): number | null {
  if (!iso) return null;
  const t = Date.parse(iso);
  if (!Number.isFinite(t) || t <= now) return null;
  return Math.ceil((t - now) / 1000);
}

function ProviderRow({ p, now }: { p: ProviderHealthRow; now: number }) {
  const { t } = useT(["missionControl", "common"]);
  const fmt = useFormat();
  const limited = secondsUntil(p.rateLimitedUntil, now);
  const quota = secondsUntil(p.quotaExhaustedUntil, now);
  return (
    <li className="mc-provider">
      <StatusDot status={p.enabled ? HEALTH_DOT[p.health] : "offline"} pulse={p.health === "RateLimited"} label={t(`common:status.health.${p.health}`)} />
      <span className="mc-provider__name">{p.name}</span>
      <span className="mc-provider__health">{p.enabled ? t(`common:status.health.${p.health}`) : t("providers.disabled")}</span>
      <span className="mc-provider__latency" title={t("providers.latencyTitle")}>
        {p.avgLatencyMs ?? p.latencyMs ? t("providers.latency", { ms: fmt.integer(p.avgLatencyMs ?? p.latencyMs ?? 0) }) : "—"}
      </span>
      <span className="mc-provider__flags">
        {limited !== null && <Badge tone="warning">{t("providers.rateLimited", { time: fmt.duration(limited * 1000) })}</Badge>}
        {quota !== null && <Badge tone="danger">{t("providers.quota", { time: fmt.duration(quota * 1000) })}</Badge>}
        {p.circuitOpen && <Badge tone="danger">{t("providers.circuit")}</Badge>}
        {p.isLocal && <Badge tone="neutral">{t("providers.local")}</Badge>}
      </span>
    </li>
  );
}

/** Provider health: status dots, latency, rate-limit state. */
export function ProvidersWidget(_props: WidgetProps) {
  const { t } = useT("missionControl");
  const { snapshot } = useMission();
  const rows = snapshot.data?.providers;
  if (!rows) return <WidgetStatus loading={snapshot.loading || !snapshot.error} error={snapshot.error} />;
  if (rows.length === 0) return <WidgetEmpty icon={<Zap size={20} />} title={t("providers.empty")} hint={t("providers.emptyHint")} />;
  const now = Date.parse(snapshot.data?.generatedAt ?? "") || Date.now();
  return (
    <ul className="mc-providers" aria-label={t("providers.listLabel")}>
      {rows.map((p) => (
        <ProviderRow key={p.providerId} p={p} now={now} />
      ))}
    </ul>
  );
}
