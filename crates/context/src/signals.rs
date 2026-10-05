//! Recency / churn signal from git history (§20).
//!
//! The VCS layer runs `git log --name-only --format=%x1e%ct` (see
//! `orkestr-vcs` `GitCli::churn_log`); [`ChurnStats::parse`] turns that output
//! into per-file commit counts and last-change times. The optimizer uses it as
//! a *multiplier* on files that are already relevant — churn alone never pulls
//! a file into the context.

use std::collections::BTreeMap;

/// Record separator emitted by the `%x1e` format.
pub const RECORD_SEP: char = '\u{1e}';

/// Per-file churn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileChurn {
    /// Commits touching the file in the window.
    pub commits: u32,
    /// Unix time of the newest such commit.
    pub last_ts: i64,
}

/// Churn of a repository window.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChurnStats {
    /// Path (`/`-separated, repo-relative) → churn.
    pub files: BTreeMap<String, FileChurn>,
    /// Commits parsed.
    pub commits: u32,
    /// Newest commit time seen.
    pub newest_ts: i64,
}

impl ChurnStats {
    /// Parses `git log --name-only --format=%x1e%ct` output.
    #[must_use]
    pub fn parse(out: &str) -> Self {
        let mut stats = ChurnStats::default();
        for record in out.split(RECORD_SEP) {
            let mut lines = record.lines().map(str::trim).filter(|l| !l.is_empty());
            let Some(ts) = lines.next().and_then(|l| l.parse::<i64>().ok()) else { continue };
            stats.commits += 1;
            stats.newest_ts = stats.newest_ts.max(ts);
            for path in lines {
                let path = path.replace('\\', "/");
                let e = stats.files.entry(path).or_default();
                e.commits += 1;
                e.last_ts = e.last_ts.max(ts);
            }
        }
        stats
    }

    /// Relevance multiplier for `path` in `[1.0, 1.6]`: more commits and more
    /// recent changes weigh more (half-life ≈ 14 days relative to the newest
    /// commit in the window).
    #[must_use]
    pub fn boost(&self, path: &str) -> f64 {
        let Some(c) = self.files.get(path) else { return 1.0 };
        let freq = (f64::from(c.commits)).ln_1p() / 4.0_f64.ln_1p().max(1e-9); // 3 commits ≈ 1.0
        #[allow(clippy::cast_precision_loss)]
        let age_days = ((self.newest_ts - c.last_ts).max(0) as f64) / 86_400.0;
        let recency = 0.5_f64.powf(age_days / 14.0);
        1.0 + 0.3 * freq.min(1.0) + 0.3 * recency
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_boosts() {
        let out = "\u{1e}1700000000\n\nsrc/a.rs\nsrc/b.rs\n\u{1e}1690000000\n\nsrc/a.rs\n\u{1e}1600000000\n\nsrc\\c.rs\n\u{1e}garbage\n";
        let s = ChurnStats::parse(out);
        assert_eq!(s.commits, 3);
        assert_eq!(s.files["src/a.rs"], FileChurn { commits: 2, last_ts: 1_700_000_000 });
        assert_eq!(s.files["src/c.rs"].commits, 1);
        assert!(s.boost("src/a.rs") > s.boost("src/b.rs"), "more commits weigh more");
        assert!(s.boost("src/b.rs") > s.boost("src/c.rs"), "recent weighs more");
        assert!((s.boost("missing.rs") - 1.0).abs() < f64::EPSILON);
        assert!(s.boost("src/a.rs") <= 1.6 + 1e-9);
        assert_eq!(ChurnStats::parse(""), ChurnStats::default());
    }
}
