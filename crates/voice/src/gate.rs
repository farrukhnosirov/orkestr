//! Noise rejection around speech recognition (ADR-0057): garbage heard from
//! background noise, music or the TV must never become a command.
//!
//! Three layers, all cheap and local:
//! 1. **Speech activity before STT** ([`speech_activity`]): 20 ms frames, an
//!    adaptive noise floor (a low percentile of the clip's own frame levels)
//!    and spectral flatness. Steady noise (fans, hum, white/pink noise),
//!    clicks and silence never reach an engine. whisper.cpp additionally runs
//!    the Silero VAD model when it is installed (`whisper-vad-silero`).
//! 2. **Decoder confidence** ([`Segment`], [`confidence`]): whisper's
//!    `avg_logprob` / `no_speech_prob` per segment (the OpenAI rules) give a
//!    0..1 confidence; very low ones are rejected here, the rest is passed on
//!    so the router can demand more before free text reaches an AI.
//! 3. **Text** ([`check_text`]): whisper's well-known hallucinations
//!    («Продолжение следует…», «Субтитры сделал…», «Спасибо за просмотр»,
//!    `[музыка]`, ♪), annotations only, and runaway repetitions.

use orkestr_protocol::methods::voice::SttRejectReason;

/// Frame length (20 ms at 16 kHz).
const FRAME: usize = 320;
/// FFT size for the spectral features (the frame zero-padded).
const NFFT: usize = 512;
/// Frames below this level are silence whatever the noise floor (dBFS).
const ABS_MIN_DB: f32 = -52.0;
/// A clip whose loudest frame is below this is silence (dBFS).
const PEAK_MIN_DB: f32 = -42.0;
/// A speech frame is at least this much above the clip's noise floor.
const ABOVE_FLOOR_DB: f32 = 7.0;
/// Speech is not flat: frames with a spectral flatness above this are noise.
const MAX_FLATNESS: f32 = 0.42;
/// Minimum total speech for an utterance (ms).
const MIN_SPEECH_MS: u32 = 220;

/// What the activity analysis found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Activity {
    /// Milliseconds of speech-like frames.
    pub speech_ms: u32,
    /// The clip's noise floor (dBFS).
    pub floor_db: f32,
    /// The loudest frame (dBFS).
    pub peak_db: f32,
    /// Median level of the speech-like frames above the floor (dB).
    pub snr_db: f32,
}

impl Activity {
    /// Whether the clip contains enough speech to transcribe.
    #[must_use]
    pub fn has_speech(&self) -> bool {
        self.peak_db >= PEAK_MIN_DB && self.speech_ms >= MIN_SPEECH_MS
    }
}

fn db(power: f32) -> f32 {
    10.0 * power.max(1e-12).log10()
}

/// In-place radix-2 FFT (`re`/`im` of length `NFFT`).
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        #[allow(clippy::cast_precision_loss)]
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

/// Spectral flatness (geometric / arithmetic mean of the power) over
/// 250–4000 Hz: ≈1 for white noise, low for voiced speech and tones.
fn flatness(frame: &[f32]) -> f32 {
    let mut re = vec![0.0f32; NFFT];
    let mut im = vec![0.0f32; NFFT];
    #[allow(clippy::cast_precision_loss)]
    for (i, s) in frame.iter().enumerate() {
        // Hann window
        let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FRAME as f32 - 1.0)).cos();
        re[i] = s * w;
    }
    fft(&mut re, &mut im);
    // bins: 16000 / 512 = 31.25 Hz
    let (lo, hi) = (8, 128);
    let mut log_sum = 0.0f64;
    let mut sum = 0.0f64;
    for k in lo..hi {
        let p = f64::from(re[k] * re[k] + im[k] * im[k]) + 1e-10;
        log_sum += p.ln();
        sum += p;
    }
    #[allow(clippy::cast_precision_loss)]
    let n = (hi - lo) as f64;
    #[allow(clippy::cast_possible_truncation)]
    let f = ((log_sum / n).exp() / (sum / n)) as f32;
    f
}

