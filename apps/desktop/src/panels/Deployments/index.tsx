/**
 * Deployments panel (ADR-0041): environments (create / edit with vault
 * credentials), adapter availability, deploy with a redacted plan preview and
 * approval state, live progress + log tail, history and rollback.
 */
import type {
  AdapterInfo,
  Deployment,
  DeployPlan,
  DeploymentTarget,
  EnvironmentRecord,
  Event,
  SecretReference,
} from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner, Tabs } from "@orkestr/ui";
import { clsx } from "clsx";
import { Plus, RefreshCw, Rocket } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { getClient, maybeClient } from "../../core/client";
import { useLanguage, useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import type { PanelProps } from "../types";
import "./deployments.css";
import { EnvironmentForm, type EnvironmentDraft } from "./EnvironmentForm";
import { adapterFor, isActive, kindKey, shortTime, targetKey } from "./logic";
import { AvailabilityBadge, StatusBadge, useAdapterLabel } from "./parts";

type Tab = "environments" | "adapters";
type Notice = { tone: "ok" | "error"; text: string } | null;
type Mode = { kind: "view" } | { kind: "create" } | { kind: "edit"; env: EnvironmentRecord };

const LIVE_EVENTS = new Set(["DeploymentStarted", "DeploymentProgress", "DeploymentFinished", "ApprovalResolved", "ApprovalRequested"]);
const LOG_POLL_MS = 1000;

export default function DeploymentsPanel({ projectId }: PanelProps) {
  const { t } = useT("deploy");
  if (!projectId) {
    return <EmptyState icon={<Rocket size={28} strokeWidth={1.6} />} title={t("noProject.title")} description={t("noProject.description")} />;
  }
  return <Deployments projectId={projectId} />;
}

function Deployments({ projectId }: { projectId: string }) {
  const { t } = useT("deploy");
  const lng = useLanguage();
  const adapterLabel = useAdapterLabel();
  const [tab, setTab] = useState<Tab>("environments");
  const [envs, setEnvs] = useState<EnvironmentRecord[]>([]);
  const [adapters, setAdapters] = useState<AdapterInfo[]>([]);
  const [secrets, setSecrets] = useState<SecretReference[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [history, setHistory] = useState<Deployment[]>([]);
  const [mode, setMode] = useState<Mode>({ kind: "view" });
  const [notice, setNotice] = useState<Notice>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<"delete" | "rollback" | null>(null);
  const [deployOpen, setDeployOpen] = useState(false);
  const [version, setVersion] = useState("");
  const [sourceDir, setSourceDir] = useState("");
  const [plan, setPlan] = useState<DeployPlan | null>(null);
  const [approvals, setApprovals] = useState<Record<string, string>>({});
  const [progress, setProgress] = useState<Record<string, string>>({});
  const [logFor, setLogFor] = useState<string | null>(null);
  const [log, setLog] = useState<string[]>([]);

  const env = envs.find((e) => e.environment.id === selected) ?? null;

  const loadEnvs = useCallback(async () => {
    const c = getClient();
    const [e, a, s] = await Promise.all([
      c.call("environments.list", { projectId }),
      c.call("deploy.adapters", {}),
      c.call("secrets.list", { projectId }).catch(() => ({ secrets: [] as SecretReference[] })),
    ]);
    setEnvs(e.environments);
    setAdapters(a.adapters);
    setSecrets(s.secrets);
    setLoaded(true);
    setSelected((cur) => (cur && e.environments.some((x) => x.environment.id === cur) ? cur : (e.environments[0]?.environment.id ?? null)));
  }, [projectId]);

  const loadHistory = useCallback(async (envId: string | null) => {
    if (!envId) {
      setHistory([]);
      return;
    }
    const c = getClient();
    const r = await c.call("deploy.list", { projectId, environmentId: envId, limit: 25 });
    setHistory(r.deployments);
    const waiting = r.deployments.filter((d) => d.status === "AwaitingApproval");
    for (const d of waiting) {
      const g = await c.call("deploy.get", { deploymentId: d.id });
      if (g.approvalId) setApprovals((cur) => ({ ...cur, [d.id]: g.approvalId as string }));
    }
  }, [projectId]);

  const fail = useCallback((e: unknown) => setNotice({ tone: "error", text: describeError(e) }), []);

  const refresh = useCallback(() => {
    loadEnvs().catch(fail);
  }, [loadEnvs, fail]);

  useEffect(refresh, [refresh]);
  useEffect(() => {
    loadHistory(selected).catch(fail);
  }, [selected, loadHistory, fail]);

  // Live updates: progress lines, reload on start / finish / approvals.
  const reloadTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    const c = maybeClient();
    if (!c) return;
    const off = c.subscribe([topic.project(projectId)], (ev: Event) => {
      if (!LIVE_EVENTS.has(ev.kind)) return;
      if (ev.kind === "DeploymentProgress") {
        const p = ev.payload as { deploymentId: string; message?: string; step: string };
        setProgress((cur) => ({ ...cur, [p.deploymentId]: p.message ?? p.step }));
        return;
      }
      if (reloadTimer.current) clearTimeout(reloadTimer.current);
      reloadTimer.current = setTimeout(() => {
        loadEnvs().catch(() => {});
        loadHistory(selected).catch(() => {});
      }, 150);
    });
    return () => {
      if (reloadTimer.current) clearTimeout(reloadTimer.current);
      off();
    };
  }, [projectId, selected, loadEnvs, loadHistory]);

  // Log tail of the selected (or newest active) deployment.
  const active = history.find((d) => isActive(d.status)) ?? null;
  const logTarget = logFor ?? active?.id ?? history[0]?.id ?? null;
  const logDep = history.find((d) => d.id === logTarget) ?? null;
  useEffect(() => {
    setLog([]);
    if (!logTarget) return;
    let offset = 0;
    let stop = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = async () => {
      try {
        const r = await getClient().call("deploy.logs", { deploymentId: logTarget, offset });
        if (stop) return;
        offset = Number(r.nextOffset);
        if (r.lines.length) setLog((cur) => [...cur, ...r.lines].slice(-2000));
        if (!r.complete) timer = setTimeout(tick, LOG_POLL_MS);
        else loadHistory(selected).catch(() => {});
      } catch {
        /* the log view is best effort */
      }
    };
    void tick();
    return () => {
      stop = true;
      if (timer) clearTimeout(timer);
    };
  }, [logTarget, selected, loadHistory]);

  const run = async (key: string, fn: () => Promise<string | null>) => {
    setBusy(key);
    setNotice(null);
    try {
      const text = await fn();
      if (text) setNotice({ tone: "ok", text });
    } catch (e) {
      fail(e);
    } finally {
      setBusy(null);
      setConfirm(null);
    }
  };

  const targetOf = (key: string): DeploymentTarget | undefined => adapters.find((a) => targetKey(a.target) === key)?.target;

  const save = (d: EnvironmentDraft) =>
    run("save", async () => {
      const c = getClient();
      const target = targetOf(d.targetKey);
      if (mode.kind === "edit") {
        await c.call("environments.update", {
          environmentId: mode.env.environment.id,
          name: d.name,
          kind: d.kind,
          protected: d.protected,
          variables: d.variables,
          secretRefs: d.secretRefs,
          urls: d.urls,
          ...(target ? { target } : {}),
          adapterSettings: d.settings,
        });
        setMode({ kind: "view" });
        await loadEnvs();
        return t("notice.updated");
      }
      const r = await c.call("environments.create", {
        projectId,
        name: d.name,
        kind: d.kind,
        protected: d.protected,
        variables: d.variables,
        secretRefs: d.secretRefs,
        urls: d.urls,
        ...(target ? { target } : {}),
        adapterSettings: d.settings,
      });
      setMode({ kind: "view" });
      await loadEnvs();
      setSelected(r.environment.environment.id);
      return t("notice.created");
    });

  const deployParams = () => ({
    environmentId: env?.environment.id ?? "",
    ...(version.trim() ? { version: version.trim() } : {}),
    ...(sourceDir.trim() ? { sourceDir: sourceDir.trim() } : {}),
  });

  const previewPlan = () =>
    run("plan", async () => {
      setPlan((await getClient().call("deploy.plan", deployParams())).plan);
      return null;
    });

  const start = () =>
    run("deploy", async () => {
      const r = await getClient().call("deploy.start", deployParams());
      if (r.approvalId) setApprovals((cur) => ({ ...cur, [r.deployment.id]: r.approvalId as string }));
      setDeployOpen(false);
      setPlan(null);
      setLogFor(r.deployment.id);
      await loadHistory(env?.environment.id ?? null);
      return r.approvalId ? t("notice.awaitingApproval") : t("notice.started");
    });

  const rollback = () =>
    run("rollback", async () => {
      const r = await getClient().call("deploy.rollback", { environmentId: env?.environment.id ?? "" });
      if (r.approvalId) setApprovals((cur) => ({ ...cur, [r.deployment.id]: r.approvalId as string }));
      setLogFor(r.deployment.id);
      await loadHistory(env?.environment.id ?? null);
      return r.approvalId ? t("notice.awaitingApproval") : t("notice.rollback");
    });

  const resolve = (approvalId: string, decision: "Approve" | "Reject") =>
    run("approve", async () => {
      await getClient().call("approvals.resolve", { approvalId, decision });
      await loadHistory(env?.environment.id ?? null);
      return decision === "Approve" ? t("notice.approved") : t("notice.rejected");
    });

  const cancel = (id: string) =>
    run("cancel", async () => {
      await getClient().call("deploy.cancel", { deploymentId: id });
      await loadHistory(env?.environment.id ?? null);
      return t("notice.cancelled");
    });

  const remove = () =>
    run("delete", async () => {
      await getClient().call("environments.delete", { environmentId: env?.environment.id ?? "" });
      setSelected(null);
      await loadEnvs();
      return t("notice.deleted");
    });

  const current = useMemo(() => history.find((d) => d.status === "Succeeded") ?? null, [history]);
  const tabs = [
    { id: "environments" as const, label: t("tabs.environments") },
    { id: "adapters" as const, label: t("tabs.adapters") },
  ];

  return (
    <div className="dp-panel" data-testid="deployments-panel">
      <div className="dp-toolbar">
        <Tabs items={tabs} value={tab} onChange={setTab} aria-label={t("tabs.environments")} />
        <div className="dp-row">
          <Button size="sm" variant="primary" leftIcon={<Plus size={14} />} onClick={() => setMode({ kind: "create" })} data-testid="env-new">
            {t("actions.newEnvironment")}
          </Button>
          <IconButton label={t("actions.refresh")} icon={<RefreshCw size={14} />} onClick={refresh} tooltip />
        </div>
      </div>
      {notice && (
        <p className={notice.tone === "ok" ? "dp-ok" : "dp-error"} role={notice.tone === "ok" ? "status" : "alert"} data-testid="dp-notice">
          {notice.text}
        </p>
      )}
      {!loaded && (
        <div className="dp-center">
          <Spinner />
        </div>
      )}
      {loaded && tab === "adapters" && (
        <ul className="dp-adapters" data-testid="adapters">
          {adapters.map((a) => (
            <li key={a.id} className="dp-card" data-testid={`adapter-${a.id}`}>
              <div className="dp-row dp-row--between">
                <strong>{adapterLabel(a.id)}</strong>
                <AvailabilityBadge adapter={a} />
              </div>
              <p className="dp-muted">{a.verification === "live" ? t("adapters.live") : t("adapters.requestShape")}</p>
              <p className="dp-muted">{t("adapters.settings", { keys: Object.keys((a.settingsSchema as { properties?: object })?.properties ?? {}).join(", ") })}</p>
            </li>
          ))}
        </ul>
      )}
      {loaded && tab === "environments" && (
        <div className="dp-body">
          <aside className="dp-list" aria-label={t("env.list")}>
            {envs.length === 0 && <EmptyState title={t("env.empty")} description={t("env.emptyHint")} />}
            {envs.map((e) => {
              const ad = adapterFor(adapters, e.target);
              return (
                <button
                  key={e.environment.id}
                  type="button"
                  className={clsx("dp-env", e.environment.id === selected && "dp-env--active")}
                  onClick={() => {
                    setSelected(e.environment.id);
                    setMode({ kind: "view" });
                    setLogFor(null);
                    setPlan(null);
                    setDeployOpen(false);
                  }}
                  data-testid={`env-${e.environment.name}`}
                >
                  <span className="dp-row dp-row--between">
                    <strong>{e.environment.name}</strong>
                    {e.lastDeployment && <StatusBadge status={e.lastDeployment.status} />}
                  </span>
                  <span className="dp-muted">
                    {t(`kind.${kindKey(e.environment.kind)}`)} · {ad ? adapterLabel(ad.id) : t("env.noTarget")}
                    {e.environment.protected && (
                      <Badge tone="warning" className="dp-inline">
                        {t("env.protected")}
                      </Badge>
                    )}
                  </span>
                </button>
              );
            })}
          </aside>
          <section className="dp-detail">
            {mode.kind !== "view" ? (
              <EnvironmentForm
                key={mode.kind === "edit" ? mode.env.environment.id : "new"}
                initial={mode.kind === "edit" ? mode.env : null}
                adapters={adapters}
                secrets={secrets}
                busy={busy === "save"}
                onSave={save}
                onCancel={() => setMode({ kind: "view" })}
              />
            ) : !env ? (
              envs.length > 0 && <p className="dp-muted">{t("env.select")}</p>
            ) : (
              <div className="dp-stack" data-testid="env-detail">
                <header className="dp-row dp-row--between">
                  <div>
                    <h3 className="dp-h3">{env.environment.name}</h3>
                    <p className="dp-muted" data-testid="env-current">
                      {t("env.current")}: {current ? (current.version ?? "") : t("env.nothing")}
                      {current?.url && (
                        <a className="dp-link" href={current.url} target="_blank" rel="noreferrer">
                          {current.url}
                        </a>
                      )}
                    </p>
                  </div>
                  <div className="dp-row">
                    <Button size="sm" variant="primary" onClick={() => setDeployOpen((v) => !v)} disabled={!env.target || !!active} data-testid="deploy-open">
                      {t("actions.deploy")}
                    </Button>
                    {confirm === "rollback" ? (
                      <Button size="sm" variant="danger" loading={busy === "rollback"} onClick={rollback} data-testid="rollback-confirm">
                        {t("actions.confirmRollback")}
                      </Button>
                    ) : (
                      <Button size="sm" onClick={() => setConfirm("rollback")} disabled={!current || !!active} data-testid="rollback">
                        {t("actions.rollback")}
                      </Button>
                    )}
                    <Button size="sm" variant="ghost" onClick={() => setMode({ kind: "edit", env })} data-testid="env-edit">
                      {t("actions.edit")}
                    </Button>
                    {confirm === "delete" ? (
                      <Button size="sm" variant="danger" loading={busy === "delete"} onClick={remove} data-testid="env-delete-confirm">
                        {t("actions.confirmDelete")}
                      </Button>
                    ) : (
                      <Button size="sm" variant="ghost" onClick={() => setConfirm("delete")} disabled={!!active} data-testid="env-delete">
                        {t("actions.delete")}
                      </Button>
                    )}
                  </div>
                </header>

                {deployOpen && (
                  <div className="dp-card" data-testid="deploy-box">
                    <div className="dp-grid2">
                      <Input label={t("deploy.version")} value={version} onChange={(e) => setVersion(e.target.value)} data-testid="deploy-version" />
                      <Input label={t("deploy.sourceDir")} value={sourceDir} onChange={(e) => setSourceDir(e.target.value)} />
                    </div>
                    <div className="dp-row">
                      <Button size="sm" onClick={previewPlan} loading={busy === "plan"} data-testid="deploy-plan">
                        {t("actions.plan")}
                      </Button>
                      <Button size="sm" variant="primary" onClick={start} loading={busy === "deploy"} disabled={!plan} data-testid="deploy-start">
                        {t("actions.confirmDeploy")}
                      </Button>
                      <Button size="sm" variant="ghost" onClick={() => setDeployOpen(false)}>
                        {t("actions.close")}
                      </Button>
                    </div>
                    {plan && <PlanView plan={plan} />}
                  </div>
                )}

                {active && (
                  <div className="dp-card dp-active" data-testid="deploy-active">
                    <div className="dp-row dp-row--between">
                      <span className="dp-row">
                        <StatusBadge status={active.status} testId="active-status" />
                        <code>{active.version}</code>
                        {active.status === "Running" && <Spinner size={14} />}
                      </span>
                      <Button size="sm" variant="danger" onClick={() => cancel(active.id)} loading={busy === "cancel"} data-testid="deploy-cancel">
                        {t("actions.cancel")}
                      </Button>
                    </div>
                    {progress[active.id] && <p className="dp-muted dp-mono">{progress[active.id]}</p>}
                    {active.status === "AwaitingApproval" && (
                      <div className="dp-row">
                        <span className="dp-warn">{t("deploy.awaiting")}</span>
                        {approvals[active.id] && (
                          <>
                            <Button size="sm" variant="primary" onClick={() => resolve(approvals[active.id] as string, "Approve")} data-testid="approve">
                              {t("actions.approve")}
                            </Button>
                            <Button size="sm" variant="ghost" onClick={() => resolve(approvals[active.id] as string, "Reject")} data-testid="reject">
                              {t("actions.reject")}
                            </Button>
                          </>
                        )}
                      </div>
                    )}
                  </div>
                )}

                <div>
                  <h4 className="dp-h4">{t("history.title")}</h4>
                  {history.length === 0 && <p className="dp-muted">{t("history.empty")}</p>}
                  <ul className="dp-history" data-testid="history">
                    {history.map((d) => (
                      <li key={d.id}>
                        <button type="button" className={clsx("dp-hrow", d.id === logTarget && "dp-hrow--active")} onClick={() => setLogFor(d.id)} data-testid={`history-${d.id}`}>
                          <StatusBadge status={d.status} />
                          <code>{d.version}</code>
                          <span className="dp-muted">{shortTime(d.finishedAt ?? d.startedAt ?? d.createdAt, lng)}</span>
                          {d.url && <span className="dp-muted dp-ellipsis">{d.url}</span>}
                        </button>
                      </li>
                    ))}
                  </ul>
                </div>

                {logDep && (
                  <div>
                    <h4 className="dp-h4">
                      {t("log.title", { version: logDep.version ?? "" })}
                      {isActive(logDep.status) && (
                        <Badge tone="info" className="dp-inline">
                          {t("log.live")}
                        </Badge>
                      )}
                    </h4>
                    <pre className="dp-log" data-testid="deploy-log">
                      {log.length === 0 ? t("log.empty") : log.join("\n")}
                    </pre>
                  </div>
                )}
              </div>
            )}
          </section>
        </div>
      )}
    </div>
  );
}

function PlanView({ plan }: { plan: DeployPlan }) {
  const { t } = useT("deploy");
  return (
    <div className="dp-plan" data-testid="deploy-plan-view">
      <Badge tone={plan.requiresApproval ? "warning" : "success"} data-testid="plan-approval">
        {plan.requiresApproval ? t("deploy.requiresApproval") : t("deploy.noApproval")}
      </Badge>
      <h5 className="dp-h5">{t("deploy.steps")}</h5>
      <ol className="dp-steps">
        {plan.steps.map((s, i) => (
          <li key={i}>{s}</li>
        ))}
      </ol>
      <h5 className="dp-h5">{t("deploy.requests")}</h5>
      <ul className="dp-requests">
        {plan.requests.map((r, i) => (
          <li key={i}>
            <code>
              {r.method} {r.target}
            </code>
            {r.body && <pre className="dp-log dp-log--small">{r.body}</pre>}
          </li>
        ))}
      </ul>
      {plan.warnings.length > 0 && (
        <>
          <h5 className="dp-h5">{t("deploy.warnings")}</h5>
          <ul className="dp-warnings" data-testid="plan-warnings">
            {plan.warnings.map((w, i) => (
              <li key={i} className="dp-warn">
                {w}
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}
