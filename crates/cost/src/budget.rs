//! Budgets (§22 Smart Budgeting): storage, evaluation, scope-chain checks and
//! threshold alerts.
//!
//! **Scope chain.** A call is described by a [`BudgetContext`] (project,
//! task, provider, agent). Every enabled budget whose scope matches applies:
//! Global always; Project/Task/Provider/Agent when the id matches. `PerTask`
//! budgets apply only when the context has a task.
//!
//! **Unrestricted.** An applicable *unrestricted* budget exempts the call from
//! the limits of every **broader** scope ([`scope_rank`]: Global < Project <
//! Provider < Agent < Task). E.g. "Ollama unrestricted" (provider) ignores the
//! project's $100/month, while a task's own $5 limit still applies.
//!
//! **Thresholds.** After each ingested record, applicable limited budgets that
//! reach 50 %, their `warnAt` fraction or 100 % fire one `BudgetThreshold`
//! event — at most once per budget, period and level (`budget_alerts`).

use chrono::Utc;
use orkestr_domain::{now, AgentId, BudgetPeriod, ProjectId, ProviderId, TaskId};
use orkestr_protocol::event::{BudgetThresholdPayload, EventKind};
use orkestr_protocol::methods::cost::{
    BudgetCheckEntry, BudgetLevel, BudgetOutcome, BudgetRule, BudgetScope, BudgetStatus, CheckBudgetResult,
    SetBudgetParams,
};
use orkestr_protocol::NewEvent;
use orkestr_storage::rusqlite::{self, params, params_from_iter, Connection, OptionalExtension};
use orkestr_storage::StorageError;

use crate::{micros_to_usd, CostEngine, CostError, CostResult};

/// Default warning fraction.
pub const DEFAULT_WARN_AT: f64 = 0.8;
/// Fixed alert levels in addition to `warnAt`.
pub const ALERT_LEVELS: &[f64] = &[0.5, 1.0];
/// Max budget limit (USD).
pub const MAX_LIMIT_USD: f64 = 1_000_000_000.0;

/// What a call is attributed to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BudgetContext {
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Provider.
    pub provider_id: Option<ProviderId>,
    /// Agent.
    pub agent_id: Option<AgentId>,
}

/// Breadth rank of a scope (lower = broader).
#[must_use]
pub fn scope_rank(scope: &BudgetScope) -> u8 {
    match scope {
        BudgetScope::Global => 0,
        BudgetScope::Project { .. } => 1,
        BudgetScope::Provider { .. } => 2,
        BudgetScope::Agent { .. } => 3,
        BudgetScope::Task { .. } => 4,
    }
}

/// Evaluates one limited budget: `(outcome, projected fraction)`.
///
/// Exceeded when the projected spend goes over the limit or the limit is
/// already used up; Warn at/above `warn_at`; else Allow.
#[must_use]
pub fn evaluate_limit(spent: f64, estimate: f64, limit: f64, warn_at: f64) -> (BudgetOutcome, f64) {
    let projected = spent + estimate.max(0.0);
    if limit <= 0.0 {
        let outcome = if projected > 0.0 { BudgetOutcome::Exceeded } else { BudgetOutcome::Allow };
        return (outcome, if projected > 0.0 { f64::INFINITY } else { 0.0 });
    }
    let fraction = projected / limit;
    let eps = 1e-9;
    let outcome = if projected > limit + eps || (spent >= limit - eps && estimate > 0.0) {
        BudgetOutcome::Exceeded
    } else if fraction + eps >= warn_at {
        BudgetOutcome::Warn
    } else {
        BudgetOutcome::Allow
    };
    (outcome, fraction)
}

/// For each rule, whether a narrower applicable unrestricted rule exempts it.
#[must_use]
pub fn exempt_flags(rules: &[&BudgetRule]) -> Vec<bool> {
    let narrowest_unrestricted = rules.iter().filter(|r| r.unrestricted).map(|r| scope_rank(&r.scope)).max();
    rules.iter().map(|r| !r.unrestricted && narrowest_unrestricted.is_some_and(|u| scope_rank(&r.scope) < u)).collect()
}

