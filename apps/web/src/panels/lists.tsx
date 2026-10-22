import type { Approval, Project, Task } from "@orkestr/protocol";
import { Badge, Button, EmptyState, Spinner } from "@orkestr/ui";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { useDeps } from "../deps";
import { type RemoteClient, callWithStepUp, errorCode } from "../remote";

function useLoad<T>(load: () => Promise<T>, deps: unknown[]) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reload = useCallback(() => {
    setError(null);
    load().then(setData, (e: unknown) => setError(errorCode(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
  useEffect(reload, [reload]);
  return { data, error, reload };
}

function Status({ error, loading }: { error: string | null; loading: boolean }) {
  const { t } = useTranslation();
  if (error) return <p role="alert" className="web-error">{t(`errors.${error}`, { defaultValue: t("errors.internal") })}</p>;
  return loading ? <Spinner label={t("common.loading")} /> : null;
}

export function ProjectsPanel({
  client,
  selected,
  onSelect,
}: {
  client: RemoteClient;
  selected: string | null;
  onSelect: (id: string) => void;
}) {
  const { t } = useTranslation();
  const { data, error } = useLoad(() => client.call("projects.list", {}), [client]);
  const projects: Project[] = data?.projects ?? [];
  return (
    <section aria-label={t("tabs.projects")}>
      <Status error={error} loading={!data} />
      {data && projects.length === 0 && <EmptyState title={t("projects.empty")} />}
      <ul className="web-list">
        {projects.map((p) => (
          <li key={p.id}>
            <button
              type="button"
              className="web-row"
              aria-pressed={selected === p.id}
              onClick={() => onSelect(p.id)}
            >
              {p.name}
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

export function TasksPanel({ client, projectId }: { client: RemoteClient; projectId: string | null }) {
  const { t } = useTranslation();
  const { data, error } = useLoad(
    () => client.call("tasks.list", projectId ? { projectId } : {}),
    [client, projectId],
  );
  const tasks: Task[] = data?.tasks ?? [];
  return (
    <section aria-label={t("tabs.tasks")}>
      <Status error={error} loading={!data} />
      {data && tasks.length === 0 && <EmptyState title={t("tasks.empty")} />}
      <ul className="web-list">
        {tasks.map((task) => (
          <li key={task.id} className="web-row">
            <span>{task.title}</span>
            <Badge>{t(`taskStatus.${task.status}`, { defaultValue: task.status })}</Badge>
          </li>
        ))}
      </ul>
    </section>
  );
}

export function ApprovalsPanel({ client }: { client: RemoteClient }) {
  const { t } = useTranslation();
  const { stepUp } = useDeps();
  const { data, error, reload } = useLoad(() => client.call("approvals.list", { status: "Pending" }), [client]);
  const [busy, setBusy] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const approvals: Approval[] = data?.approvals ?? [];

  const resolve = async (a: Approval, decision: "Approve" | "Reject") => {
    setBusy(a.id);
    setActionError(null);
    try {
      await callWithStepUp(client, "approvals.resolve", { approvalId: a.id, decision }, stepUp);
      reload();
    } catch (e) {
      setActionError(errorCode(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section aria-label={t("tabs.approvals")}>
      <div className="web-toolbar">
        <Button size="sm" variant="ghost" onClick={reload}>
          {t("common.refresh")}
        </Button>
      </div>
      <Status error={error ?? actionError} loading={!data} />
      {data && approvals.length === 0 && <EmptyState title={t("approvals.empty")} />}
      <ul className="web-list">
        {approvals.map((a) => (
          <li key={a.id} className="web-row web-row--col">
            <div>
              <Badge tone={a.risk === "High" || a.risk === "Critical" ? "danger" : "warning"}>
                {t(`risk.${a.risk}`, { defaultValue: a.risk })}
              </Badge>{" "}
              <span>{a.actionSummary}</span>
            </div>
            <div className="web-actions">
              <Button size="sm" variant="primary" loading={busy === a.id} onClick={() => void resolve(a, "Approve")}>
                {t("approvals.approve")}
              </Button>
              <Button size="sm" variant="danger" disabled={busy === a.id} onClick={() => void resolve(a, "Reject")}>
                {t("approvals.reject")}
              </Button>
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}
