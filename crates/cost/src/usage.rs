//! Usage ingest and aggregates.

use chrono::{SecondsFormat, Utc};
use orkestr_domain::{AgentId, ExecutionId, ProjectId, ProviderId, TaskId, Timestamp, UsageRecord, UsageRecordId};
use orkestr_protocol::event::BudgetThresholdPayload;
use orkestr_protocol::methods::cost::{
    BudgetLevel, BudgetScope, CostBreakdownRow, CostDimension, CostGranularity, CostPoint, CostRecordsParams,
    CostSummaryResult, CostWindow, UsageTotals,
};
use orkestr_storage::rusqlite::{self, params, params_from_iter, Row};
use orkestr_storage::StorageError;

use crate::budget::BudgetContext;
use crate::{micros_to_usd, usd_to_micros, CostEngine, CostError, CostResult};

const TOTALS_SQL: &str = "COALESCE(SUM(cost_micros), 0),
    COALESCE(SUM(CASE WHEN estimated THEN cost_micros ELSE 0 END), 0),
    COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(cached_input_tokens), 0),
    COUNT(*), COALESCE(SUM(CASE WHEN success THEN 0 ELSE 1 END), 0)";

fn ts_key(t: &Timestamp) -> String {
    t.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn u(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

fn i(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Reads [`TOTALS_SQL`] columns starting at `at`.
fn totals(r: &Row<'_>, at: usize) -> rusqlite::Result<UsageTotals> {
    Ok(UsageTotals {
        cost_usd: micros_to_usd(r.get(at)?),
        estimated_cost_usd: micros_to_usd(r.get(at + 1)?),
        input_tokens: u(r.get(at + 2)?),
        output_tokens: u(r.get(at + 3)?),
        cached_input_tokens: u(r.get(at + 4)?),
        requests: u(r.get(at + 5)?),
        failures: u(r.get(at + 6)?),
    })
}

fn parse_id<T: std::str::FromStr>(v: Option<String>) -> rusqlite::Result<Option<T>>
where
    T::Err: std::fmt::Display,
{
    v.map(|s| {
        s.parse::<T>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::<dyn std::error::Error + Send + Sync>::from(e.to_string()),
            )
        })
    })
    .transpose()
}

fn record_from_row(r: &Row<'_>) -> rusqlite::Result<UsageRecord> {
    let ts: String = r.get(1)?;
    let ts = chrono::DateTime::parse_from_rfc3339(&ts)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?
        .with_timezone(&Utc);
    Ok(UsageRecord {
        id: parse_id::<UsageRecordId>(r.get(0)?)?.unwrap_or_default(),
        ts,
        provider_id: parse_id::<ProviderId>(r.get(2)?)?.unwrap_or_default(),
        model: r.get(3)?,
        project_id: parse_id::<ProjectId>(r.get(4)?)?,
        task_id: parse_id::<TaskId>(r.get(5)?)?,
        agent_id: parse_id::<AgentId>(r.get(6)?)?,
        execution_id: parse_id::<ExecutionId>(r.get(7)?)?,
        input_tokens: u(r.get(8)?),
        output_tokens: u(r.get(9)?),
        cached_input_tokens: u(r.get(10)?),
        reasoning_tokens: u(r.get(11)?),
        cost_usd: micros_to_usd(r.get(12)?),
        estimated: r.get(13)?,
        latency_ms: r.get::<_, Option<i64>>(14)?.map(u),
        success: r.get(15)?,
    })
}

const RECORD_COLS: &str = "id, ts, provider_id, model, project_id, task_id, agent_id, execution_id, input_tokens,
    output_tokens, cached_input_tokens, reasoning_tokens, cost_micros, estimated, latency_ms, success";