fn worst(a: BudgetOutcome, b: BudgetOutcome) -> BudgetOutcome {
    use BudgetOutcome::{Allow, Exceeded, Warn};
    match (a, b) {
        (Exceeded, _) | (_, Exceeded) => Exceeded,
        (Warn, _) | (_, Warn) => Warn,
        _ => Allow,
    }
}

fn scope_cols(scope: &BudgetScope) -> (&'static str, Option<String>) {
    match scope {
        BudgetScope::Global => ("Global", None),
        BudgetScope::Project { project_id } => ("Project", Some(project_id.to_string())),
        BudgetScope::Task { task_id } => ("Task", Some(task_id.to_string())),
        BudgetScope::Provider { provider_id } => ("Provider", Some(provider_id.to_string())),
        BudgetScope::Agent { agent_id } => ("Agent", Some(agent_id.to_string())),
    }
}

fn period_name(p: BudgetPeriod) -> &'static str {
    match p {
        BudgetPeriod::PerTask => "PerTask",
        BudgetPeriod::Daily => "Daily",
        BudgetPeriod::Monthly => "Monthly",
        BudgetPeriod::Total => "Total",
    }
}

/// Human-readable label used in `BudgetThreshold.scope`, e.g. `project:<id>/Monthly`.
#[must_use]
pub fn scope_label(rule: &BudgetRule) -> String {
    let (kind, id) = scope_cols(&rule.scope);
    let base = match id {
        Some(id) => format!("{}:{id}", kind.to_lowercase()),
        None => kind.to_lowercase(),
    };
    format!("{base}/{}", period_name(rule.period))
}

/// Whether a rule applies to a context.
#[must_use]
pub fn applies(rule: &BudgetRule, ctx: &BudgetContext) -> bool {
    let scope_ok = match rule.scope {
        BudgetScope::Global => true,
        BudgetScope::Project { project_id } => ctx.project_id == Some(project_id),
        BudgetScope::Task { task_id } => ctx.task_id == Some(task_id),
        BudgetScope::Provider { provider_id } => ctx.provider_id == Some(provider_id),
        BudgetScope::Agent { agent_id } => ctx.agent_id == Some(agent_id),
    };
    scope_ok && (rule.period != BudgetPeriod::PerTask || ctx.task_id.is_some())
}

/// Spend of `rule` in its current period, as `(micros, period key)`.
/// `PerTask` without a task in `ctx` reports the most expensive task.
pub(crate) fn spent_micros(
    c: &Connection,
    rule: &BudgetRule,
    ctx: &BudgetContext,
    day: &str,
    month: &str,
) -> rusqlite::Result<(i64, String)> {
    let mut clauses: Vec<&str> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    match rule.scope {
        BudgetScope::Global => {}
        BudgetScope::Project { project_id } => {
            clauses.push("project_id = ?");
            args.push(project_id.to_string());
        }
        BudgetScope::Task { task_id } => {
            clauses.push("task_id = ?");
            args.push(task_id.to_string());
        }
        BudgetScope::Provider { provider_id } => {
            clauses.push("provider_id = ?");
            args.push(provider_id.to_string());
        }
        BudgetScope::Agent { agent_id } => {
            clauses.push("agent_id = ?");
            args.push(agent_id.to_string());
        }
    }
    let key = match rule.period {
        BudgetPeriod::Daily => {
            clauses.push("day = ?");
            args.push(day.to_owned());
            day.to_owned()
        }
        BudgetPeriod::Monthly => {
            clauses.push("month = ?");
            args.push(month.to_owned());
            month.to_owned()
        }
        BudgetPeriod::Total => "total".to_owned(),
        BudgetPeriod::PerTask => match ctx.task_id {
            Some(t) => {
                clauses.push("task_id = ?");
                args.push(t.to_string());
                format!("task:{t}")
            }
            None => {
                clauses.push("task_id IS NOT NULL");
                let where_sql = clauses.join(" AND ");
                let sql = format!(
                    "SELECT COALESCE(MAX(s), 0) FROM (SELECT SUM(cost_micros) AS s FROM usage_records
                     WHERE {where_sql} GROUP BY task_id)"
                );
                let v: i64 = c.query_row(&sql, params_from_iter(args.iter()), |r| r.get(0))?;
                return Ok((v, "per-task-max".to_owned()));
            }
        },
    };
    let where_sql = if clauses.is_empty() { "1 = 1".to_owned() } else { clauses.join(" AND ") };
    let sql = format!("SELECT COALESCE(SUM(cost_micros), 0) FROM usage_records WHERE {where_sql}");
    let v: i64 = c.query_row(&sql, params_from_iter(args.iter()), |r| r.get(0))?;
    Ok((v, key))
}

