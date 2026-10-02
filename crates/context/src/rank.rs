//! Relevance ranking of files for a task (BM25 + explicit mentions).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::index::{FileIndex, IndexedFile};

/// BM25 `k1`.
pub const K1: f64 = 1.2;
/// BM25 `b`.
pub const B: f64 = 0.75;
/// Field weight of path tokens.
pub const PATH_WEIGHT: u32 = 3;
/// Field weight of symbol tokens.
pub const SYMBOL_WEIGHT: u32 = 2;
/// Field weight of content-sample tokens.
pub const CONTENT_WEIGHT: u32 = 1;
/// Score added to files the task mentions explicitly.
pub const MENTION_BOOST: f64 = 1000.0;

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "do", "for", "from", "in", "into", "is", "it", "of", "on", "or",
    "the", "this", "that", "to", "with", "we", "you", "should", "must", "please", "can", "fn", "pub", "let", "const",
    "use", "import", "export", "return", "self", "mut", "function", "def", "var", "new", "if", "else", "src", "lib",
    "mod", "index", "make", "add", "fix", "update", "change", "file", "files", "code", "so", "not", "all", "when",
];

/// A ranked file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankedFile {
    /// Path.
    pub path: String,
    /// Score (higher is better).
    pub score: f64,
    /// Whether the task mentions this file.
    pub mentioned: bool,
    /// Query terms that matched (sorted).
    pub matched_terms: Vec<String>,
}

/// Splits text into lowercase identifier-aware terms: `computeNetSalary` →
/// `compute`, `net`, `salary`, `computenetsalary`; `snake_case` → `snake`,
/// `case`, `snake_case`. Stopwords and 1-char tokens are dropped.
#[must_use]
pub fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if word.is_empty() {
            continue;
        }
        let parts = split_identifier(word);
        if parts.len() > 1 {
            let whole = word.to_lowercase();
            if keep(&whole) {
                out.push(whole);
            }
        }
        for p in parts {
            if keep(&p) {
                out.push(p);
            }
        }
    }
    out
}

fn keep(t: &str) -> bool {
    t.chars().count() > 1 && !STOPWORDS.contains(&t) && !t.chars().all(|c| c.is_ascii_digit())
}

fn split_identifier(word: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for seg in word.split('_').filter(|s| !s.is_empty()) {
        let chars: Vec<char> = seg.chars().collect();
        let mut cur = String::new();
        for (i, &c) in chars.iter().enumerate() {
            let prev = i.checked_sub(1).and_then(|j| chars.get(j)).copied();
            let next = chars.get(i + 1).copied();
            let boundary = match prev {
                Some(p) => {
                    (c.is_uppercase() && (p.is_lowercase() || p.is_ascii_digit()))
                        || (c.is_uppercase() && p.is_uppercase() && next.is_some_and(char::is_lowercase))
                        || (c.is_ascii_digit() != p.is_ascii_digit())
                }
                None => false,
            };
            if boundary && !cur.is_empty() {
                parts.push(std::mem::take(&mut cur).to_lowercase());
            }
            cur.push(c);
        }
        if !cur.is_empty() {
            parts.push(cur.to_lowercase());
        }
    }
    parts
}

static PATH_MENTION: Lazy<Regex> = Lazy::new(|| {
    #[allow(clippy::expect_used)]
    Regex::new(r"[A-Za-z0-9_.\-/\\]*[A-Za-z0-9_\-]\.[A-Za-z0-9]{1,8}\b|[A-Za-z0-9_.\-]+(?:[/\\][A-Za-z0-9_.\-]+)+")
        .expect("valid mention regex")
});

