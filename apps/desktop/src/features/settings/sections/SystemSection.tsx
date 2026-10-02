import type { ResourceSample } from "@orkestr/protocol";
import { Badge, Button, EmptyState, Spinner } from "@orkestr/ui";
import { AlertTriangle, Cpu, HardDrive, MemoryStick, MonitorCog, RefreshCw } from "lucide-react";
import { useEffect } from "react";

import { useFormat, useT } from "../../../i18n";
import { SettingGroup } from "../controls";
import type { SettingsSectionProps } from "../sections";
import { useSystemStore } from "../systemStore";
import { SystemChecksList } from "./SystemChecksList";

function Meter({ label, value, detail }: { label: string; value: number; detail: string }) {
  const pct = Math.max(0, Math.min(100, value));
  const tone = pct > 90 ? "danger" : pct > 70 ? "warning" : "ok";
  return (
    <div className="sys-meter">
      <div className="sys-meter__head">
        <span>{label}</span>
        <span className="sys-meter__value">{detail}</span>
      </div>
      <div
        className={`sys-meter__track sys-meter__track--${tone}`}
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(pct)}
      >
        <div className="sys-meter__fill" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

export function LiveResources({ sample }: { sample: ResourceSample | null }) {
  const { t } = useT("settings");
  const fmt = useFormat();
  if (!sample) return <div className="set-loading"><Spinner size={16} /> {t("system.sampling")}</div>;
  const mem = sample.memoryTotalBytes > 0 ? (sample.memoryUsedBytes / sample.memoryTotalBytes) * 100 : 0;
  const disk =
    sample.diskTotalBytes > 0 ? ((sample.diskTotalBytes - sample.diskFreeBytes) / sample.diskTotalBytes) * 100 : 0;
  return (
    <div className="sys-live">
      <Meter label={t("system.cpu")} value={sample.cpuPercent} detail={fmt.percent(sample.cpuPercent / 100)} />
      <Meter
        label={t("system.memory")}
        value={mem}
        detail={t("system.usedOfTotal", { used: fmt.bytes(sample.memoryUsedBytes), total: fmt.bytes(sample.memoryTotalBytes) })}
      />
      <Meter label={t("system.dataDisk")} value={disk} detail={t("system.free", { size: fmt.bytes(sample.diskFreeBytes) })} />
      <div className="sys-cores" aria-label={t("system.perCore")}>
        {sample.perCorePercent.map((c, i) => (
          <span
            key={i}
            className="sys-cores__bar"
            title={t("system.coreLoad", { index: i, percent: fmt.percent(c / 100) })}
            style={{ ["--h" as string]: `${Math.max(4, c)}%` }}
          />
        ))}
      </div>
      {sample.coreProcess && (
        <div className="sys-proc">
          {t("system.coreProcess", {
            count: sample.coreProcess.cpuCores,
            cores: fmt.number(sample.coreProcess.cpuCores, { minimumFractionDigits: 2, maximumFractionDigits: 2 }),
            memory: fmt.bytes(sample.coreProcess.memoryBytes),
          })}
        </div>
      )}
      {sample.topProcesses.length > 0 && (
        <table className="sys-table sys-table--compact">
          <caption className="sr-only">{t("system.busiest")}</caption>
          <thead>
            <tr>
              <th>{t("system.process")}</th>
              <th>{t("system.cpu")}</th>
              <th>{t("system.memory")}</th>
            </tr>
          </thead>
          <tbody>
            {sample.topProcesses.map((p) => (
              <tr key={p.pid}>
                <td>{p.name}</td>
                <td>{fmt.percent(p.cpuPercent / 100, 1)}</td>
                <td>{fmt.bytes(p.memoryBytes)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

/** Settings → System: node inventory, runtimes, local models, live resources, checks. */
export function SystemSection(_props: SettingsSectionProps) {
  const { info, infoLoading, infoError, checks, checksLoading, checksError, sample, loadInfo, runChecks, watchResources } =
    useSystemStore();
  const { t } = useT(["settings", "common"]);
  const fmt = useFormat();

  useEffect(() => {
    if (!useSystemStore.getState().info) void loadInfo();
    if (!useSystemStore.getState().checks) void runChecks();
    return watchResources();
  }, [loadInfo, runChecks, watchResources]);

  return (
    <div className="set-section">
      <SettingGroup title={t("system.liveResources")}>
        <LiveResources sample={sample} />
      </SettingGroup>

      <SettingGroup title={t("system.thisMachine")}>
        <div className="set-group__actions">
          <Button
            size="sm"
            variant="ghost"
            leftIcon={<RefreshCw size={13} />}
            loading={infoLoading}
            onClick={() => void loadInfo(true)}
          >
            {t("system.redetect")}
          </Button>
        </div>
        {infoError && !info ? (
          <EmptyState
            icon={<AlertTriangle size={20} />}
            title={t("system.inspectFailed")}
            description={infoError}
            action={<Button onClick={() => void loadInfo(true)}>{t("common:actions.retry")}</Button>}
          />
        ) : !info ? (
          <div className="set-loading">
            <Spinner size={16} /> {t("system.detecting")}
          </div>
        ) : (
          <>
            <div className="sys-cards">
              <div className="sys-card">
                <MonitorCog size={16} />
                <div>
                  <div className="sys-card__title">{info.osVersion}</div>
                  <div className="sys-card__sub">
                    {info.kernelVersion
                      ? t("system.hostKernel", { host: info.hostname, arch: info.arch, kernel: info.kernelVersion })
                      : t("system.host", { host: info.hostname, arch: info.arch })}
                  </div>
                </div>
              </div>
              <div className="sys-card">
                <Cpu size={16} />
                <div>
                  <div className="sys-card__title">{info.cpu.model}</div>
                  <div className="sys-card__sub">
                    {[
                      t("system.cpuCores", { cores: info.cpu.physicalCores ?? "?", threads: info.cpu.logicalCores }),
                      info.cpu.frequencyMhz > 0
                        ? t("system.frequency", {
                            value: fmt.number(info.cpu.frequencyMhz / 1000, { minimumFractionDigits: 2, maximumFractionDigits: 2 }),
                          })
                        : null,
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </div>
                </div>
              </div>
              <div className="sys-card">
                <MemoryStick size={16} />
                <div>
                  <div className="sys-card__title">{t("system.ram", { size: fmt.bytes(info.memoryTotalBytes) })}</div>
                  <div className="sys-card__sub">{t("system.swap", { size: fmt.bytes(info.swapTotalBytes) })}</div>
                </div>
              </div>
            </div>

            <h4 className="sys-subtitle">{t("system.graphics")}</h4>
            {info.gpus.length === 0 ? (
              <p className="set-muted">{t("system.noGpu")}</p>
            ) : (
              <ul className="sys-list">
                {info.gpus.map((g) => (
                  <li key={`${g.name}-${g.source}`}>
                    <span className="sys-list__main">{g.name}</span>
                    <span className="set-muted">
                      {[
                        g.vramBytes != null ? fmt.bytes(g.vramBytes) : t("system.vramUnknown"),
                        g.driverVersion ? t("system.driver", { version: g.driverVersion }) : null,
                        g.source,
                      ]
                        .filter(Boolean)
                        .join(" · ")}
                    </span>
                  </li>
                ))}
              </ul>
            )}

            <h4 className="sys-subtitle">{t("system.disks")}</h4>
            <ul className="sys-list">
              {info.disks.map((d) => (
                <li key={d.mountPoint}>
                  <HardDrive size={13} />
                  <span className="sys-list__main">
                    {d.mountPoint} {d.isDataVolume && <Badge tone="accent">{t("system.orkestrData")}</Badge>}
                  </span>
                  <span className="set-muted">
                    {[
                      t("system.diskFree", { free: fmt.bytes(d.availableBytes), total: fmt.bytes(d.totalBytes) }),
                      d.fileSystem,
                      d.kind.toUpperCase(),
                    ].join(" · ")}
                  </span>
                </li>
              ))}
            </ul>

            <h4 className="sys-subtitle">{t("system.runtimes")}</h4>
            <table className="sys-table">
              <thead>
                <tr>
                  <th>{t("system.runtime")}</th>
                  <th>{t("system.version")}</th>
                  <th>{t("system.location")}</th>
                </tr>
              </thead>
              <tbody>
                {info.runtimes.map((r) => (
                  <tr key={r.id} className={r.available ? undefined : "sys-table__row--missing"}>
                    <td>
                      {r.name} {r.available ? <Badge tone="success">{t("system.found")}</Badge> : <Badge>{t("system.missing")}</Badge>}
                    </td>
                    <td>{r.version ?? "—"}</td>
                    <td className="sys-table__path" title={r.detail ?? r.path ?? undefined}>
                      {r.available ? r.path : (r.detail ?? t("system.notFound"))}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>

            <h4 className="sys-subtitle">{t("system.localModels")}</h4>
            {info.localModels.ollamaReachable ? (
              info.localModels.models.length === 0 ? (
                <p className="set-muted">{t("system.ollamaEmpty", { endpoint: info.localModels.ollamaEndpoint })}</p>
              ) : (
                <ul className="sys-list">
                  {info.localModels.models.map((m) => (
                    <li key={m.name}>
                      <span className="sys-list__main">{m.name}</span>
                      <span className="set-muted">
                        {[m.parameterSize, m.quantization, m.sizeBytes != null ? fmt.bytes(m.sizeBytes) : null]
                          .filter(Boolean)
                          .join(" · ")}
                      </span>
                    </li>
                  ))}
                </ul>
              )
            ) : (
              <p className="set-muted">
                {t("system.ollamaUnreachable", { endpoint: info.localModels.ollamaEndpoint })}
              </p>
            )}
            <p className="set-muted set-muted--small">
              {t("system.detectedAt", { date: fmt.date(info.detectedAt, "dateTime"), duration: fmt.duration(info.detectionMs) })}
            </p>
          </>
        )}
      </SettingGroup>

      <SettingGroup title={t("system.checks")}>
        <div className="set-group__actions">
          <Button
            size="sm"
            variant="ghost"
            leftIcon={<RefreshCw size={13} />}
            loading={checksLoading}
            onClick={() => void runChecks()}
          >
            {t("system.runAgain")}
          </Button>
        </div>
        {checksError && !checks ? (
          <div className="set-alert" role="alert">
            <AlertTriangle size={14} /> {checksError}
          </div>
        ) : !checks ? (
          <div className="set-loading">
            <Spinner size={16} /> {t("system.checking")}
          </div>
        ) : (
          <SystemChecksList checks={checks.checks} />
        )}
      </SettingGroup>
    </div>
  );
}