/// Analyses 16 kHz mono PCM for speech activity.
#[must_use]
pub fn speech_activity(samples: &[i16]) -> Activity {
    let frames: Vec<Vec<f32>> =
        samples.as_chunks::<FRAME>().0.iter().map(|c| c.iter().map(|&s| f32::from(s) / 32768.0).collect()).collect();
    if frames.is_empty() {
        return Activity { speech_ms: 0, floor_db: -120.0, peak_db: -120.0, snr_db: 0.0 };
    }
    #[allow(clippy::cast_precision_loss)]
    let levels: Vec<f32> = frames.iter().map(|f| db(f.iter().map(|x| x * x).sum::<f32>() / FRAME as f32)).collect();
    let mut sorted = levels.clone();
    sorted.sort_by(f32::total_cmp);
    let floor = sorted[sorted.len() * 15 / 100];
    let peak = sorted[sorted.len() - 1];
    let mut speech = 0u32;
    let mut snrs = Vec::new();
    for (f, &l) in frames.iter().zip(&levels) {
        if l < ABS_MIN_DB || l < floor + ABOVE_FLOOR_DB {
            continue;
        }
        if flatness(f) > MAX_FLATNESS {
            continue;
        }
        speech += 1;
        snrs.push(l - floor);
    }
    snrs.sort_by(f32::total_cmp);
    let snr = snrs.get(snrs.len() / 2).copied().unwrap_or(0.0);
    Activity { speech_ms: speech * 20, floor_db: floor, peak_db: peak, snr_db: snr }
}

/// One decoded whisper segment.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// Text.
    pub text: String,
    /// Mean token log-probability.
    pub avg_logprob: f32,
    /// Probability of "no speech" for the window.
    pub no_speech_prob: f32,
    /// Number of tokens (weights the average).
    pub tokens: usize,
}

/// Below this mean log-probability a transcript is rejected outright.
pub const REJECT_LOGPROB: f32 = -1.15;
/// The same with `NoiseRejection::Strict`.
pub const STRICT_LOGPROB: f32 = -0.8;
/// `NoiseRejection::Strict`: speech must stand this far above the noise floor (dB).
pub const STRICT_SNR_DB: f32 = 12.0;

/// Whether whisper just read its prompt back (it does that on near-silence):
/// five or more consecutive prompt words.
#[must_use]
pub fn copies_prompt(text: &str, prompt: &str) -> bool {
    let t = normalize(text);
    let p = normalize(prompt);
    let words: Vec<&str> = t.split(' ').filter(|w| !w.is_empty()).collect();
    words.len() >= 5 && words.windows(5).any(|w| format!(" {p} ").contains(&format!(" {} ", w.join(" "))))
}

