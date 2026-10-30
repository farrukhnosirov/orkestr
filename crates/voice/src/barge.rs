//! Voice barge-in (ADR-0053): a low-latency stop-phrase spotter that runs
//! while the assistant speaks or works, without the wake word.
//!
//! - [`StopPhrases`] classifies recognized text: configured hard / soft
//!   phrases, synonyms in short utterances («ну хватит»), truncated last words
//!   of partial results («хват» → «хватит»), and **self-echo rejection** (a
//!   phrase the assistant is saying right now is never a hit).
//! - [`BargeSpotter`] is one session fed with the shared microphone's 16 kHz
//!   PCM: either a Vosk grammar recognizer restricted to the stop phrases
//!   (fast, preferred) or, without a Vosk model, short voiced windows that the
//!   caller transcribes with an installed offline engine. Both are behind an
//!   energy gate; Vosk final results also need a minimum confidence.
//!
//! Audio is never stored: chunks are dropped after recognition.

use orkestr_protocol::methods::voice::{BargeEngine, BargeHit, BargeInConfig, StopKind};

use crate::audio::SAMPLE_RATE;

/// Utterances longer than this many words are conversation, not a stop command.
const MAX_UTTERANCE_WORDS: usize = 5;
/// Words besides the phrase («ну», «джарвис», «уже») tolerated in an utterance.
const MAX_EXTRA_WORDS: usize = 2;
/// A truncated last word must have at least this many letters…
const MIN_PREFIX_CHARS: usize = 4;
/// …and cover at least this share (percent) of the phrase word.
const MIN_PREFIX_PCT: usize = 60;
/// Marker for Vosk's out-of-grammar token.
const UNK: &str = "<unk>";

/// Normalizes text into lower-case word tokens (`ё` → `е`, punctuation dropped, `[unk]` kept as a marker).
#[must_use]
pub fn tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|w| {
            if w.eq_ignore_ascii_case("[unk]") {
                return Some(UNK.to_owned());
            }
            let t: String = w
                .chars()
                .flat_map(char::to_lowercase)
                .map(|c| if c == 'ё' { 'е' } else { c })
                .filter(|c| c.is_alphanumeric() || *c == '\'')
                .collect();
            (!t.is_empty()).then_some(t)
        })
        .collect()
}

#[derive(Debug, Clone)]
struct Entry {
    phrase: String,
    words: Vec<String>,
    kind: StopKind,
}

/// A matched stop phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopMatch {
    /// The configured phrase.
    pub phrase: String,
    /// Hard / soft.
    pub kind: StopKind,
}

/// The configured stop phrases and their classifier.
#[derive(Debug, Clone)]
pub struct StopPhrases {
    entries: Vec<Entry>,
}

fn word_matches(heard: &str, phrase_word: &str, allow_prefix: bool) -> bool {
    if heard == phrase_word {
        return true;
    }
    if !allow_prefix {
        return false;
    }
    let (h, p) = (heard.chars().count(), phrase_word.chars().count());
    h >= MIN_PREFIX_CHARS && h < p && h * 100 >= p * MIN_PREFIX_PCT && phrase_word.starts_with(heard)
}

