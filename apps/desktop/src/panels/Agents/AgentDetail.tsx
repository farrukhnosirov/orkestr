import type { AgentInstance, AgentToolCallView, Approval } from "@orkestr/protocol";
import { Badge, Button, EmptyState, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, ChevronRight, GitBranch, OctagonX, Pause, Play, ShieldAlert, TriangleAlert, Wrench, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { useFormat, useLanguage, useT } from "../../i18n";
import { resolveApproval } from "../../features/security/store";
import { useSecurityStore } from "../../features/security/store";
import { agentActions, errorMessage, useAgentTranscript } from "./data";
import { TOOL_TONE, elapsed, formatCost, formatTokens, isActive, roleLabel, timeline } from "./model";
import { RoleAvatar, StateChip } from "./parts";
import { useAgentsUi } from "./store";
import { agentName, agentNote, agentToolSummary, approvalReason, approvalSummary, PAUSED_ACTIVITY } from "../../lib/coreText";

function ToolCallItem({ view }: { view: AgentToolCallView }) {
  const { t } = useT("agents");
  const fmt = useFormat();
  const c = view.toolCall;
  const args = useMemo(() => JSON.stringify(c.input, null, 2), [c.input]);
  return (
    <details className="agents-tool">
      <summary>
        <ChevronRight size={13} className="agents-tool__chevron" aria-hidden="true" />
        <Wrench size={13} aria-hidden="true" />
        <span className="agents-tool__name">{c.tool}</span>
        <span className="agents-tool__summary">{c.outputSummary ? agentToolSummary(c.outputSummary) : ""}</span>
        <Badge tone={TOOL_TONE[c.status]}>{t(`toolStatus.${c.status}`)}</Badge>
        {view.durationMs !== null && <span className="agents-muted">{fmt.duration(view.durationMs)}</span>}
      </summary>
      <div className="agents-tool__body">
        <div className="agents-tool__label">{t("detail.arguments")}</div>
        <pre className="agents-pre">{args}</pre>
        {view.result !== null && (
          <>
            <div className="agents-tool__label">{t("detail.result")}</div>
            <pre className="agents-pre">{view.result}</pre>
          </>
        )}
      </div>
    </details>
  );
}

function ApprovalCard({ approval }: { approval: Approval }) {
  const { t } = useT(["agents", "common"]);
  const toast = useAgentsUi((s) => s.toast);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const decide = async (decision: "Approve" | "Reject") => {
    setBusy(true);
    try {
      await resolveApproval(approval.id, decision, note);
    } catch (e) {
      toast("danger", errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="agents-approval" role="group" aria-label={t("approval.group", { action: approvalSummary(approval.actionSummary) })}>
      <div className="agents-approval__head">
        <ShieldAlert size={15} aria-hidden="true" />
        <strong>{approvalSummary(approval.actionSummary)}</strong>
        <Badge tone={approval.risk === "Low" ? "info" : approval.risk === "Medium" ? "warning" : "danger"}>{t(`risk.${approval.risk}`)}</Badge>
      </div>
      <p className="agents-approval__reason">{approvalReason(approval.reason)}</p>
      {approval.commandPreview && <pre className="agents-pre">{approval.commandPreview}</pre>}
      <div className="agents-approval__actions">
        <input
          className="agents-input"
          placeholder={t("approval.notePlaceholder")}
          value={note}
          aria-label={t("approval.noteLabel")}
          onChange={(e) => setNote(e.target.value)}
        />
        <Button size="sm" variant="primary" leftIcon={<Check size={14} />} loading={busy} onClick={() => void decide("Approve")}>
          {t("common:actions.approve")}
        </Button>
        <Button size="sm" variant="danger" leftIcon={<X size={14} />} disabled={busy} onClick={() => void decide("Reject")}>
          {t("common:actions.reject")}
        </Button>
      </div>
    </div>
  );
}

export function AgentDetail({ agent, taskTitle, now }: { agent: AgentInstance; taskTitle: string | null; now: number }) {
  const { t } = useT("agents");
  const lng = useLanguage();
  const { transcript, loading, error } = useAgentTranscript(agent.id);
  const pending = useSecurityStore((s) => s.pending);
  const approvals = pending.filter((a) => a.agentId === agent.id && a.status === "Pending");
  const toast = useAgentsUi((s) => s.toast);
  const [busy, setBusy] = useState<string | null>(null);
  const items = useMemo(() => timeline(transcript), [transcript]);
  const bottom = useRef<HTMLDivElement>(null);
  const active = isActive(agent.state);
  const paused = agent.state === "Waiting" && agent.activity === PAUSED_ACTIVITY;

  useEffect(() => {
    // Scroll only the transcript itself: `scrollIntoView` would also scroll
    // every overflow-hidden ancestor (dock groups, the app frame) and shift
    // the whole shell out of the window.
    const log = bottom.current?.parentElement;
    if (log) log.scrollTop = log.scrollHeight;
  }, [items.length]);

  const act = async (what: string, fn: () => Promise<AgentInstance>) => {
    setBusy(what);
    try {
      const a = await fn();
      if (what === "stop") toast("info", t("stopped", { name: agentName(a.name) }));
    } catch (e) {
      toast("danger", errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="agents-detail" aria-label={t("detail.region", { name: agentName(agent.name) })}>
      <header className="agents-detail__head">
        <RoleAvatar role={agent.role} state={agent.state} size={40} />
        <div className="agents-detail__title">
          <div className="agents-detail__name">
            <h3>{agentName(agent.name)}</h3>
            <StateChip state={agent.state} />
          </div>
          <div className="agents-detail__sub">
            <span>{roleLabel(agent.role)}</span>
            <span>·</span>
            <span>{taskTitle ?? t("list.noTask")}</span>
          </div>
        </div>
        <div className="agents-detail__actions">
          {active && agent.executionStyle !== "cli" && (
            <Button
              size="sm"
              variant="ghost"
              leftIcon={paused ? <Play size={14} /> : <Pause size={14} />}
              loading={busy === "pause"}
              onClick={() => void act("pause", () => (paused ? agentActions.resume(agent.id) : agentActions.pause(agent.id)))}
            >
              {paused ? t("detail.resume") : t("detail.pause")}
            </Button>
          )}
          {active && (
            <Button
              size="sm"
              variant="danger"
              leftIcon={<OctagonX size={14} />}
              loading={busy === "stop"}
              onClick={() => void act("stop", () => agentActions.stop(agent.id))}
            >
              {t("detail.stop")}
            </Button>
          )}
        </div>
      </header>
      <dl className="agents-facts">
        <div>
          <dt>{t("detail.model")}</dt>
          <dd>{agent.model?.model ?? "—"}</dd>
        </div>
        <div>
          <dt>{t("detail.worktree")}</dt>
          <dd title={agent.worktree?.path}>
            {agent.worktree ? (
              <>
                <GitBranch size={12} aria-hidden="true" /> {agent.worktree.branch || agent.worktreePolicy}
              </>
            ) : (
              t("detail.none")
            )}
          </dd>
        </div>
        <div>
          <dt>{t("detail.turns")}</dt>
          <dd>
            {agent.turnsUsed} / {agent.maxTurns || "—"}
          </dd>
        </div>
        <div>
          <dt>{t("detail.tokens")}</dt>
          <dd>{formatTokens(agent.usage.inputTokens + agent.usage.outputTokens, lng)}</dd>
        </div>
        <div>
          <dt>{t("detail.cost")}</dt>
          <dd>
            {formatCost(agent.usage.costUsd, lng)}
            {agent.budget?.maxCostUsd != null && ` / ${formatCost(agent.budget.maxCostUsd, lng)}`}
          </dd>
        </div>
        <div>
          <dt>{t("detail.elapsed")}</dt>
          <dd>{elapsed(agent, now, lng)}</dd>
        </div>
      </dl>
      {agent.lastError && (
        <p className="agents-error" role="alert">
          <TriangleAlert size={14} aria-hidden="true" /> {agent.lastError}
        </p>
      )}
      {approvals.length > 0 && (
        <div className="agents-approvals">
          {approvals.map((a) => (
            <ApprovalCard key={a.id} approval={a} />
          ))}
        </div>
      )}
      <div className="agents-transcript" aria-label={t("detail.transcript")} role="log">
        {loading && items.length === 0 ? (
          <div className="agents-loading">
            <Spinner size={16} /> {t("detail.loadingTranscript")}
          </div>
        ) : error ? (
          <p className="agents-error">{error}</p>
        ) : items.length === 0 ? (
          <EmptyState title={t("detail.noActivity.title")} description={t("detail.noActivity.description")} />
        ) : (
          items.map((it) =>
            it.kind === "tool" ? (
              <ToolCallItem key={it.id} view={it.call} />
            ) : (
              <div key={it.id} className={clsx("agents-msg", `agents-msg--${it.role.toLowerCase()}`, it.streaming && "agents-msg--streaming")}>
                <div className="agents-msg__role">{it.role === "Assistant" ? agentName(agent.name) : it.role === "User" ? t("detail.roleTask") : "Orkestr"}</div>
                <div className="agents-msg__text">{it.role === "System" ? agentNote(it.text) : it.role === "User" ? it.text.replace(/^# Task: /, `# ${t("detail.roleTask")}: `) : it.text}</div>
              </div>
            ),
          )
        )}
        <div ref={bottom} />
      </div>
    </section>
  );
}