pub(crate) fn load_rules(c: &Connection) -> Result<Vec<BudgetRule>, StorageError> {
    let mut stmt = c.prepare("SELECT doc FROM budgets ORDER BY created_at, id")?;
    let docs = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    docs.iter().map(|d| Ok(serde_json::from_str(d)?)).collect()
}

fn status_of(rule: BudgetRule, spent_micros: i64, period_key: String) -> BudgetStatus {
    let spent = micros_to_usd(spent_micros);
    let (fraction, level) = if !rule.enabled {
        (rule.limit_usd.filter(|l| *l > 0.0).map(|l| spent / l), BudgetLevel::Disabled)
    } else if rule.unrestricted {
        (None, BudgetLevel::Unrestricted)
    } else {
        let limit = rule.limit_usd.unwrap_or(0.0);
        let (outcome, fraction) = evaluate_limit(spent, 0.0, limit, rule.warn_at);
        let exhausted = limit <= 0.0 || spent >= limit - 1e-9;
        let level = match outcome {
            _ if exhausted && limit > 0.0 => BudgetLevel::Exceeded,
            BudgetOutcome::Exceeded => BudgetLevel::Exceeded,
            BudgetOutcome::Warn => BudgetLevel::Warn,
            BudgetOutcome::Allow => BudgetLevel::Ok,
        };
        (fraction.is_finite().then_some(fraction), level)
    };
    BudgetStatus { budget: rule, spent_usd: spent, fraction, level, period_key }
}

fn validate(p: &SetBudgetParams) -> CostResult<(Option<f64>, bool, f64, Option<String>)> {
    let unrestricted = p.unrestricted.unwrap_or(false);
    let limit = if unrestricted {
        None
    } else {
        let l = p.limit_usd.ok_or_else(|| CostError::Invalid("limitUsd is required unless unrestricted".into()))?;
        if !l.is_finite() || l < 0.0 || l > MAX_LIMIT_USD {
            return Err(CostError::Invalid("limitUsd must be a finite amount between 0 and 1e9".into()));
        }
        Some(l)
    };
    let warn_at = p.warn_at.unwrap_or(DEFAULT_WARN_AT);
    if !(warn_at.is_finite() && warn_at > 0.0 && warn_at < 1.0) {
        return Err(CostError::Invalid("warnAt must be between 0 and 1 (exclusive)".into()));
    }
    let name = p.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned);
    if name.as_ref().is_some_and(|n| n.chars().count() > 120) {
        return Err(CostError::Invalid("name must be at most 120 characters".into()));
    }
    Ok((limit, unrestricted, warn_at, name))
}

impl CostEngine {
    fn period_keys(&self) -> (String, String) {
        let t = Utc::now();
        (self.calendar.day_key(&t), self.calendar.month_key(&t))
    }

