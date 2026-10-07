//! # orkestr-cost
//!
//! Cost & Limit Center (MASTER_SPEC §22):
//!
//! * [`CostEngine::ingest`] stores [`UsageRecord`]s (append-only) and fires
//!   `BudgetThreshold` events when an applicable budget crosses 50 %, its
//!   `warnAt` fraction or 100 % — once per budget, period and level.
//! * Aggregates: [`CostEngine::summary`], [`CostEngine::by_dimension`]
//!   (provider / model / agent / project / task), [`CostEngine::timeseries`]
//!   (day / month, zero-filled) and raw [`CostEngine::records`].
//! * Budgets ([`budget`]): scopes Global / Project / Task / Provider / Agent,
//!   periods Daily / Monthly / Total / PerTask, a USD limit or an explicit
//!   *unrestricted* flag. [`CostEngine::check`] evaluates the whole scope chain
//!   for an estimated call → Allow | Warn | Exceeded.
//! * Benchmarks ([`benchmarks`]): per-execution quality signals collected
//!   from events (first-pass success, builds, defects, retries, review
//!   rejections, post-merge bugs) joined with usage → model benchmark table,
//!   agent performance per template / instance and router statistics.
//!
//! Calendar periods use the core's local UTC offset at startup
//! ([`Calendar::local`]); keys are stored with each record.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod benchmarks;
pub mod budget;
pub mod calendar;
pub mod schema;
pub mod stats;
mod usage;

use orkestr_events::EventBus;
use orkestr_storage::{Db, StorageError};

pub use budget::{evaluate_limit, exempt_flags, scope_rank, BudgetContext};
pub use calendar::Calendar;
pub use orkestr_domain::UsageRecord;
pub use schema::cost_migrations;

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum CostError {
    /// Entity not found.
    #[error("{0} not found")]
    NotFound(String),
    /// Invalid input.
    #[error("{0}")]
    Invalid(String),
    /// Storage failure.
    #[error("storage: {0}")]
    Storage(#[from] StorageError),
    /// Event publication failed.
    #[error("event publication failed: {0}")]
    Events(String),
}

/// Result alias.
pub type CostResult<T> = Result<T, CostError>;

/// Converts USD to integer micro-dollars (storage unit).
#[must_use]
pub fn usd_to_micros(usd: f64) -> i64 {
    if !usd.is_finite() {
        return 0;
    }
    // Saturating float→int conversion is the documented behaviour of `as`.
    #[allow(clippy::cast_possible_truncation)]
    let v = (usd * 1_000_000.0).round() as i64;
    v
}

/// Converts micro-dollars back to USD.
#[must_use]
pub fn micros_to_usd(micros: i64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let v = micros as f64 / 1_000_000.0;
    v
}

/// The cost engine. Cheap to clone.
#[derive(Debug, Clone)]
pub struct CostEngine {
    db: Db,
    events: Option<EventBus>,
    calendar: Calendar,
}

impl CostEngine {
    /// Engine over a database with [`cost_migrations`] applied, using the
    /// local calendar.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db, events: None, calendar: Calendar::local() }
    }

    /// Attaches an event bus for `BudgetThreshold` events.
    #[must_use]
    pub fn with_events(mut self, events: EventBus) -> Self {
        self.events = Some(events);
        self
    }

    /// Overrides the calendar (tests, explicit time zones).
    #[must_use]
    pub fn with_calendar(mut self, calendar: Calendar) -> Self {
        self.calendar = calendar;
        self
    }

    /// The calendar in use.
    #[must_use]
    pub fn calendar(&self) -> Calendar {
        self.calendar
    }

    /// Underlying database.
    #[must_use]
    pub fn db(&self) -> &Db {
        &self.db
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use chrono::{DateTime, Utc};
    use orkestr_domain::{ProviderId, UsageRecord, UsageRecordId};
    use orkestr_events::EventBus;
    use orkestr_storage::Db;

    use super::*;

    pub async fn engine() -> (CostEngine, EventBus) {
        let db = Db::open_in_memory().unwrap();
        db.migrate(cost_migrations()).await.unwrap();
        let bus = EventBus::in_memory(256);
        (CostEngine::new(db).with_events(bus.clone()).with_calendar(Calendar::utc()), bus)
    }

    pub fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    pub fn record(provider: ProviderId, model: &str, cost: f64, at: &str) -> UsageRecord {
        UsageRecord {
            id: UsageRecordId::new(),
            ts: ts(at),
            provider_id: provider,
            model: model.into(),
            project_id: None,
            task_id: None,
            agent_id: None,
            execution_id: None,
            input_tokens: 1000,
            output_tokens: 200,
            cached_input_tokens: 0,
            reasoning_tokens: 0,
            cost_usd: cost,
            api_equivalent_usd: None,
            estimated: false,
            latency_ms: Some(400),
            success: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn micros_roundtrip() {
        assert_eq!(usd_to_micros(1.234_567_4), 1_234_567);
        assert_eq!(usd_to_micros(f64::NAN), 0);
        assert!((micros_to_usd(2_500_000) - 2.5).abs() < 1e-12);
    }
}
