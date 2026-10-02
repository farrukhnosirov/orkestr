import type { ResourceSample } from "@orkestr/protocol";
import { Badge, Button, EmptyState, Spinner } from "@orkestr/ui";
import { AlertTriangle, Cpu, HardDrive, MemoryStick, MonitorCog, RefreshCw } from "lucide-react";
import { useEffect } from "react";

import { formatBytes, SettingGroup } from "../controls";
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
  if (!sample) return <div className="set-loading"><Spinner size={16} /> Sampling resources…</div>;
  const mem = sample.memoryTotalBytes > 0 ? (sample.memoryUsedBytes / sample.memoryTotalBytes) * 100 : 0;
  const disk =
    sample.diskTotalBytes > 0 ? ((sample.diskTotalBytes - sample.diskFreeBytes) / sample.diskTotalBytes) * 100 : 0;
  return (
    <div className="sys-live">
      <Meter label="CPU" value={sample.cpuPercent} detail={`${sample.cpuPercent.toFixed(0)} %`} />
      <Meter
        label="Memory"
        value={mem}
        detail={`${formatBytes(sample.memoryUsedBytes)} / ${formatBytes(sample.memoryTotalBytes)}`}
      />
      <Meter label="Data disk" value={disk} detail={`${formatBytes(sample.diskFreeBytes)} free`} />
      <div className="sys-cores" aria-label="Per-core CPU usage">
        {sample.perCorePercent.map((c, i) => (
          <span
            key={i}
            className="sys-cores__bar"
            title={`Core ${i}: ${c.toFixed(0)} %`}
            style={{ ["--h" as string]: `${Math.max(4, c)}%` }}
          />
        ))}
      </div>
      {sample.coreProcess && (
        <div className="sys-proc">
          Orkestr core: {sample.coreProcess.cpuCores.toFixed(2)} cores · {formatBytes(sample.coreProcess.memoryBytes)}
        </div>
      )}
      {sample.topProcesses.length > 0 && (
        <table className="sys-table sys-table--compact">
          <caption className="sr-only">Busiest processes</caption>
          <thead>
            <tr>
              <th>Process</th>
              <th>CPU</th>
              <th>Memory</th>
            </tr>
          </thead>
          <tbody>
            {sample.topProcesses.map((p) => (
              <tr key={p.pid}>
                <td>{p.name}</td>
                <td>{p.cpuPercent.toFixed(1)} %</td>
                <td>{formatBytes(p.memoryBytes)}</td>
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

  useEffect(() => {
    if (!useSystemStore.getState().info) void loadInfo();
    if (!useSystemStore.getState().checks) void runChecks();
    return watchResources();
  }, [loadInfo, runChecks, watchResources]);

  return (
    <div className="set-section">
      <SettingGroup title="Live resources">
        <LiveResources sample={sample} />
      </SettingGroup>

      <SettingGroup title="This machine">
        <div className="set-group__actions">
          <Button
            size="sm"
            variant="ghost"
            leftIcon={<RefreshCw size={13} />}
            loading={infoLoading}
            onClick={() => void loadInfo(true)}
          >
            Re-detect
          </Button>
        </div>
        {infoError && !info ? (
          <EmptyState
            icon={<AlertTriangle size={20} />}
            title="Could not inspect this machine"
            description={infoError}
            action={<Button onClick={() => void loadInfo(true)}>Retry</Button>}
          />
        ) : !info ? (
          <div className="set-loading">
            <Spinner size={16} /> Detecting hardware, runtimes and local models… (first run can take a few seconds)
          </div>
        ) : (
          <>
            <div className="sys-cards">
              <div className="sys-card">
                <MonitorCog size={16} />
                <div>
                  <div className="sys-card__title">{info.osVersion}</div>
                  <div className="sys-card__sub">
                    {info.hostname} · {info.arch}
                    {info.kernelVersion ? ` · kernel ${info.kernelVersion}` : ""}
                  </div>
                </div>
              </div>
              <div className="sys-card">
                <Cpu size={16} />
                <div>
                  <div className="sys-card__title">{info.cpu.model}</div>
                  <div className="sys-card__sub">
                    {info.cpu.physicalCores ?? "?"} cores / {info.cpu.logicalCores} threads
                    {info.cpu.frequencyMhz > 0 ? ` · ${(info.cpu.frequencyMhz / 1000).toFixed(2)} GHz` : ""}
                  </div>
                </div>
              </div>
              <div className="sys-card">
                <MemoryStick size={16} />
                <div>
                  <div className="sys-card__title">{formatBytes(info.memoryTotalBytes)} RAM</div>
                  <div className="sys-card__sub">{formatBytes(info.swapTotalBytes)} swap</div>
                </div>
              </div>
            </div>

            <h4 className="sys-subtitle">Graphics</h4>
            {info.gpus.length === 0 ? (
              <p className="set-muted">No GPU detected.</p>
            ) : (
              <ul className="sys-list">
                {info.gpus.map((g) => (
                  <li key={`${g.name}-${g.source}`}>
                    <span className="sys-list__main">{g.name}</span>
                    <span className="set-muted">
                      {g.vramBytes != null ? formatBytes(g.vramBytes) : "VRAM unknown"}
                      {g.driverVersion ? ` · driver ${g.driverVersion}` : ""} · {g.source}
                    </span>
                  </li>
                ))}
              </ul>
            )}

            <h4 className="sys-subtitle">Disks</h4>
            <ul className="sys-list">
              {info.disks.map((d) => (
                <li key={d.mountPoint}>
                  <HardDrive size={13} />
                  <span className="sys-list__main">
                    {d.mountPoint} {d.isDataVolume && <Badge tone="accent">Orkestr data</Badge>}
                  </span>
                  <span className="set-muted">
                    {formatBytes(d.availableBytes)} free of {formatBytes(d.totalBytes)} · {d.fileSystem} · {d.kind.toUpperCase()}
                  </span>
                </li>
              ))}
            </ul>

            <h4 className="sys-subtitle">Developer runtimes</h4>
            <table className="sys-table">
              <thead>
                <tr>
                  <th>Runtime</th>
                  <th>Version</th>
                  <th>Location</th>
                </tr>
              </thead>
              <tbody>
                {info.runtimes.map((r) => (
                  <tr key={r.id} className={r.available ? undefined : "sys-table__row--missing"}>
                    <td>
                      {r.name} {r.available ? <Badge tone="success">found</Badge> : <Badge>missing</Badge>}
                    </td>
                    <td>{r.version ?? "—"}</td>
                    <td className="sys-table__path" title={r.detail ?? r.path ?? undefined}>
                      {r.available ? r.path : (r.detail ?? "not found")}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>

            <h4 className="sys-subtitle">Local models</h4>
            {info.localModels.ollamaReachable ? (
              info.localModels.models.length === 0 ? (
                <p className="set-muted">Ollama is running at {info.localModels.ollamaEndpoint} but has no models pulled.</p>
              ) : (
                <ul className="sys-list">
                  {info.localModels.models.map((m) => (
                    <li key={m.name}>
                      <span className="sys-list__main">{m.name}</span>
                      <span className="set-muted">
                        {[m.parameterSize, m.quantization, m.sizeBytes != null ? formatBytes(m.sizeBytes) : null]
                          .filter(Boolean)
                          .join(" · ")}
                      </span>
                    </li>
                  ))}
                </ul>
              )
            ) : (
              <p className="set-muted">
                Ollama is not reachable at {info.localModels.ollamaEndpoint}. Install and start Ollama to use local models.
              </p>
            )}
            <p className="set-muted set-muted--small">
              Detected {new Date(info.detectedAt).toLocaleString()} in {info.detectionMs} ms.
            </p>
          </>
        )}
      </SettingGroup>

      <SettingGroup title="Environment checks">
        <div className="set-group__actions">
          <Button
            size="sm"
            variant="ghost"
            leftIcon={<RefreshCw size={13} />}
            loading={checksLoading}
            onClick={() => void runChecks()}
          >
            Run again
          </Button>
        </div>
        {checksError && !checks ? (
          <div className="set-alert" role="alert">
            <AlertTriangle size={14} /> {checksError}
          </div>
        ) : !checks ? (
          <div className="set-loading">
            <Spinner size={16} /> Checking git, shells, data directory, web view and keyring…
          </div>
        ) : (
          <SystemChecksList checks={checks.checks} />
        )}
      </SettingGroup>
    </div>
  );
}