impl CostEngine {
    /// Stores a usage record, then fires budget thresholds for its context.
    /// Returns the threshold payloads that were published.
    pub async fn ingest(&self, record: &UsageRecord) -> CostResult<Vec<BudgetThresholdPayload>> {
        if record.model.trim().is_empty() || record.model.len() > 200 {
            return Err(CostError::Invalid("model must be 1..=200 characters".into()));
        }
        if !record.cost_usd.is_finite() || record.cost_usd < 0.0 {
            return Err(CostError::Invalid("costUsd must be a non-negative amount".into()));
        }
        let r = record.clone();
        let day = self.calendar.day_key(&r.ts);
        let month = self.calendar.month_key(&r.ts);
        let inserted = self
            .db
            .call(move |c| {
                let res = c.execute(
                    "INSERT INTO usage_records (id, ts, day, month, provider_id, model, project_id, task_id, agent_id,
                        execution_id, input_tokens, output_tokens, cached_input_tokens, reasoning_tokens, cost_micros,
                        estimated, latency_ms, success)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                    params![
                        r.id.to_string(),
                        ts_key(&r.ts),
                        day,
                        month,
                        r.provider_id.to_string(),
                        r.model.trim(),
                        r.project_id.map(|v| v.to_string()),
                        r.task_id.map(|v| v.to_string()),
                        r.agent_id.map(|v| v.to_string()),
                        r.execution_id.map(|v| v.to_string()),
                        i(r.input_tokens),
                        i(r.output_tokens),
                        i(r.cached_input_tokens),
                        i(r.reasoning_tokens),
                        usd_to_micros(r.cost_usd),
                        r.estimated,
                        r.latency_ms.map(i),
                        r.success
                    ],
                );
                match res {
                    Ok(_) => Ok(true),
                    Err(e) => {
                        let e = StorageError::from(e);
                        if e.is_unique_violation() {
                            Ok(false)
                        } else {
                            Err(e)
                        }
                    }
                }
            })
            .await?;
        if !inserted {
            // Idempotent: the same record id is ingested once.
            tracing::debug!(usage_id = %record.id, "duplicate usage record ignored");
            return Ok(Vec::new());
        }
        let ctx = BudgetContext {
            project_id: record.project_id,
            task_id: record.task_id,
            provider_id: Some(record.provider_id),
            agent_id: record.agent_id,
        };
        self.fire_thresholds(ctx).await
    }

    async fn totals_where(&self, where_sql: String, args: Vec<String>) -> CostResult<UsageTotals> {
        Ok(self
            .db
            .call(move |c| {
                let sql = format!("SELECT {TOTALS_SQL} FROM usage_records WHERE {where_sql}");
                Ok(c.query_row(&sql, params_from_iter(args.iter()), |r| totals(r, 0))?)
            })
            .await?)
    }

    /// Today / month / total totals and the highest used budget fraction.
    pub async fn summary(&self, project: Option<ProjectId>) -> CostResult<CostSummaryResult> {
        let now = Utc::now();
        let day = self.calendar.day_key(&now);
        let month = self.calendar.month_key(&now);
        let (base, base_args) = match project {
            Some(p) => ("project_id = ?".to_owned(), vec![p.to_string()]),
            None => ("1 = 1".to_owned(), Vec::new()),
        };
        let with = |extra: &str, v: &str| {
            let mut a = base_args.clone();
            a.push(v.to_owned());
            (format!("{base} AND {extra}"), a)
        };
        let (w, a) = with("day = ?", &day);
        let today = self.totals_where(w, a).await?;
        let (w, a) = with("month = ?", &month);
        let month_totals = self.totals_where(w, a).await?;
        let total = self.totals_where(base.clone(), base_args.clone()).await?;

        let mut budget_used: Option<f64> = None;
        let mut budget_id = None;
        for s in self.list_budgets(project).await? {
            let relevant = matches!(s.budget.scope, BudgetScope::Global)
                || matches!((s.budget.scope, project), (BudgetScope::Project { project_id }, Some(p)) if project_id == p);
            let usable = s.budget.period != orkestr_domain::BudgetPeriod::PerTask
                && !matches!(s.level, BudgetLevel::Disabled | BudgetLevel::Unrestricted);
            if let (true, true, Some(f)) = (relevant, usable, s.fraction) {
                if budget_used.is_none_or(|b| f > b) {
                    budget_used = Some(f);
                    budget_id = Some(s.budget.id.clone());
                }
            }
        }
        Ok(CostSummaryResult { today, month: month_totals, total, budget_used, budget_id, day, month_key: month })
    }

    /// Breakdown by one dimension, most expensive first.
    pub async fn by_dimension(
        &self,
        dimension: CostDimension,
        window: CostWindow,
        project: Option<ProjectId>,
        limit: u32,
    ) -> CostResult<Vec<CostBreakdownRow>> {
        let now = Utc::now();
        let mut clauses: Vec<String> = Vec::new();
        let mut args: Vec<String> = Vec::new();
        if let Some(p) = project {
            clauses.push("project_id = ?".into());
            args.push(p.to_string());
        }
        match window {
            CostWindow::Today => {
                clauses.push("day = ?".into());
                args.push(self.calendar.day_key(&now));
            }
            CostWindow::Month => {
                clauses.push("month = ?".into());
                args.push(self.calendar.month_key(&now));
            }
            CostWindow::Last30Days => {
                clauses.push("day >= ?".into());
                args.push(self.calendar.last_days(&now, 30).into_iter().next().unwrap_or_default());
            }
            CostWindow::All => {}
        }
        let where_sql = if clauses.is_empty() { "1 = 1".to_owned() } else { clauses.join(" AND ") };
        let group = match dimension {
            CostDimension::Provider => "provider_id, NULL",
            CostDimension::Model => "provider_id, model",
            CostDimension::Agent => "agent_id, NULL",
            CostDimension::Project => "project_id, NULL",
            CostDimension::Task => "task_id, NULL",
        };
        let rows = self
            .db
            .call(move |c| {
                let sql = format!(
                    "SELECT {group}, {TOTALS_SQL}, AVG(latency_ms) FROM usage_records WHERE {where_sql}
                     GROUP BY 1, 2 ORDER BY 3 DESC, 1 ASC, 2 ASC LIMIT {limit}"
                );
                let mut stmt = c.prepare(&sql)?;
                let rows = stmt
                    .query_map(params_from_iter(args.iter()), |r| {
                        let key: Option<String> = r.get(0)?;
                        let second: Option<String> = r.get(1)?;
                        Ok((key, second, totals(r, 2)?, r.get::<_, Option<f64>>(9)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        Ok(rows
            .into_iter()
            .map(|(key, model, totals, avg_latency_ms)| {
                let provider_id = match dimension {
                    CostDimension::Provider | CostDimension::Model => key.as_deref().and_then(|k| k.parse().ok()),
                    _ => None,
                };
                let key_str = match (&key, &model) {
                    (Some(k), Some(m)) => format!("{k}/{m}"),
                    (Some(k), None) => k.clone(),
                    (None, _) => "none".to_owned(),
                };
                CostBreakdownRow { key: key_str, model, provider_id, totals, avg_latency_ms }
            })
            .collect())
    }

    /// Zero-filled time series ending with the current bucket.
    pub async fn timeseries(
        &self,
        granularity: CostGranularity,
        buckets: u32,
        project: Option<ProjectId>,
    ) -> CostResult<Vec<CostPoint>> {
        let now = Utc::now();
        let keys = match granularity {
            CostGranularity::Day => self.calendar.last_days(&now, buckets),
            CostGranularity::Month => self.calendar.last_months(&now, buckets),
        };
        let Some(first) = keys.first().cloned() else { return Ok(Vec::new()) };
        let col = match granularity {
            CostGranularity::Day => "day",
            CostGranularity::Month => "month",
        };
        let mut args = vec![first];
        let project_sql = match project {
            Some(p) => {
                args.push(p.to_string());
                " AND project_id = ?"
            }
            None => "",
        };
        let rows: Vec<(String, UsageTotals)> = self
            .db
            .call(move |c| {
                let sql = format!(
                    "SELECT {col}, {TOTALS_SQL} FROM usage_records WHERE {col} >= ?{project_sql} GROUP BY {col}"
                );
                let mut stmt = c.prepare(&sql)?;
                let rows = stmt
                    .query_map(params_from_iter(args.iter()), |r| Ok((r.get(0)?, totals(r, 1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        let map: std::collections::HashMap<String, UsageTotals> = rows.into_iter().collect();
        Ok(keys
            .into_iter()
            .map(|bucket| {
                let totals = map.get(&bucket).copied().unwrap_or_default();
                CostPoint { bucket, totals }
            })
            .collect())
    }

    /// Raw records, newest first.
    pub async fn records(&self, p: CostRecordsParams) -> CostResult<Vec<UsageRecord>> {
        let limit = p.limit.unwrap_or(100).clamp(1, 1000);
        let mut clauses: Vec<&str> = Vec::new();
        let mut args: Vec<String> = Vec::new();
        let filters = [
            ("project_id = ?", p.project_id.map(|v| v.to_string())),
            ("task_id = ?", p.task_id.map(|v| v.to_string())),
            ("provider_id = ?", p.provider_id.map(|v| v.to_string())),
            ("agent_id = ?", p.agent_id.map(|v| v.to_string())),
            ("ts < ?", p.before.as_ref().map(ts_key)),
        ];
        for (clause, value) in filters {
            if let Some(v) = value {
                clauses.push(clause);
                args.push(v);
            }
        }
        let where_sql = if clauses.is_empty() { "1 = 1".to_owned() } else { clauses.join(" AND ") };
        Ok(self
            .db
            .call(move |c| {
                let sql = format!(
                    "SELECT {RECORD_COLS} FROM usage_records WHERE {where_sql} ORDER BY ts DESC, seq DESC LIMIT {limit}"
                );
                let mut stmt = c.prepare(&sql)?;
                let rows =
                    stmt.query_map(params_from_iter(args.iter()), record_from_row)?.collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{engine, record};
    use chrono::Duration;

    #[tokio::test]
    async fn aggregates_by_dimension_day_and_month() {
        let (eng, _bus) = engine().await;
        let p1 = ProjectId::new();
        let p2 = ProjectId::new();
        let openai = ProviderId::new();
        let anthropic = ProviderId::new();
        let agent = AgentId::new();
        let now = Utc::now();
        let today = now.to_rfc3339();
        let yesterday = (now - Duration::days(1)).to_rfc3339();
        let long_ago = (now - Duration::days(400)).to_rfc3339();

        let mut a = record(openai, "gpt-5", 1.0, &today);
        a.project_id = Some(p1);
        a.agent_id = Some(agent);
        let mut b = record(anthropic, "claude", 2.5, &today);
        b.project_id = Some(p1);
        b.estimated = true;
        b.success = false;
        b.latency_ms = Some(1000);
        let mut c = record(openai, "gpt-5", 0.5, &yesterday);
        c.project_id = Some(p2);
        let d = record(openai, "gpt-4", 10.0, &long_ago);
        for r in [&a, &b, &c, &d] {
            eng.ingest(r).await.unwrap();
        }
        // Idempotent re-ingest.
        eng.ingest(&a).await.unwrap();

        let s = eng.summary(None).await.unwrap();
        assert!((s.today.cost_usd - 3.5).abs() < 1e-9);
        assert!((s.today.estimated_cost_usd - 2.5).abs() < 1e-9);
        assert_eq!(s.today.requests, 2);
        assert_eq!(s.today.failures, 1);
        assert!((s.total.cost_usd - 14.0).abs() < 1e-9);
        assert_eq!(s.total.input_tokens, 4000);
        assert!(s.budget_used.is_none());
        let s1 = eng.summary(Some(p1)).await.unwrap();
        assert!((s1.total.cost_usd - 3.5).abs() < 1e-9);

        let by_provider = eng.by_dimension(CostDimension::Provider, CostWindow::All, None, 50).await.unwrap();
        assert_eq!(by_provider.len(), 2);
        assert_eq!(by_provider[0].provider_id, Some(openai));
        assert!((by_provider[0].totals.cost_usd - 11.5).abs() < 1e-9);

        let by_model = eng.by_dimension(CostDimension::Model, CostWindow::Last30Days, None, 50).await.unwrap();
        assert_eq!(by_model.len(), 2);
        assert_eq!(by_model[0].model.as_deref(), Some("claude"));
        assert_eq!(by_model[0].key, format!("{anthropic}/claude"));
        assert_eq!(by_model[0].avg_latency_ms, Some(1000.0));

        let by_agent = eng.by_dimension(CostDimension::Agent, CostWindow::Today, None, 50).await.unwrap();
        assert_eq!(
            by_agent.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
            vec!["none", agent.to_string().as_str()]
        );
        let by_project = eng.by_dimension(CostDimension::Project, CostWindow::All, Some(p2), 50).await.unwrap();
        assert_eq!(by_project.len(), 1);

        let days = eng.timeseries(CostGranularity::Day, 7, None).await.unwrap();
        assert_eq!(days.len(), 7);
        assert_eq!(days.last().unwrap().bucket, eng.calendar().day_key(&now));
        assert!((days[6].totals.cost_usd - 3.5).abs() < 1e-9);
        assert!((days[5].totals.cost_usd - 0.5).abs() < 1e-9);
        assert_eq!(days[0].totals.requests, 0);
        let months = eng.timeseries(CostGranularity::Month, 24, Some(p1)).await.unwrap();
        assert_eq!(months.len(), 24);
        assert!((months.iter().map(|m| m.totals.cost_usd).sum::<f64>() - 3.5).abs() < 1e-9);

        let recs = eng.records(CostRecordsParams::default()).await.unwrap();
        assert_eq!(recs.len(), 4);
        assert_eq!(recs[3], d);
        assert!(recs[0].ts >= recs[1].ts);
        let page = eng
            .records(CostRecordsParams { before: Some(recs[1].ts), limit: Some(10), ..CostRecordsParams::default() })
            .await
            .unwrap();
        assert_eq!(page.len(), 2);
        let filtered = eng
            .records(CostRecordsParams { provider_id: Some(anthropic), ..CostRecordsParams::default() })
            .await
            .unwrap();
        assert_eq!(filtered, vec![b.clone()]);
    }

    #[tokio::test]
    async fn invalid_records_and_budget_used() {
        let (eng, _bus) = engine().await;
        let p = ProviderId::new();
        let now = Utc::now().to_rfc3339();
        assert!(eng.ingest(&record(p, "", 1.0, &now)).await.is_err());
        assert!(eng.ingest(&record(p, "m", -1.0, &now)).await.is_err());
        assert!(eng.ingest(&record(p, "m", f64::NAN, &now)).await.is_err());
        eng.set_budget(orkestr_protocol::methods::cost::SetBudgetParams {
            id: None,
            name: Some("Global daily".into()),
            scope: BudgetScope::Global,
            period: orkestr_domain::BudgetPeriod::Daily,
            limit_usd: Some(4.0),
            unrestricted: None,
            warn_at: None,
            enabled: None,
        })
        .await
        .unwrap();
        let fired = eng.ingest(&record(p, "m", 3.0, &now)).await.unwrap();
        assert_eq!(fired.len(), 1);
        let s = eng.summary(None).await.unwrap();
        assert!((s.budget_used.unwrap() - 0.75).abs() < 1e-9);
    }
}
