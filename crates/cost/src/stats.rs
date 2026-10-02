//! Model benchmarking and agent performance (§22) — **skeleton**.
//!
//! What can be derived from usage records today is computed for real:
//! samples, call success rate, latency (avg / p50 / p95) and cost. Quality
//! signals that need the task engine and review loop (first-pass success,
//! build success, defects found, retries, review rejections, post-merge
//! bugs, completed/failed tasks) are `None` until those engines feed them
//! (Phase 2) — never fabricated.

use chrono::{Duration, Utc};
use orkestr_domain::{AgentId, ProviderId};
use serde::{Deserialize, Serialize};

use crate::{micros_to_usd, CostEngine, CostResult};

/// Per-model statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelBenchmark {
    /// Provider.
    pub provider_id: ProviderId,
    /// Model.
    pub model: String,
    /// Number of calls.
    pub samples: u64,
    /// Successful calls / calls.
    pub call_success_rate: f64,
    /// Average latency (ms) of calls that reported one.
    pub latency_avg_ms: Option<f64>,
    /// Median latency.
    pub latency_p50_ms: Option<u64>,
    /// 95th percentile latency.
    pub latency_p95_ms: Option<u64>,
    /// Average cost per call (USD).
    pub cost_avg_usd: f64,
    /// Total cost (USD).
    pub cost_total_usd: f64,
    /// First-pass task success rate — fed by the task engine (Phase 2).
    pub first_pass_success_rate: Option<f64>,
    /// Build success rate — fed by quality gates (Phase 2).
    pub build_success_rate: Option<f64>,
    /// Average retries per task — fed by the task engine (Phase 2).
    pub retries_avg: Option<f64>,
    /// Average defects found in review — fed by the review loop (Phase 2).
    pub defects_found_avg: Option<f64>,
}

/// Per-agent statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPerformance {
    /// Agent.
    pub agent_id: AgentId,
    /// Model calls.
    pub calls: u64,
    /// Failed calls.
    pub failed_calls: u64,
    /// Total cost (USD).
    pub cost_usd: f64,
    /// Average latency (ms).
    pub latency_avg_ms: Option<f64>,
    /// Completed tasks — fed by the task engine (Phase 2).
    pub tasks_completed: Option<u64>,
    /// Failed tasks — fed by the task engine (Phase 2).
    pub tasks_failed: Option<u64>,
    /// Review rejection rate — fed by the review loop (Phase 2).
    pub review_rejection_rate: Option<f64>,
    /// Post-merge bugs — fed by the merge manager (Phase 2).
    pub post_merge_bugs: Option<u64>,
}