fn contains_seq(hay: &[String], needle: &[String]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

impl StopPhrases {
    /// Phrases from lists (blank / duplicate entries are skipped; a phrase in both lists is hard).
    #[must_use]
    pub fn new(hard: &[String], soft: &[String]) -> Self {
        let mut entries: Vec<Entry> = Vec::new();
        for (list, kind) in [(hard, StopKind::Hard), (soft, StopKind::Soft)] {
            for p in list {
                let words = tokens(p);
                if words.is_empty() || words.iter().any(|w| w == UNK) || entries.iter().any(|e| e.words == words) {
                    continue;
                }
                entries.push(Entry { phrase: p.trim().to_owned(), words, kind });
            }
        }
        // Longer phrases first («не надо» before «надо»), hard before soft.
        entries.sort_by(|a, b| {
            b.words.len().cmp(&a.words.len()).then((a.kind == StopKind::Soft).cmp(&(b.kind == StopKind::Soft)))
        });
        Self { entries }
    }

    /// From the barge-in configuration.
    #[must_use]
    pub fn from_config(c: &BargeInConfig) -> Self {
        Self::new(&c.hard_phrases, &c.soft_phrases)
    }

    /// Whether there is nothing to listen for.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Vosk grammar: every phrase plus `[unk]` (absorbs other speech).
    /// `latin = false` leaves out Latin-script phrases (not in a Russian model's vocabulary).
    #[must_use]
    pub fn grammar(&self, latin: bool) -> String {
        let mut v: Vec<String> = self
            .entries
            .iter()
            .filter(|e| latin || !e.words.iter().any(|w| w.chars().any(|c| c.is_ascii_alphabetic())))
            .map(|e| e.words.join(" "))
            .collect();
        v.push("[unk]".into());
        serde_json::to_string(&v).unwrap_or_else(|_| "[\"[unk]\"]".into())
    }

    /// Classifies recognized text. `spoken` is what the assistant is saying
    /// right now: a phrase contained in it is self-echo and never matches.
    #[must_use]
    pub fn classify(&self, heard: &str, spoken: Option<&str>) -> Option<StopMatch> {
        let words = tokens(heard);
        let real = words.iter().filter(|w| *w != UNK).count();
        if real == 0 || words.len() > MAX_UTTERANCE_WORDS {
            return None;
        }
        let echo = spoken.map(tokens).unwrap_or_default();
        let mut best: Option<&Entry> = None;
        'entries: for e in &self.entries {
            let n = e.words.len();
            if n > words.len() {
                continue;
            }
            for start in 0..=words.len() - n {
                let ok = (0..n).all(|i| {
                    let at = start + i;
                    word_matches(&words[at], &e.words[i], at + 1 == words.len())
                });
                if !ok {
                    continue;
                }
                if words.len() - n > MAX_EXTRA_WORDS {
                    continue 'entries;
                }
                // Self-echo: the assistant is saying this phrase (or everything that was heard).
                let heard_real: Vec<String> = words.iter().filter(|w| *w != UNK).cloned().collect();
                if contains_seq(&echo, &e.words) || contains_seq(&echo, &heard_real) {
                    continue 'entries;
                }
                let better = match best {
                    None => true,
                    Some(b) => b.kind == StopKind::Soft && e.kind == StopKind::Hard,
                };
                if better {
                    best = Some(e);
                }
                continue 'entries;
            }
        }
        best.map(|e| StopMatch { phrase: e.phrase.clone(), kind: e.kind })
    }
}

/// Tuning derived from the sensitivity (0..=100).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    /// Chunks quieter than this (dBFS) do not open the energy gate.
    pub gate_db: f32,
    /// Minimum confidence of a Vosk final result.
    pub min_conf: f32,
    /// A stop phrase counts once the speaker paused this long after it (ms of
    /// audio): «стоп» followed by a pause, not «Оркестр…» going on.
    pub pause_ms: u32,
}

impl Tuning {
    /// Higher sensitivity: quieter speech, lower confidence and shorter pauses accepted.
    #[must_use]
    pub fn from_sensitivity(s: u8) -> Self {
        let s = f32::from(s.min(100)) / 100.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Self { gate_db: -32.0 - 22.0 * s, min_conf: 0.9 - 0.4 * s, pause_ms: (260.0 - 120.0 * s).round() as u32 }
    }
}

/// RMS level of 16-bit PCM in dBFS (−100 for silence).
#[must_use]
pub fn rms_db(pcm: &[i16]) -> f32 {
    if pcm.is_empty() {
        return -100.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let mean = pcm.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>() / pcm.len() as f64;
    let rms = mean.sqrt() / 32768.0;
    #[allow(clippy::cast_possible_truncation)]
    let db = if rms <= 1e-5 { -100.0 } else { (20.0 * rms.log10()) as f32 };
    db
}

fn ms_of(samples: usize) -> u32 {
    u32::try_from(samples as u64 * 1000 / u64::from(SAMPLE_RATE)).unwrap_or(u32::MAX)
}

/// How long after the last voiced chunk a hit still counts (the word's tail).
const GATE_HANG_MS: u32 = 700;
/// A chunk this far (dB) below the utterance's peak is a pause, even above
/// the gate (residual echo of the assistant's own voice after echo cancellation).
const PAUSE_BELOW_PEAK_DB: f32 = 18.0;
/// Extra pause required after a soft phrase.
const SOFT_EXTRA_PAUSE_MS: u32 = 120;
/// The utterance peak is forgotten after this much quiet.
const PEAK_RESET_MS: u32 = 1000;
/// Fallback windows: silence that ends a window, and its maximum length.
const WINDOW_END_SILENCE_MS: u32 = 350;
const WINDOW_MAX_MS: u32 = 2000;
/// Audio kept before the gate opened (the word's onset).
const WINDOW_PREROLL_MS: u32 = 200;

/// A grammar final result stays acceptable this long (waiting for the pause).
const FINAL_WAIT_MS: u32 = 600;
/// A stop command is short: the voiced utterance around it may last at most this long.
const MAX_UTTERANCE_MS: u32 = 1600;
/// Silence that separates utterances.
const UTTERANCE_GAP_MS: u32 = 400;

/// The recognizer behind a session.
pub enum Recognizer {
    /// Vosk recognizer restricted to the stop phrases (a few ms per 100 ms
    /// chunk; a full-vocabulary decoder costs ~20× more and is not run).
    Vosk(orkestr_platform::vosk::VoskStream),
    /// Voiced windows handed to the caller for transcription.
    Window,
}

impl std::fmt::Debug for Recognizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Vosk(_) => "Vosk",
            Self::Window => "Window",
        })
    }
}

