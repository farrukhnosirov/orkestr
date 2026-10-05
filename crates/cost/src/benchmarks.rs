//! Execution benchmarks: model benchmarking and agent performance (§22,
//! §68.38–39).
//!
//! ## Collection
//!
//! Every agent instance is one *execution* (the runtime spawns one instance
//! per task run). The core feeds every published event to
//! [`CostEngine::apply_benchmark_events`]; this module turns them into
//! per-execution quality signals, stored in `benchmark_executions`:
//!
//! | event | effect |
//! |---|---|
//! | `AgentSpawned` | new execution (template, role, project, task, spawn model) |
//! | `AgentStarted` / `AgentStopped` | start / end, outcome, stop reason |
//! | `TaskStatusChanged` Review/Testing → Running | fix loop of the task's *author* |
//! | `TaskStatusChanged` Failed → Ready / Recovering | retry of the author |
//! | `TaskStatusChanged` → Completed / Failed / Cancelled | final task status and *first-pass success* |
//! | `ReviewFinished` | review, findings (defects), rejection (ChangesRequested / Rejected) |
//! | `BuildSucceeded` / `BuildFailed` | build success of the emitting agent (else the task author) |
//! | `TestRunFinished` | failing tests (defects) |
//! | `TaskCreated` of a bug task linked to a completed task | post-merge bug of that task's author |
//!
//! The *author* of a task is its most recent non-reviewer execution. A bug
//! task is linked through a tag `bug:<taskId>` (fits the 40-character task
//! tag limit; `fixes:` / `bug-of:` / `regression-of:` / `caused-by:` are
//! accepted where longer tags are allowed),
//! or through `parentId`
//! when it carries a `bug` / `regression` tag ([`is_bug_task`],
//! [`linked_tasks`]).
//!
//! Events are applied exactly once: the last applied `seq` is stored in
//! `benchmark_meta` in the same transaction, so the collector can replay the
//! persistent event log after a restart without double counting.
//!
//! ## Queries
//!
//! Tokens, cost, latency and call success come from `usage_records` (joined
//! by `agent_id`); the effective model of an execution is the model of its
//! latest call (fallback: the model it was spawned with). Rates are `None`
//! until there is data behind them — nothing is fabricated.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{Duration, SecondsFormat, Utc};
use orkestr_domain::{
    AgentId, AgentInstance, AgentRole, ProjectId, ProviderId, ReviewState, Task, TaskId, TaskStatus, Timestamp,
};
use orkestr_protocol::event::{EventKind, StopReason};
use orkestr_protocol::methods::benchmarks::{
    AgentGroupBy, AgentPerformanceParams, AgentPerformanceRow, BenchmarkRecord, BenchmarkRecordsParams,
    ExecutionOutcome, ModelBenchmarkRow, ModelBenchmarksParams,
};
use orkestr_protocol::Event;
use orkestr_storage::rusqlite::{self, params, params_from_iter, Connection, OptionalExtension, Row};

use crate::stats::percentile;
use crate::{micros_to_usd, CostEngine, CostResult};

/// Key of the applied-event cursor in `benchmark_meta`.
pub const CURSOR_KEY: &str = "events_cursor";
/// Default analytics window (days).
pub const DEFAULT_DAYS: u32 = 30;
/// Max analytics window (days).
pub const MAX_DAYS: u32 = 365;
/// Tag prefixes linking a bug task to the task that introduced the bug.
pub const BUG_LINK_PREFIXES: &[&str] = &["bug:", "fixes:", "bug-of:", "regression-of:", "caused-by:"];
/// Max executions loaded for one aggregate.
const MAX_AGGREGATE_ROWS: u32 = 50_000;

/// Display label of a role; identical to the routing `purpose` the agent
/// runtime uses, so router statistics can be looked up by purpose.
#[must_use]
pub fn role_label(role: &AgentRole) -> String {
    match role {
        AgentRole::Custom(s) => s.clone(),
        AgentRole::UiUx => "UI/UX".into(),
        AgentRole::Qa => "QA".into(),
        AgentRole::ProductManager => "Product Manager".into(),
        AgentRole::DevOps => "DevOps".into(),
        other => format!("{other:?}"),
    }
}

/// Template label from an instance name (`"Backend Agent #3"` → `"Backend Agent"`).
#[must_use]
pub fn template_label(name: &str) -> String {
    let trimmed = name.trim();
    if let Some((head, tail)) = trimmed.rsplit_once(" #") {
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) && !head.trim().is_empty() {
            return head.trim().to_owned();
        }
    }
    trimmed.to_owned()
}

/// Whether a task is a bug / regression report.
#[must_use]
pub fn is_bug_task(task: &Task) -> bool {
    task.tags.iter().any(|t| {
        let t = t.trim().to_lowercase();
        t == "bug" || t == "regression" || t == "kind:bug" || BUG_LINK_PREFIXES.iter().any(|p| t.starts_with(p))
    })
}

/// Tasks a bug task is linked to (deduplicated, in tag order, parent last).
#[must_use]
pub fn linked_tasks(task: &Task) -> Vec<TaskId> {
    let mut out: Vec<TaskId> = Vec::new();
    for tag in &task.tags {
        let lower = tag.trim().to_lowercase();
        for p in BUG_LINK_PREFIXES {
            if let Some(rest) = lower.strip_prefix(p) {
                if let Ok(id) = rest.trim().parse::<TaskId>() {
                    if id != task.id && !out.contains(&id) {
                        out.push(id);
                    }
                }
            }
        }
    }
    let tagged_bug =
        task.tags.iter().any(|t| matches!(t.trim().to_lowercase().as_str(), "bug" | "regression" | "kind:bug"));
    if let Some(parent) = task.parent_id {
        if (tagged_bug || !out.is_empty()) && !out.contains(&parent) {
            out.push(parent);
        }
    }
    out
}

fn ts_key(t: &Timestamp) -> String {
    t.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn parse_ts(s: &str) -> Option<Timestamp> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

fn outcome_str(o: ExecutionOutcome) -> &'static str {
    match o {
        ExecutionOutcome::Running => "Running",
        ExecutionOutcome::Succeeded => "Succeeded",
        ExecutionOutcome::Failed => "Failed",
        ExecutionOutcome::Cancelled => "Cancelled",
    }
}

fn parse_outcome(s: &str) -> ExecutionOutcome {
    match s {
        "Succeeded" => ExecutionOutcome::Succeeded,
        "Failed" => ExecutionOutcome::Failed,
        "Cancelled" => ExecutionOutcome::Cancelled,
        _ => ExecutionOutcome::Running,
    }
}

