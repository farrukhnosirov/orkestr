/**
 * Bug Detective panel (Phase 7, ADR-0042). **Owner: WP6.**
 *
 * Incidents of the active project (live via `IncidentOpened` /
 * `IncidentStageChanged`, plus a slow poll), open alerts with "Investigate",
 * a manual report form, and per incident: stage timeline, hypothesis card
 * with confidence, reproduction / fix test output, evidence viewer, the diff
 * of the fix branch, and the two human gates (approve fix, approve merge)
 * plus cancel, and the detective settings (`settings.tsx`). Everything goes through `detective.*` RPC; areas a core does
 * not implement yet (`unimplementedFeature`) degrade to a notice.
 */
import type {
  Alert,
  Event,
  Evidence,
  FileDiff,
  Incident,
  IncidentRun,
  IncidentStage,
} from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Spinner } from "@orkestr/ui";
import { Bug, Check, Code2, GitMerge, Search, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { getClient, maybeClient } from "../../core/client";
import { t as tr, useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { openFileInVsCode } from "../../features/vscode/quickAccess";
import type { PanelProps } from "../types";
import {
  budgetPct,
  confidencePct,
  confidenceTone,
  groupEvidence,
  isGate,
  isTerminal,
  sortIncidents,
  stageTone,
  timeline,
  triggerLabel,
  usd,
} from "./model";
import { DetectiveSettingsForm } from "./settings";
import "./detective.css";

const POLL_MS = 5000;

function isUnimplemented(e: unknown): boolean {
  return (
    typeof e === "object" &&
    e !== null &&
    "data" in e &&
    !!(e as { data?: { unimplementedFeature?: unknown } }).data
      ?.unimplementedFeature
  );
}

export default function BugDetectivePanel({ projectId }: PanelProps) {
  const { t } = useT("detective");
  if (!projectId) {
    return (
      <EmptyState
        icon={<Bug size={28} strokeWidth={1.6} />}
        title={t("title")}
        description={t("noProject")}
      />
    );
  }
  return <Detective projectId={projectId} />;
}

function Detective({ projectId }: { projectId: string }) {
  const { t } = useT("detective");
  const [incidents, setIncidents] = useState<Incident[]>([]);
  const [alerts, setAlerts] = useState<Alert[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [showClosed, setShowClosed] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [off, setOff] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [reporting, setReporting] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const lastActive = useRef(new Map<string, IncidentStage>());

  const refresh = useCallback(async () => {
    const c = getClient();
    const [inc, al] = await Promise.allSettled([
      c.call("detective.list", {
        projectId,
        includeClosed: showClosed,
        limit: 100,
      }),
      c.call("monitor.alerts", {
        projectId,
        includeResolved: false,
        limit: 20,
      }),
    ]);
    if (inc.status === "fulfilled") {
      setIncidents(sortIncidents(inc.value.incidents));
      setError(null);
      setOff(false);
    } else if (isUnimplemented(inc.reason)) setOff(true);
    else setError(describeError(inc.reason));
    setAlerts(al.status === "fulfilled" ? al.value.alerts : []);
    setLoaded(true);
  }, [projectId, showClosed]);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    const c = maybeClient();
    if (!c) return undefined;
    return c.subscribe([topic.project(projectId)], (ev: Event) => {
      if (ev.kind === "IncidentStageChanged") {
        if (!isTerminal(ev.payload.from))
          lastActive.current.set(ev.payload.incidentId, ev.payload.from);
        void refresh();
      } else if (ev.kind === "IncidentOpened") {
        void refresh();
      }
    });
  }, [projectId, refresh]);

  const act = useCallback(
    async (fn: () => Promise<unknown>) => {
      try {
        await fn();
        setError(null);
      } catch (e) {
        setError(describeError(e));
      }
      await refresh();
    },
    [refresh],
  );

  const openFromAlert = (a: Alert) =>
    act(async () => {
      const r = await getClient().call("detective.open", {
        projectId,
        trigger: { type: "Alert", alertId: a.id },
      });
      setSelected(r.incident.id);
    });

  const current = incidents.find((i) => i.id === selected) ?? incidents[0];
  const investigated = new Set(
    incidents
      .filter((i) => !isTerminal(i.stage) && i.trigger.type === "Alert")
      .map((i) => (i.trigger.type === "Alert" ? i.trigger.alertId : "")),
  );

  if (off) {
    return (
      <EmptyState
        icon={<Bug size={28} strokeWidth={1.6} />}
        title={t("title")}
        description={t("unavailable")}
      />
    );
  }
  return (
    <div className="bd-panel" data-testid="detective-panel">
      <div className="bd-toolbar">
        <strong>{t("title")}</strong>
        <div className="bd-toolbar-actions">
          <label className="bd-check">
            <input
              type="checkbox"
              checked={showClosed}
              onChange={(e) => setShowClosed(e.target.checked)}
            />{" "}
            {t("showClosed")}
          </label>
          <Button
            size="sm"
            variant="primary"
            onClick={() => setReporting((v) => !v)}
            data-testid="detective-report"
          >
            {t("report.open")}
          </Button>
          <Button
            size="sm"
            variant={settingsOpen ? "secondary" : "ghost"}
            aria-expanded={settingsOpen}
            onClick={() => setSettingsOpen((v) => !v)}
            data-testid="detective-settings-toggle"
          >
            {t("settings.open")}
          </Button>
          <Button size="sm" onClick={() => void refresh()}>
            {t("refresh")}
          </Button>
        </div>
      </div>
      {error && (
        <div className="bd-error" role="alert">
          {error}
        </div>
      )}
      {settingsOpen && (
        <DetectiveSettingsForm onClose={() => setSettingsOpen(false)} />
      )}
      {reporting && (
        <ReportForm
          onCancel={() => setReporting(false)}
          onSubmit={(description, budgetUsd) =>
            act(async () => {
              const r = await getClient().call("detective.open", {
                projectId,
                trigger: { type: "Manual", description },
                ...(budgetUsd ? { budgetUsd } : {}),
              });
              setReporting(false);
              setSelected(r.incident.id);
            })
          }
        />
      )}
      <div className="bd-body">
        <aside className="bd-side">
          {alerts.length > 0 && (
            <section className="bd-alerts" aria-label={t("alerts.title")}>
              <h4>{t("alerts.title")}</h4>
              {alerts.map((a) => (
                <div key={a.id} className="bd-alert">
                  <Badge
                    tone={
                      a.severity === "Critical"
                        ? "danger"
                        : a.severity === "Warning"
                          ? "warning"
                          : "info"
                    }
                  >
                    {a.severity}
                  </Badge>
                  <span className="bd-alert-title" title={a.message}>
                    {a.title}
                  </span>
                  <Button
                    size="sm"
                    leftIcon={<Search size={12} />}
                    disabled={investigated.has(a.id)}
                    onClick={() => void openFromAlert(a)}
                    data-testid={`investigate-${a.id}`}
                  >
                    {investigated.has(a.id)
                      ? t("alerts.investigating")
                      : t("alerts.investigate")}
                  </Button>
                </div>
              ))}
            </section>
          )}
          {!loaded ? (
            <Spinner label={t("loading")} />
          ) : incidents.length === 0 ? (
            <p className="bd-muted">{t("empty")}</p>
          ) : (
            <ul className="bd-list" aria-label={t("incidents")}>
              {incidents.map((i) => (
                <li key={i.id}>
                  <button
                    type="button"
                    className={`bd-item${current?.id === i.id ? " bd-item--active" : ""}`}
                    onClick={() => setSelected(i.id)}
                    data-testid={`incident-${i.id}`}
                  >
                    <span className="bd-item-title">{i.title}</span>
                    <Badge tone={stageTone(i.stage)}>
                      {t(`stage.${i.stage}`)}
                    </Badge>
                    <span className="bd-muted bd-small">
                      {new Date(i.createdAt).toLocaleString()}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </aside>
        <main className="bd-main">
          {current ? (
            <IncidentView
              key={current.id}
              inc={current}
              lastActive={lastActive.current.get(current.id)}
              onApproveFix={() =>
                act(() =>
                  getClient().call("detective.approveFix", {
                    incidentId: current.id,
                  }),
                )
              }
              onApproveMerge={() =>
                act(() =>
                  getClient().call("detective.approveMerge", {
                    incidentId: current.id,
                  }),
                )
              }
              onCancel={() =>
                act(() =>
                  getClient().call("detective.cancel", {
                    incidentId: current.id,
                  }),
                )
              }
            />
          ) : (
            loaded && (
              <EmptyState
                icon={<Bug size={28} strokeWidth={1.6} />}
                title={t("title")}
                description={t("intro")}
              />
            )
          )}
        </main>
      </div>
    </div>
  );
}

function ReportForm({
  onSubmit,
  onCancel,
}: {
  onSubmit: (d: string, budget?: number) => void;
  onCancel: () => void;
}) {
  const { t } = useT("detective");
  const [text, setText] = useState("");
  const [budget, setBudget] = useState("");
  const b = Number(budget);
  return (
    <form
      className="bd-report"
      onSubmit={(e) => {
        e.preventDefault();
        if (text.trim()) onSubmit(text.trim(), budget && b > 0 ? b : undefined);
      }}
    >
      <label>
        {t("report.description")}
        <textarea
          value={text}
          rows={4}
          onChange={(e) => setText(e.target.value)}
          placeholder={t("report.placeholder")}
          data-testid="report-text"
        />
      </label>
      <label className="bd-inline">
        {t("report.budget")}
        <input
          type="number"
          min="0.01"
          step="0.01"
          value={budget}
          onChange={(e) => setBudget(e.target.value)}
          placeholder="2.00"
        />
      </label>
      <div className="bd-row">
        <Button
          type="submit"
          size="sm"
          variant="primary"
          disabled={!text.trim()}
          data-testid="report-submit"
        >
          {t("report.submit")}
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={onCancel}>
          {t("cancel")}
        </Button>
      </div>
    </form>
  );
}

interface ViewProps {
  inc: Incident;
  lastActive?: IncidentStage;
  onApproveFix: () => Promise<void>;
  onApproveMerge: () => Promise<void>;
  onCancel: () => Promise<void>;
}

function IncidentView({
  inc,
  lastActive,
  onApproveFix,
  onApproveMerge,
  onCancel,
}: ViewProps) {
  const { t } = useT("detective");
  const [confirmMerge, setConfirmMerge] = useState(false);
  const [busy, setBusy] = useState(false);
  const [focus, setFocus] = useState<string | null>(null);
  const run = (fn: () => Promise<void>) => async () => {
    setBusy(true);
    try {
      await fn();
    } finally {
      setBusy(false);
      setConfirmMerge(false);
    }
  };
  const trig = triggerLabel(inc);
  const trigText =
    trig.key === "trigger.alert"
      ? t("trigger.alert", { id: trig.values.id })
      : trig.key === "trigger.log"
        ? t("trigger.log", { source: trig.values.source })
        : trig.key === "trigger.test"
          ? t("trigger.test", { run: trig.values.run })
          : t("trigger.manual");
  const h = inc.hypothesis;
  return (
    <div className="bd-incident" data-testid="incident-view">
      <header className="bd-head">
        <div>
          <h3>{inc.title}</h3>
          <div className="bd-muted bd-small">
            {trigText} ·{" "}
            {t("budget", { spent: usd(inc.spentUsd), cap: usd(inc.budgetUsd) })}
          </div>
          <div className="bd-meter" aria-label={t("budgetLabel")}>
            <span style={{ width: `${budgetPct(inc)}%` }} />
          </div>
        </div>
        <div className="bd-row">
          {inc.stage === "AwaitingFixApproval" && (
            <Button
              size="sm"
              variant="primary"
              leftIcon={<Check size={14} />}
              loading={busy}
              onClick={run(onApproveFix)}
              data-testid="approve-fix"
            >
              {t("actions.approveFix")}
            </Button>
          )}
          {inc.stage === "AwaitingMergeApproval" &&
            (confirmMerge ? (
              <Button
                size="sm"
                variant="danger"
                leftIcon={<GitMerge size={14} />}
                loading={busy}
                onClick={run(onApproveMerge)}
                data-testid="confirm-merge"
              >
                {t("actions.confirmMerge", { branch: inc.fixBranch ?? "" })}
              </Button>
            ) : (
              <Button
                size="sm"
                variant="primary"
                leftIcon={<GitMerge size={14} />}
                onClick={() => setConfirmMerge(true)}
                data-testid="approve-merge"
              >
                {t("actions.approveMerge")}
              </Button>
            ))}
          {!isTerminal(inc.stage) && (
            <Button
              size="sm"
              variant="ghost"
              leftIcon={<X size={14} />}
              disabled={busy}
              onClick={run(onCancel)}
              data-testid="cancel-incident"
            >
              {t("actions.cancel")}
            </Button>
          )}
        </div>
      </header>

      {isGate(inc.stage) && (
        <p className="bd-gate">
          {t(inc.stage === "AwaitingFixApproval" ? "gate.fix" : "gate.merge", {
            branch: inc.fixBranch ?? `detective/${inc.id}`,
          })}
        </p>
      )}
      {inc.error && (
        <p
          className={inc.stage === "Failed" ? "bd-error" : "bd-muted"}
          data-testid="incident-error"
        >
          {inc.error}
        </p>
      )}

      <ol className="bd-timeline" aria-label={t("timeline")}>
        {timeline(inc, lastActive).map((s) => (
          <li
            key={s.stage}
            className={`bd-step bd-step--${s.state}`}
            title={t(`stepState.${s.state}`)}
          >
            <span className="bd-dot" />
            {t(`stage.${s.stage}`)}
          </li>
        ))}
      </ol>

      {h && (
        <section className="bd-card" data-testid="hypothesis">
          <div className="bd-card-head">
            <h4>{t("hypothesis.title")}</h4>
            <Badge tone={confidenceTone(h.confidence)}>
              {t("hypothesis.confidence", { pct: `${confidencePct(h.confidence)}%` })}
            </Badge>
          </div>
          <div className="bd-meter bd-meter--conf">
            <span style={{ width: `${confidencePct(h.confidence)}%` }} />
          </div>
          <p>
            <strong>{h.summary}</strong>
          </p>
          <h5>{t("hypothesis.rootCause")}</h5>
          <p className="bd-pre-wrap">{h.rootCause}</p>
          <h5>{t("hypothesis.fix")}</h5>
          <p className="bd-pre-wrap">{h.proposedFix}</p>
          {h.suspectedFiles.length > 0 && (
            <>
              <h5>{t("hypothesis.files")}</h5>
              <ul className="bd-files">
                {h.suspectedFiles.map((f) => (
                  <li key={f}>
                    <code>{f}</code>
                    <IconButton
                      size="sm"
                      label={tr("vscode:openFile.action")}
                      icon={<Code2 size={12} />}
                      onClick={() => void openFileInVsCode({ projectId: inc.projectId, path: f })}
                    />
                  </li>
                ))}
              </ul>
            </>
          )}
          {h.reproduction && (
            <p className="bd-small">
              {t("hypothesis.reproduction")} <code>{h.reproduction}</code>
            </p>
          )}
          <div className="bd-refs">
            {h.evidenceRefs.map((r) => (
              <button
                key={r}
                type="button"
                className="bd-ref"
                onClick={() => setFocus(r)}
              >
                {r}
              </button>
            ))}
          </div>
        </section>
      )}

      {inc.reproduction && (
        <RunCard
          title={t("runs.reproduction")}
          run={inc.reproduction}
          expectFail
        />
      )}
      {inc.fixTests && (
        <RunCard title={t("runs.fixTests")} run={inc.fixTests} />
      )}
      {inc.fixBranch && <FixDiff inc={inc} />}
      {inc.reviewId && (
        <p className="bd-small bd-muted">
          {t("review", { id: inc.reviewId.slice(0, 8) })}
        </p>
      )}

      <EvidenceViewer items={inc.evidence} focus={focus} />
    </div>
  );
}

function RunCard({
  title,
  run,
  expectFail,
}: {
  title: string;
  run: IncidentRun;
  expectFail?: boolean;
}) {
  const { t } = useT("detective");
  const good = expectFail ? !run.passed : run.passed;
  return (
    <section className="bd-card">
      <div className="bd-card-head">
        <h4>{title}</h4>
        <Badge tone={good ? "success" : "warning"}>
          {run.passed ? t("runs.passed") : t("runs.failed")}
        </Badge>
      </div>
      <code className="bd-small">{run.command}</code>
      {run.outputTail && <pre className="bd-pre">{run.outputTail}</pre>}
    </section>
  );
}

function FixDiff({ inc }: { inc: Incident }) {
  const { t } = useT("detective");
  const [files, setFiles] = useState<FileDiff[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const branch = inc.fixBranch ?? "";
  const ready =
    inc.stage === "Testing" ||
    inc.stage === "Reviewing" ||
    inc.stage === "AwaitingMergeApproval" ||
    inc.stage === "Resolved" ||
    !!inc.fixTests;
  useEffect(() => {
    if (!ready || inc.stage === "Resolved") return;
    let alive = true;
    getClient()
      .call("vcs.diff", {
        projectId: inc.projectId,
        target: { kind: "Range", from: "HEAD", to: branch },
        maxPatchBytes: 128 * 1024,
      })
      .then((d) => alive && setFiles(d.files))
      .catch((e: unknown) => alive && setErr(describeError(e)));
    return () => {
      alive = false;
    };
  }, [inc.projectId, branch, ready, inc.stage, inc.updatedAt]);
  return (
    <section className="bd-card" data-testid="fix-diff">
      <div className="bd-card-head">
        <h4>{t("diff.title")}</h4>
        <code className="bd-small">{branch}</code>
      </div>
      {inc.stage === "Resolved" ? (
        <p className="bd-muted">{t("diff.merged")}</p>
      ) : !ready ? (
        <p className="bd-muted">{t("diff.pending")}</p>
      ) : err ? (
        <p className="bd-error">{err}</p>
      ) : files === null ? (
        <Spinner size={14} />
      ) : files.length === 0 ? (
        <p className="bd-muted">{t("diff.empty")}</p>
      ) : (
        files.map((f) => (
          <details key={f.path} open>
            <summary>
              <code>{f.path}</code>{" "}
              <span className="bd-add">+{f.additions}</span>{" "}
              <span className="bd-del">−{f.deletions}</span>
            </summary>
            <pre className="bd-pre bd-diff">
              {f.patch.split("\n").map((l, i) => (
                <span
                  key={i}
                  className={
                    l.startsWith("+")
                      ? "bd-add"
                      : l.startsWith("-")
                        ? "bd-del"
                        : undefined
                  }
                >
                  {l}
                  {"\n"}
                </span>
              ))}
            </pre>
          </details>
        ))
      )}
    </section>
  );
}

function EvidenceViewer({
  items,
  focus,
}: {
  items: Evidence[];
  focus: string | null;
}) {
  const { t } = useT("detective");
  const groups = useMemo(() => groupEvidence(items), [items]);
  const refs = useRef(new Map<string, HTMLDetailsElement>());
  useEffect(() => {
    if (!focus) return;
    const el = refs.current.get(focus);
    if (el) {
      el.open = true;
      el.scrollIntoView?.({ block: "nearest" });
    }
  }, [focus]);
  if (items.length === 0) return null;
  return (
    <section className="bd-card" data-testid="evidence">
      <h4>{t("evidence.title", { count: items.length })}</h4>
      {groups.map((g) => (
        <div key={g.kind} className="bd-ev-group">
          <h5>{t(`kind.${g.kind}`)}</h5>
          {g.items.map((e) => (
            <details
              key={e.id}
              className={focus === e.id ? "bd-ev bd-ev--focus" : "bd-ev"}
              ref={(el) => {
                if (el) refs.current.set(e.id, el);
              }}
            >
              <summary>
                <span className="bd-ev-id">{e.id}</span> {e.title}
                {e.reference && (
                  <code className="bd-small"> {e.reference}</code>
                )}
              </summary>
              <pre className="bd-pre">{e.content}</pre>
            </details>
          ))}
        </div>
      ))}
    </section>
  );
}