/// Nearest-rank percentile of sorted values.
#[must_use]
pub fn percentile(sorted: &[u64], p: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rank = ((p.clamp(0.0, 1.0) * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted.get(rank - 1).copied()
}

#[allow(clippy::cast_precision_loss)]
fn ratio(a: u64, b: u64) -> f64 {
    if b == 0 {
        0.0
    } else {
        a as f64 / b as f64
    }
}

type Row = (String, String, i64, bool, Option<i64>, Option<String>);

impl CostEngine {
    async fn rows_since(&self, days: u32) -> CostResult<Vec<Row>> {
        let since =
            (Utc::now() - Duration::days(i64::from(days.max(1)))).to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
        Ok(self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT provider_id, model, cost_micros, success, latency_ms, agent_id FROM usage_records
                     WHERE ts >= ?1 ORDER BY provider_id, model",
                )?;
                let rows = stmt
                    .query_map([since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?)
    }

    /// Model benchmarks over the last `days` days, most used first.
    pub async fn model_benchmarks(&self, days: u32) -> CostResult<Vec<ModelBenchmark>> {
        let rows = self.rows_since(days).await?;
        let mut groups: std::collections::BTreeMap<(String, String), Vec<&Row>> = std::collections::BTreeMap::new();
        for r in &rows {
            groups.entry((r.0.clone(), r.1.clone())).or_default().push(r);
        }
        let mut out = Vec::new();
        for ((provider, model), rs) in groups {
            let Ok(provider_id) = provider.parse::<ProviderId>() else { continue };
            let samples = rs.len() as u64;
            let ok = rs.iter().filter(|r| r.3).count() as u64;
            let cost: i64 = rs.iter().map(|r| r.2).sum();
            let mut lat: Vec<u64> = rs.iter().filter_map(|r| r.4).filter_map(|v| u64::try_from(v).ok()).collect();
            lat.sort_unstable();
            #[allow(clippy::cast_precision_loss)]
            let latency_avg_ms = (!lat.is_empty()).then(|| lat.iter().sum::<u64>() as f64 / lat.len() as f64);
            let cost_total_usd = micros_to_usd(cost);
            #[allow(clippy::cast_precision_loss)]
            let cost_avg_usd = cost_total_usd / samples.max(1) as f64;
            out.push(ModelBenchmark {
                provider_id,
                model,
                samples,
                call_success_rate: ratio(ok, samples),
                latency_avg_ms,
                latency_p50_ms: percentile(&lat, 0.5),
                latency_p95_ms: percentile(&lat, 0.95),
                cost_avg_usd,
                cost_total_usd,
                first_pass_success_rate: None,
                build_success_rate: None,
                retries_avg: None,
                defects_found_avg: None,
            });
        }
        out.sort_by(|a, b| b.samples.cmp(&a.samples).then_with(|| a.model.cmp(&b.model)));
        Ok(out)
    }

    /// Agent performance over the last `days` days, most expensive first.
    pub async fn agent_performance(&self, days: u32) -> CostResult<Vec<AgentPerformance>> {
        let rows = self.rows_since(days).await?;
        let mut groups: std::collections::BTreeMap<String, Vec<&Row>> = std::collections::BTreeMap::new();
        for r in rows.iter().filter(|r| r.5.is_some()) {
            if let Some(a) = &r.5 {
                groups.entry(a.clone()).or_default().push(r);
            }
        }
        let mut out = Vec::new();
        for (agent, rs) in groups {
            let Ok(agent_id) = agent.parse::<AgentId>() else { continue };
            let lat: Vec<i64> = rs.iter().filter_map(|r| r.4).collect();
            #[allow(clippy::cast_precision_loss)]
            let latency_avg_ms = (!lat.is_empty()).then(|| lat.iter().sum::<i64>() as f64 / lat.len() as f64);
            out.push(AgentPerformance {
                agent_id,
                calls: rs.len() as u64,
                failed_calls: rs.iter().filter(|r| !r.3).count() as u64,
                cost_usd: micros_to_usd(rs.iter().map(|r| r.2).sum()),
                latency_avg_ms,
                tasks_completed: None,
                tasks_failed: None,
                review_rejection_rate: None,
                post_merge_bugs: None,
            });
        }
        out.sort_by(|a, b| b.cost_usd.total_cmp(&a.cost_usd).then_with(|| a.agent_id.cmp(&b.agent_id)));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{engine, record};

    #[test]
    fn percentiles() {
        assert_eq!(percentile(&[], 0.5), None);
        assert_eq!(percentile(&[10], 0.95), Some(10));
        let v: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&v, 0.5), Some(50));
        assert_eq!(percentile(&v, 0.95), Some(95));
    }

    #[tokio::test]
    async fn benchmarks_and_agents() {
        let (eng, _bus) = engine().await;
        let p = ProviderId::new();
        let agent = AgentId::new();
        let now = Utc::now().to_rfc3339();
        for (i, cost) in [1.0, 2.0, 3.0].iter().enumerate() {
            let mut r = record(p, "m1", *cost, &now);
            r.latency_ms = Some(100 * (i as u64 + 1));
            r.success = i != 2;
            r.agent_id = Some(agent);
            eng.ingest(&r).await.unwrap();
        }
        eng.ingest(&record(p, "m2", 0.5, &now)).await.unwrap();
        let b = eng.model_benchmarks(30).await.unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].model, "m1");
        assert_eq!(b[0].samples, 3);
        assert!((b[0].call_success_rate - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(b[0].latency_p50_ms, Some(200));
        assert!((b[0].cost_avg_usd - 2.0).abs() < 1e-9);
        assert_eq!(b[0].first_pass_success_rate, None, "not fabricated");
        let a = eng.agent_performance(30).await.unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].calls, 3);
        assert_eq!(a[0].failed_calls, 1);
        assert!((a[0].cost_usd - 6.0).abs() < 1e-9);
    }
}