fn stop_outcome(reason: StopReason) -> ExecutionOutcome {
    match reason {
        StopReason::Completed => ExecutionOutcome::Succeeded,
        StopReason::LimitReached | StopReason::Error => ExecutionOutcome::Failed,
        StopReason::UserRequested | StopReason::EmergencyStop | StopReason::Shutdown => ExecutionOutcome::Cancelled,
    }
}

/// What applying a batch of events did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// Events with `seq` above the cursor.
    pub applied: u32,
    /// Events that changed at least one execution.
    pub changed: u32,
    /// Executions finished or tasks reached a terminal state (routing
    /// statistics should be refreshed).
    pub finished: u32,
    /// Cursor after the batch.
    pub cursor: u64,
}

/// The task's author execution: newest non-reviewer execution (else newest).
fn author(
    c: &Connection,
    task: TaskId,
    exclude: Option<AgentId>,
    only_completed: bool,
) -> rusqlite::Result<Option<String>> {
    let completed = if only_completed { " AND task_status = 'Completed'" } else { "" };
    let sql = format!(
        "SELECT agent_id FROM benchmark_executions WHERE task_id = ?1 AND agent_id IS NOT ?2{completed}
         ORDER BY (role = 'Reviewer') ASC, started_at DESC, agent_id DESC LIMIT 1"
    );
    c.query_row(&sql, params![task.to_string(), exclude.map(|a| a.to_string())], |r| r.get(0)).optional()
}

