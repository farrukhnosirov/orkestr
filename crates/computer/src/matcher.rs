//! Fast local matcher: phrase templates compiled to token patterns
//! (concepts / stems + `{slot}`s), aligned with the utterance by a small
//! dynamic program (missing words, extra words, fuzzy tokens, multi-word
//! slots). Runs before any AI call; ~10k patterns take a few milliseconds.

use std::collections::{BTreeMap, HashMap};

use orkestr_protocol::methods::computer::{AppContext, WindowInfo};

use crate::text::{parse_duration_secs, parse_number, token_cost, tokenize, Token};

/// Minimum score of an accepted match.
pub const MATCH_THRESHOLD: f32 = 0.74;

const MISSING: f32 = 1.0;
const EXTRA: f32 = 0.45;
/// A slot costs a little (literal matches are more specific), longer slots barely more.
const SLOT_BASE: f32 = 0.08;
const SLOT_TOKEN: f32 = 0.005;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    Lit(String),
    Slot(String),
}

/// A compiled phrase.
#[derive(Debug, Clone)]
struct Pattern {
    owner: usize,
    phrase: String,
    items: Vec<Item>,
    literals: usize,
    slots: usize,
}

/// Something that owns phrases (a command) as seen by the matcher.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Phrase templates.
    pub phrases: Vec<String>,
    /// Active-window filter (command's or its set's).
    pub context: Option<AppContext>,
    /// Ranking bonus (user sets win ties over the catalogue).
    pub bonus: f32,
}

/// A raw match (owner = index of the [`Entry`]).
#[derive(Debug, Clone, PartialEq)]
pub struct RawMatch {
    /// Entry index.
    pub owner: usize,
    /// Template.
    pub phrase: String,
    /// Score 0..1 (without ranking bonuses).
    pub score: f32,
    /// Ranking score.
    pub rank: f32,
    /// Slot values (original text).
    pub slots: BTreeMap<String, String>,
}

/// The compiled index.
#[derive(Debug, Default, Clone)]
pub struct Matcher {
    patterns: Vec<Pattern>,
    entries: Vec<Entry>,
    /// Literal key → (pattern, occurrences) using it: the quick reject and a
    /// score upper bound without scanning every pattern.
    vocab: HashMap<String, Vec<(usize, u16)>>,
}

/// Compiles one template into items.
fn compile(phrase: &str) -> Vec<Item> {
    let mut items = Vec::new();
    for piece in phrase.split_whitespace() {
        if let Some(name) = piece.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
            items.push(Item::Slot(name.to_owned()));
            continue;
        }
        for t in tokenize(piece) {
            items.push(Item::Lit(t.key));
        }
    }
    items
}

/// True when `active` satisfies the filter.
#[must_use]
pub fn context_matches(ctx: &AppContext, active: Option<&WindowInfo>) -> bool {
    if ctx.is_empty() {
        return true;
    }
    let Some(w) = active else { return false };
    let process = w.process.to_lowercase();
    let process = process.trim_end_matches(".exe");
    let title = w.title.to_lowercase();
    ctx.processes.iter().any(|p| p.to_lowercase().trim_end_matches(".exe") == process)
        || ctx.title_contains.iter().any(|t| !t.is_empty() && title.contains(&t.to_lowercase()))
}

impl Matcher {
    /// Builds the index.
    #[must_use]
    pub fn new(entries: Vec<Entry>) -> Self {
        let mut patterns = Vec::new();
        for (owner, e) in entries.iter().enumerate() {
            for phrase in &e.phrases {
                let items = compile(phrase);
                let literals = items.iter().filter(|i| matches!(i, Item::Lit(_))).count();
                let slots = items.len() - literals;
                if literals == 0 {
                    continue; // a bare "{text}" would match everything
                }
                patterns.push(Pattern { owner, phrase: phrase.clone(), items, literals, slots });
            }
        }
        let mut vocab: HashMap<String, Vec<(usize, u16)>> = HashMap::new();
        for (i, p) in patterns.iter().enumerate() {
            for item in &p.items {
                if let Item::Lit(k) = item {
                    let list = vocab.entry(k.clone()).or_default();
                    match list.last_mut() {
                        Some((j, n)) if *j == i => *n += 1,
                        _ => list.push((i, 1)),
                    }
                }
            }
        }
        Self { patterns, entries, vocab }
    }

