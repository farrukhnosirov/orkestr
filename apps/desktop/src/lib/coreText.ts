/**
 * Display texts for fixed English strings that the core stores or sends as
 * data: built-in agent template names/descriptions (and the agent instance
 * names derived from them, "QA Engineer #1"), agent activity lines and the
 * labels of automatic snapshots. The stored values stay English (they are
 * data, shared with other clients); only their display is localized.
 * Unknown text (user-entered names, labels, free text) is returned as is.
 */
import type { WaitReason } from "@orkestr/protocol";

import { localizePipelineText } from "../features/pipeline/text";
import { modelName, providerName, reasonText } from "../features/scheduler/model";
import { formatBytes, formatCurrency, formatNumber, formatPercent, i18n, t } from "../i18n";
import { formatDuration, formatInteger } from "../i18n/format";

/** English built-in template name → key under `coreText:templates`. */
const BUILTIN = {
  "Software Architect": "architect",
  "Backend Engineer": "backend",
  "Frontend Engineer": "frontend",
  "Database Engineer": "database",
  "QA Engineer": "qa",
  "Security Reviewer": "security",
  "DevOps Engineer": "devops",
  "UI/UX Designer": "uiux",
  "Product Manager": "productManager",
  "Code Reviewer": "reviewer",
  "Researcher": "researcher",
} as const;
type BuiltinKey = (typeof BUILTIN)[keyof typeof BUILTIN];

const builtinKey = (name: string): BuiltinKey | undefined => BUILTIN[name as keyof typeof BUILTIN];

/** Name of a template (built-in names are translated). */
export function templateName(name: string): string {
  const key = builtinKey(name);
  return key ? t(`coreText:templates.${key}.name`) : name;
}

/** Description of a template; only the original built-in English description is translated. */
export function templateDescription(name: string, description: string): string {
  const key = builtinKey(name);
  if (!key) return description;
  return description.trim() === t(`coreText:templates.${key}.description`, { lng: "en" }) ? t(`coreText:templates.${key}.description`) : description;
}

/** English names of the seeded built-in teams → key under `teams:builtin`. */
const BUILTIN_TEAMS = {
  "ERP Team": "erp",
  "Web Team": "web",
  "Mobile Team": "mobile",
  "Security Team": "security",
  "Rapid Prototype Team": "rapid",
} as const;

/**
 * A team name the core wrote into text (notices, task reasons, lead agent
 * names, scheduler rows, events): the seeded built-in names are translated,
 * user names stay as typed.
 */
export function builtinTeamName(name: string): string {
  const key = BUILTIN_TEAMS[name as keyof typeof BUILTIN_TEAMS];
  return key ? t(`teams:builtin.${key}.name`) : name;
}

/**
 * Agent instance name: "QA Engineer #1" → «Тестировщик #1»; a team lead
 * "Web Team — Software Architect" → «Веб-команда — Архитектор ПО».
 */