fn exists(c: &Connection, agent: AgentId) -> rusqlite::Result<bool> {
    Ok(c.query_row("SELECT 1 FROM benchmark_executions WHERE agent_id = ?1", [agent.to_string()], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Execution an event about builds/tests is attributed to.
fn target(c: &Connection, ev: &Event) -> rusqlite::Result<Option<String>> {
    if let Some(a) = ev.agent_id {
        if exists(c, a)? {
            return Ok(Some(a.to_string()));
        }
    }
    match ev.task_id {
        Some(t) => author(c, t, None, false),
        None => Ok(None),
    }
}

fn bump(c: &Connection, agent: &str, sets: &str, now: &str) -> rusqlite::Result<bool> {
    let sql = format!("UPDATE benchmark_executions SET {sets}, updated_at = ?2 WHERE agent_id = ?1");
    Ok(c.execute(&sql, params![agent, now])? > 0)
}

fn spawn(c: &Connection, a: &AgentInstance, at: &str) -> rusqlite::Result<bool> {
    let n = c.execute(
        "INSERT INTO benchmark_executions (agent_id, agent_name, template_id, template_name, role, project_id, task_id,
            provider_id, model, started_at, outcome, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'Running', ?10)
         ON CONFLICT(agent_id) DO NOTHING",
        params![
            a.id.to_string(),
            a.name,
            a.template_id.map(|t| t.to_string()),
            template_label(&a.name),
            role_label(&a.role),
            a.project_id.to_string(),
            a.task_id.map(|t| t.to_string()),
            a.model.as_ref().map(|m| m.provider_id.to_string()),
            a.model.as_ref().map(|m| m.model.clone()),
            at,
        ],
    )?;
    Ok(n > 0)
}

/// Applies one event to the execution table. Returns `(changed, finished)`.
///
/// # Errors
/// SQLite failures.
pub fn apply_event(c: &Connection, ev: &Event) -> rusqlite::Result<(bool, bool)> {
    let now = ts_key(&ev.ts);
    match &ev.kind {
        EventKind::AgentSpawned(p) => Ok((spawn(c, &p.agent, &now)?, false)),
        EventKind::AgentStarted(p) => {
            let n = c.execute(
                "UPDATE benchmark_executions SET task_id = COALESCE(?2, task_id), outcome = 'Running', ended_at = NULL,
                    stop_reason = NULL, updated_at = ?3 WHERE agent_id = ?1",
                params![p.agent_id.to_string(), p.task_id.map(|t| t.to_string()), now],
            )?;
            Ok((n > 0, false))
        }
        EventKind::AgentStopped(p) => {
            let n = c.execute(
                "UPDATE benchmark_executions SET ended_at = ?2, stop_reason = ?3, outcome = ?4, updated_at = ?2
                 WHERE agent_id = ?1 AND ended_at IS NULL",
                params![p.agent_id.to_string(), now, format!("{:?}", p.reason), outcome_str(stop_outcome(p.reason))],
            )?;
            Ok((n > 0, n > 0))
        }
        EventKind::TaskStatusChanged(p) => {
            let task = p.task_id;
            let mut changed = false;
            let mut finished = false;
            match (p.from, p.to) {
                (TaskStatus::Review | TaskStatus::Testing, TaskStatus::Running) => {
                    if let Some(a) = author(c, task, None, false)? {
                        changed |= bump(c, &a, "fix_loops = fix_loops + 1", &now)?;
                    }
                }
                (TaskStatus::Failed, TaskStatus::Ready | TaskStatus::Recovering) => {
                    if let Some(a) = author(c, task, None, false)? {
                        changed |= bump(c, &a, "retries = retries + 1", &now)?;
                    }
                }
                _ => {}
            }
            match p.to {
                TaskStatus::Completed => {
                    let n = c.execute(
                        "UPDATE benchmark_executions SET task_status = 'Completed', updated_at = ?2,
                            first_pass = CASE
                                WHEN outcome = 'Cancelled' THEN NULL
                                WHEN outcome != 'Failed' AND fix_loops = 0 AND review_rejections = 0 AND retries = 0 THEN 1
                                ELSE 0 END
                         WHERE task_id = ?1 AND task_status IS NULL",
                        params![task.to_string(), now],
                    )?;
                    changed |= n > 0;
                    finished |= n > 0;
                }
                TaskStatus::Failed => {
                    if let Some(a) = author(c, task, None, false)? {
                        c.execute(
                            "UPDATE benchmark_executions SET outcome = 'Failed' WHERE agent_id = ?1 AND outcome = 'Succeeded'",
                            [&a],
                        )?;
                    }
                    let n = c.execute(
                        "UPDATE benchmark_executions SET task_status = 'Failed', updated_at = ?2,
                            first_pass = CASE WHEN outcome = 'Cancelled' THEN NULL ELSE 0 END
                         WHERE task_id = ?1 AND task_status IS NULL",
                        params![task.to_string(), now],
                    )?;
                    changed |= n > 0;
                    finished |= n > 0;
                }
                TaskStatus::Cancelled => {
                    let n = c.execute(
                        "UPDATE benchmark_executions SET task_status = 'Cancelled', updated_at = ?2
                         WHERE task_id = ?1 AND task_status IS NULL",
                        params![task.to_string(), now],
                    )?;
                    changed |= n > 0;
                }
                _ => {}
            }
            Ok((changed, finished))
        }
        EventKind::ReviewFinished(p) => {
            let Some(task) = ev.task_id else { return Ok((false, false)) };
            let Some(a) = author(c, task, p.reviewer_agent_id, false)? else { return Ok((false, false)) };
            let rejected = matches!(p.verdict, ReviewState::ChangesRequested | ReviewState::Rejected);
            let sets = format!(
                "reviews = reviews + 1, review_findings = review_findings + {}, review_rejections = review_rejections + {}",
                p.findings,
                u32::from(rejected)
            );
            Ok((bump(c, &a, &sets, &now)?, false))
        }
        EventKind::BuildSucceeded(_) | EventKind::BuildFailed(_) => {
            let Some(a) = target(c, ev)? else { return Ok((false, false)) };
            let sets = if matches!(ev.kind, EventKind::BuildSucceeded(_)) {
                "builds_ok = builds_ok + 1"
            } else {
                "builds_failed = builds_failed + 1"
            };
            Ok((bump(c, &a, sets, &now)?, false))
        }
        EventKind::TestRunFinished(p) => {
            let Some(a) = target(c, ev)? else { return Ok((false, false)) };
            let sets = format!("test_runs = test_runs + 1, tests_failed = tests_failed + {}", p.failed);
            Ok((bump(c, &a, &sets, &now)?, false))
        }
        EventKind::TaskCreated(p) if is_bug_task(&p.task) => {
            let mut changed = false;
            for linked in linked_tasks(&p.task) {
                if let Some(a) = author(c, linked, None, true)? {
                    changed |= bump(c, &a, "post_merge_bugs = post_merge_bugs + 1", &now)?;
                }
            }
            Ok((changed, false))
        }
        _ => Ok((false, false)),
    }
}

/// Reads the applied-event cursor.
fn cursor(c: &Connection) -> rusqlite::Result<u64> {
    let v: Option<String> =
        c.query_row("SELECT value FROM benchmark_meta WHERE key = ?1", [CURSOR_KEY], |r| r.get(0)).optional()?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

/// Raw execution row.
struct ExecRow {
    record: BenchmarkRecord,
    template_name: String,
    reviews: u32,
    spawn_provider: Option<ProviderId>,
    spawn_model: Option<String>,
}

const EXEC_COLS: &str = "agent_id, agent_name, template_id, template_name, role, project_id, task_id, provider_id, model,
    started_at, ended_at, outcome, stop_reason, task_status, first_pass, builds_ok, builds_failed, tests_failed, reviews,
    review_rejections, review_findings, fix_loops, retries, post_merge_bugs";

fn opt_parse<T: std::str::FromStr>(v: Option<String>) -> Option<T> {
    v.and_then(|s| s.parse().ok())
}

fn u32_of(v: i64) -> u32 {
    u32::try_from(v.max(0)).unwrap_or(u32::MAX)
}

fn exec_from_row(r: &Row<'_>) -> rusqlite::Result<Option<ExecRow>> {
    let agent: String = r.get(0)?;
    let Ok(agent_id) = agent.parse::<AgentId>() else { return Ok(None) };
    let started: String = r.get(9)?;
    let Some(started_at) = parse_ts(&started) else { return Ok(None) };
    let ended_at = r.get::<_, Option<String>>(10)?.as_deref().and_then(parse_ts);
    let template_name: String = r.get(3)?;
    let tests_failed = u32_of(r.get(17)?);
    let review_findings = u32_of(r.get(20)?);
    let spawn_provider: Option<ProviderId> = opt_parse(r.get(7)?);
    let spawn_model: Option<String> = r.get(8)?;
    let duration_ms = ended_at.map(|e| u64::try_from((e - started_at).num_milliseconds().max(0)).unwrap_or(0));
    let record = BenchmarkRecord {
        agent_id,
        agent_name: r.get(1)?,
        template_id: r.get(2)?,
        role: r.get(4)?,
        project_id: opt_parse(r.get(5)?),
        task_id: opt_parse(r.get(6)?),
        provider_id: spawn_provider,
        model: spawn_model.clone(),
        started_at,
        ended_at,
        outcome: parse_outcome(&r.get::<_, String>(11)?),
        stop_reason: r.get(12)?,
        task_status: r.get(13)?,
        first_pass: r.get::<_, Option<i64>>(14)?.map(|v| v != 0),
        builds_ok: u32_of(r.get(15)?),
        builds_failed: u32_of(r.get(16)?),
        tests_failed,
        review_rejections: u32_of(r.get(19)?),
        fix_loops: u32_of(r.get(21)?),
        retries: u32_of(r.get(22)?),
        defects_found: review_findings.saturating_add(tests_failed),
        post_merge_bugs: u32_of(r.get(23)?),
        calls: 0,
        failed_calls: 0,
        input_tokens: 0,
        output_tokens: 0,
        cost_usd: 0.0,
        latency_avg_ms: None,
        duration_ms,
    };
    Ok(Some(ExecRow { record, template_name, reviews: u32_of(r.get(18)?), spawn_provider, spawn_model }))
}

/// Usage aggregate of one agent.
struct AgentUsage {
    calls: u64,
    failed: u64,
    input: u64,
    output: u64,
    cost_micros: i64,
    latency_avg: Option<f64>,
    last_provider: Option<ProviderId>,
    last_model: Option<String>,
}

fn usage_for(c: &Connection, agents: &[String]) -> rusqlite::Result<HashMap<String, AgentUsage>> {
    let mut out = HashMap::new();
    for chunk in agents.chunks(400) {
        let marks = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT agent_id, COUNT(*), COALESCE(SUM(CASE WHEN success THEN 0 ELSE 1 END), 0),
                COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(cost_micros), 0),
                AVG(latency_ms),
                (SELECT x.provider_id || char(31) || x.model FROM usage_records x WHERE x.agent_id = u.agent_id
                 ORDER BY x.ts DESC, x.seq DESC LIMIT 1)
             FROM usage_records u WHERE agent_id IN ({marks}) GROUP BY agent_id"
        );
        let mut stmt = c.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| {
            let last: Option<String> = r.get(7)?;
            let (lp, lm) = match last.as_deref().and_then(|s| s.split_once('\u{1f}')) {
                Some((p, m)) => (p.parse().ok(), Some(m.to_owned())),
                None => (None, None),
            };
            Ok((
                r.get::<_, String>(0)?,
                AgentUsage {
                    calls: u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
                    failed: u64::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
                    input: u64::try_from(r.get::<_, i64>(3)?).unwrap_or(0),
                    output: u64::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
                    cost_micros: r.get(5)?,
                    latency_avg: r.get(6)?,
                    last_provider: lp,
                    last_model: lm,
                },
            ))
        })?;
        for row in rows {
            let (k, v) = row?;
            out.insert(k, v);
        }
    }
    Ok(out)
}