    /// Number of compiled patterns.
    #[must_use]
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }

    /// Best matches for `text` (best first, at most `limit`, one per owner).
    /// `min_score` 0 returns near misses too (suggestions).
    #[must_use]
    pub fn find(&self, text: &str, active: Option<&WindowInfo>, limit: usize, min_score: f32) -> Vec<RawMatch> {
        let input = tokenize(text);
        if input.is_empty() {
            return Vec::new();
        }
        // quick reject: some literal of a pattern must hit some token; the
        // hit count bounds the score from above (literals that hit nothing
        // are missing, input tokens no literal can take are extra)
        let mut hits = vec![0_u16; self.patterns.len()];
        for (key, list) in &self.vocab {
            if input.iter().any(|t| token_cost(key, t).is_some()) {
                for &(i, n) in list {
                    hits[i] += n;
                }
            }
        }
        let mut best: BTreeMap<usize, RawMatch> = BTreeMap::new();
        for (p, &h) in self.patterns.iter().zip(&hits) {
            if h == 0 {
                continue;
            }
            if p.slots == 0 && min_score > 0.0 {
                let h = usize::from(h).min(p.literals).min(input.len());
                #[allow(clippy::cast_precision_loss)]
                let lower = MISSING * (p.literals - h) as f32 + EXTRA * (input.len() - h) as f32;
                #[allow(clippy::cast_precision_loss)]
                let upper = 1.0 - lower / (p.literals as f32 + 0.5);
                if upper < min_score {
                    continue;
                }
            }
            let entry = &self.entries[p.owner];
            let contextual = entry.context.as_ref().is_some_and(|c| !c.is_empty());
            if contextual && !entry.context.as_ref().is_some_and(|c| context_matches(c, active)) {
                continue;
            }
            let Some((cost, spans)) = align(&p.items, &input) else { continue };
            #[allow(clippy::cast_precision_loss)]
            let score = (1.0 - cost / (p.literals as f32 + 0.5)).clamp(0.0, 1.0);
            if score < min_score {
                continue;
            }
            let Some(slots) = slot_values(text, &input, &spans) else { continue };
            #[allow(clippy::cast_precision_loss)]
            let rank = score + 0.004 * p.literals as f32 - 0.006 * p.slots as f32
                + entry.bonus
                + if contextual { 0.03 } else { 0.0 };
            let m = RawMatch { owner: p.owner, phrase: p.phrase.clone(), score, rank, slots };
            match best.get(&p.owner) {
                Some(b) if b.rank >= m.rank => {}
                _ => {
                    best.insert(p.owner, m);
                }
            }
        }
        let mut out: Vec<RawMatch> = best.into_values().collect();
        out.sort_by(|a, b| b.rank.total_cmp(&a.rank));
        out.truncate(limit);
        out
    }
}

/// Aligns pattern items with input tokens. Returns the cost and, per slot,
/// `(name, first token, end token)`.
/// `(slot name, first token, end token)`.
type Span = (String, usize, usize);

fn align(items: &[Item], input: &[Token]) -> Option<(f32, Vec<Span>)> {
    let (n, m) = (input.len(), items.len());
    let inf = f32::INFINITY;
    let mut dp = vec![vec![inf; m + 1]; n + 1];
    // back: (prev i, prev j, slot capture?)
    let mut back = vec![vec![(usize::MAX, usize::MAX, false); m + 1]; n + 1];
    dp[0][0] = 0.0;
    for i in 0..=n {
        for j in 0..=m {
            let cur = dp[i][j];
            if !cur.is_finite() {
                continue;
            }
            let mut relax = |ni: usize, nj: usize, c: f32, slot: bool| {
                if cur + c < dp[ni][nj] {
                    dp[ni][nj] = cur + c;
                    back[ni][nj] = (i, j, slot);
                }
            };
            if i < n {
                relax(i + 1, j, EXTRA, false);
            }
            if j < m {
                match &items[j] {
                    Item::Lit(k) => {
                        if i < n {
                            if let Some(c) = token_cost(k, &input[i]) {
                                relax(i + 1, j + 1, c, false);
                            }
                        }
                        relax(i, j + 1, MISSING, false);
                    }
                    Item::Slot(_) => {
                        for e in i + 1..=n {
                            #[allow(clippy::cast_precision_loss)]
                            relax(e, j + 1, SLOT_BASE + SLOT_TOKEN * (e - i - 1) as f32, true);
                        }
                    }
                }
            }
        }
    }
    let cost = dp[n][m];
    if !cost.is_finite() {
        return None;
    }
    // every literal missing = no match at all
    let mut spans = Vec::new();
    let (mut i, mut j) = (n, m);
    let mut matched_literal = false;
    while i > 0 || j > 0 {
        let (pi, pj, slot) = back[i][j];
        if pi == usize::MAX {
            return None;
        }
        if slot {
            if let Item::Slot(name) = &items[pj] {
                spans.push((name.clone(), pi, i));
            }
        } else if pj + 1 == j && pi + 1 == i {
            matched_literal = true;
        }
        i = pi;
        j = pj;
    }
    if !matched_literal {
        return None;
    }
    spans.reverse();
    Some((cost, spans))
}