/// What feeding a chunk produced.
#[derive(Debug, Default, PartialEq)]
pub struct FeedOutcome {
    /// A stop phrase was heard.
    pub hit: Option<BargeHit>,
    /// (Window engine) a voiced window to transcribe and pass to [`BargeSpotter::classify_text`].
    pub window: Option<Vec<i16>>,
}

/// One barge-in session.
#[derive(Debug)]
pub struct BargeSpotter {
    phrases: StopPhrases,
    tuning: Tuning,
    rec: Recognizer,
    /// Audio fed so far, samples.
    pos: usize,
    /// Position of the last voiced chunk (above the gate and near the utterance's peak).
    loud_at: Option<usize>,
    /// Loudest chunk of the current utterance, dBFS.
    peak_db: f32,
    /// A matching grammar partial waiting to become stable: (match, heard, since).
    pending: Option<(StopMatch, String, usize)>,
    /// A matching grammar final waiting for verification: (match, heard, confidence, at).
    final_pending: Option<(StopMatch, String, f32, usize)>,
    /// Start of the current voiced utterance.
    utter_start: Option<usize>,
    /// Window engine: audio of the current window and the trailing silence.
    window: Vec<i16>,
    in_window: bool,
    silent_ms: u32,
}

impl BargeSpotter {
    /// A session over `rec`.
    #[must_use]
    pub fn new(phrases: StopPhrases, sensitivity: u8, rec: Recognizer) -> Self {
        Self {
            phrases,
            tuning: Tuning::from_sensitivity(sensitivity),
            rec,
            pos: 0,
            loud_at: None,
            peak_db: -100.0,
            pending: None,
            final_pending: None,
            utter_start: None,
            window: Vec::new(),
            in_window: false,
            silent_ms: 0,
        }
    }

    /// The engine kind.
    #[must_use]
    pub fn engine(&self) -> BargeEngine {
        match self.rec {
            Recognizer::Vosk(_) => BargeEngine::VoskGrammar,
            Recognizer::Window => BargeEngine::LocalWindow,
        }
    }

    /// Speech was heard recently and the speaker has paused since: a stop
    /// phrase may be accepted now.
    /// Soft phrases («окей», «всё») are short, common words that other speech
    /// is easily forced onto: they need a longer pause.
    fn gate_open(&self, kind: StopKind) -> bool {
        let pause = self.tuning.pause_ms + if kind == StopKind::Soft { SOFT_EXTRA_PAUSE_MS } else { 0 };
        let short =
            self.utter_start.zip(self.loud_at).is_some_and(|(s, e)| ms_of(e.saturating_sub(s)) <= MAX_UTTERANCE_MS);
        short
            && self.loud_at.is_some_and(|at| {
                let quiet = ms_of(self.pos.saturating_sub(at));
                quiet <= GATE_HANG_MS && quiet >= pause
            })
    }

    fn hit(&mut self, m: StopMatch, heard: String, confidence: f32) -> BargeHit {
        self.pending = None;
        self.final_pending = None;
        self.utter_start = None;
        if let Recognizer::Vosk(grammar) = &mut self.rec {
            grammar.reset();
        }
        self.window.clear();
        self.in_window = false;
        BargeHit { phrase: m.phrase, kind: m.kind, heard, confidence }
    }