/// Loads executions matching `where_sql` with usage joined.
fn load(c: &Connection, where_sql: &str, args: &[String], limit: u32) -> rusqlite::Result<Vec<ExecRow>> {
    let sql = format!(
        "SELECT {EXEC_COLS} FROM benchmark_executions WHERE {where_sql}
         ORDER BY started_at DESC, agent_id DESC LIMIT {limit}"
    );
    let mut stmt = c.prepare(&sql)?;
    let mut rows: Vec<ExecRow> = Vec::new();
    for r in stmt.query_map(params_from_iter(args.iter()), exec_from_row)? {
        if let Some(row) = r? {
            rows.push(row);
        }
    }
    let ids: Vec<String> = rows.iter().map(|r| r.record.agent_id.to_string()).collect();
    let usage = usage_for(c, &ids)?;
    for row in &mut rows {
        if let Some(u) = usage.get(&row.record.agent_id.to_string()) {
            let rec = &mut row.record;
            rec.calls = u.calls;
            rec.failed_calls = u.failed;
            rec.input_tokens = u.input;
            rec.output_tokens = u.output;
            rec.cost_usd = micros_to_usd(u.cost_micros);
            rec.latency_avg_ms = u.latency_avg;
            if let (Some(p), Some(m)) = (u.last_provider, u.last_model.clone()) {
                rec.provider_id = Some(p);
                rec.model = Some(m);
            }
        }
        if row.record.model.is_none() {
            row.record.provider_id = row.spawn_provider;
            row.record.model = row.spawn_model.clone();
        }
    }
    Ok(rows)
}

fn since_key(days: u32) -> String {
    ts_key(&(Utc::now() - Duration::days(i64::from(days.clamp(1, MAX_DAYS)))))
}

#[allow(clippy::cast_precision_loss)]
fn rate(num: u64, den: u64) -> Option<f64> {
    (den > 0).then(|| num as f64 / den as f64)
}

#[allow(clippy::cast_precision_loss)]
fn avg(sum: f64, n: u64) -> Option<f64> {
    (n > 0).then(|| sum / n as f64)
}

/// One usage row for model aggregates.
struct UsageRow {
    provider: String,
    model: String,
    role: Option<String>,
    success: bool,
    latency: Option<u64>,
    cost_micros: i64,
    input: u64,
    output: u64,
}

/// A model aggregate key: provider, model and (optionally) role.
type ModelKey = (String, String, Option<String>);

/// Aggregates executions + usage into model rows, keyed by provider, model
/// and (when `by_role`) role.
fn aggregate_models(execs: &[ExecRow], usage: &[UsageRow], by_role: bool) -> Vec<(Option<String>, ModelBenchmarkRow)> {
    #[derive(Default)]
    struct Acc {
        roles: BTreeSet<String>,
        executions: u64,
        finished: u64,
        first_pass_ok: u64,
        first_pass_n: u64,
        builds_ok: u64,
        builds_total: u64,
        defects: u64,
        retries: u64,
        duration_sum: f64,
        duration_n: u64,
        exec_cost_micros: i64,
        exec_tokens: u64,
        calls: u64,
        ok_calls: u64,
        latencies: Vec<u64>,
        input: u64,
        output: u64,
        cost_micros: i64,
    }
    let mut accs: BTreeMap<ModelKey, Acc> = BTreeMap::new();
    for e in execs {
        let r = &e.record;
        let (Some(p), Some(m)) = (r.provider_id, r.model.clone()) else { continue };
        let key = (p.to_string(), m, by_role.then(|| r.role.clone()));
        let a = accs.entry(key).or_default();
        a.roles.insert(r.role.clone());
        a.executions += 1;
        if r.task_status.is_some() {
            a.finished += 1;
            a.defects += u64::from(r.defects_found);
        }
        if let Some(fp) = r.first_pass {
            a.first_pass_n += 1;
            a.first_pass_ok += u64::from(fp);
        }
        a.builds_ok += u64::from(r.builds_ok);
        a.builds_total += u64::from(r.builds_ok) + u64::from(r.builds_failed);
        a.retries += u64::from(r.retries);
        if let Some(d) = r.duration_ms {
            #[allow(clippy::cast_precision_loss)]
            {
                a.duration_sum += d as f64;
            }
            a.duration_n += 1;
        }
        a.exec_cost_micros += crate::usd_to_micros(r.cost_usd);
        a.exec_tokens += r.input_tokens + r.output_tokens;
    }
    for u in usage {
        let key = (u.provider.clone(), u.model.clone(), if by_role { u.role.clone() } else { None });
        if by_role && key.2.is_none() {
            continue;
        }
        let a = accs.entry(key).or_default();
        if let Some(role) = &u.role {
            a.roles.insert(role.clone());
        }
        a.calls += 1;
        a.ok_calls += u64::from(u.success);
        if let Some(l) = u.latency {
            a.latencies.push(l);
        }
        a.input += u.input;
        a.output += u.output;
        a.cost_micros += u.cost_micros;
    }
    let mut out = Vec::new();
    for ((provider, model, role), mut a) in accs {
        let Ok(provider_id) = provider.parse::<ProviderId>() else { continue };
        a.latencies.sort_unstable();
        #[allow(clippy::cast_precision_loss)]
        let latency_avg_ms =
            (!a.latencies.is_empty()).then(|| a.latencies.iter().sum::<u64>() as f64 / a.latencies.len() as f64);
        let cost_total_usd = micros_to_usd(a.cost_micros);
        #[allow(clippy::cast_precision_loss)]
        let row = ModelBenchmarkRow {
            provider_id,
            model,
            roles: a.roles.into_iter().collect(),
            executions: a.executions,
            finished: a.finished,
            calls: a.calls,
            call_success_rate: rate(a.ok_calls, a.calls),
            first_pass_success_rate: rate(a.first_pass_ok, a.first_pass_n),
            build_success_rate: rate(a.builds_ok, a.builds_total),
            defects_found_avg: avg(a.defects as f64, a.finished),
            retries_avg: avg(a.retries as f64, a.executions),
            latency_avg_ms,
            latency_p50_ms: percentile(&a.latencies, 0.5),
            latency_p95_ms: percentile(&a.latencies, 0.95),
            duration_avg_ms: avg(a.duration_sum, a.duration_n),
            input_tokens: a.input,
            output_tokens: a.output,
            tokens_avg: avg(a.exec_tokens as f64, a.executions),
            cost_total_usd,
            cost_avg_usd: if a.executions > 0 {
                avg(micros_to_usd(a.exec_cost_micros), a.executions)
            } else {
                avg(cost_total_usd, a.calls)
            },
            routing_score: None,
        };
        out.push((role, row));
    }
    out.sort_by(|a, b| {
        b.1.executions
            .cmp(&a.1.executions)
            .then_with(|| b.1.calls.cmp(&a.1.calls))
            .then_with(|| a.1.model.cmp(&b.1.model))
            .then_with(|| a.1.provider_id.cmp(&b.1.provider_id))
            .then_with(|| a.0.cmp(&b.0))
    });
    out
}

