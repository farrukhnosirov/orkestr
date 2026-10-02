//! Safe FTS5 query construction.
//!
//! User input is never passed to `MATCH` verbatim (FTS5 query syntax would
//! let `"`, `*`, `NEAR`, `-`, column filters … through and cause syntax
//! errors). Instead words are extracted and each is emitted as a quoted
//! prefix term: `"word"*`.

/// Max number of terms taken from a query.
pub const MAX_TERMS: usize = 16;
/// Max characters of one term.
pub const MAX_TERM_LEN: usize = 64;

/// Snippet start marker (STX) wrapped around matches in snippets.
pub const MARK_START: char = '\u{2}';
/// Snippet end marker (ETX).
pub const MARK_END: char = '\u{3}';

/// How terms are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combine {
    /// Every term must match.
    All,
    /// Any term may match.
    Any,
}

/// Splits text into lowercase alphanumeric words (unicode aware).
#[must_use]
pub fn terms(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in query.split(|c: char| !c.is_alphanumeric()) {
        if word.is_empty() {
            continue;
        }
        let w: String = word.chars().take(MAX_TERM_LEN).collect::<String>().to_lowercase();
        if !out.contains(&w) {
            out.push(w);
        }
        if out.len() == MAX_TERMS {
            break;
        }
    }
    out
}

/// Builds an FTS5 `MATCH` expression, or `None` when the query has no words.
#[must_use]
pub fn match_expr(query: &str, combine: Combine) -> Option<String> {
    let terms = terms(query);
    if terms.is_empty() {
        return None;
    }
    let sep = match combine {
        Combine::All => " AND ",
        Combine::Any => " OR ",
    };
    Some(terms.iter().map(|t| format!("\"{}\"*", t.replace('"', ""))).collect::<Vec<_>>().join(sep))
}

/// Converts an FTS5 bm25 rank (lower = better, usually negative) into a
/// positive "higher is better" score.
#[must_use]
pub fn score_from_rank(rank: f64) -> f64 {
    if rank.is_finite() {
        -rank
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_input_is_neutralized() {
        assert_eq!(match_expr("  ", Combine::All), None);
        assert_eq!(match_expr("\"* NEAR(", Combine::All).as_deref(), Some("\"near\"*"));
        assert_eq!(match_expr("title:foo -bar", Combine::All).as_deref(), Some("\"title\"* AND \"foo\"* AND \"bar\"*"));
        assert_eq!(match_expr("Auth auth AUTH", Combine::Any).as_deref(), Some("\"auth\"*"));
    }

    #[test]
    fn unicode_words_and_limits() {
        assert_eq!(terms("Привет, мир! snake_case"), vec!["привет", "мир", "snake", "case"]);
        let many = (0..40).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ");
        assert_eq!(terms(&many).len(), MAX_TERMS);
        assert_eq!(terms(&"x".repeat(500))[0].len(), MAX_TERM_LEN);
    }

    #[test]
    fn rank_to_score() {
        assert!(score_from_rank(-3.0) > score_from_rank(-1.0));
        assert_eq!(score_from_rank(f64::NAN), 0.0);
    }
}