/// Slot texts from the original utterance; typed slots are validated
/// (`n`, `percent`, `count` numbers; `duration` durations).
fn slot_values(text: &str, input: &[Token], spans: &[Span]) -> Option<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (name, a, b) in spans {
        let start = input[*a].start;
        let end = input[*b - 1].end;
        let raw = text.get(start..end)?.trim().to_owned();
        let value = match name.as_str() {
            "n" | "percent" | "count" => parse_number(&raw.to_lowercase())?.to_string(),
            "duration" => parse_duration_secs(&raw)?.to_string(),
            _ => raw,
        };
        out.insert(name.clone(), value);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(phrases: &[&str]) -> Entry {
        Entry { phrases: phrases.iter().map(|s| (*s).to_owned()).collect(), context: None, bonus: 0.0 }
    }

    fn m() -> Matcher {
        Matcher::new(vec![
            entry(&["открой блокнот", "запусти блокнот", "open notepad"]),
            entry(&["открой {app}", "запусти {app}", "open {app}"]),
            entry(&["закрой {app}", "close {app}"]),
            entry(&["прокрути вниз на {n}", "scroll down {n}"]),
            entry(&["прокрути вниз", "scroll down"]),
            entry(&["напечатай {text}", "type {text}"]),
            entry(&["таймер на {duration}", "set a timer for {duration}"]),
        ])
    }

    #[test]
    fn exact_beats_generic() {
        let r = m().find("Открой блокнот", None, 3, MATCH_THRESHOLD);
        assert_eq!(r[0].owner, 0);
        assert!(r[0].score > 0.95);
        let r = m().find("открой калькулятор", None, 3, MATCH_THRESHOLD);
        assert_eq!(r[0].owner, 1);
        assert_eq!(r[0].slots["app"], "калькулятор");
    }

    #[test]
    fn morphology_fillers_and_typos() {
        assert_eq!(m().find("запустите, пожалуйста, блокнот", None, 1, MATCH_THRESHOLD)[0].owner, 0);
        assert_eq!(m().find("открой блакнот", None, 1, MATCH_THRESHOLD)[0].owner, 0);
        let r = m().find("закрой браузер", None, 1, MATCH_THRESHOLD);
        assert_eq!((r[0].owner, r[0].slots["app"].as_str()), (2, "браузер"));
    }

    #[test]
    fn numeric_slots_are_validated() {
        let r = m().find("прокрути вниз на пять", None, 1, MATCH_THRESHOLD);
        assert_eq!((r[0].owner, r[0].slots["n"].as_str()), (3, "5"));
        let r = m().find("прокрути вниз", None, 1, MATCH_THRESHOLD);
        assert_eq!(r[0].owner, 4);
        let r = m().find("поставь таймер на 5 минут", None, 1, MATCH_THRESHOLD);
        assert_eq!((r[0].owner, r[0].slots["duration"].as_str()), (6, "300"));
    }

    #[test]
    fn text_slots_keep_case() {
        let r = m().find("Напечатай Привет, Мир", None, 1, MATCH_THRESHOLD);
        assert_eq!(r[0].slots["text"], "Привет, Мир");
    }

    #[test]
    fn unrelated_text_does_not_match() {
        assert!(m().find("какая сегодня погода в Москве", None, 1, MATCH_THRESHOLD).is_empty());
        assert!(m().find("", None, 1, MATCH_THRESHOLD).is_empty());
    }

    #[test]
    fn context_filters() {
        let mut e = entry(&["новая вкладка"]);
        e.context = Some(AppContext { processes: vec!["chrome".into()], title_contains: vec![] });
        let mx = Matcher::new(vec![e]);
        assert!(mx.find("новая вкладка", None, 1, MATCH_THRESHOLD).is_empty());
        let w = WindowInfo {
            handle: "1".into(),
            title: "x".into(),
            process: "chrome.exe".into(),
            pid: 1,
            active: true,
            minimized: false,
        };
        assert_eq!(mx.find("новая вкладка", Some(&w), 1, MATCH_THRESHOLD).len(), 1);
    }
}