/// Keeps the segments whisper itself would call speech (OpenAI rule:
/// `no_speech_prob > 0.6` with `avg_logprob < -1` is silence) and returns
/// `(text, confidence 0..1)`; `None` when nothing is left.
#[must_use]
pub fn confidence(segments: &[Segment]) -> Option<(String, f32)> {
    let kept: Vec<&Segment> = segments
        .iter()
        .filter(|s| !s.text.trim().is_empty())
        .filter(|s| !(s.no_speech_prob > 0.6 && s.avg_logprob < -1.0))
        .collect();
    if kept.is_empty() {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let (sum, n) = kept.iter().fold((0.0f32, 0.0f32), |(a, n), s| {
        let w = s.tokens.max(1) as f32;
        (a + s.avg_logprob * w, n + w)
    });
    let avg = sum / n.max(1.0);
    let text = kept.iter().map(|s| s.text.trim()).collect::<Vec<_>>().join(" ");
    Some((text, avg.exp().clamp(0.0, 1.0)))
}

/// Whole transcripts whisper invents for noise / music / silence (normalized:
/// lower case, `ё`→`е`, letters, digits and single spaces).
const EXACT: &[&str] = &[
    "музыка",
    "музыкальная заставка",
    "веселая музыка",
    "спокойная музыка",
    "грустная музыка",
    "динамичная музыка",
    "аплодисменты",
    "смех",
    "тишина",
    "шум",
    "звонок",
    "вздох",
    "кашель",
    "без звука",
    "а",
    "э",
    "м",
    "ну",
    "music",
    "applause",
    "laughter",
    "silence",
    "noise",
    "you",
    "oh",
    "uh",
    "um",
    "hmm",
];

/// Fragments that only occur in whisper's subtitle/outro hallucinations.
const CONTAINS: &[&str] = &[
    "продолжение следует",
    "субтитры",
    "субтитров",
    "редактор субтитров",
    "корректор",
    "спасибо за просмотр",
    "спасибо за внимание",
    "благодарю за просмотр",
    "подписывайтесь на канал",
    "подписывайся на канал",
    "подпишитесь на канал",
    "ставьте лайки",
    "ставьте лайк",
    "до новых встреч",
    "до встречи в следующем",
    "увидимся в следующем",
    "в следующем видео",
    "dimatorzok",
    "диматорзок",
    "игорь негода",
    "amara org",
    "амара орг",
    "thank you for watching",
    "thanks for watching",
    "please subscribe",
    "subscribe to",
    "subtitles by",
    "captions by",
    "transcribed by",
    "see you next time",
    "see you in the next",
    "www ",
    " com",
];

/// Normalizes for matching.
#[must_use]
pub fn normalize(text: &str) -> String {
    let lower = text.to_lowercase().replace('ё', "е");
    let mut out = String::with_capacity(lower.len());
    let mut space = true;
    for c in lower.chars() {
        if c.is_alphanumeric() {
            out.push(c);
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out.trim().to_owned()
}

/// Removes `[музыка]`, `(смех)`, `*вздох*`, `♪ … ♪` annotations.
#[must_use]
pub fn strip_annotations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut close: Option<char> = None;
    for c in text.chars() {
        match close {
            Some(end) if c == end => close = None,
            Some(_) => {}
            None => match c {
                '[' => close = Some(']'),
                '(' => close = Some(')'),
                '*' => close = Some('*'),
                '♪' | '♫' | '♬' | '🎵' | '🎶' => {}
                _ => out.push(c),
            },
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn repetitive(words: &[&str]) -> bool {
    if words.len() < 4 {
        return false;
    }
    let mut counts = std::collections::HashMap::<&str, usize>::new();
    for w in words {
        *counts.entry(w).or_default() += 1;
    }
    let max = counts.values().copied().max().unwrap_or(0);
    // «да, нет, нет, нет, нет, нет» / «спасибо спасибо спасибо спасибо»
    if max >= 4 && max * 2 >= words.len() {
        return true;
    }
    // the same bigram three times or more
    let mut bigrams = std::collections::HashMap::<(&str, &str), usize>::new();
    for p in words.windows(2) {
        *bigrams.entry((p[0], p[1])).or_default() += 1;
    }
    bigrams.values().any(|&n| n >= 3) && counts.len() * 2 <= words.len()
}

/// The same sentence of 2+ words twice or more («Старжонный дубный рюк.
/// Старжонный дубный рюк.»): whisper looping on babble / TV.
fn repeated_sentences(text: &str) -> bool {
    let mut seen = std::collections::HashSet::new();
    text.split(['.', '!', '?', '\n', '…']).map(normalize).filter(|s| s.split(' ').count() >= 2).any(|s| !seen.insert(s))
}

/// Checks a transcript: `Ok(cleaned)` or the reason it is not speech.
pub fn check_text(text: &str) -> Result<String, SttRejectReason> {
    let cleaned = strip_annotations(text);
    let norm = normalize(&cleaned);
    if norm.is_empty() || !norm.chars().any(char::is_alphabetic) {
        return Err(if text.trim().is_empty() { SttRejectReason::NoSpeech } else { SttRejectReason::Hallucination });
    }
    if EXACT.contains(&norm.as_str()) {
        return Err(SttRejectReason::Hallucination);
    }
    let padded = format!(" {norm} ");
    if CONTAINS.iter().any(|f| padded.contains(f)) {
        return Err(SttRejectReason::Hallucination);
    }
    let words: Vec<&str> = norm.split(' ').collect();
    if repetitive(&words) || repeated_sentences(&cleaned) {
        return Err(SttRejectReason::Hallucination);
    }
    Ok(cleaned.trim().to_owned())
}

/// Words that help whisper with the assistant's vocabulary (initial prompt).
const PROMPT_RU: &str = "Джарвис, Оркестр. Открой ютуб, сделай громче, выключи звук, запусти тесты в проекте, \
                         покажи статус, какая погода, напиши в телеграм.";
const PROMPT_EN: &str = "Jarvis, Orkestr. Open YouTube, turn it up, mute, run the tests, show the status.";

/// The initial prompt for whisper: the wake phrases first, then common
/// command vocabulary in the utterance's language (short — whisper copies a
/// long prompt into silence).
#[must_use]
pub fn initial_prompt(language: Option<&str>, wake_phrases: &[String]) -> String {
    let base = if language.is_some_and(|l| l.starts_with("en")) { PROMPT_EN } else { PROMPT_RU };
    let extra: Vec<&str> = wake_phrases
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty() && p.chars().count() <= 32 && !base.to_lowercase().contains(&p.to_lowercase()))
        .take(4)
        .collect();
    if extra.is_empty() {
        base.to_owned()
    } else {
        format!("{}. {base}", extra.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, secs: f32, amp: f32) -> Vec<i16> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
        (0..(16_000.0 * secs) as usize)
            .map(|i| (amp * 32767.0 * (2.0 * std::f32::consts::PI * freq * i as f32 / 16_000.0).sin()) as i16)
            .collect()
    }

    fn lcg_noise(n: usize, amp: f32) -> Vec<i16> {
        let mut x: u32 = 12345;
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                ((f32::from(u16::try_from(x >> 16).unwrap_or(0)) / 65535.0 - 0.5) * 2.0 * amp * 32767.0) as i16
            })
            .collect()
    }

    #[test]
    fn silence_and_steady_noise_have_no_speech() {
        assert!(!speech_activity(&vec![0; 32_000]).has_speech());
        assert!(!speech_activity(&lcg_noise(64_000, 0.3)).has_speech(), "white noise");
        assert!(!speech_activity(&tone(50.0, 3.0, 0.3)).has_speech(), "hum");
    }

    #[test]
    fn a_voice_like_burst_over_silence_is_speech() {
        // 1 s of a harmonic "voice" (150 Hz + harmonics) amplitude-modulated at 4 Hz, between pauses
        let mut v = vec![0i16; 8000];
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        v.extend((0..16_000).map(|i| {
            let t = i as f32 / 16_000.0;
            let env = 0.5 + 0.5 * (2.0 * std::f32::consts::PI * 4.0 * t).sin();
            let s: f32 = (1..8).map(|h| (2.0 * std::f32::consts::PI * 150.0 * h as f32 * t).sin() / h as f32).sum();
            (0.15 * env * s * 32767.0) as i16
        }));
        v.extend(vec![0i16; 8000]);
        let a = speech_activity(&v);
        assert!(a.has_speech(), "{a:?}");
    }

    #[test]
    fn whisper_hallucinations_are_rejected() {
        for t in [
            "",
            "[музыка]",
            "[Музыка] [Аплодисменты]",
            "(смех)",
            "♪ ♪ ♪",
            "Продолжение следует...",
            "Субтитры сделал DimaTorzok",
            "Спасибо за просмотр!",
            "Thank you for watching.",
            "Музыка",
            "Да, нет, нет, нет, нет, нет, нет.",
            "Старжонный дубный рюк.\n Старжонный дубный рюк.\n Сушить за етчим.",
            "Субтитры создавал DimaTorzok",
            "you",
            "...",
        ] {
            assert!(check_text(t).is_err(), "{t:?} must be rejected");
        }
        assert_eq!(check_text("").unwrap_err(), SttRejectReason::NoSpeech);
    }

    #[test]
    fn real_commands_pass_the_text_check() {
        for t in [
            "Джарвис, открой ютуб.",
            "Сделай громче",
            "Какая погода завтра в Москве?",
            "Выключи компьютер через час.",
            "Open the settings",
            "[музыка] Открой браузер",
            "нет, нет, не надо",
        ] {
            assert!(check_text(t).is_ok(), "{t:?} must pass");
        }
        assert_eq!(check_text("[музыка] Открой браузер").unwrap(), "Открой браузер");
    }

    #[test]
    fn confidence_follows_whisper_rules() {
        let seg = |text: &str, lp: f32, ns: f32| Segment {
            text: text.into(),
            avg_logprob: lp,
            no_speech_prob: ns,
            tokens: 5,
        };
        assert!(confidence(&[seg(" музыка", -1.4, 0.8)]).is_none(), "no-speech window");
        let (t, c) = confidence(&[seg(" открой ютуб", -0.1, 0.01)]).unwrap();
        assert_eq!(t, "открой ютуб");
        assert!(c > 0.85, "{c}");
        let (_, c) = confidence(&[seg(" смещая музыка", -0.9, 0.3)]).unwrap();
        assert!(c < 0.45, "{c}");
    }

    #[test]
    fn prompt_includes_wake_phrases_once() {
        let p = initial_prompt(Some("ru"), &["Пятница".into(), "Джарвис".into()]);
        assert!(p.starts_with("Пятница. Джарвис"), "{p}");
        assert_eq!(p.matches("Джарвис").count(), 1);
        assert!(initial_prompt(Some("en"), &[]).starts_with("Jarvis"));
        assert!(copies_prompt("Открой ютуб, сделай громче, выключи звук.", &p));
        assert!(!copies_prompt("Открой ютуб", &p));
    }
}
