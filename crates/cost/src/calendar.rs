//! Calendar periods (day / month keys) in a fixed UTC offset.

use chrono::{DateTime, Datelike, Duration, FixedOffset, Local, NaiveDate, Offset, Utc};

/// Maps timestamps to `YYYY-MM-DD` / `YYYY-MM` keys in one UTC offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Calendar {
    offset: FixedOffset,
}

impl Calendar {
    /// The core's local offset at the time of the call ("calendar day, local
    /// time of the core" — `BudgetPeriod::Daily`).
    #[must_use]
    pub fn local() -> Self {
        Self { offset: Local::now().offset().fix() }
    }

    /// UTC.
    #[must_use]
    pub fn utc() -> Self {
        Self { offset: Utc.fix() }
    }

    /// A fixed offset in seconds east of UTC (falls back to UTC when out of range).
    #[must_use]
    pub fn with_offset_secs(secs: i32) -> Self {
        Self { offset: FixedOffset::east_opt(secs).unwrap_or_else(|| Utc.fix()) }
    }

    fn date(&self, ts: &DateTime<Utc>) -> NaiveDate {
        ts.with_timezone(&self.offset).date_naive()
    }

    /// Day key, e.g. `2026-10-02`.
    #[must_use]
    pub fn day_key(&self, ts: &DateTime<Utc>) -> String {
        self.date(ts).format("%Y-%m-%d").to_string()
    }

    /// Month key, e.g. `2026-10`.
    #[must_use]
    pub fn month_key(&self, ts: &DateTime<Utc>) -> String {
        self.date(ts).format("%Y-%m").to_string()
    }

    /// The last `n` day keys ending with the day of `now`, oldest first.
    #[must_use]
    pub fn last_days(&self, now: &DateTime<Utc>, n: u32) -> Vec<String> {
        let today = self.date(now);
        (0..i64::from(n))
            .rev()
            .filter_map(|i| today.checked_sub_signed(Duration::days(i)))
            .map(|d| d.format("%Y-%m-%d").to_string())
            .collect()
    }

    /// The last `n` month keys ending with the month of `now`, oldest first.
    #[must_use]
    pub fn last_months(&self, now: &DateTime<Utc>, n: u32) -> Vec<String> {
        let today = self.date(now);
        let current = today.year() * 12 + i32::try_from(today.month0()).unwrap_or(0);
        let n = i32::try_from(n).unwrap_or(i32::MAX);
        (0..n)
            .rev()
            .map(|i| {
                let m = current - i;
                format!("{:04}-{:02}", m.div_euclid(12), m.rem_euclid(12) + 1)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::ts;

    #[test]
    fn keys_respect_offset() {
        let t = ts("2026-10-01T23:30:00Z");
        assert_eq!(Calendar::utc().day_key(&t), "2026-10-01");
        let plus3 = Calendar::with_offset_secs(3 * 3600);
        assert_eq!(plus3.day_key(&t), "2026-10-02");
        assert_eq!(plus3.month_key(&ts("2026-09-30T22:00:00Z")), "2026-10");
    }

    #[test]
    fn ranges() {
        let cal = Calendar::utc();
        let t = ts("2026-03-02T10:00:00Z");
        assert_eq!(cal.last_days(&t, 3), vec!["2026-02-28", "2026-03-01", "2026-03-02"]);
        assert_eq!(cal.last_months(&t, 4), vec!["2025-12", "2026-01", "2026-02", "2026-03"]);
        assert!(cal.last_days(&t, 0).is_empty());
    }
}
