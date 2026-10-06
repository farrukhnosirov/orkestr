/**
 * Execution Replay (§21, §68.50) — the "Replay" tab of the Logs panel.
 * Pick a Manager run (or one of its tasks), filter by agent / phase, then
 * step through the reconstructed timeline (request → plan → agents →
 * delegation → tools → files → tests → reviews → approvals → merge) or play
 * it at a chosen speed. The state at the cursor (task statuses, agents,
 * files changed, counters) comes from `replay.step`. Only recorded, visible
 * actions exist — hidden reasoning is never stored.
 */
import type {
  ReplayPhase,
  ReplayRunsResult,
  ReplayStep,
  ReplayStepResult,
  ReplayTimeline,
  ReplayTimelineParams,
} from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { AlertTriangle, FileCode, History, Pause, Play, RefreshCw, SkipBack, SkipForward, StepBack, StepForward } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { openInEditor } from "../../../features/files/actions";
import { useFormat, useT } from "../../../i18n";
import { maybeClient } from "../../../core/client";
import { errorMessage, useRpcQuery } from "../../../lib/rpcQuery";
import { useAppStore } from "../../../store/app";
import { clock, moveCursor, offsetMs, PHASES, SPEEDS, segments, statusTone, stepDelay } from "./model";
import "./replay.css";

const STATE_DEBOUNCE_MS = 120;

export interface ReplayViewProps {
  projectId: string | null;
}