export function agentName(name: string): string {
  const lead = name.match(/^(.+?) — (.+)$/);
  if (lead) return `${builtinTeamName(lead[1] ?? "")} — ${agentName(lead[2] ?? "")}`;
  const m = name.match(/^(.+?)( #\d+)$/);
  if (m && builtinKey(m[1] ?? "")) return `${templateName(m[1] ?? "")}${m[2] ?? ""}`;
  return templateName(name);
}

const ACTIVITY = {
  "Reading the task": "readingTask",
  "Paused by the user": "paused",
  Resumed: "resumed",
  "Waiting for the model": "waitingModel",
  Finished: "finished",
  "Waiting for the user's answer": "waitingUser",
  "Waiting for approval to start Codex": "waitingCodex",
  // crates/agent-runtime/src/runtime.rs
  Spawned: "spawned",
  Stopped: "stopped",
  "Interrupted: Orkestr stopped while the agent was running": "interrupted",
  // Stop details that become the activity (crates/agent-runtime/src/run.rs)
  "stopped by the user": "stoppedByUser",
  "stopped by the user (task cancelled)": "stoppedTaskCancelled",
  "emergency stop": "emergencyStop",
  "the core is shutting down": "coreShutdown",
  // Team leads (crates/orchestrator/src/team_lead.rs)
  "Decomposing the task": "decomposing",
  "Coordinating executors": "coordinating",
  "Waiting for the reviewer": "waitingReviewer",
  "Delegated task done": "delegatedDone",
  Cancelled: "cancelled",
} as const;

/** Display names of the CLI provider kinds in "Running {kind:?} CLI" (crates/agent-runtime/src/cli.rs). */
const CLI_NAME: Record<string, string> = { ClaudeCode: "Claude Code", Codex: "Codex" };

/** The core's text for a paused agent (compare against the raw value, not the display text). */
export const PAUSED_ACTIVITY = "Paused by the user";

/** Activity line of an agent ("Finished" → «Работа завершена»). Tool activity (`write_file src/a.ts`) stays as is. */
export function agentActivity(activity: string): string {
  const key = ACTIVITY[activity as keyof typeof ACTIVITY];
  if (key) return t(`coreText:activity.${key}`);
  const m = activity.match(/^Waiting for approval: ([\s\S]+)$/);
  if (m) return t("coreText:activity.waitingApproval", { action: m[1] });
  const cli = activity.match(/^Running (\w+) CLI$/);
  if (cli) return t("coreText:activity.runningCli", { cli: CLI_NAME[cli[1] ?? ""] ?? cli[1] });
  return activity;
}

const STOP_DETAIL = {
  "stopped by the user": "byUser",
  "stopped by the user (task cancelled)": "byUserCancelled",
  "the core is shutting down": "shutdown",
  "interrupted by a core restart": "restart",
  "emergency stop": "emergency",
} as const;

/** Detail of an `AgentStopped` event (crates/agent-runtime run.rs / runtime.rs); error texts stay as is. */
export function agentStopDetail(detail: string): string {
  const key = STOP_DETAIL[detail as keyof typeof STOP_DETAIL];
  if (key) return t(`coreText:stopDetail.${key}`);
  return limitReason(detail) ?? detail;
}

const LEAD_REASON = {
  "emergency stop": "emergencyStop",
  "the Manager run was cancelled": "runCancelled",
  "Orkestr is shutting down": "shutdown",
  "Orkestr stopped while the team lead was working — resume the run to retry": "interrupted",
} as const;

/** Why a team lead run stopped (crates/orchestrator team_lead.rs / teams.rs); other texts stay as is. */
export function leadReason(reason: string): string {
  const key = LEAD_REASON[reason as keyof typeof LEAD_REASON];
  if (key) return t(`coreText:leadReason.${key}`);
  let m: RegExpMatchArray | null;
  if ((m = reason.match(/^budget exhausted: \$([\d.]+) spent of \$([\d.]+)$/))) {
    return t("coreText:leadReason.budget", { spent: formatCurrency(Number(m[1])), limit: formatCurrency(Number(m[2])) });
  }
  if ((m = reason.match(/^the team's time limit of (\d+) min was reached$/))) return t("coreText:leadReason.timeLimit", { mins: Number(m[1]) });
  if ((m = reason.match(/^the lead could not decompose the task: ([\s\S]+)$/))) return t("coreText:leadReason.decompose", { detail: m[1] });
  return reason;
}

const MERGE_DETAIL = {
  "Orkestr stopped while the merge was running": "stopped",
  "the quality checks failed on the integration branch": "checksFailed",
  aborted: "aborted",
} as const;
const APPROVAL_DETAIL_RE = /^(Rejected|Expired|Cancelled|Approved|Pending)(?:: ([\s\S]+))?$/;

/**
 * Detail of a merge proposal's status (crates/orchestrator merge/mod.rs):
 * the approval outcome ("Rejected: <note>") and the fixed failure sentences
 * are translated; git / error texts stay as is.
 */
export function mergeDetail(detail: string): string {
  const key = MERGE_DETAIL[detail as keyof typeof MERGE_DETAIL];
  if (key) return t(`coreText:mergeDetail.${key}`);
  let m: RegExpMatchArray | null;
  if ((m = detail.match(APPROVAL_DETAIL_RE))) {
    const status = t(`common:status.approval.${m[1] as (typeof APPROVAL_STATUSES)[number]}`);
    return m[2] ? t("coreText:mergeDetail.approvalNote", { status, note: m[2] }) : t("coreText:mergeDetail.approval", { status });
  }
  if ((m = detail.match(/^approval failed: ([\s\S]+)$/))) return t("coreText:mergeDetail.approvalFailed", { detail: m[1] });
  if ((m = detail.match(/^the quality checks could not run: ([\s\S]+)$/))) return t("coreText:mergeDetail.checksError", { detail: m[1] });
  return detail;
}

/** Why a provider fell back (`ProviderError::class()` code: rate_limited, network, ...). */
export function fallbackReason(code: string): string {
  return t(`notifications:fallbackReason.${code}`, { defaultValue: code });
}

const DEFAULT_SNAPSHOT = {
  "Manual snapshot": "manual",
  "Before risky action": "beforeRisky",
  "Task checkpoint": "taskCheckpoint",
  "Before rollback": "beforeRollback",
  "Periodic snapshot": "periodic",
} as const;

/** Label of a snapshot; automatic labels written by the core are translated. */
export function snapshotLabel(label: string): string {
  const key = DEFAULT_SNAPSHOT[label as keyof typeof DEFAULT_SNAPSHOT];
  if (key) return t(`coreText:snapshot.${key}`);
  let m: RegExpMatchArray | null;
  if ((m = label.match(/^Before (.+ #\d+) changes$/))) return t("coreText:snapshot.beforeAgent", { agent: agentName(m[1] ?? "") });
  if ((m = label.match(/^(.+ #\d+) finished$/))) return t("coreText:snapshot.agentFinished", { agent: agentName(m[1] ?? "") });
  if ((m = label.match(/^(.+ #\d+) stopped$/))) return t("coreText:snapshot.agentStopped", { agent: agentName(m[1] ?? "") });
  if ((m = label.match(/^Before discarding (\d+) path\(s\)$/))) return t("coreText:snapshot.beforeDiscard", { count: Number(m[1]) });
  if ((m = label.match(/^Before removing worktree (.+)$/))) return t("coreText:snapshot.beforeRemoveWorktree", { path: m[1] });
  if ((m = label.match(/^Before rollback to “(.+)”$/))) return t("coreText:snapshot.beforeRollbackTo", { label: snapshotLabel(m[1] ?? "") });
  if ((m = label.match(/^Before restoring (.+)$/))) return t("coreText:snapshot.beforeRestore", { path: m[1] });
  if ((m = label.match(/^Before rolling back task (\S+)$/))) return t("coreText:snapshot.beforeTaskRollback", { id: m[1] });
  if ((m = label.match(/^Before merge into (.+)$/))) return t("coreText:snapshot.beforeMerge", { branch: m[1] });
  return label;
}

const TERMINAL_TITLE = { "Agent commands": "agentCommands", Commands: "commands" } as const;

/** Title of a terminal session; the core's default titles of command views are translated. */
export function terminalTitle(title: string): string {
  const key = TERMINAL_TITLE[title as keyof typeof TERMINAL_TITLE];
  return key ? t(`coreText:terminal.${key}`) : title;
}

const AGENT_STATES = ["Idle", "Listening", "Thinking", "Coding", "Testing", "Reviewing", "Waiting", "Error", "Completed", "Stopped"] as const;
const APPROVAL_STATUSES = ["Pending", "Approved", "Rejected", "Expired", "Cancelled"] as const;
const AGENT_STATE_RE = new RegExp(`^(.+) is (${AGENT_STATES.join("|")})$`);

/** Summary line of a Manager tool call (fixed English shapes, crates/orchestrator/src/tools.rs and ui_tools.rs). */
export function managerToolSummary(summary: string): string {
  let m: RegExpMatchArray | null;
  // `ask_user`: the bare decision status (`{status:?}`).
  if ((APPROVAL_STATUSES as readonly string[]).includes(summary)) return t(`common:status.approval.${summary as (typeof APPROVAL_STATUSES)[number]}`);
  if ((m = summary.match(/^started (.+)$/))) return t("coreText:tool.started", { agent: agentName(m[1] ?? "") });
  if ((m = summary.match(/^stopped (.+)$/))) return t("coreText:tool.stopped", { agent: agentName(m[1] ?? "") });
  if ((m = summary.match(AGENT_STATE_RE))) {
    return t("coreText:tool.agentState", {
      agent: agentName(m[1] ?? ""),
      state: t(`common:status.agent.${m[2] as (typeof AGENT_STATES)[number]}`),
    });
  }
  if ((m = summary.match(/^recorded "([\s\S]*)"$/))) return t("coreText:tool.recorded", { title: m[1] });
  if ((m = summary.match(/^UI change proposed: ([\s\S]*)$/))) return t("coreText:tool.uiChangeProposed", { summary: m[1] });
  const n = (i: number) => Number(m?.[i] ?? 0);
  if ((m = summary.match(/^plan with (\d+) task\(s\)$/))) return t("coreText:tool.plan", { count: n(1) });
  if ((m = summary.match(/^(\d+) project\(s\)$/))) return t("common:count.projects", { count: n(1) });
  if ((m = summary.match(/^(\d+) task\(s\)$/))) return t("common:count.tasks", { count: n(1) });
  if ((m = summary.match(/^(\d+) agent\(s\)$/))) return t("common:count.agents", { count: n(1) });
  if ((m = summary.match(/^(\d+) node\(s\), (\d+) edge\(s\)$/))) return t("coreText:tool.graph", { nodes: n(1), edges: n(2) });
  if ((m = summary.match(/^(\d+) provider\(s\)$/))) return t("coreText:tool.providers", { count: n(1) });
  if ((m = summary.match(/^(\d+) pending approval\(s\)$/))) return t("coreText:tool.approvals", { count: n(1) });
  if ((m = summary.match(/^(\d+) changed file\(s\)$/))) return t("coreText:tool.changedFiles", { count: n(1) });
  if ((m = summary.match(/^(\d+) hit\(s\)$/))) return t("coreText:tool.hits", { count: n(1) });
  if ((m = summary.match(/^(\d+) snapshot\(s\)$/))) return t("coreText:tool.snapshots", { count: n(1) });
  if ((m = summary.match(/^(\d+) task\(s\), (\d+) running agent\(s\)$/))) return t("coreText:tool.overview", { tasks: n(1), agents: n(2) });
  return summary;
}

/** A system note in an agent's transcript (fixed English shapes, crates/agent-runtime/src/run.rs). */
export function agentNote(text: string): string {
  let m: RegExpMatchArray | null;
  if ((m = text.match(/^Started on “([\s\S]+)” with (\S+) — ([\s\S]+)\.$/))) return t("coreText:note.started", { task: m[1], model: m[2], place: m[3] });
  if ((m = text.match(/^Checkpoint (\S+) taken before the first change\.$/))) return t("coreText:note.checkpoint", { id: m[1] });
  if ((m = text.match(/^Command approved: ([\s\S]+)$/))) return t("coreText:note.commandApproved", { command: m[1] });
  if ((m = text.match(/^Provider fallback: (\S+) → (\S+) \(([\s\S]*)\)\. A structured handoff was created\.$/))) {
    return t("coreText:note.fallback", { from: m[1], to: m[2], reason: fallbackReason(m[3] ?? "") });
  }
  if ((m = text.match(/^Result \((\w+)\): ([\s\S]*)$/))) return t("coreText:note.result", { status: m[1], summary: m[2] });
  if ((m = text.match(/^Stopped: ([\s\S]*)$/))) return t("coreText:note.stopped", { detail: agentStopDetail(m[1] ?? "") });
  return text;
}

/** Role names as the core writes them into task tags (orkestr_agent_runtime::prompt::role_name). */
const ROLE_TAG = {
  Manager: "Manager",
  Architect: "Architect",
  Backend: "Backend",
  Frontend: "Frontend",
  Database: "Database",
  QA: "Qa",
  Security: "Security",
  DevOps: "DevOps",
  "UI/UX": "UiUx",
  "Product Manager": "ProductManager",
  Reviewer: "Reviewer",
  Researcher: "Researcher",
} as const;

/** A task tag: role tags, the Manager's source tag and `epic:` tags are shown localized. */
export function taskTag(tag: string): string {
  const role = ROLE_TAG[tag as keyof typeof ROLE_TAG];
  if (role) return t(`agents:roles.${role}`);
  if (tag === "manager") return t("coreText:tag.manager");
  const m = tag.match(/^epic:([\s\S]+)$/);
  if (m) return t("coreText:tag.epic", { name: m[1] });
  return tag;
}

/** A plan warning of the core (fixed English shapes, crates/orchestrator/src/plan.rs and planner.rs). */
export function planWarning(w: string): string {
  let m: RegExpMatchArray | null;
  if (w === "cost estimate unavailable: the model's price is unknown (local, subscription or mock)") return t("coreText:warning.noCost");
  if (w === "the first answer was invalid and was repaired") return t("coreText:warning.repaired");
  if ((m = w.match(/^duplicate task key `(.+)` renamed to `(.+)`$/))) return t("coreText:warning.duplicateKey", { key: m[1], renamed: m[2] });
  if ((m = w.match(/^task (\S+) had no title$/))) return t("coreText:warning.noTitle", { key: m[1] });
  if ((m = w.match(/^task (\S+): unknown role `(.+)` mapped to Backend$/))) return t("coreText:warning.unknownRole", { key: m[1], role: m[2] });
  if ((m = w.match(/^task (\S+): no role given, using Backend$/))) return t("coreText:warning.noRole", { key: m[1] });
  if ((m = w.match(/^task (\S+): dependency on itself dropped$/))) return t("coreText:warning.selfDependency", { key: m[1] });
  if ((m = w.match(/^task (\S+): unknown dependency `(.+)` dropped$/))) return t("coreText:warning.unknownDependency", { key: m[1], dep: m[2] });
  if ((m = w.match(/^task (\S+): file `(.+)` ignored \(not a relative path\)$/))) return t("coreText:warning.badFile", { key: m[1], file: m[2] });
  return w;
}

/** Summary of an agent tool call (fixed English shapes, crates/agent-runtime/src/tools and run.rs). */
export function agentToolSummary(summary: string): string {
  let m: RegExpMatchArray | null;
  if (summary === "no changes") return t("coreText:agentTool.noChanges");
  if ((m = summary.match(/^listed (.*) \((\d+) entries\)$/))) return t("coreText:agentTool.listed", { path: m[1], count: Number(m[2]) });
  if ((m = summary.match(/^(\d+) match\(es\) for \/([\s\S]*)\/$/))) return t("coreText:agentTool.matches", { pattern: m[2], count: Number(m[1]) });
  if ((m = summary.match(/^read (.+) \((\d+) lines\)$/))) return t("coreText:agentTool.read", { path: m[1], count: Number(m[2]) });
  if ((m = summary.match(/^created (\S.*)$/))) return t("coreText:agentTool.created", { path: m[1] });
  if ((m = summary.match(/^updated (\S.*)$/))) return t("coreText:agentTool.updated", { path: m[1] });
  if ((m = summary.match(/^edited (.+) \((\d+) change\(s\)\)$/))) return t("coreText:agentTool.edited", { path: m[1], count: Number(m[2]) });
  if ((m = summary.match(/^(\d+) changed file\(s\)$/))) return t("coreText:tool.changedFiles", { count: Number(m[1]) });
  if ((m = summary.match(/^(\d+) file\(s\), \+(\d+) -(\d+)$/))) return t("coreText:agentTool.diff", { count: Number(m[1]), added: m[2], removed: m[3] });
  if ((m = summary.match(/^commit (\S+) on (.+)$/))) return t("coreText:agentTool.commit", { id: m[1], branch: m[2] });
  if ((m = summary.match(/^memory: ([\s\S]+)$/))) return t("coreText:agentTool.memory", { title: m[1] });
  if ((m = summary.match(/^decision: ([\s\S]+)$/))) return t("coreText:agentTool.decision", { title: m[1] });
  if ((m = summary.match(/^snapshot ([\s\S]+)$/))) return t("coreText:agentTool.snapshot", { label: snapshotLabel(m[1] ?? "") });
  if ((m = summary.match(/^`([\s\S]+)` → exit (\S+)$/))) return t("coreText:agentTool.exit", { command: m[1], code: m[2] });
  return summary;
}

/**
 * Fixed reasons of task status changes (`TaskStatusChanged.reason`, task
 * history): machine codes (crates/task-engine dag.rs / service.rs,
 * agent-runtime run.rs) and fixed English sentences (orchestrator).
 */
const TASK_REASON = {
  created: "created",
  retry: "retry",
  resume: "resume",
  recovered_fresh: "recoveredFresh",
  crash_recovery: "crashRecovery",
  shutdown: "shutdown",
  dependency_failed: "dependencyFailed",
  dependency_cancelled: "dependencyCancelled",
  dependency_pending: "dependencyPending",
  dependencies_completed: "dependenciesCompleted",
  agent_started: "agentStarted",
  agent_paused: "agentPaused",
  agent_resumed: "agentResumed",
  agent_stopping: "agentStopping",
  agent_finishing: "agentFinishing",
  awaiting_approval: "awaitingApproval",
  approval_resolved: "approvalResolved",
  agent_result_done: "agentResultDone",
  agent_result_partial: "agentResultPartial",
  agent_result_blocked: "agentResultBlocked",
  agent_result_failed: "agentResultFailed",
  EmergencyStop: "emergencyStop",
  "assigned to an agent": "assigned",
  "stopped by the user": "stoppedByUser",
  "cancelled by the user": "cancelledByUser",
  "the user marked the task failed": "userMarkedFailed",
  "code review approved": "reviewApproved",
  "code review rejected the change": "reviewRejected",
  "quality pipeline passed (review, build, tests, security)": "pipelinePassed",
  "quality pipeline: code review": "pipelineReview",
  "quality pipeline: build and tests": "pipelineBuildTests",
  "code review requested changes — self-healing fixes them": "reviewChangesHealing",
  "waiting for the user's acceptance": "awaitingAcceptance",
  "self-healing waits for the user's decision": "healingAwaitsDecision",
  "accepted by the team lead": "teamLeadAccepted",
  "the executor reported a partial result": "executorPartial",
  "all child tasks completed": "childrenCompleted",
  "manager run started": "managerRunStarted",
  "accepted by the AI Manager: the agent reported done": "managerAccepted",
  // Older cores wrote this roadmap note into the reason.
  "accepted by the AI Manager: the agent reported done (automated review/test loops arrive in Phase 2)": "managerAccepted",
} as const;

/** A translate function (a hook's `t` re-renders on a language change). */
export type TextT = (key: string, opts?: Record<string, unknown>) => string;

/**
 * Self-Healing escalation reasons (`healing::Escalation::reason`, orchestrator
 * healing.rs): disabled, attempts used, time limit, budget spent, no progress.
 * Unknown text is returned as is.
 */
export function healingReason(reason: string, tr: TextT = t as unknown as TextT): string {
  const r = reason.trim();
  let m: RegExpExecArray | null;
  if (/^self-healing is disabled for this project$/.test(r)) return tr("pipeline:text.reasonDisabled");
  if (/^no progress: the last fix produced the same failure$/.test(r)) return tr("pipeline:text.noProgress");
  if ((m = /^(\d+) of (\d+) fix attempt\(s\) used$/.exec(r))) return tr("pipeline:text.attemptsUsed", { used: m[1], allowed: m[2] });
  if ((m = /^the pipeline's time limit of (\d+) min is reached$/.exec(r))) return tr("pipeline:text.reasonWallTime", { mins: m[1] });
  if ((m = /^the task's budget is spent \(\$([\d.]+) of \$([\d.]+)\)$/.exec(r))) {
    return tr("pipeline:text.reasonBudget", { spent: formatCurrency(Number(m[1])), limit: formatCurrency(Number(m[2])) });
  }
  return r;
}

/** Run-limit codes of a failed agent run (`<code>: <message>`, agent-runtime run.rs `Outcome::Limit`). */
const LIMIT_CODES = ["budget_exceeded", "run_budget_exceeded", "project_budget_exceeded", "max_turns", "time_limit"] as const;
type LimitCode = (typeof LIMIT_CODES)[number];

/**
 * The fixed English message of a run limit (run.rs) as localized text with
 * locale-formatted numbers and money; an unknown shape stays as is.
 */
function limitDetail(message: string): string {
  let m: RegExpMatchArray | null;
  if ((m = message.match(/^token budget exhausted \((\d+) of (\d+) tokens used\)$/))) {
    return t("coreText:limitDetail.tokens", { used: formatInteger(Number(m[1])), max: formatInteger(Number(m[2])) });
  }
  if ((m = message.match(/^cost budget exhausted \(\$(\d+(?:\.\d+)?) spent of \$(\d+(?:\.\d+)?)\)$/))) {
    // Spend keeps the core's four decimals: $0.2501 over a $0.25 cap must not read as "0.25 of 0.25".
    const spent = formatNumber(Number(m[1]), { style: "currency", currency: "USD", currencyDisplay: "narrowSymbol", minimumFractionDigits: 2, maximumFractionDigits: 4 });
    return t("coreText:limitDetail.cost", { spent, max: formatCurrency(Number(m[2])) });
  }
  if ((m = message.match(/^turn limit of (\d+) reached without submit_result$/))) {
    return t("coreText:limitDetail.turns", { count: Number(m[1]) });
  }
  if ((m = message.match(/^wall-clock limit reached after (\d+)s$/))) {
    return t("coreText:limitDetail.time", { duration: formatDuration(Number(m[1]) * 1000) });
  }
  if (message === "a project/global spending budget would be exceeded") return t("coreText:limitDetail.projectBudget");
  return message;
}

/** `<limit code>: <message>` of a failed run, localized; `null` when it is not a run-limit reason. */
function limitReason(text: string): string | null {
  const m = text.match(/^([a-z][a-z0-9_]*): ([\s\S]+)$/);
  const code = m?.[1];
  if (!m || !code || !(LIMIT_CODES as readonly string[]).includes(code)) return null;
  return t(`coreText:taskReason.limit.${code as LimitCode}`, { detail: limitDetail(m[2] ?? "") });
}

/**
 * Display text of a task status reason. Known codes and sentences are
 * translated (their free-text tail is kept); `null` when there is nothing
 * worth showing (no reason, or an unknown bare machine code) so the caller
 * shows its own localized default. Other free text is returned as is.
 */
export function taskReason(reason: string | null | undefined): string | null {
  const r = reason?.trim();
  if (!r) return null;
  const key = TASK_REASON[r as keyof typeof TASK_REASON];
  if (key) return t(`coreText:taskReason.${key}`);
  let m: RegExpMatchArray | null;
  if ((m = r.match(/^agent_error: ([\s\S]+)$/))) return t("coreText:taskReason.agentError", { detail: m[1] });
  if ((m = r.match(/^code review rejected the change: ([\s\S]+)$/))) return t("coreText:taskReason.reviewRejectedDetail", { detail: m[1] });
  if ((m = r.match(/^code review requested changes: ([\s\S]*)$/))) return t("coreText:taskReason.reviewChanges", { detail: m[1] });
  if ((m = r.match(/^adversarial review found unresolved critical issues: ([\s\S]*)$/))) return t("coreText:taskReason.adversarial", { detail: m[1] });
  if ((m = r.match(/^self-healing stopped \(([\s\S]+)\); the user declined further attempts$/))) {
    return t("coreText:taskReason.healingDeclined", { detail: healingReason(m[1] ?? "") });
  }
  if ((m = r.match(/^self-healing stopped \(([\s\S]+)\) and the escalation could not be filed: ([\s\S]+)$/))) {
    return t("coreText:taskReason.healingEscalationFailed", { detail: healingReason(m[1] ?? ""), error: m[2] });
  }
  if ((m = r.match(/^self-healing stopped \(([\s\S]+)\); raise the budget and retry the task to continue$/))) {
    return t("coreText:taskReason.healingRaiseBudget", { detail: healingReason(m[1] ?? "") });
  }
  if ((m = r.match(/^could not start an agent: ([\s\S]+)$/))) return t("coreText:taskReason.agentStartFailed", { detail: m[1] });
  if ((m = r.match(/^delegated to the lead of ([\s\S]+)$/))) return t("coreText:taskReason.delegated", { team: builtinTeamName(m[1] ?? "") });
  if ((m = r.match(/^quality pipeline ended: ([\s\S]+)$/))) return t("coreText:taskReason.pipelineEnded", { detail: m[1] });
  const limit = limitReason(r);
  if (limit !== null) return limit;
  // A bare machine code this client does not know: the caller shows its localized default.
  if (/^[a-z][a-z0-9]*(?:_[a-z0-9]+)+$/.test(r) || /^[A-Z][a-z]+(?:[A-Z][a-z]+)+$/.test(r)) return null;
  return r;
}

// ── AI Manager runs ─────────────────────────────────────────────────────

/** Fixed run status reasons (crates/orchestrator/src/run.rs, parallel.rs). */
const RUN_REASON = {
  "paused by the user — no new agents start": "paused",
  "paused by the user when Orkestr stopped — resume it to continue": "pausedOnShutdown",
  "Orkestr stopped while the run was active — resume it to continue": "interrupted",
  "cancelled by the user": "cancelled",
  "emergency stop": "emergency",
  "the task was deleted": "taskDeleted",
  "the project is not a git repository with a commit — agents share the checkout instead of own worktrees": "noGit",
  // Team lead runs (crates/orchestrator/src/team_lead.rs, teams.rs).
  "the Manager run was cancelled": "managerRunCancelled",
  "Orkestr is shutting down": "shuttingDown",
  "Orkestr stopped while the team lead was working — resume the run to retry": "leadInterrupted",
} as const;

const TASK_STATUS_RE = /"([^"]*)" \((\w+)\)/g;

/** A `$` amount written by the core, in the UI locale with the core's precision ("$0.5012" stays 4 decimals). */
const usd = (v: string | undefined) => {
  const digits = Math.min(6, Math.max(2, (v ?? "").split(".")[1]?.length ?? 0));
  return formatNumber(Number(v ?? 0), { style: "currency", currency: "USD", currencyDisplay: "narrowSymbol", minimumFractionDigits: digits, maximumFractionDigits: digits });
};

function taskStatusLabel(status: string): string {
  const key = `common:status.task.${status}`;
  return i18n.exists(key) ? t(key as "common:status.task.Completed") : status;
}

const waiting = (reason: string) => t("scheduler:reason.waiting", { reason });

/**
 * A scheduler wait text (`waiting: …`: crates/task-engine scheduler.rs
 * `Deferral` and orchestrator parallel.rs `hold_text`) in the UI language;
 * `null` when the shape is unknown.
 */
function waitText(text: string): string | null {
  const m0 = text.match(/^waiting: ([\s\S]+)$/);
  if (!m0) return null;
  const s = m0[1] ?? "";
  let m: RegExpMatchArray | null;
  const n = (i: number) => Number(m?.[i] ?? 0);
  let reason: WaitReason | null = null;
  if (s === "the run is paused") reason = { kind: "paused" };
  else if ((m = s.match(/^run agent limit (\d+)\/(\d+)$/))) reason = { kind: "agentBudget", spawned: n(1), limit: n(2) };
  else if ((m = s.match(/^global parallel limit (\d+)\/(\d+)$/))) reason = { kind: "globalLimit", running: n(1), limit: n(2) };
  else if ((m = s.match(/^project parallel limit (\d+)\/(\d+)$/))) reason = { kind: "projectLimit", running: n(1), limit: n(2) };
  else if ((m = s.match(/^team (.+) limit (\d+)\/(\d+)$/))) {
    return waiting(t("scheduler:reason.teamLimit", { team: m[1], running: n(2), limit: n(3) }));
  } else if ((m = s.match(/^provider (\S+) is rate-limited until (\S+)$/))) {
    return waiting(t("scheduler:reason.rateLimited", { provider: providerName(null, m[1] ?? ""), time: `${m[2] ?? ""} UTC` }));
  } else if ((m = s.match(/^provider concurrency (\d+)\/(\d+) \((\S+)\)$/))) {
    return waiting(t("scheduler:reason.providerConcurrency", { running: n(1), limit: n(2), provider: providerName(null, m[3] ?? "") }));
  } else if ((m = s.match(/^model concurrency (\d+)\/(\d+) \((.+)\)$/))) {
    return waiting(t("scheduler:reason.modelConcurrency", { running: n(1), limit: n(2), model: modelName(m[3] ?? "") }));
  } else if ((m = s.match(/^CPU would reach (\d+)% \(limit (\d+)%\)$/))) {
    return waiting(t("scheduler:reason.cpu", { projected: formatPercent(n(1) / 100), limit: formatPercent(n(2) / 100) }));
  } else if ((m = s.match(/^(\d+) MiB RAM free, (\d+) MiB needed$/))) {
    return waiting(t("scheduler:reason.memory", { free: formatBytes(n(1) * MB), needed: formatBytes(n(2) * MB) }));
  } else if ((m = s.match(/^(\d+) MiB disk free \(reserve (\d+) MiB\)$/))) {
    return waiting(t("scheduler:reason.disk", { free: formatBytes(n(1) * MB), min: formatBytes(n(2) * MB) }));
  } else if ((m = s.match(/^GPU at (\d+)% \(limit (\d+)%\)$/))) {
    return waiting(t("scheduler:reason.gpu", { usage: formatPercent(n(1) / 100), limit: formatPercent(n(2) / 100) }));
  } else if ((m = s.match(/^(.+) is being edited by task (\S+)$/))) {
    return waiting(t("scheduler:reason.fileConflict", { path: m[1] }));
  }
  return reason ? waiting(reasonText(reason)) : null;
}

const MB = 1024 * 1024;

/** One part of a run status reason in the UI language; `null` when the shape is unknown. */
function runReasonPart(part: string): string | null {
  const key = RUN_REASON[part as keyof typeof RUN_REASON];
  if (key) return t(`coreText:runReason.${key}`);
  let m: RegExpMatchArray | null;
  const n = (i: number) => Number(m?.[i] ?? 0);
  if ((m = part.match(/^budget exhausted: \$([\d.]+) spent of \$([\d.]+)$/))) {
    return t("coreText:runReason.budget", { spent: usd(m[1]), limit: usd(m[2]) });
  }
  if ((m = part.match(/^budget exhausted: an agent could not continue within the remaining run budget \(\$([\d.]+) spent of \$([\d.]+)\)$/))) {
    return t("coreText:runReason.budgetAgent", { spent: usd(m[1]), limit: usd(m[2]) });
  }
  if ((m = part.match(/^time limit of (\d+) min reached$/))) return t("coreText:runReason.timeLimit", { count: n(1) });
  if ((m = part.match(/^(\d+) consecutive task failures \(limit (\d+)\)$/))) {
    return t("coreText:runReason.consecutiveFailures", { count: n(1), limit: n(2) });
  }
  if ((m = part.match(/^(\d+) task\(s\) were cancelled$/))) return t("coreText:runReason.tasksCancelled", { count: n(1) });
  if ((m = part.match(/^(\d+) task\(s\) wait for your review$/))) return t("coreText:runReason.review", { count: n(1) });
  if ((m = part.match(/^(\d+) agent\(s\) wait for an approval$/))) return t("coreText:runReason.approval", { count: n(1) });
  if ((m = part.match(/^agent limit reached \((\d+) agents spawned\)$/))) return t("coreText:runReason.agentLimit", { spawned: n(1) });
  if ((m = part.match(/^no task can proceed: ([\s\S]*)$/))) {
    const list = (m[1] ?? "").replace(TASK_STATUS_RE, (_all, title: string, status: string) =>
      t("coreText:runReason.stuckTask", { title, status: taskStatusLabel(status) }),
    );
    return t("coreText:runReason.stuck", { list });
  }
  if ((m = part.match(/^could not start an agent for "([\s\S]+?)": ([\s\S]+)$/))) {
    return t("coreText:runReason.agentStartFailed", { task: m[1], detail: m[2] });
  }
  if ((m = part.match(/^could not delegate "([\s\S]+?)" to the team lead: ([\s\S]+)$/))) {
    return t("coreText:runReason.delegateFailed", { task: m[1], detail: m[2] });
  }
  if ((m = part.match(/^team delegation failed: ([\s\S]+)$/))) return t("coreText:runReason.teamDelegationFailed", { detail: m[1] });
  if ((m = part.match(/^the team's time limit of (\d+) min was reached$/))) return t("coreText:runReason.timeLimit", { count: n(1) });
  if ((m = part.match(/^sub-task "([\s\S]+?)" was deleted$/))) return t("coreText:runReason.subtaskDeleted", { task: m[1] });
  if ((m = part.match(/^sub-task "([\s\S]+?)" failed after (\d+) attempt\(s\)(?:: ([\s\S]+))?$/))) {
    const detail = m[3] ? (taskReason(m[3]) ?? m[3]) : null;
    return detail
      ? t("coreText:runReason.subtaskFailedWhy", { task: m[1], count: n(2), detail })
      : t("coreText:runReason.subtaskFailed", { task: m[1], count: n(2) });
  }
  if ((m = part.match(/^sub-task "([\s\S]+?)" is (\w+)$/))) return t("coreText:runReason.subtaskStatus", { task: m[1], status: taskStatusLabel(m[2] ?? "") });
  if ((m = part.match(/^the lead could not decompose the task: ([\s\S]+)$/))) return t("coreText:runReason.decomposeFailed", { detail: m[1] });
  if ((m = part.match(/^could not create the sub-tasks: ([\s\S]+)$/))) return t("coreText:runReason.subtasksFailed", { detail: m[1] });
  if ((m = part.match(/^could not create the review sub-task: ([\s\S]+)$/))) return t("coreText:runReason.reviewSubtaskFailed", { detail: m[1] });
  if ((m = part.match(/^could not retry "([\s\S]+?)": ([\s\S]+)$/))) return t("coreText:runReason.retryFailed", { task: m[1], detail: m[2] });
  if ((m = part.match(/^the template of "([\s\S]+?)" is missing$/))) return t("coreText:runReason.templateMissing", { task: m[1] });
  if ((m = part.match(/^ended (\w+)$/))) return t("coreText:runReason.ended", { status: taskStatusLabel(m[1] ?? "") });
  if ((m = part.match(/^quality pipeline: ([\s\S]+)$/))) {
    return t("coreText:runReason.pipeline", { detail: localizePipelineText(m[1] ?? "") });
  }
  if ((m = part.match(/^(\d+) task\(s\) (waiting: [\s\S]+)$/))) {
    const w = waitText(m[2] ?? "");
    if (w) return t("coreText:runReason.waitingTasks", { count: n(1), reason: w });
  }
  return waitText(part);
}

/**
 * Display text of an AI Manager run status reason (`ManagerRun.statusReason`).
 * The orchestrator writes fixed English shapes (joined with "; " while the
 * run waits); known shapes are shown in the UI language with locale number
 * and currency formatting, anything else (error messages) as written.
 */
export function runReason(reason: string | null | undefined): string | null {
  const r = reason?.trim();
  if (!r) return null;
  const whole = runReasonPart(r);
  if (whole !== null) return whole;
  return r
    .split("; ")
    .map((p) => runReasonPart(p) ?? p)
    .join("; ");
}

/** Display text of a run task's `lastError` (run shapes, task status reasons, pipeline texts). */
export function runTaskError(error: string | null | undefined): string | null {
  const e = error?.trim();
  if (!e) return null;
  return runReasonPart(e) ?? taskReason(e) ?? localizePipelineText(e);
}

// ---------------------------------------------------------------------------
// Approval requests: the core writes `actionSummary` / `reason` in English
// (agent tool asks, questions, shell gate, merge, Self-Healing escalation,
// Manager gate, Codex, permission engine reasons). Display only.
// ---------------------------------------------------------------------------

const AGENT_TOOLS = [
  "read_file",
  "write_file",
  "edit_file",
  "list_dir",
  "search_files",
  "run_command",
  "git_commit",
  "git_diff",
  "git_status",
  "create_snapshot",
  "record_memory",
  "record_decision",
  "ask_user",
  "submit_result",
] as const;

/** `prompt::role_name` (English display) → `agents:roles` key. */
const ROLE_KEYS: Record<string, string> = {
  Manager: "Manager",
  Architect: "Architect",
  Backend: "Backend",
  Frontend: "Frontend",
  Database: "Database",
  QA: "Qa",
  Security: "Security",
  DevOps: "DevOps",
  "UI/UX": "UiUx",
  "Product Manager": "ProductManager",
  Reviewer: "Reviewer",
  Researcher: "Researcher",
};

const CAPABILITIES = [
  "Read",
  "Write",
  "Delete",
  "Terminal",
  "Git",
  "Network",
  "Browser",
  "DbRead",
  "DbWrite",
  "Docker",
  "Ssh",
  "Deploy",
  "ComputerControl",
  "Secrets",
] as const;
const MODES = ["Ask", "SemiAuto", "FullAuto"] as const;
const STAGES = ["Author", "Review", "Build", "Test", "Security", "Merge"] as const;

function capabilityLabel(cap: string): string {
  return (CAPABILITIES as readonly string[]).includes(cap) ? t(`security:capability.${cap as (typeof CAPABILITIES)[number]}`) : cap;
}

function roleLabel(role: string): string {
  const key = ROLE_KEYS[role];
  return key ? t(`agents:roles.${key as "Backend"}`) : role;
}

/** Display text of an approval's `actionSummary`. Unknown text is returned as is. */
export function approvalSummary(summary: string): string {
  const s = summary.trim();
  let m: RegExpMatchArray | null;
  if ((m = s.match(/^(.+?) wants to ([a-z_]+)(?: ([\s\S]*))?$/)) && (AGENT_TOOLS as readonly string[]).includes(m[2] ?? "")) {
    const agent = agentName(m[1] ?? "");
    const action = t(`coreText:approval.action.${m[2] as (typeof AGENT_TOOLS)[number]}`);
    const resource = (m[3] ?? "").trim();
    return resource ? t("coreText:approval.wantsTo", { agent, action, resource }) : t("coreText:approval.wantsToBare", { agent, action });
  }
  if ((m = s.match(/^Question from ([\s\S]+)$/))) return t("coreText:approval.questionFrom", { agent: agentName(m[1] ?? "") });
  if (s === "Run command (agent terminal)") return t("coreText:approval.runCommandTerminal");
  if (s === "Run command") return t("coreText:approval.runCommand");
  if ((m = s.match(/^Merge (\d+) branch\(es\) into `([\s\S]+)`$/))) return t("coreText:approval.merge", { count: Number(m[1]), target: m[2] });
  if ((m = s.match(/^Self-healing gave up on "([\s\S]*)" — continue fixing\?$/))) return t("coreText:approval.healingGaveUp", { task: m[1] });
  if ((m = s.match(/^Let ([\s\S]+) run Codex in its sandbox$/))) return t("coreText:approval.codex", { agent: agentName(m[1] ?? "") });
  if ((m = s.match(/^Start a ([\s\S]+?) agent on "([\s\S]*)"$/))) return t("coreText:approval.startAgent", { role: roleLabel(m[1] ?? ""), task: m[2] });
  if ((m = s.match(/^Create (\d+) task\(s\)$/))) return t("coreText:approval.createTasks", { count: Number(m[1]) });
  return s;
}

function healingOffer(offer: string): string {
  let m: RegExpMatchArray | null;
  if ((m = offer.match(/^another (\d+) min of self-healing$/))) return t("coreText:approval.reason.offerMinutes", { count: Number(m[1]) });
  if ((m = offer.match(/^another \$([\d.]+) of self-healing spend$/))) {
    return t("coreText:approval.reason.offerBudget", { amount: formatCurrency(Number(m[1])) });
  }
  if ((m = offer.match(/^(\d+) more attempt\(s\) \(a different model is tried\)$/))) {
    return t("coreText:approval.reason.offerAttempts", { count: Number(m[1]) });
  }
  return offer;
}

const ENGINE_WHAT: Record<string, "network" | "secret_file" | "outside_worktree"> = {
  "the command uses the network": "network",
  "the command reads a secrets file": "secret_file",
  "the command reads files outside the worktree": "outside_worktree",
};

/** Display text of an approval's `reason`. Unknown text (e.g. a guard's danger reason) is returned as is. */
export function approvalReason(reason: string): string {
  const r = reason.trim();
  let m: RegExpMatchArray | null;
  if ((m = r.match(/^The agent asks: ([\s\S]*)$/))) return t("coreText:approval.reason.agentAsks", { question: m[1] });
  if (r === "The Merge Manager merges agent branches only with your approval (merge policy).") return t("coreText:approval.reason.mergePolicy");
  if (r === "the AI Manager's approval policy is Ask") return t("coreText:approval.reason.managerAsk");
  if (r === "the AI Manager asks for your decision") return t("coreText:approval.reason.managerDecision");
  if (r.startsWith("Codex runs commands inside its own workspace-write sandbox;")) return t("coreText:approval.reason.codex");
  if (
    (m = r.match(
      /^The quality pipeline could not fix the (\w+) stage automatically: ([\s\S]+?)\. Last failure: ([\s\S]*)\. Approve to grant ([\s\S]+); reject to mark the task failed\.$/,
    ))
  ) {
    const stage = (STAGES as readonly string[]).includes(m[1] ?? "") ? t(`pipeline:stage.${m[1] as (typeof STAGES)[number]}`) : (m[1] ?? "");
    return t("coreText:approval.reason.healing", { stage, reason: healingReason(m[2] ?? ""), failure: m[3], offer: healingOffer(m[4] ?? "") });
  }
  if ((m = r.match(/^([\s\S]+) \((\w+) requires approval\)$/)) && ENGINE_WHAT[m[1] ?? ""]) {
    const what = t(`coreText:approval.reason.what.${ENGINE_WHAT[m[1] ?? ""] ?? "outside_worktree"}`);
    return t("coreText:approval.reason.whatRequires", { what, capability: capabilityLabel(m[2] ?? "") });
  }
  if (r === "computer control always requires approval") return t("coreText:approval.reason.computerControl");
  if ((m = r.match(/^a permission rule requires approval for (\w+)$/))) return t("coreText:approval.reason.rule", { capability: capabilityLabel(m[1] ?? "") });
  if (r === "reading a secrets file requires approval") return t("coreText:approval.reason.secretFile");
  if ((m = r.match(/^(\w+) requires approval in (\w+) mode$/)) && (MODES as readonly string[]).includes(m[2] ?? "")) {
    return t("coreText:approval.reason.modeDefault", {
      capability: capabilityLabel(m[1] ?? ""),
      mode: t(`common:status.approvalMode.${m[2] as (typeof MODES)[number]}`),
    });
  }
  return r;
}