fn window_filter(project: Option<ProjectId>, role: Option<&str>, days: u32) -> (String, Vec<String>) {
    let mut clauses = vec!["started_at >= ?".to_owned()];
    let mut args = vec![since_key(days)];
    if let Some(p) = project {
        clauses.push("project_id = ?".into());
        args.push(p.to_string());
    }
    if let Some(r) = role {
        clauses.push("role = ?".into());
        args.push(r.to_owned());
    }
    (clauses.join(" AND "), args)
}

fn load_usage(
    c: &Connection,
    project: Option<ProjectId>,
    role: Option<&str>,
    days: u32,
) -> rusqlite::Result<Vec<UsageRow>> {
    let mut sql =
        "SELECT u.provider_id, u.model, e.role, u.success, u.latency_ms, u.cost_micros, u.input_tokens, u.output_tokens
         FROM usage_records u LEFT JOIN benchmark_executions e ON e.agent_id = u.agent_id
         WHERE u.ts >= ?"
            .to_owned();
    let mut args = vec![since_key(days)];
    if let Some(p) = project {
        sql.push_str(" AND u.project_id = ?");
        args.push(p.to_string());
    }
    if let Some(r) = role {
        sql.push_str(" AND e.role = ?");
        args.push(r.to_owned());
    }
    let mut stmt = c.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(args.iter()), |r| {
            Ok(UsageRow {
                provider: r.get(0)?,
                model: r.get(1)?,
                role: r.get(2)?,
                success: r.get(3)?,
                latency: r.get::<_, Option<i64>>(4)?.and_then(|v| u64::try_from(v).ok()),
                cost_micros: r.get(5)?,
                input: u64::try_from(r.get::<_, i64>(6)?).unwrap_or(0),
                output: u64::try_from(r.get::<_, i64>(7)?).unwrap_or(0),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Routing statistics of one model (all roles when `role` is `None`).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRoutingStats {
    /// Role (routing purpose), `None` = all roles.
    pub role: Option<String>,
    /// Aggregate row.
    pub row: ModelBenchmarkRow,
}

impl CostEngine {
    /// Applies a batch of events (ascending `seq`) exactly once.
    pub async fn apply_benchmark_events(&self, events: Vec<Event>) -> CostResult<ApplyReport> {
        Ok(self
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                let mut report = ApplyReport { cursor: cursor(&tx)?, ..ApplyReport::default() };
                for ev in &events {
                    if ev.seq <= report.cursor {
                        continue;
                    }
                    let (changed, finished) = apply_event(&tx, ev)?;
                    report.applied += 1;
                    report.changed += u32::from(changed);
                    report.finished += u32::from(finished);
                    report.cursor = ev.seq;
                }
                tx.execute(
                    "INSERT INTO benchmark_meta (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![CURSOR_KEY, report.cursor.to_string()],
                )?;
                tx.commit()?;
                Ok(report)
            })
            .await?)
    }

    /// Last applied event `seq`.
    pub async fn benchmark_cursor(&self) -> CostResult<u64> {
        Ok(self.db.call(|c| Ok(cursor(c)?)).await?)
    }

    /// Raw execution records, newest first.
    pub async fn benchmark_records(&self, p: BenchmarkRecordsParams) -> CostResult<Vec<BenchmarkRecord>> {
        let limit = p.limit.unwrap_or(100).clamp(1, 1000);
        let mut clauses: Vec<String> = Vec::new();
        let mut args: Vec<String> = Vec::new();
        for (clause, value) in [
            ("project_id = ?", p.project_id.map(|v| v.to_string())),
            ("task_id = ?", p.task_id.map(|v| v.to_string())),
            ("agent_id = ?", p.agent_id.map(|v| v.to_string())),
        ] {
            if let Some(v) = value {
                clauses.push(clause.into());
                args.push(v);
            }
        }
        if let Some(prov) = p.provider_id {
            clauses.push(
                "(provider_id = ? OR agent_id IN (SELECT agent_id FROM usage_records WHERE provider_id = ?))".into(),
            );
            args.push(prov.to_string());
            args.push(prov.to_string());
        }
        if let Some(m) = p.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            clauses.push("(model = ? OR agent_id IN (SELECT agent_id FROM usage_records WHERE model = ?))".into());
            args.push(m.to_owned());
            args.push(m.to_owned());
        }
        let where_sql = if clauses.is_empty() { "1 = 1".to_owned() } else { clauses.join(" AND ") };
        let (prov, model) = (p.provider_id, p.model.clone());
        let rows = self.db.call(move |c| Ok(load(c, &where_sql, &args, limit)?)).await?;
        // The effective model is the latest call's; drop executions that only
        // matched through an earlier model.
        Ok(rows
            .into_iter()
            .map(|r| r.record)
            .filter(|r| prov.is_none_or(|p| r.provider_id == Some(p)))
            .filter(|r| model.as_deref().map(str::trim).is_none_or(|m| m.is_empty() || r.model.as_deref() == Some(m)))
            .collect())
    }

    /// Model benchmark table (most executions first).
    pub async fn model_benchmark_rows(&self, p: ModelBenchmarksParams) -> CostResult<Vec<ModelBenchmarkRow>> {
        let days = p.days.unwrap_or(DEFAULT_DAYS).clamp(1, MAX_DAYS);
        let role = p.role.clone().map(|r| r.trim().to_owned()).filter(|r| !r.is_empty());
        let project = p.project_id;
        let rows = self
            .db
            .call(move |c| {
                let (w, a) = window_filter(project, role.as_deref(), days);
                let execs = load(c, &w, &a, MAX_AGGREGATE_ROWS)?;
                let usage = load_usage(c, project, role.as_deref(), days)?;
                Ok(aggregate_models(&execs, &usage, false))
            })
            .await?;
        Ok(rows.into_iter().map(|(_, r)| r).collect())
    }

    /// Routing statistics: per model overall and per model + role.
    pub async fn model_routing_stats(&self, days: u32) -> CostResult<Vec<ModelRoutingStats>> {
        Ok(self
            .db
            .call(move |c| {
                let (w, a) = window_filter(None, None, days);
                let execs = load(c, &w, &a, MAX_AGGREGATE_ROWS)?;
                let usage = load_usage(c, None, None, days)?;
                let mut out: Vec<ModelRoutingStats> = aggregate_models(&execs, &usage, false)
                    .into_iter()
                    .map(|(role, row)| ModelRoutingStats { role, row })
                    .collect();
                out.extend(
                    aggregate_models(&execs, &usage, true)
                        .into_iter()
                        .map(|(role, row)| ModelRoutingStats { role, row }),
                );
                Ok(out)
            })
            .await?)
    }

    /// Agent performance per template or instance (most executions first).
    pub async fn agent_performance_rows(&self, p: AgentPerformanceParams) -> CostResult<Vec<AgentPerformanceRow>> {
        let days = p.days.unwrap_or(DEFAULT_DAYS).clamp(1, MAX_DAYS);
        let group = p.group_by.unwrap_or_default();
        let project = p.project_id;
        let execs = self
            .db
            .call(move |c| {
                let (w, a) = window_filter(project, None, days);
                Ok(load(c, &w, &a, MAX_AGGREGATE_ROWS)?)
            })
            .await?;
        #[derive(Default)]
        struct Acc {
            name: String,
            role: String,
            template_id: Option<String>,
            agent_id: Option<AgentId>,
            executions: u64,
            completed: u64,
            failed: u64,
            cancelled: u64,
            running: u64,
            fp_ok: u64,
            fp_n: u64,
            reviewed: u64,
            rejected: u64,
            rejections: u64,
            bugs: u64,
            retries: u64,
            duration_sum: f64,
            duration_n: u64,
            cost: f64,
            tokens: u64,
            models: BTreeSet<String>,
        }
        let mut accs: BTreeMap<String, Acc> = BTreeMap::new();
        for e in &execs {
            let r = &e.record;
            let template_key = r.template_id.clone().unwrap_or_else(|| format!("name:{}", e.template_name));
            let key = match group {
                AgentGroupBy::Template => template_key.clone(),
                AgentGroupBy::Instance => r.agent_id.to_string(),
            };
            let a = accs.entry(key).or_default();
            if a.executions == 0 {
                a.name = match group {
                    AgentGroupBy::Template => e.template_name.clone(),
                    AgentGroupBy::Instance => r.agent_name.clone(),
                };
                a.role = r.role.clone();
                a.template_id = r.template_id.clone();
                a.agent_id = matches!(group, AgentGroupBy::Instance).then_some(r.agent_id);
            }
            a.executions += 1;
            match r.outcome {
                ExecutionOutcome::Succeeded => a.completed += 1,
                ExecutionOutcome::Failed => a.failed += 1,
                ExecutionOutcome::Cancelled => a.cancelled += 1,
                ExecutionOutcome::Running => a.running += 1,
            }
            if let Some(fp) = r.first_pass {
                a.fp_n += 1;
                a.fp_ok += u64::from(fp);
            }
            if e.reviews > 0 {
                a.reviewed += 1;
                a.rejected += u64::from(r.review_rejections > 0);
            }
            a.rejections += u64::from(r.review_rejections);
            a.bugs += u64::from(r.post_merge_bugs);
            a.retries += u64::from(r.retries);
            if let Some(d) = r.duration_ms {
                #[allow(clippy::cast_precision_loss)]
                {
                    a.duration_sum += d as f64;
                }
                a.duration_n += 1;
            }
            a.cost += r.cost_usd;
            a.tokens += r.input_tokens + r.output_tokens;
            if let Some(m) = &r.model {
                a.models.insert(m.clone());
            }
        }
        let mut out: Vec<AgentPerformanceRow> = accs
            .into_iter()
            .map(|(key, a)| AgentPerformanceRow {
                key,
                name: a.name,
                role: a.role,
                template_id: a.template_id,
                agent_id: a.agent_id,
                executions: a.executions,
                completed: a.completed,
                failed: a.failed,
                cancelled: a.cancelled,
                running: a.running,
                first_pass_success_rate: rate(a.fp_ok, a.fp_n),
                review_rejection_rate: rate(a.rejected, a.reviewed),
                review_rejections: a.rejections,
                post_merge_bugs: a.bugs,
                retries: a.retries,
                duration_avg_ms: avg(a.duration_sum, a.duration_n),
                cost_usd: a.cost,
                tokens: a.tokens,
                models: a.models.into_iter().collect(),
            })
            .collect();
        out.sort_by(|a, b| {
            b.executions.cmp(&a.executions).then_with(|| a.name.cmp(&b.name)).then_with(|| a.key.cmp(&b.key))
        });
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{engine, record};
    use orkestr_domain::{now, AgentState, AgentTemplateId, EventId, ModelRef, UsageSummary, WorktreePolicy};
    use orkestr_protocol::event::{
        AgentSpawnedPayload, AgentStartedPayload, AgentStoppedPayload, BuildFinishedPayload, ReviewFinishedPayload,
        TaskPayload, TaskStatusChangedPayload, TestRunFinishedPayload,
    };

    struct Seq(u64);

    impl Seq {
        fn ev(&mut self, kind: EventKind, task: Option<TaskId>, agent: Option<AgentId>) -> Event {
            self.0 += 1;
            Event {
                seq: self.0,
                id: EventId::new(),
                ts: now(),
                project_id: None,
                task_id: task,
                agent_id: agent,
                node_id: None,
                correlation_id: None,
                kind,
            }
        }
    }

    fn agent(
        project: ProjectId,
        task: TaskId,
        name: &str,
        role: AgentRole,
        template: AgentTemplateId,
        model: &ModelRef,
    ) -> AgentInstance {
        AgentInstance {
            id: AgentId::new(),
            template_id: Some(template),
            project_id: project,
            task_id: Some(task),
            team_id: None,
            parent_agent_id: None,
            name: name.into(),
            role,
            state: AgentState::Idle,
            model: Some(model.clone()),
            worktree: None,
            usage: UsageSummary::default(),
            turns_used: 0,
            last_error: None,
            created_at: now(),
            started_at: None,
            stopped_at: None,
            last_activity_at: None,
            template_version: None,
            worktree_policy: WorktreePolicy::None,
            max_turns: 10,
            budget: None,
            execution_id: None,
            execution_style: None,
            cli_session: None,
            activity: None,
        }
    }

    fn status(seq: &mut Seq, task: TaskId, from: TaskStatus, to: TaskStatus) -> Event {
        seq.ev(
            EventKind::TaskStatusChanged(TaskStatusChangedPayload { task_id: task, from, to, reason: None }),
            Some(task),
            None,
        )
    }

    #[test]
    fn labels_and_bug_links() {
        assert_eq!(template_label("Backend Agent #12"), "Backend Agent");
        assert_eq!(template_label("Release #notes"), "Release #notes");
        assert_eq!(template_label("Solo"), "Solo");
        assert_eq!(role_label(&AgentRole::UiUx), "UI/UX");
        assert_eq!(role_label(&AgentRole::Backend), "Backend");
        let p = ProjectId::new();
        let origin = TaskId::new();
        let parent = TaskId::new();
        let mut t = Task::new(p, "Crash on save", now());
        assert!(!is_bug_task(&t));
        t.parent_id = Some(parent);
        assert!(linked_tasks(&t).is_empty(), "a plain subtask is not a bug link");
        t.tags = vec!["Bug".into(), format!("fixes:{origin}"), "fixes:not-an-id".into()];
        assert!(is_bug_task(&t));
        assert_eq!(linked_tasks(&t), vec![origin, parent]);
    }

    #[tokio::test]
    async fn collects_quality_signals_exactly_once() {
        let (eng, _bus) = engine().await;
        let project = ProjectId::new();
        let (pa, pb) = (ProviderId::new(), ProviderId::new());
        let fast = ModelRef { provider_id: pa, model: "fast-1".into() };
        let smart = ModelRef { provider_id: pb, model: "smart-2".into() };
        let backend_t = AgentTemplateId::new();
        let reviewer_t = AgentTemplateId::new();
        let (t1, t2, t3) = (TaskId::new(), TaskId::new(), TaskId::new());
        let mut seq = Seq(0);
        let mut evs = Vec::new();

        // t1: backend with `fast`, completed first pass, one build ok.
        let a1 = agent(project, t1, "Backend #1", AgentRole::Backend, backend_t, &fast);
        evs.push(seq.ev(EventKind::AgentSpawned(AgentSpawnedPayload { agent: a1.clone() }), Some(t1), Some(a1.id)));
        evs.push(seq.ev(
            EventKind::AgentStarted(AgentStartedPayload { agent_id: a1.id, task_id: Some(t1) }),
            Some(t1),
            Some(a1.id),
        ));
        evs.push(seq.ev(
            EventKind::BuildSucceeded(BuildFinishedPayload {
                build_id: "b".into(),
                exit_code: Some(0),
                duration_ms: 5,
                summary: String::new(),
            }),
            Some(t1),
            Some(a1.id),
        ));
        evs.push(seq.ev(
            EventKind::AgentStopped(AgentStoppedPayload {
                agent_id: a1.id,
                reason: StopReason::Completed,
                detail: None,
            }),
            None,
            Some(a1.id),
        ));
        evs.push(status(&mut seq, t1, TaskStatus::Running, TaskStatus::Review));
        let r1 = agent(project, t1, "Reviewer #1", AgentRole::Reviewer, reviewer_t, &smart);
        evs.push(seq.ev(EventKind::AgentSpawned(AgentSpawnedPayload { agent: r1.clone() }), Some(t1), Some(r1.id)));
        evs.push(seq.ev(
            EventKind::ReviewFinished(ReviewFinishedPayload {
                reviewer_agent_id: Some(r1.id),
                verdict: ReviewState::Approved,
                findings: 1,
                summary: String::new(),
            }),
            Some(t1),
            Some(r1.id),
        ));
        evs.push(status(&mut seq, t1, TaskStatus::Review, TaskStatus::Completed));

        // t2: backend with `smart`, changes requested, failing tests, fix loop, then completed.
        let a2 = agent(project, t2, "Backend #2", AgentRole::Backend, backend_t, &smart);
        evs.push(seq.ev(EventKind::AgentSpawned(AgentSpawnedPayload { agent: a2.clone() }), Some(t2), Some(a2.id)));
        evs.push(seq.ev(
            EventKind::BuildFailed(BuildFinishedPayload {
                build_id: "b".into(),
                exit_code: Some(1),
                duration_ms: 5,
                summary: String::new(),
            }),
            Some(t2),
            None,
        ));
        evs.push(seq.ev(
            EventKind::TestRunFinished(TestRunFinishedPayload {
                run_id: "r".into(),
                passed: 3,
                failed: 2,
                skipped: 0,
                duration_ms: 9,
                summary: String::new(),
            }),
            Some(t2),
            Some(a2.id),
        ));
        evs.push(status(&mut seq, t2, TaskStatus::Running, TaskStatus::Review));
        evs.push(seq.ev(
            EventKind::ReviewFinished(ReviewFinishedPayload {
                reviewer_agent_id: None,
                verdict: ReviewState::ChangesRequested,
                findings: 3,
                summary: String::new(),
            }),
            Some(t2),
            None,
        ));
        evs.push(status(&mut seq, t2, TaskStatus::Review, TaskStatus::Running));
        evs.push(seq.ev(
            EventKind::AgentStopped(AgentStoppedPayload {
                agent_id: a2.id,
                reason: StopReason::Completed,
                detail: None,
            }),
            None,
            Some(a2.id),
        ));
        evs.push(status(&mut seq, t2, TaskStatus::Running, TaskStatus::Completed));

        // t3: backend with `fast` fails, retried.
        let a3 = agent(project, t3, "Backend #3", AgentRole::Backend, backend_t, &fast);
        evs.push(seq.ev(EventKind::AgentSpawned(AgentSpawnedPayload { agent: a3.clone() }), Some(t3), Some(a3.id)));
        evs.push(seq.ev(
            EventKind::AgentStopped(AgentStoppedPayload { agent_id: a3.id, reason: StopReason::Error, detail: None }),
            None,
            Some(a3.id),
        ));
        evs.push(status(&mut seq, t3, TaskStatus::Running, TaskStatus::Failed));
        evs.push(status(&mut seq, t3, TaskStatus::Failed, TaskStatus::Ready));

        // A bug filed against t1 after it completed.
        let mut bug = Task::new(project, "Regression in t1", now());
        bug.tags = vec![format!("bug:{t1}")];
        evs.push(seq.ev(EventKind::TaskCreated(TaskPayload { task: bug }), None, None));

        let report = eng.apply_benchmark_events(evs.clone()).await.unwrap();
        assert_eq!(report.cursor, seq.0);
        assert!(report.finished >= 3);
        // Replaying the same events changes nothing (cursor).
        let again = eng.apply_benchmark_events(evs).await.unwrap();
        assert_eq!(again.applied, 0);
        assert_eq!(eng.benchmark_cursor().await.unwrap(), seq.0);
        // A late stop notice (e.g. `agents.stop` on a finished agent) does not
        // rewrite a finished execution.
        let late = seq.ev(
            EventKind::AgentStopped(AgentStoppedPayload {
                agent_id: a1.id,
                reason: StopReason::UserRequested,
                detail: None,
            }),
            None,
            Some(a1.id),
        );
        assert_eq!(eng.apply_benchmark_events(vec![late]).await.unwrap().changed, 0);

        // Usage joined by agent: a1 switched to `smart` for its last call.
        let ts = Utc::now().to_rfc3339();
        for (a, m, cost, ok) in
            [(&a1, &fast, 0.1, true), (&a1, &smart, 0.2, true), (&a2, &smart, 0.5, false), (&a3, &fast, 0.05, true)]
        {
            let mut r = record(m.provider_id, &m.model, cost, &ts);
            r.agent_id = Some(a.id);
            r.project_id = Some(project);
            r.success = ok;
            eng.ingest(&r).await.unwrap();
        }
        // A call without an execution (e.g. the Manager).
        eng.ingest(&record(pa, "fast-1", 0.01, &ts)).await.unwrap();

        let recs = eng.benchmark_records(BenchmarkRecordsParams::default()).await.unwrap();
        assert_eq!(recs.len(), 4);
        let rec = |id: AgentId| recs.iter().find(|r| r.agent_id == id).unwrap().clone();
        let r_a1 = rec(a1.id);
        assert_eq!(r_a1.first_pass, Some(true));
        assert_eq!(r_a1.builds_ok, 1);
        assert_eq!(r_a1.defects_found, 1, "review findings count as defects");
        assert_eq!(r_a1.post_merge_bugs, 1);
        assert_eq!(r_a1.model.as_deref(), Some("smart-2"), "latest call decides the model");
        assert_eq!(r_a1.calls, 2);
        assert_eq!(r_a1.outcome, ExecutionOutcome::Succeeded);
        assert_eq!(r_a1.task_status.as_deref(), Some("Completed"));
        let r_a2 = rec(a2.id);
        assert_eq!(r_a2.first_pass, Some(false));
        assert_eq!((r_a2.builds_failed, r_a2.tests_failed, r_a2.review_rejections, r_a2.fix_loops), (1, 2, 1, 1));
        assert_eq!(r_a2.defects_found, 5);
        assert_eq!(r_a2.failed_calls, 1);
        let r_a3 = rec(a3.id);
        assert_eq!((r_a3.outcome, r_a3.first_pass, r_a3.retries), (ExecutionOutcome::Failed, Some(false), 1));
        let r_r1 = rec(r1.id);
        assert_eq!(r_r1.outcome, ExecutionOutcome::Running);
        assert_eq!(r_r1.first_pass, Some(true), "reviewer shares the task outcome");

        // Filters.
        let by_model = eng
            .benchmark_records(BenchmarkRecordsParams {
                model: Some("fast-1".into()),
                ..BenchmarkRecordsParams::default()
            })
            .await
            .unwrap();
        assert_eq!(by_model.iter().map(|r| r.agent_id).collect::<Vec<_>>(), vec![a3.id]);
        let by_task = eng
            .benchmark_records(BenchmarkRecordsParams { task_id: Some(t1), ..BenchmarkRecordsParams::default() })
            .await
            .unwrap();
        assert_eq!(by_task.len(), 2);

        // Model table.
        let models = eng.model_benchmark_rows(ModelBenchmarksParams::default()).await.unwrap();
        let smart_row = models.iter().find(|r| r.model == "smart-2").unwrap();
        assert_eq!(smart_row.executions, 3, "a1 (last call), a2, reviewer (spawn model)");
        assert_eq!(smart_row.calls, 2);
        assert_eq!(smart_row.call_success_rate, Some(0.5));
        assert_eq!(smart_row.first_pass_success_rate, Some(2.0 / 3.0));
        assert_eq!(smart_row.build_success_rate, Some(0.5));
        let fast_row = models.iter().find(|r| r.model == "fast-1").unwrap();
        assert_eq!(fast_row.executions, 1);
        assert_eq!(fast_row.calls, 3, "includes the call without an execution");
        assert_eq!(fast_row.first_pass_success_rate, Some(0.0));
        assert_eq!(fast_row.retries_avg, Some(1.0));
        let backend_only = eng
            .model_benchmark_rows(ModelBenchmarksParams {
                role: Some("Backend".into()),
                ..ModelBenchmarksParams::default()
            })
            .await
            .unwrap();
        assert_eq!(backend_only.iter().find(|r| r.model == "smart-2").unwrap().executions, 2);
        assert_eq!(backend_only.iter().find(|r| r.model == "fast-1").unwrap().calls, 2);

        // Routing stats include per-role rows.
        let stats = eng.model_routing_stats(30).await.unwrap();
        assert!(stats.iter().any(|s| s.role.as_deref() == Some("Reviewer") && s.row.model == "smart-2"));
        assert!(stats.iter().any(|s| s.role.is_none() && s.row.model == "fast-1"));

        // Agent performance.
        let per_template = eng.agent_performance_rows(AgentPerformanceParams::default()).await.unwrap();
        let backend = per_template.iter().find(|r| r.template_id == Some(backend_t.to_string())).unwrap();
        assert_eq!((backend.executions, backend.completed, backend.failed), (3, 2, 1));
        assert_eq!(backend.name, "Backend");
        assert_eq!(backend.first_pass_success_rate, Some(1.0 / 3.0));
        assert_eq!(backend.review_rejection_rate, Some(0.5));
        assert_eq!((backend.post_merge_bugs, backend.retries), (1, 1));
        assert!((backend.cost_usd - 0.85).abs() < 1e-9);
        assert_eq!(backend.models, vec!["fast-1".to_owned(), "smart-2".to_owned()]);
        let per_instance = eng
            .agent_performance_rows(AgentPerformanceParams {
                group_by: Some(AgentGroupBy::Instance),
                ..AgentPerformanceParams::default()
            })
            .await
            .unwrap();
        assert_eq!(per_instance.len(), 4);
        assert!(per_instance.iter().all(|r| r.agent_id.is_some()));
        let other_project = eng
            .agent_performance_rows(AgentPerformanceParams {
                project_id: Some(ProjectId::new()),
                ..AgentPerformanceParams::default()
            })
            .await
            .unwrap();
        assert!(other_project.is_empty());
    }
}