export function ReplayView({ projectId }: ReplayViewProps) {
  const { t } = useT(["replay", "common", "logs"]);
  const fmt = useFormat();
  const connected = useAppStore((s) => s.connection.state === "open");
  const [runId, setRunId] = useState<string | null>(null);
  const [scope, setScope] = useState<string>(""); // "" = whole run, else task id
  const [agentId, setAgentId] = useState<string>("");
  const [hiddenPhases, setHiddenPhases] = useState<Set<ReplayPhase>>(() => new Set());
  const [cursor, setCursor] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeed] = useState(1);
  const [state, setState] = useState<ReplayStepResult | null>(null);
  const rowRefs = useRef(new Map<number, HTMLButtonElement>());

  useEffect(() => {
    setRunId(null);
    setScope("");
    setAgentId("");
  }, [projectId]);

  const topics = projectId ? [topic.project(projectId)] : [];
  const runsQuery = useRpcQuery<ReplayRunsResult>(
    `replay-runs|${projectId ?? ""}`,
    (c) => c.call("replay.runs", { projectId: projectId ?? "" }),
    { enabled: projectId !== null, topics, reloadOn: ["ManagerRunUpdated"] },
  );
  const runs = runsQuery.data?.runs ?? [];
  const run = runs.find((r) => r.runId === runId) ?? null;

  // Default to the newest run; a selection that is not one of this project's
  // runs (e.g. carried over a project switch) falls back to it as well.
  useEffect(() => {
    if (runs[0] && (!runId || !runs.some((r) => r.runId === runId))) setRunId(runs[0].runId);
  }, [runId, runs]);

  const params = useMemo<ReplayTimelineParams | null>(() => {
    if (!projectId || !runId) return null;
    const p: ReplayTimelineParams = scope ? { projectId, taskId: scope } : { projectId, runId };
    if (agentId) p.agentId = agentId;
    if (hiddenPhases.size > 0) p.phases = PHASES.filter((ph) => !hiddenPhases.has(ph));
    return p;
  }, [projectId, runId, scope, agentId, hiddenPhases]);
  const paramsKey = JSON.stringify(params);

  const timelineQuery = useRpcQuery<ReplayTimeline>(
    `replay-tl|${paramsKey}`,
    async (c) => (await c.call("replay.timeline", params ?? { projectId: "" })).timeline,
    {
      enabled: params !== null,
      topics,
      reloadOn: run?.status === "Running" || run?.status === "Paused" ? ["ManagerRunUpdated", "ToolCallFinished", "TaskStatusChanged"] : [],
    },
  );
  const timeline = timelineQuery.data;
  const steps = timeline?.steps ?? [];
  const total = steps.length;

  // New subject / filters → back to the first step.
  useEffect(() => {
    setCursor(0);
    setPlaying(false);
    setState(null);
  }, [paramsKey]);

  // Keep the cursor inside the timeline.
  useEffect(() => {
    if (total > 0 && cursor > total - 1) setCursor(total - 1);
  }, [total, cursor]);

  // Playback.
  useEffect(() => {
    if (!playing) return;
    if (cursor >= total - 1) {
      setPlaying(false);
      return;
    }
    const id = setTimeout(() => setCursor((c) => moveCursor(c, total, "next")), stepDelay(speed));
    return () => clearTimeout(id);
  }, [playing, cursor, total, speed]);

  // State at the cursor (step-through API; latest request wins).
  useEffect(() => {
    if (!params || total === 0 || !connected) return;
    const client = maybeClient();
    if (!client) return;
    let cancelled = false;
    const id = setTimeout(() => {
      client
        .call("replay.step", { ...params, index: cursor, direction: "Goto" })
        .then((res) => {
          if (!cancelled) setState(res);
        })
        .catch((e: unknown) => console.warn("replay.step failed", errorMessage(e)));
    }, STATE_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(id);
    };
    // `params` is captured through paramsKey.
  }, [paramsKey, cursor, total, connected]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    rowRefs.current.get(cursor)?.scrollIntoView?.({ block: "nearest" });
  }, [cursor]);

  const agentName = useMemo(() => {
    const m = new Map((timeline?.agents ?? []).map((a) => [a.agentId, a.name] as const));
    return (id: string | null) => (id ? m.get(id) || `…${id.slice(-6)}` : null);
  }, [timeline]);
  const taskTitle = useMemo(() => {
    const m = new Map((timeline?.tasks ?? run?.tasks ?? []).map((x) => [x.taskId, x.title] as const));
    return (id: string | null) => (id ? m.get(id) || `…${id.slice(-6)}` : null);
  }, [timeline, run]);

  if (!projectId) {
    return (
      <div className="replay replay--center">
        <EmptyState icon={<History size={26} strokeWidth={1.6} />} title={t("noProject.title")} description={t("noProject.description")} />
      </div>
    );
  }
  if (runsQuery.error && runs.length === 0) {
    return (
      <div className="replay replay--center">
        <EmptyState
          icon={<AlertTriangle size={26} strokeWidth={1.6} />}
          title={t("error.title")}
          description={runsQuery.error}
          action={
            <Button onClick={runsQuery.reload} leftIcon={<RefreshCw size={14} />}>
              {t("common:actions.retry")}
            </Button>
          }
        />
      </div>
    );
  }
  if (!runsQuery.data) {
    return (
      <div className="replay replay--center" role="status">
        <Spinner size={18} /> {t("loadingRuns")}
      </div>
    );
  }
  if (runs.length === 0) {
    return (
      <div className="replay replay--center">
        <EmptyState icon={<History size={26} strokeWidth={1.6} />} title={t("noRuns.title")} description={t("noRuns.description")} />
      </div>
    );
  }

  const current = steps[cursor] ?? null;
  const togglePhase = (ph: ReplayPhase) =>
    setHiddenPhases((prev) => {
      const next = new Set(prev);
      if (next.has(ph)) next.delete(ph);
      else next.add(ph);
      return next;
    });
  const go = (action: "first" | "prev" | "next" | "last") => {
    setPlaying(false);
    setCursor((c) => moveCursor(c, total, action));
  };

  return (
    <div className="replay">
      <div className="replay-pickers">
        <label className="replay-field">
          <span>{t("picker.run")}</span>
          <select
            value={runId ?? ""}
            onChange={(e) => {
              setRunId(e.target.value || null);
              setScope("");
              setAgentId("");
            }}
          >
            {runs.map((r) => (
              <option key={r.runId} value={r.runId}>
                {t("picker.runOption", {
                  goal: r.goal.length > 60 ? `${r.goal.slice(0, 59)}…` : r.goal,
                  status: t(`logs:enums.runStatus.${r.status}`),
                  time: fmt.date(r.startedAt, "dateTime"),
                })}
              </option>
            ))}
          </select>
        </label>
        <label className="replay-field">
          <span>{t("picker.scope")}</span>
          <select value={scope} onChange={(e) => setScope(e.target.value)}>
            <option value="">{t("picker.wholeRun")}</option>
            {(run?.tasks ?? []).map((task) => (
              <option key={task.taskId} value={task.taskId}>
                {task.title}
              </option>
            ))}
          </select>
        </label>
        <label className="replay-field">
          <span>{t("picker.agent")}</span>
          <select value={agentId} onChange={(e) => setAgentId(e.target.value)}>
            <option value="">{t("picker.allAgents")}</option>
            {(timeline?.agents ?? []).map((a) => (
              <option key={a.agentId} value={a.agentId}>
                {a.name || `…${a.agentId.slice(-6)}`}
              </option>
            ))}
          </select>
        </label>
      </div>

      <div className="replay-phases" role="group" aria-label={t("picker.phases")}>
        {PHASES.map((ph) => {
          const count = timeline?.phases.find((p) => p.phase === ph)?.steps ?? 0;
          const on = !hiddenPhases.has(ph);
          return (
            <button
              key={ph}
              type="button"
              className={clsx("replay-chip", `replay-phase--${ph.toLowerCase()}`, !on && "replay-chip--off")}
              aria-pressed={on}
              onClick={() => togglePhase(ph)}
            >
              {t(`phase.${ph}`)}
              {on && count > 0 && <span className="replay-chip__count">{count}</span>}
            </button>
          );
        })}
      </div>

      <div className="replay-player" role="toolbar" aria-label={t("player.label")}>
        <IconButton label={t("player.first")} icon={<SkipBack size={15} />} onClick={() => go("first")} disabled={total === 0 || cursor === 0} tooltip="bottom" />
        <IconButton label={t("player.prev")} icon={<StepBack size={15} />} onClick={() => go("prev")} disabled={total === 0 || cursor === 0} tooltip="bottom" />
        <IconButton
          label={playing ? t("player.pause") : t("player.play")}
          icon={playing ? <Pause size={15} /> : <Play size={15} />}
          variant="solid"
          disabled={total === 0}
          onClick={() => {
            if (!playing && cursor >= total - 1) setCursor(0);
            setPlaying((p) => !p);
          }}
          tooltip="bottom"
        />
        <IconButton label={t("player.next")} icon={<StepForward size={15} />} onClick={() => go("next")} disabled={total === 0 || cursor >= total - 1} tooltip="bottom" />
        <IconButton label={t("player.last")} icon={<SkipForward size={15} />} onClick={() => go("last")} disabled={total === 0 || cursor >= total - 1} tooltip="bottom" />
        <label className="replay-field replay-field--inline">
          <span>{t("player.speed")}</span>
          <select value={String(speed)} onChange={(e) => setSpeed(Number(e.target.value))}>
            {SPEEDS.map((s) => (
              <option key={s} value={String(s)}>
                {t("player.speedValue", { speed: fmt.number(s) })}
              </option>
            ))}
          </select>
        </label>
        <input
          className="replay-seek"
          type="range"
          min={0}
          max={Math.max(0, total - 1)}
          value={Math.min(cursor, Math.max(0, total - 1))}
          onChange={(e) => {
            setPlaying(false);
            setCursor(Number(e.target.value));
          }}
          aria-label={t("player.seek")}
          aria-valuetext={t("player.position", { current: cursor + 1, total })}
          disabled={total === 0}
        />
        <span className="replay-position">{total > 0 ? t("player.position", { current: cursor + 1, total }) : null}</span>
        {timelineQuery.loading && <Spinner size={14} />}
      </div>
      {timeline?.truncated && (
        <div className="replay-banner" role="note">
          <AlertTriangle size={14} /> {t("player.truncated")}
        </div>
      )}

      <div className="replay-body">
        <div className="replay-timeline" role="list" aria-label={t("timeline.label")}>
          {!timeline ? (
            timelineQuery.error ? (
              <EmptyState icon={<AlertTriangle size={24} strokeWidth={1.6} />} title={t("error.title")} description={timelineQuery.error} />
            ) : (
              <div className="replay--center replay-loading" role="status">
                <Spinner size={18} /> {t("loading")}
              </div>
            )
          ) : steps.length === 0 ? (
            <EmptyState icon={<History size={24} strokeWidth={1.6} />} title={t("empty.title")} description={t("empty.description")} />
          ) : (
            segments(steps).map((seg) => (
              <div key={`${seg.phase}-${seg.steps[0]?.index ?? 0}`} className={clsx("replay-segment", `replay-phase--${seg.phase.toLowerCase()}`)}>
                <div className="replay-segment__phase">{t(`phase.${seg.phase}`)}</div>
                {seg.steps.map((s) => (
                  <StepRow
                    key={s.index}
                    step={s}
                    offset={offsetMs(steps, s)}
                    current={s.index === cursor}
                    future={s.index > cursor}
                    agent={agentName(s.agentId)}
                    onSelect={() => {
                      setPlaying(false);
                      setCursor(s.index);
                    }}
                    rowRef={(el) => {
                      if (el) rowRefs.current.set(s.index, el);
                      else rowRefs.current.delete(s.index);
                    }}
                  />
                ))}
              </div>
            ))
          )}
        </div>
        <div className="replay-side">
          <StepDetail step={current} agent={agentName(current?.agentId ?? null)} task={taskTitle(current?.taskId ?? null)} />
          {state && state.index === cursor && <StateCard result={state} agentName={agentName} />}
        </div>
      </div>
    </div>
  );
}