    /// Feeds the next chunk of 16 kHz mono PCM.
    pub fn feed(&mut self, pcm: &[i16], spoken: Option<&str>) -> FeedOutcome {
        let db = rms_db(pcm);
        let loud = db >= self.tuning.gate_db;
        self.pos += pcm.len();
        if self.loud_at.is_none_or(|at| ms_of(self.pos - at) > PEAK_RESET_MS) {
            self.peak_db = -100.0;
        }
        if loud {
            self.peak_db = self.peak_db.max(db);
            if db >= self.peak_db - PAUSE_BELOW_PEAK_DB {
                let chunk_start = self.pos - pcm.len();
                let gap = self.loud_at.is_none_or(|at| ms_of(chunk_start.saturating_sub(at)) >= UTTERANCE_GAP_MS);
                if gap {
                    self.utter_start = Some(self.pos - pcm.len());
                }
                self.loud_at = Some(self.pos);
            }
        }
        match &mut self.rec {
            Recognizer::Vosk(grammar) => {
                use orkestr_platform::vosk::VoskEvent;
                match grammar.accept(pcm) {
                    VoskEvent::Final { text, words } => {
                        self.pending = None;
                        if let Some(m) = self.phrases.classify(&text, spoken) {
                            let conf = words
                                .iter()
                                .filter(|w| w.word != "[unk]")
                                .map(|w| w.conf)
                                .fold(f32::INFINITY, f32::min);
                            let conf = if conf.is_finite() { conf } else { 1.0 };
                            if conf >= self.tuning.min_conf {
                                self.final_pending = Some((m, text, conf, self.pos));
                            }
                        }
                    }
                    VoskEvent::Partial(text) => {
                        let m = self.phrases.classify(&text, spoken);
                        let same = matches!((&m, &self.pending), (Some(a), Some((b, _, _))) if a.phrase == b.phrase);
                        if !same {
                            self.pending = m.map(|m| (m, text, self.pos));
                        }
                    }
                    VoskEvent::None => {}
                }
                if let Some((m, heard, conf, at)) = self.final_pending.clone() {
                    if self.gate_open(m.kind) {
                        return FeedOutcome { hit: Some(self.hit(m, heard, conf)), window: None };
                    }
                    if ms_of(self.pos - at) > FINAL_WAIT_MS {
                        self.final_pending = None;
                    }
                }
                if let Some((m, heard, _)) = self.pending.clone() {
                    if self.gate_open(m.kind) {
                        return FeedOutcome { hit: Some(self.hit(m, heard, 1.0)), window: None };
                    }
                }
                FeedOutcome::default()
            }
            Recognizer::Window => self.feed_window(pcm, loud),
        }
    }

    fn feed_window(&mut self, pcm: &[i16], loud: bool) -> FeedOutcome {
        let preroll = (SAMPLE_RATE * WINDOW_PREROLL_MS / 1000) as usize;
        if !self.in_window {
            self.window.extend_from_slice(pcm);
            if self.window.len() > preroll + pcm.len() {
                let cut = self.window.len() - preroll - pcm.len();
                self.window.drain(..cut);
            }
            if loud {
                self.in_window = true;
                self.silent_ms = 0;
            }
            return FeedOutcome::default();
        }
        self.window.extend_from_slice(pcm);
        self.silent_ms = if loud { 0 } else { self.silent_ms + ms_of(pcm.len()) };
        if self.silent_ms >= WINDOW_END_SILENCE_MS || ms_of(self.window.len()) >= WINDOW_MAX_MS {
            self.in_window = false;
            self.silent_ms = 0;
            return FeedOutcome { hit: None, window: Some(std::mem::take(&mut self.window)) };
        }
        FeedOutcome::default()
    }

