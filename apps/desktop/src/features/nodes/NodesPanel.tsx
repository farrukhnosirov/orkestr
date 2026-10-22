/**
 * Nodes panel (Phase 6, multi-node — ADR-0033 / ADR-0039).
 *
 * Tabs: **Nodes** (registry with health / trust / resources / slots / leases,
 * Drain / Set policy / Revoke, Enroll a node, This computer as a node),
 * **Jobs** (state, node, placement explanation, Migrate / Cancel) and
 * **Models** (local store, Import, Distribute). Everything goes through RPC;
 * a core that answers `unimplementedFeature` (RR-G14) gets a notice per area
 * instead of an error. Data refreshes on a short poll while the panel is open.
 */
import type { JobView, NodeSnapshot, StoredModel } from "@orkestr/protocol";
import { Button, EmptyState, Tabs } from "@orkestr/ui";
import { Server } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { EnrollNode, ThisComputerAsNode } from "./Enrollment";
import { JobsTab } from "./JobsTab";
import { isUnimplemented, snapshotsOf } from "./logic";
import { ModelsTab } from "./ModelsTab";
import { NodeCard } from "./NodeCard";
import "../settings/settings.css";
import "./nodes.css";

type Tab = "nodes" | "jobs" | "models";
const POLL_MS = 5000;

export function NodesPanel() {
  const { t } = useT("nodes");
  const [tab, setTab] = useState<Tab>("nodes");
  const [nodes, setNodes] = useState<NodeSnapshot[]>([]);
  const [jobs, setJobs] = useState<JobView[]>([]);
  const [models, setModels] = useState<StoredModel[]>([]);
  const [off, setOff] = useState({ nodes: false, jobs: false, models: false });
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  const refresh = useCallback(async () => {
    const c = getClient();
    const [n, j, m] = await Promise.allSettled([c.call("nodes.list", {}), c.call("jobs.list", {}), c.call("models.list", {})]);
    const errors: unknown[] = [];
    const flags = { nodes: false, jobs: false, models: false };
    if (n.status === "fulfilled") setNodes(snapshotsOf(n.value));
    else if (isUnimplemented(n.reason)) flags.nodes = true;
    else errors.push(n.reason);
    if (j.status === "fulfilled") setJobs(j.value.jobs);
    else if (isUnimplemented(j.reason)) flags.jobs = true;
    else errors.push(j.reason);
    if (m.status === "fulfilled") setModels(m.value.models);
    else if (isUnimplemented(m.reason)) flags.models = true;
    else errors.push(m.reason);
    setOff(flags);
    setError(errors.length ? describeError(errors[0]) : null);
    setLoaded(true);
  }, []);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  const remote = nodes.filter((s) => !s.isCoordinator);
  return (
    <div className="nd-panel" data-testid="nodes-panel">
      <div className="nd-toolbar">
        <Tabs<Tab>
          aria-label={t("title")}
          value={tab}
          onChange={setTab}
          items={[
            { id: "nodes", label: t("tabs.nodes") },
            { id: "jobs", label: t("tabs.jobs") },
            { id: "models", label: t("tabs.models") },
          ]}
        />
        <Button size="sm" onClick={() => void refresh()} data-testid="nodes-refresh">
          {t("refresh")}
        </Button>
      </div>
      {error && (
        <p className="nd-error" role="alert">
          {error}
        </p>
      )}
      <div className="nd-body" role="tabpanel" aria-label={t(`tabs.${tab}`)}>
        {tab === "nodes" && (
          <div className="nd-stack">
            {off.nodes ? (
              <p className="nd-muted" role="status">
                {t("unavailable")}
              </p>
            ) : (
              <>
                {loaded && remote.length === 0 && (
                  <EmptyState icon={<Server size={24} />} title={t("empty.title")} description={t("empty.description")} />
                )}
                <ul className="nd-list" aria-label={t("tabs.nodes")}>
                  {nodes.map((s) => (
                    <NodeCard key={s.node.id} snap={s} onChanged={refresh} />
                  ))}
                </ul>
              </>
            )}
            <EnrollNode onChanged={refresh} />
            <ThisComputerAsNode />
          </div>
        )}
        {tab === "jobs" && <JobsTab jobs={jobs} unavailable={off.jobs} nodes={nodes} onChanged={refresh} />}
        {tab === "models" && <ModelsTab models={models} unavailable={off.models} nodes={nodes} onChanged={refresh} />}
      </div>
    </div>
  );
}