    /// Lists budgets with their current spend. With a project, other projects'
    /// project budgets are left out.
    pub async fn list_budgets(&self, project: Option<ProjectId>) -> CostResult<Vec<BudgetStatus>> {
        let (day, month) = self.period_keys();
        let out = self
            .db
            .call(move |c| {
                let mut out = Vec::new();
                for rule in load_rules(c)? {
                    if let (Some(p), BudgetScope::Project { project_id }) = (project, rule.scope) {
                        if p != project_id {
                            continue;
                        }
                    }
                    let (spent, key) = spent_micros(c, &rule, &BudgetContext::default(), &day, &month)?;
                    out.push(status_of(rule, spent, key));
                }
                Ok(out)
            })
            .await?;
        Ok(out)
    }

    /// Current status of one budget.
    pub async fn budget_status(&self, id: &str) -> CostResult<BudgetStatus> {
        let (day, month) = self.period_keys();
        let key = id.to_owned();
        let found = self
            .db
            .call(move |c| {
                let doc: Option<String> =
                    c.query_row("SELECT doc FROM budgets WHERE id = ?1", [&key], |r| r.get(0)).optional()?;
                let Some(doc) = doc else { return Ok(None) };
                let rule: BudgetRule = serde_json::from_str(&doc)?;
                let (spent, pk) = spent_micros(c, &rule, &BudgetContext::default(), &day, &month)?;
                Ok(Some(status_of(rule, spent, pk)))
            })
            .await?;
        found.ok_or_else(|| CostError::NotFound(format!("budget {id}")))
    }