/// Paths / file names mentioned in a task text (normalized, `/`-separated).
#[must_use]
pub fn mentioned_paths(task: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in PATH_MENTION.find_iter(task) {
        let p =
            m.as_str().replace('\\', "/").trim_start_matches("./").trim_matches('/').trim_end_matches('.').to_owned();
        if p.len() >= 3 && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

fn is_mentioned(path: &str, mentions: &[String]) -> bool {
    mentions.iter().any(|m| {
        path == m
            || path.ends_with(&format!("/{m}"))
            || path.starts_with(&format!("{m}/"))
            || path.contains(&format!("/{m}/"))
    })
}

fn doc_terms(f: &IndexedFile) -> HashMap<String, u32> {
    let mut tf: HashMap<String, u32> = HashMap::new();
    for t in terms(&f.path) {
        *tf.entry(t).or_default() += PATH_WEIGHT;
    }
    for s in &f.symbols {
        for t in terms(&s.name) {
            *tf.entry(t).or_default() += SYMBOL_WEIGHT;
        }
    }
    for t in terms(&f.sample) {
        *tf.entry(t).or_default() += CONTENT_WEIGHT;
    }
    tf
}

/// Ranks files for `task`: BM25 over weighted fields plus [`MENTION_BOOST`]
/// for explicitly mentioned paths. Files with score 0 are dropped. Ordering
/// is deterministic (score desc, then path asc).
#[must_use]
pub fn rank_files(index: &FileIndex, task: &str, limit: usize) -> Vec<RankedFile> {
    let query: BTreeSet<String> = terms(task).into_iter().collect();
    let mentions = mentioned_paths(task);
    if index.files.is_empty() || (query.is_empty() && mentions.is_empty()) {
        return Vec::new();
    }
    let docs: Vec<HashMap<String, u32>> = index.files.iter().map(doc_terms).collect();
    let lens: Vec<f64> = docs.iter().map(|d| f64::from(d.values().sum::<u32>())).collect();
    #[allow(clippy::cast_precision_loss)]
    let n = docs.len() as f64;
    let avg_len = (lens.iter().sum::<f64>() / n).max(1.0);
    let mut df: BTreeMap<&str, f64> = BTreeMap::new();
    for q in &query {
        let count = docs.iter().filter(|d| d.contains_key(q)).count();
        #[allow(clippy::cast_precision_loss)]
        df.insert(q.as_str(), count as f64);
    }
    let mut ranked: Vec<RankedFile> = Vec::new();
    for ((file, doc), len) in index.files.iter().zip(&docs).zip(&lens) {
        let mut score = 0.0;
        let mut matched = Vec::new();
        for q in &query {
            let Some(&tf) = doc.get(q) else { continue };
            let dfq = df.get(q.as_str()).copied().unwrap_or(0.0);
            let idf = ((n - dfq + 0.5) / (dfq + 0.5) + 1.0).ln();
            let tf = f64::from(tf);
            score += idf * (tf * (K1 + 1.0)) / (tf + K1 * (1.0 - B + B * len / avg_len));
            matched.push(q.clone());
        }
        let mentioned = is_mentioned(&file.path, &mentions);
        if mentioned {
            score += MENTION_BOOST;
        }
        if score > 0.0 {
            ranked.push(RankedFile { path: file.path.clone(), score, mentioned, matched_terms: matched });
        }
    }
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    ranked.truncate(limit);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_terms() {
        assert_eq!(terms("computeNetSalary"), vec!["computenetsalary", "compute", "net", "salary"]);
        assert_eq!(terms("HTTPServer v2"), vec!["httpserver", "http", "server", "v2"]);
        assert_eq!(terms("snake_case_name"), vec!["snake_case_name", "snake", "case", "name"]);
        assert_eq!(terms("the a to 123 x"), Vec::<String>::new());
        assert_eq!(terms("Зарплата расчёт"), vec!["зарплата", "расчёт"]);
    }

    #[test]
    fn mentions() {
        let m = mentioned_paths("Fix overtime in src/payroll/calc.rs and check README.md, also crates\\core.");
        assert_eq!(m, vec!["src/payroll/calc.rs", "README.md", "crates/core"]);
        assert!(is_mentioned("crates/core/src/lib.rs", &m));
        assert!(is_mentioned("docs/README.md", &m));
        assert!(!is_mentioned("src/payroll/calc_test.rs", &m));
    }

    #[test]
    fn ranking_prefers_paths_and_symbols_and_is_deterministic() {
        let idx = FileIndex::from_sources([
            ("src/payroll/salary.rs", "pub fn compute_net_salary() {}\n"),
            ("src/payroll/tax.rs", "pub fn tax_rate() {} // salary mentioned once\n"),
            ("src/ui/button.tsx", "export function Button() {}\n"),
            ("docs/overtime.md", "# Overtime\nRules for overtime.\n"),
            ("src/z.rs", "fn z() {}"),
            ("src/a.rs", "fn z() {}"),
        ]);
        let r = rank_files(&idx, "Fix the net salary computation", 10);
        assert_eq!(r[0].path, "src/payroll/salary.rs");
        assert!(r.iter().all(|f| f.path != "src/ui/button.tsx"));
        assert!(r[0].matched_terms.contains(&"salary".to_owned()));

        let r = rank_files(&idx, "update button.tsx overtime", 10);
        assert_eq!(r[0].path, "src/ui/button.tsx");
        assert!(r[0].mentioned);
        assert_eq!(r[1].path, "docs/overtime.md");

        // Ties sort by path.
        let r = rank_files(&idx, "z", 10);
        assert!(r.is_empty(), "single-char terms are ignored");
        let r1 = rank_files(&idx, "tax salary rules", 10);
        let r2 = rank_files(&idx, "tax salary rules", 10);
        assert_eq!(r1, r2);
        assert!(rank_files(&idx, "", 10).is_empty());
        assert_eq!(rank_files(&idx, "salary", 1).len(), 1);
    }
}
