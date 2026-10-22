/**
 * One node: health + trust badges, resources (CPU / RAM / GPUs with VRAM,
 * OS / arch, tools, local models), slots and leases, and the actions
 * Drain / Set policy / Revoke (with confirmation).
 */
import type { NodeSnapshot, TrustLevel } from "@orkestr/protocol";
import { Badge, Button, Input, StatusDot } from "@orkestr/ui";
import { useState } from "react";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { formatBytes, formatDate, formatPercent } from "../../i18n/format";
import { Segmented } from "../settings/controls";
import { parseProjects, statusDot, statusTone, toMb, TRUST_LEVELS } from "./logic";
import { NoticeLine, useAction } from "./useAction";

export function NodeCard({ snap, onChanged }: { snap: NodeSnapshot; onChanged: () => Promise<void> }) {
  const { t } = useT("nodes");
  const { t: tc } = useT("common");
  const { busy, notice, run } = useAction(onChanged);
  const [confirmRevoke, setConfirmRevoke] = useState(false);
  const [editPolicy, setEditPolicy] = useState(false);
  const [trust, setTrust] = useState<TrustLevel>(snap.trustLevel);
  const [projects, setProjects] = useState(snap.allowedProjects.join(", "));
  const [maxJobs, setMaxJobs] = useState(String(snap.slots.max || 1));
  const n = snap.node;
  const r = n.resources;
  const id = n.id;
  const revoked = snap.status === "Revoked";
  const draining = snap.status === "Draining";

  const drain = () =>
    run("drain", async () => {
      await getClient().call("nodes.drain", { nodeId: id, drain: !draining });
      return draining ? t("node.undrained", { name: n.name }) : t("node.drained", { name: n.name });
    });
  const revoke = () =>
    run("revoke", async () => {
      await getClient().call("nodes.revoke", { nodeId: id });
      setConfirmRevoke(false);
      return t("node.revoked", { name: n.name });
    });
  const savePolicy = () =>
    run("policy", async () => {
      const max = Number.parseInt(maxJobs, 10);
      await getClient().call("nodes.setPolicy", {
        nodeId: id,
        trustLevel: trust,
        allowedProjects: parseProjects(projects),
        ...(Number.isFinite(max) && max > 0 ? { maxConcurrentJobs: max } : {}),
      });
      setEditPolicy(false);
      return t("policy.saved");
    });

  return (
    <li className="nd-card" data-testid={`node-${id}`} aria-label={n.name}>
      <div className="nd-card__head">
        <StatusDot status={statusDot(snap.status)} label={t(`status.${snap.status}`)} />
        <strong className="nd-card__name">{n.name}</strong>
        {snap.isCoordinator && <Badge tone="accent">{t("node.coordinator")}</Badge>}
        <Badge tone={statusTone(snap.status)} data-testid={`node-status-${id}`}>
          {t(`status.${snap.status}`)}
        </Badge>
        <Badge tone="violet" data-testid={`node-trust-${id}`}>
          {t(`trust.${snap.trustLevel}`)}
        </Badge>
        {snap.lastHeartbeatAt && <span className="nd-muted">{t("node.lastHeartbeat", { time: formatDate(snap.lastHeartbeatAt, "timeSeconds") })}</span>}
      </div>
      <dl className="nd-facts">
        <dt>{t("node.platform")}</dt>
        <dd>
          {t(`os.${n.os}`)} · {t(`arch.${n.arch}`)}
        </dd>
        <dt>{t("node.cpu")}</dt>
        <dd>{t("node.cpuValue", { cores: r.cpuCores, usage: formatPercent(r.cpuUsagePercent / 100) })}</dd>
        <dt>{t("node.memory")}</dt>
        <dd>{t("node.memoryValue", { used: formatBytes(r.memoryUsedBytes), total: formatBytes(r.memoryTotalBytes) })}</dd>
        <dt>{t("node.gpus")}</dt>
        <dd data-testid={`node-gpus-${id}`}>
          {r.gpus.length === 0
            ? t("node.none")
            : r.gpus.map((g, i) => (
                <span key={`${g.name}-${i}`} className="nd-chip">
                  {g.vramBytes != null ? t("node.gpuValue", { name: g.name, vram: formatBytes(g.vramBytes), mb: toMb(g.vramBytes) }) : g.name}
                </span>
              ))}
        </dd>
        <dt>{t("node.tools")}</dt>
        <dd>
          {n.tools.length === 0
            ? t("node.none")
            : n.tools.map((tool) => (
                <span key={tool} className="nd-chip">
                  {tool}
                </span>
              ))}
        </dd>
        <dt>{t("node.models")}</dt>
        <dd>
          {n.localModels.length === 0
            ? t("node.none")
            : n.localModels.map((m) => (
                <span key={m} className="nd-chip">
                  {m}
                </span>
              ))}
        </dd>
        <dt>{t("node.slots")}</dt>
        <dd data-testid={`node-slots-${id}`}>{t("node.slotsValue", { used: snap.slots.used, max: snap.slots.max })}</dd>
        <dt>{t("node.projects")}</dt>
        <dd>{snap.allowedProjects.includes("*") ? t("node.allProjects") : snap.allowedProjects.join(", ") || t("node.none")}</dd>
      </dl>
      {snap.leases.length > 0 && (
        <ul className="nd-leases" aria-label={t("node.leases")}>
          {snap.leases.map((l) => (
            <li key={`${l.jobId}-${l.epoch}`}>
              <code>{l.jobId.slice(0, 8)}</code> · {t(`lease.${l.phase}`)} · {t("node.epoch", { epoch: l.epoch })}
              {l.progress != null && <> · {formatPercent(l.progress)}</>}
            </li>
          ))}
        </ul>
      )}
      {!snap.isCoordinator && !revoked && (
        <div className="nd-actions">
          <Button size="sm" loading={busy === "drain"} disabled={busy !== null} onClick={() => void drain()} data-testid={`node-drain-${id}`}>
            {draining ? t("node.undrain") : t("node.drain")}
          </Button>
          <Button size="sm" disabled={busy !== null} onClick={() => setEditPolicy((v) => !v)} aria-expanded={editPolicy} data-testid={`node-policy-${id}`}>
            {t("policy.edit")}
          </Button>
          {confirmRevoke ? (
            <span className="nd-confirm" role="group" aria-label={t("node.revokeConfirm", { name: n.name })}>
              <span>{t("node.revokeConfirm", { name: n.name })}</span>
              <Button size="sm" variant="danger" loading={busy === "revoke"} disabled={busy !== null} onClick={() => void revoke()} data-testid={`node-revoke-confirm-${id}`}>
                {t("node.revoke")}
              </Button>
              <Button size="sm" disabled={busy !== null} onClick={() => setConfirmRevoke(false)}>
                {tc("actions.cancel")}
              </Button>
            </span>
          ) : (
            <Button size="sm" variant="danger" disabled={busy !== null} onClick={() => setConfirmRevoke(true)} data-testid={`node-revoke-${id}`}>
              {t("node.revoke")}
            </Button>
          )}
        </div>
      )}
      {editPolicy && (
        <div className="nd-form" data-testid={`node-policy-form-${id}`}>
          <Segmented<TrustLevel>
            label={t("policy.trust")}
            value={trust}
            options={TRUST_LEVELS.map((v) => ({ value: v, label: t(`trust.${v}`) }))}
            onChange={setTrust}
          />
          <p className="nd-muted">{t(`trustHint.${trust}`)}</p>
          <Input label={t("policy.projects")} value={projects} onChange={(e) => setProjects(e.target.value)} />
          <Input label={t("policy.maxJobs")} type="number" min={1} value={maxJobs} onChange={(e) => setMaxJobs(e.target.value)} />
          <div className="nd-actions">
            <Button size="sm" variant="primary" loading={busy === "policy"} disabled={busy !== null} onClick={() => void savePolicy()} data-testid={`node-policy-save-${id}`}>
              {t("policy.save")}
            </Button>
          </div>
        </div>
      )}
      <NoticeLine notice={notice} />
    </li>
  );
}