    /// Classifies the transcript of a window from [`FeedOutcome::window`].
    pub fn classify_text(&mut self, text: &str, spoken: Option<&str>) -> Option<BargeHit> {
        let m = self.phrases.classify(text, spoken)?;
        Some(self.hit(m, text.trim().to_owned(), 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phrases() -> StopPhrases {
        StopPhrases::from_config(&BargeInConfig::default())
    }

    fn kind(text: &str) -> Option<StopKind> {
        phrases().classify(text, None).map(|m| m.kind)
    }

    #[test]
    fn synonyms_are_hard_or_soft() {
        for h in
            ["стоп", "Хватит!", "прекрати", "отмена", "перестань", "не надо", "достаточно", "Stop", "enough", "cancel"]
        {
            assert_eq!(kind(h), Some(StopKind::Hard), "{h}");
        }
        for s in ["окей", "ок", "тихо", "тишина", "замолчи", "заткнись", "помолчи", "всё", "все", "shut up"]
        {
            assert_eq!(kind(s), Some(StopKind::Soft), "{s}");
        }
    }

    #[test]
    fn short_utterances_around_a_phrase_match_and_hard_wins() {
        assert_eq!(kind("ну хватит уже"), Some(StopKind::Hard));
        assert_eq!(kind("джарвис стоп"), Some(StopKind::Hard));
        assert_eq!(kind("окей стоп"), Some(StopKind::Hard), "hard wins over soft");
        assert_eq!(kind("[unk] стоп"), Some(StopKind::Hard));
        let m = phrases().classify("не надо", None).unwrap();
        assert_eq!(m.phrase, "не надо");
    }

    #[test]
    fn truncated_last_word_matches_but_short_fragments_do_not() {
        assert_eq!(kind("хват"), Some(StopKind::Hard));
        assert_eq!(kind("прекра"), Some(StopKind::Hard));
        assert_eq!(kind("замол"), Some(StopKind::Soft));
        assert_eq!(kind("ст"), None);
        assert_eq!(kind("хв"), None);
        // only the last word may be truncated
        assert_eq!(kind("хват уже"), None);
    }

    #[test]
    fn noise_and_conversation_are_rejected() {
        for n in [
            "",
            "[unk]",
            "[unk] [unk]",
            "открой ютуб",
            "стопка книг лежит на столе рядом",
            "мне хватит одного яблока на ужин сегодня",
        ] {
            assert_eq!(kind(n), None, "{n}");
        }
    }

    #[test]
    fn self_echo_is_rejected() {
        let p = phrases();
        assert!(p.classify("всё", Some("Всё готово, сэр.")).is_none());
        assert!(p.classify("достаточно", Some("Места на диске достаточно.")).is_none());
        assert!(p.classify("стоп", Some("Всё готово, сэр.")).is_some(), "a phrase Jarvis is not saying still counts");
        assert!(p.classify("стоп", None).is_some());
    }

    #[test]
    fn custom_lists_and_grammar() {
        let p = StopPhrases::new(&["Хорош".into(), " ".into()], &["ладно".into(), "хорош".into()]);
        assert_eq!(p.classify("хорош", None).unwrap().kind, StopKind::Hard, "in both lists → hard, once");
        let g: Vec<String> = serde_json::from_str(&p.grammar(false)).unwrap();
        assert_eq!(g, vec!["хорош", "ладно", "[unk]"]);
        let en = StopPhrases::new(&["stop".into(), "стоп".into()], &[]);
        assert_eq!(en.grammar(false), r#"["стоп","[unk]"]"#);
        assert_eq!(en.grammar(true), r#"["stop","стоп","[unk]"]"#);
        assert!(StopPhrases::new(&[], &[]).is_empty());
    }

    #[test]
    fn tuning_follows_sensitivity() {
        let lo = Tuning::from_sensitivity(0);
        let hi = Tuning::from_sensitivity(100);
        assert!(hi.gate_db < lo.gate_db && hi.min_conf < lo.min_conf && hi.pause_ms < lo.pause_ms);
        assert_eq!(Tuning::from_sensitivity(50).pause_ms, 200);
    }

    fn tone(ms: u32, amp: f32) -> Vec<i16> {
        let n = (SAMPLE_RATE * ms / 1000) as usize;
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        (0..n).map(|i| ((i as f32 * 0.07).sin() * amp * 32767.0) as i16).collect()
    }

    #[test]
    fn window_engine_cuts_voiced_windows_and_classifies() {
        let mut s = BargeSpotter::new(phrases(), 50, Recognizer::Window);
        assert_eq!(s.engine(), BargeEngine::LocalWindow);
        assert_eq!(s.feed(&tone(100, 0.0), None), FeedOutcome::default());
        let mut window = None;
        for chunk in [tone(100, 0.3), tone(100, 0.3), tone(100, 0.0), tone(100, 0.0), tone(100, 0.0), tone(100, 0.0)] {
            let out = s.feed(&chunk, None);
            if out.window.is_some() {
                window = out.window;
            }
        }
        let w = window.expect("a window after 350 ms of silence");
        assert!(w.len() >= (SAMPLE_RATE / 1000 * 500) as usize, "speech + silence + preroll: {}", w.len());
        let hit = s.classify_text("Стоп.", Some("Открываю YouTube, сэр.")).unwrap();
        assert_eq!((hit.kind, hit.phrase.as_str()), (StopKind::Hard, "стоп"));
        assert!(s.classify_text("открой ютуб", None).is_none());
    }

    #[test]
    fn rms_levels() {
        assert!(rms_db(&[]) <= -100.0);
        assert!(rms_db(&tone(100, 0.0)) <= -100.0);
        let db = rms_db(&tone(100, 0.5));
        assert!((-10.0..-5.0).contains(&db), "{db}");
    }
}