function StepRow({
  step,
  offset,
  current,
  future,
  agent,
  onSelect,
  rowRef,
}: {
  step: ReplayStep;
  offset: number;
  current: boolean;
  future: boolean;
  agent: string | null;
  onSelect: () => void;
  rowRef: (el: HTMLButtonElement | null) => void;
}) {
  const { t } = useT("replay");
  const fmt = useFormat();
  return (
    <button
      ref={rowRef}
      type="button"
      role="listitem"
      className={clsx("replay-step", `replay-step--${statusTone(step.status)}`, current && "replay-step--current", future && "replay-step--future")}
      aria-current={current ? "step" : undefined}
      onClick={onSelect}
    >
      <span className="replay-step__time" title={fmt.date(step.ts, "dateTime")}>
        {clock(step.ts)}
        <span className="replay-step__offset">+{fmt.duration(offset)}</span>
      </span>
      <span className="replay-step__dot" />
      <span className="replay-step__kind">{t(`kind.${step.kind}` as "kind.ToolCall", { defaultValue: step.kind })}</span>
      <span className="replay-step__title">{step.title}</span>
      {agent && <span className="replay-step__agent">{agent}</span>}
    </button>
  );
}

function StepDetail({ step, agent, task }: { step: ReplayStep | null; agent: string | null; task: string | null }) {
  const { t } = useT(["replay", "common", "logs", "merge", "teams"]);
  const fmt = useFormat();
  if (!step) {
    return (
      <section className="replay-card" aria-label={t("detail.label")}>
        <p className="replay-muted">{t("detail.none")}</p>
      </section>
    );
  }
  const detail = (() => {
    if (!step.detail) return null;
    if (step.kind === "FileChanged") return t(`fileChange.${step.detail}` as "fileChange.Created", { defaultValue: step.detail });
    if (step.kind === "ReviewFinished") return t("detail.findings", { count: Number(step.detail) || 0 });
    if (step.kind === "RollbackPerformed") return t("detail.filesRestored", { count: Number(step.detail) || 0 });
    if (step.kind === "MergeProposalUpdated") {
      const [status = "", ...rest] = step.detail.split(": ");
      const label = t(`merge:status.${status}` as "merge:status.Proposed", { defaultValue: status });
      return rest.length > 0 ? `${label}: ${rest.join(": ")}` : label;
    }
    if (step.kind === "TeamLeadRunUpdated") {
      return t(`teams:leadStatus.${step.detail}` as "teams:leadStatus.Planning", { defaultValue: step.detail });
    }
    return step.detail;
  })();
  const rows: Array<[string, string]> = [];
  rows.push([t("detail.time"), fmt.date(step.ts, "timeSeconds")]);
  if (step.seq !== null) rows.push([t("detail.seq"), t("detail.seqValue", { seq: step.seq })]);
  if (agent) rows.push([t("detail.agent"), agent]);
  if (task) rows.push([t("detail.task"), task]);
  if (step.durationMs !== null) rows.push([t("detail.duration"), fmt.duration(step.durationMs)]);
  if (step.taskStatus) rows.push([t("detail.taskStatus"), t(`common:status.task.${step.taskStatus}`)]);
  if (step.agentState) rows.push([t("detail.agentState"), t(`common:status.agent.${step.agentState}`)]);
  if (step.toolStatus) rows.push([t("detail.toolStatus"), t(`logs:enums.toolStatus.${step.toolStatus}`)]);
  if (step.approvalStatus) rows.push([t("detail.approval"), t(`common:status.approval.${step.approvalStatus}`)]);
  if (step.verdict) rows.push([t("detail.verdict"), t(`logs:enums.reviewVerdict.${step.verdict}`)]);
  if (step.testsPassed !== null || step.testsFailed !== null) {
    rows.push([t("detail.tests"), t("detail.testsValue", { passed: step.testsPassed ?? 0, failed: step.testsFailed ?? 0 })]);
  }
  if (step.runStatus) rows.push([t("detail.runStatus"), t(`logs:enums.runStatus.${step.runStatus}`)]);
  if (step.correlationId) rows.push([t("detail.correlation"), `…${step.correlationId.slice(-8)}`]);
  return (
    <section className="replay-card" aria-label={t("detail.label")}>
      <header className="replay-card__head">
        <Badge tone={statusTone(step.status)}>{t(`kind.${step.kind}` as "kind.ToolCall", { defaultValue: step.kind })}</Badge>
        <span className="replay-muted">{t(`phase.${step.phase}`)}</span>
      </header>
      {step.title && <p className="replay-card__title">{step.title}</p>}
      {detail && <p className="replay-card__detail">{detail}</p>}
      <dl className="replay-props">
        {rows.map(([k, v]) => (
          <div key={k}>
            <dt>{k}</dt>
            <dd>{v}</dd>
          </div>
        ))}
      </dl>
      {step.files.length > 0 && (
        <>
          <h4>{t("detail.files")}</h4>
          <ul className="replay-files">
            {step.files.map((f) => (
              <li key={f}>
                <button type="button" className="replay-link" onClick={() => openInEditor(f)} aria-label={t("detail.openFile", { path: f })}>
                  <FileCode size={12} /> {f}
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
      <p className="replay-note">{t("detail.noCot")}</p>
    </section>
  );
}

function StateCard({ result, agentName }: { result: ReplayStepResult; agentName: (id: string | null) => string | null }) {
  const { t } = useT(["replay", "common"]);
  const s = result.state;
  return (
    <section className="replay-card" aria-label={t("state.label")}>
      <h4>{t("state.label")}</h4>
      <div className="replay-counters">
        <span>
          {t("state.toolCalls", { count: s.toolCalls })}
          {s.toolFailures > 0 && <span className="replay-danger"> · {t("state.toolFailures", { count: s.toolFailures })}</span>}
        </span>
        <span>{t("state.approvals", { pending: s.approvalsPending, resolved: s.approvalsResolved })}</span>
        <span>{t("state.tests", { passed: s.testsPassed, failed: s.testsFailed })}</span>
        <span>{t("state.reviews", { count: s.reviews })}</span>
      </div>
      {s.tasks.length > 0 && (
        <>
          <h5>{t("state.tasks")}</h5>
          <ul className="replay-state-list">
            {s.tasks.map((task) => (
              <li key={task.taskId}>
                <span className="replay-state-list__name">{task.title}</span>
                <span className="replay-muted">{task.status ? t(`common:status.task.${task.status}`) : t("state.notStarted")}</span>
              </li>
            ))}
          </ul>
        </>
      )}
      {s.agents.length > 0 && (
        <>
          <h5>{t("state.agents")}</h5>
          <ul className="replay-state-list">
            {s.agents.map((a) => (
              <li key={a.agentId}>
                <span className="replay-state-list__name">{a.name || agentName(a.agentId)}</span>
                <span className="replay-muted">
                  {a.state ? t(`common:status.agent.${a.state}`) : null} · {a.active ? t("state.active") : t("state.stopped")}
                </span>
              </li>
            ))}
          </ul>
        </>
      )}
      <h5>{t("state.files")}</h5>
      {s.filesChanged.length === 0 ? (
        <p className="replay-muted">{t("state.noFiles")}</p>
      ) : (
        <ul className="replay-files">
          {s.filesChanged.map((f) => (
            <li key={f}>
              <button type="button" className="replay-link" onClick={() => openInEditor(f)} aria-label={t("detail.openFile", { path: f })}>
                <FileCode size={12} /> {f}
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