    /// Creates or replaces a budget. Without `id`, an existing budget with the
    /// same scope and period is replaced. Fired alerts of the budget are reset.
    pub async fn set_budget(&self, p: SetBudgetParams) -> CostResult<BudgetStatus> {
        let (limit, unrestricted, warn_at, name) = validate(&p)?;
        let (scope_kind, scope_id) = scope_cols(&p.scope);
        let period = period_name(p.period);
        let requested_id = p.id.clone();
        let scope = p.scope;
        let budget_period = p.period;
        let enabled = p.enabled.unwrap_or(true);
        let id = self
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                let existing: Option<(String, String)> = match &requested_id {
                    Some(id) => {
                        let row = tx
                            .query_row("SELECT id, doc FROM budgets WHERE id = ?1", [id], |r| {
                                Ok((r.get(0)?, r.get(1)?))
                            })
                            .optional()?;
                        if row.is_none() {
                            return Err(StorageError::NotFound(format!("budget {id}")));
                        }
                        row
                    }
                    None => tx
                        .query_row(
                            "SELECT id, doc FROM budgets WHERE scope = ?1 AND IFNULL(scope_id, '') = IFNULL(?2, '')
                             AND period = ?3",
                            params![scope_kind, scope_id, period],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()?,
                };
                let t = now();
                let rule = match existing {
                    Some((id, doc)) => {
                        let old: BudgetRule = serde_json::from_str(&doc)?;
                        BudgetRule {
                            id,
                            name,
                            scope,
                            period: budget_period,
                            limit_usd: limit,
                            unrestricted,
                            warn_at,
                            enabled,
                            created_at: old.created_at,
                            updated_at: t,
                        }
                    }
                    None => BudgetRule {
                        id: uuid::Uuid::now_v7().to_string(),
                        name,
                        scope,
                        period: budget_period,
                        limit_usd: limit,
                        unrestricted,
                        warn_at,
                        enabled,
                        created_at: t,
                        updated_at: t,
                    },
                };
                let doc = serde_json::to_string(&rule)?;
                let res = tx.execute(
                    "INSERT INTO budgets (id, scope, scope_id, period, created_at, updated_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(id) DO UPDATE SET scope = excluded.scope, scope_id = excluded.scope_id,
                        period = excluded.period, updated_at = excluded.updated_at, doc = excluded.doc",
                    params![
                        rule.id,
                        scope_kind,
                        scope_id,
                        period,
                        rule.created_at.to_rfc3339(),
                        rule.updated_at.to_rfc3339(),
                        doc
                    ],
                );
                match res {
                    Ok(_) => {}
                    Err(e) => {
                        let err = StorageError::from(e);
                        if err.is_unique_violation() {
                            return Err(StorageError::Conflict(
                                "another budget already exists for this scope and period".into(),
                            ));
                        }
                        return Err(err);
                    }
                }
                tx.execute("DELETE FROM budget_alerts WHERE budget_id = ?1", [&rule.id])?;
                tx.commit()?;
                Ok(rule.id)
            })
            .await
            .map_err(|e| match e {
                StorageError::NotFound(m) => CostError::NotFound(m),
                StorageError::Conflict(m) => CostError::Invalid(m),
                other => CostError::Storage(other),
            })?;
        tracing::info!(budget_id = %id, "budget saved");
        self.budget_status(&id).await
    }

    /// Deletes a budget.
    pub async fn delete_budget(&self, id: &str) -> CostResult<()> {
        let key = id.to_owned();
        let n = self.db.call(move |c| Ok(c.execute("DELETE FROM budgets WHERE id = ?1", [key])?)).await?;
        if n == 0 {
            return Err(CostError::NotFound(format!("budget {id}")));
        }
        Ok(())
    }

    /// Evaluates every applicable budget for a call with `estimate_usd`.
    pub async fn check(&self, ctx: BudgetContext, estimate_usd: f64) -> CostResult<CheckBudgetResult> {
        if !estimate_usd.is_finite() || estimate_usd < 0.0 {
            return Err(CostError::Invalid("estimatedCostUsd must be a non-negative amount".into()));
        }
        let (day, month) = self.period_keys();
        let entries = self
            .db
            .call(move |c| {
                let rules: Vec<BudgetRule> =
                    load_rules(c)?.into_iter().filter(|r| r.enabled && applies(r, &ctx)).collect();
                let refs: Vec<&BudgetRule> = rules.iter().collect();
                let exempt = exempt_flags(&refs);
                let mut entries = Vec::with_capacity(rules.len());
                for (rule, exempt) in rules.iter().zip(exempt) {
                    let (spent, _) = spent_micros(c, rule, &ctx, &day, &month)?;
                    let spent = micros_to_usd(spent);
                    let projected = spent + estimate_usd;
                    let (outcome, fraction) = match (rule.unrestricted, rule.limit_usd) {
                        (false, Some(limit)) => {
                            let (o, f) = evaluate_limit(spent, estimate_usd, limit, rule.warn_at);
                            (o, f.is_finite().then_some(f))
                        }
                        _ => (BudgetOutcome::Allow, None),
                    };
                    entries.push(BudgetCheckEntry {
                        budget_id: rule.id.clone(),
                        scope: rule.scope,
                        period: rule.period,
                        spent_usd: spent,
                        projected_usd: projected,
                        limit_usd: if rule.unrestricted { None } else { rule.limit_usd },
                        fraction,
                        outcome,
                        exempt,
                    });
                }
                Ok(entries)
            })
            .await?;
        let mut outcome = BudgetOutcome::Allow;
        let mut threshold: Option<f64> = None;
        for e in entries.iter().filter(|e| !e.exempt && e.limit_usd.is_some()) {
            outcome = worst(outcome, e.outcome);
            if e.outcome == BudgetOutcome::Exceeded && e.fraction.is_none() {
                threshold = Some(f64::MAX);
            }
            if let Some(f) = e.fraction {
                threshold = Some(threshold.map_or(f, |t: f64| t.max(f)));
            }
        }
        Ok(CheckBudgetResult { outcome, threshold, entries })
    }

    /// Fires `BudgetThreshold` events for budgets crossing 50 % / warnAt / 100 %
    /// after usage in `ctx`. Returns the published payloads.
    pub async fn fire_thresholds(&self, ctx: BudgetContext) -> CostResult<Vec<BudgetThresholdPayload>> {
        let (day, month) = self.period_keys();
        let fired_at = Utc::now().to_rfc3339();
        let fired: Vec<(BudgetRule, BudgetThresholdPayload)> = self
            .db
            .call(move |c| {
                let mut out = Vec::new();
                for rule in load_rules(c)? {
                    let Some(limit) = rule.limit_usd.filter(|l| *l > 0.0 && !rule.unrestricted) else { continue };
                    if !rule.enabled || !applies(&rule, &ctx) {
                        continue;
                    }
                    let (spent, key) = spent_micros(c, &rule, &ctx, &day, &month)?;
                    let spent = micros_to_usd(spent);
                    let fraction = spent / limit;
                    let mut levels: Vec<f64> = ALERT_LEVELS.to_vec();
                    levels.push(rule.warn_at);
                    levels.sort_by(f64::total_cmp);
                    levels.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
                    let mut newly = false;
                    for level in levels.iter().filter(|l| fraction + 1e-9 >= **l) {
                        // Level stored in basis points so 0.8 and 0.85 stay distinct.
                        #[allow(clippy::cast_possible_truncation)]
                        let bp = (level * 10_000.0).round() as i64;
                        let n = c.execute(
                            "INSERT OR IGNORE INTO budget_alerts (budget_id, period_key, level_pct, fired_at)
                             VALUES (?1, ?2, ?3, ?4)",
                            params![rule.id, key, bp, fired_at],
                        )?;
                        newly |= n > 0;
                    }
                    if newly {
                        let payload = BudgetThresholdPayload {
                            scope: scope_label(&rule),
                            spent_usd: spent,
                            limit_usd: limit,
                            fraction,
                        };
                        out.push((rule, payload));
                    }
                }
                Ok(out)
            })
            .await?;
        let mut payloads = Vec::with_capacity(fired.len());
        for (rule, payload) in fired {
            tracing::warn!(budget_id = %rule.id, fraction = payload.fraction, "budget threshold crossed");
            if let Some(bus) = &self.events {
                let mut ev = NewEvent::new(EventKind::BudgetThreshold(payload.clone()));
                match rule.scope {
                    BudgetScope::Project { project_id } => ev = ev.project(project_id),
                    BudgetScope::Task { task_id } => ev = ev.task(task_id),
                    BudgetScope::Agent { agent_id } => ev = ev.agent(agent_id),
                    BudgetScope::Global | BudgetScope::Provider { .. } => {
                        if let Some(p) = ctx.project_id {
                            ev = ev.project(p);
                        }
                    }
                }
                bus.publish(ev).await.map_err(|e| CostError::Events(e.to_string()))?;
            }
            payloads.push(payload);
        }
        Ok(payloads)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{engine, record};
    use orkestr_events::Received;

    fn rule(scope: BudgetScope, limit: Option<f64>, unrestricted: bool) -> BudgetRule {
        BudgetRule {
            id: "x".into(),
            name: None,
            scope,
            period: BudgetPeriod::Monthly,
            limit_usd: limit,
            unrestricted,
            warn_at: 0.8,
            enabled: true,
            created_at: now(),
            updated_at: now(),
        }
    }

    fn set(scope: BudgetScope, period: BudgetPeriod, limit: Option<f64>) -> SetBudgetParams {
        SetBudgetParams {
            id: None,
            name: None,
            scope,
            period,
            limit_usd: limit,
            unrestricted: limit.is_none().then_some(true),
            warn_at: None,
            enabled: None,
        }
    }

    #[test]
    fn limit_evaluation() {
        assert_eq!(evaluate_limit(10.0, 5.0, 100.0, 0.8).0, BudgetOutcome::Allow);
        let (o, f) = evaluate_limit(75.0, 5.0, 100.0, 0.8);
        assert_eq!(o, BudgetOutcome::Warn);
        assert!((f - 0.8).abs() < 1e-12);
        assert_eq!(evaluate_limit(95.0, 6.0, 100.0, 0.8).0, BudgetOutcome::Exceeded);
        assert_eq!(evaluate_limit(100.0, 0.0, 100.0, 0.8).0, BudgetOutcome::Warn, "exactly at limit, no new spend");
        assert_eq!(evaluate_limit(100.0, 0.01, 100.0, 0.8).0, BudgetOutcome::Exceeded);
        assert_eq!(evaluate_limit(0.0, 0.0, 0.0, 0.8).0, BudgetOutcome::Allow);
        assert_eq!(evaluate_limit(0.0, 0.1, 0.0, 0.8).0, BudgetOutcome::Exceeded);
    }

    #[test]
    fn unrestricted_exempts_broader_scopes_only() {
        let g = rule(BudgetScope::Global, Some(10.0), false);
        let p = rule(BudgetScope::Project { project_id: ProjectId::new() }, Some(100.0), false);
        let prov = rule(BudgetScope::Provider { provider_id: ProviderId::new() }, None, true);
        let t = rule(BudgetScope::Task { task_id: TaskId::new() }, Some(5.0), false);
        assert_eq!(exempt_flags(&[&g, &p, &prov, &t]), vec![true, true, false, false]);
        assert_eq!(exempt_flags(&[&g, &p, &t]), vec![false, false, false]);
        assert!(scope_rank(&BudgetScope::Global) < scope_rank(&BudgetScope::Task { task_id: TaskId::new() }));
    }

    #[tokio::test]
    async fn set_list_replace_delete() {
        let (eng, _bus) = engine().await;
        let pid = ProjectId::new();
        let other = ProjectId::new();
        let b = eng
            .set_budget(set(BudgetScope::Project { project_id: pid }, BudgetPeriod::Monthly, Some(100.0)))
            .await
            .unwrap();
        assert_eq!(b.level, BudgetLevel::Ok);
        assert_eq!(b.budget.warn_at, DEFAULT_WARN_AT);
        // Same scope + period without id replaces.
        let b2 = eng
            .set_budget(set(BudgetScope::Project { project_id: pid }, BudgetPeriod::Monthly, Some(50.0)))
            .await
            .unwrap();
        assert_eq!(b2.budget.id, b.budget.id);
        assert_eq!(b2.budget.limit_usd, Some(50.0));
        eng.set_budget(set(BudgetScope::Project { project_id: other }, BudgetPeriod::Daily, Some(1.0))).await.unwrap();
        eng.set_budget(set(BudgetScope::Global, BudgetPeriod::Monthly, None)).await.unwrap();
        assert_eq!(eng.list_budgets(None).await.unwrap().len(), 3);
        let mine = eng.list_budgets(Some(pid)).await.unwrap();
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().any(|s| s.level == BudgetLevel::Unrestricted));

        // Validation.
        assert!(eng.set_budget(set(BudgetScope::Global, BudgetPeriod::Daily, Some(-1.0))).await.is_err());
        let mut no_limit = set(BudgetScope::Global, BudgetPeriod::Daily, Some(1.0));
        no_limit.limit_usd = None;
        assert!(eng.set_budget(no_limit).await.is_err());
        let mut bad_warn = set(BudgetScope::Global, BudgetPeriod::Daily, Some(1.0));
        bad_warn.warn_at = Some(1.5);
        assert!(eng.set_budget(bad_warn).await.is_err());
        let mut unknown = set(BudgetScope::Global, BudgetPeriod::Daily, Some(1.0));
        unknown.id = Some("nope".into());
        assert!(matches!(eng.set_budget(unknown).await, Err(CostError::NotFound(_))));
        // Moving a budget onto another budget's scope+period conflicts.
        let mut clash = set(BudgetScope::Project { project_id: other }, BudgetPeriod::Daily, Some(2.0));
        clash.id = Some(b.budget.id.clone());
        assert!(matches!(eng.set_budget(clash).await, Err(CostError::Invalid(_))));

        eng.delete_budget(&b.budget.id).await.unwrap();
        assert!(matches!(eng.delete_budget(&b.budget.id).await, Err(CostError::NotFound(_))));
    }

    #[tokio::test]
    async fn check_scope_chain_and_thresholds() {
        let (eng, bus) = engine().await;
        let mut sub = bus.subscribe_all();
        let pid = ProjectId::new();
        let ollama = ProviderId::new();
        let openai = ProviderId::new();
        let task = TaskId::new();
        let now_s = Utc::now().to_rfc3339();
        eng.set_budget(set(BudgetScope::Project { project_id: pid }, BudgetPeriod::Monthly, Some(10.0))).await.unwrap();
        eng.set_budget(set(BudgetScope::Provider { provider_id: ollama }, BudgetPeriod::Monthly, None)).await.unwrap();
        eng.set_budget(set(BudgetScope::Global, BudgetPeriod::PerTask, Some(5.0))).await.unwrap();

        let ctx =
            BudgetContext { project_id: Some(pid), task_id: Some(task), provider_id: Some(openai), agent_id: None };
        let r = eng.check(ctx, 1.0).await.unwrap();
        assert_eq!(r.outcome, BudgetOutcome::Allow);
        assert_eq!(r.entries.len(), 2, "provider budget of ollama does not apply to openai");

        // Spend $4.5 on the task → project at 45 %, per-task at 90 % (warn).
        let mut rec = record(openai, "gpt", 4.5, &now_s);
        rec.project_id = Some(pid);
        rec.task_id = Some(task);
        let fired = eng.ingest(&rec).await.unwrap();
        assert_eq!(fired.len(), 1, "{fired:?}");
        assert!(fired[0].scope.starts_with("global/PerTask"));
        match sub.recv().await.unwrap() {
            Received::Event(e) => assert_eq!(e.kind_name(), "BudgetThreshold"),
            other => panic!("{other:?}"),
        }
        let r = eng.check(ctx, 0.4).await.unwrap();
        assert_eq!(r.outcome, BudgetOutcome::Warn);
        let r = eng.check(ctx, 1.0).await.unwrap();
        assert_eq!(r.outcome, BudgetOutcome::Exceeded, "per-task $5 would be exceeded");

        // Ingesting again at the same level does not re-fire.
        let mut tiny = record(openai, "gpt", 0.01, &now_s);
        tiny.project_id = Some(pid);
        tiny.task_id = Some(task);
        assert!(eng.ingest(&tiny).await.unwrap().is_empty());

        // Ollama is unrestricted: it is exempt from the project limit (broader scope),
        // but the global per-task limit is broader too → also exempt.
        let other_task = TaskId::new();
        let ollama_ctx = BudgetContext {
            project_id: Some(pid),
            task_id: Some(other_task),
            provider_id: Some(ollama),
            agent_id: None,
        };
        let r = eng.check(ollama_ctx, 1000.0).await.unwrap();
        assert_eq!(r.outcome, BudgetOutcome::Allow);
        assert!(r.entries.iter().filter(|e| e.limit_usd.is_some()).all(|e| e.exempt));

        // Push the project over 100 %: one event for the highest new level.
        let mut big = record(openai, "gpt", 6.0, &now_s);
        big.project_id = Some(pid);
        big.task_id = Some(TaskId::new());
        let fired = eng.ingest(&big).await.unwrap();
        assert!(fired.iter().any(|p| p.scope.starts_with("project:") && p.fraction > 1.0), "{fired:?}");
        let statuses = eng.list_budgets(Some(pid)).await.unwrap();
        let project_status = statuses.iter().find(|s| matches!(s.budget.scope, BudgetScope::Project { .. })).unwrap();
        assert_eq!(project_status.level, BudgetLevel::Exceeded);
        let per_task = statuses.iter().find(|s| s.budget.period == BudgetPeriod::PerTask).unwrap();
        assert!((per_task.spent_usd - 6.0).abs() < 1e-9, "per-task status reports the most expensive task");
        assert!(eng.check(ctx, -1.0).await.is_err());
    }
}
