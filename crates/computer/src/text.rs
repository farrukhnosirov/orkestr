//! Text normalization for the local matcher: lowercase, `ё→е`, punctuation
//! → spaces, filler words dropped, a light Russian / English stemmer,
//! verb synonyms folded into concepts (`открой`/`запусти`/`open` → `@open`),
//! number words (ru + en) and a Levenshtein similarity.

/// One token of an utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Normalized key (concept `@open`, or stem).
    pub key: String,
    /// The lowercase word.
    pub word: String,
    /// Byte span in the original text.
    pub start: usize,
    /// End byte.
    pub end: usize,
}

/// Words that never carry meaning in a command.
const FILLERS: &[&str] = &[
    "пожалуйста",
    "плиз",
    "ка",
    "мне",
    "можешь",
    "можете",
    "могла",
    "мог",
    "бы",
    "ли",
    "давай",
    "please",
    "pls",
    "could",
    "would",
    "you",
    "can",
    "kindly",
    "the",
    "a",
    "an",
    "орк",
    "оркестр",
    "orkestr",
    "джарвис",
    "jarvis",
    "эй",
    "hey",
    "ok",
    "окей",
];

/// Verb / noun synonyms folded into one concept (matched on stems).
/// Each row: concept, stems / words.
const CONCEPTS: &[(&str, &[&str])] = &[
    ("@open", &["откр", "открыв", "запуст", "запуска", "запуск", "open", "launch", "start", "run", "стартан"]),
    ("@close", &["закр", "закрыв", "выйд", "выход", "close", "quit", "exit"]),
    ("@switch", &["переключ", "перейд", "переход", "switch", "focus", "активир", "активируй"]),
    ("@show", &["покаж", "показ", "show", "display", "отобраз"]),
    ("@scroll", &["прокрут", "прокрути", "листа", "листн", "скрол", "scroll"]),
    ("@up", &["вверх", "наверх", "up", "выше"]),
    ("@down", &["вниз", "down", "ниж", "ниже"]),
    ("@type", &["напечат", "напиш", "введ", "набер", "впечат", "type", "write"]),
    ("@press", &["нажм", "нажми", "нажат", "жми", "press", "hit", "push"]),
    ("@search", &["найд", "поищ", "ищ", "поиск", "загугл", "search", "find", "look"]),
    ("@minimize", &["сверн", "свернут", "minimize", "minimise"]),
    ("@maximize", &["разверн", "развернут", "maximize", "maximise"]),
    ("@volume", &["громкост", "звук", "volume", "sound"]),
    ("@louder", &["громч", "погромч", "louder", "прибав", "увелич", "increas", "rais"]),
    ("@quieter", &["тиш", "потиш", "quieter", "убав", "уменьш", "decreas", "lower"]),
    ("@window", &["окн", "окош", "window"]),
    ("@page", &["страниц", "page"]),
    ("@screenshot", &["скриншот", "снимок", "скрин", "screenshot"]),
    ("@settings", &["настройк", "параметр", "settings", "setting"]),
    ("@folder", &["папк", "folder", "directory", "каталог"]),
    ("@file", &["файл", "file", "документ"]),
    ("@site", &["сайт", "website", "site"]),
    ("@new", &["нов", "new", "созда", "create", "сделай", "make"]),
    ("@delete", &["удал", "delete", "remove", "сотр"]),
    ("@copy", &["скопир", "копир", "копи", "copy"]),
    ("@paste", &["встав", "paste"]),
    ("@cut", &["выреж", "вырез", "cut"]),
    ("@undo", &["отмен", "undo"]),
    ("@redo", &["повтор", "redo"]),
    ("@save", &["сохран", "save"]),
    ("@select", &["выдел", "select", "выбер"]),
    ("@all", &["все", "весь", "всю", "all", "everything"]),
    ("@tab", &["вкладк", "таб", "tab"]),
    ("@next", &["следующ", "next", "дальш", "вперед"]),
    ("@previous", &["предыдущ", "previous", "prev", "назад", "back"]),
    ("@turnon", &["включ", "enable", "turn", "вруб"]),
    ("@turnoff", &["выключ", "disable", "выруб", "отключ"]),
    ("@play", &["игра", "воспроизвед", "play", "resume", "продолж"]),
    ("@pause", &["пауз", "pause", "останов", "stop", "стоп"]),
    ("@mute", &["mute", "беззвуч", "заглуш"]),
    ("@lock", &["заблокир", "блокир", "lock"]),
    ("@timer", &["таймер", "timer"]),
    ("@remind", &["напомн", "remind", "reminder", "напоминан"]),
    ("@kill", &["убе", "убит", "заверш", "kill", "terminat"]),
    ("@go", &["перейд", "иди", "go", "navigat"]),
    ("@in", &["в", "во", "на", "in", "on", "into", "at"]),
];

/// Lowercases, folds `ё`, keeps letters / digits, splits into words with spans.
#[must_use]
pub fn words(text: &str) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start = 0usize;
    for (i, ch) in text.char_indices() {
        let keep = ch.is_alphanumeric() || ch == '+' || ch == '\'' || ch == '.' && !cur.is_empty();
        if keep {
            if cur.is_empty() {
                start = i;
            }
            for l in ch.to_lowercase() {
                cur.push(if l == 'ё' { 'е' } else { l });
            }
        } else if !cur.is_empty() {
            push_word(&mut out, &mut cur, start, i);
        }
    }
    if !cur.is_empty() {
        push_word(&mut out, &mut cur, start, text.len());
    }
    out
}

fn push_word(out: &mut Vec<(String, usize, usize)>, cur: &mut String, start: usize, end: usize) {
    // A trailing dot is punctuation ("блокнот."), an inner one is a domain ("ya.ru").
    let mut w = std::mem::take(cur);
    let mut end = end;
    while w.ends_with('.') {
        w.pop();
        end -= 1;
    }
    if !w.is_empty() {
        out.push((w, start, end));
    }
}

fn is_cyrillic(s: &str) -> bool {
    s.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c))
}

const RU_SUFFIXES: &[&str] = &[
    "ывайте",
    "ивайте",
    "айтесь",
    "итесь",
    "ующий",
    "ающий",
    "ями",
    "ами",
    "ого",
    "его",
    "ому",
    "ему",
    "ыми",
    "ими",
    "ешь",
    "ете",
    "ите",
    "ать",
    "ять",
    "ить",
    "еть",
    "уть",
    "ыть",
    "ой",
    "ей",
    "ий",
    "ый",
    "ая",
    "яя",
    "ое",
    "ее",
    "ую",
    "юю",
    "ые",
    "ие",
    "ов",
    "ев",
    "ам",
    "ям",
    "ах",
    "ях",
    "ом",
    "ем",
    "ть",
    "те",
    "ай",
    "яй",
    "уй",
    "ся",
    "сь",
    "а",
    "я",
    "о",
    "е",
    "и",
    "ы",
    "у",
    "ю",
    "ь",
    "й",
];

/// Light stemmer: strips one common inflection ending, keeping ≥ 3 letters.
#[must_use]
pub fn stem(word: &str) -> String {
    let n = word.chars().count();
    if n <= 3 || word.chars().any(|c| c.is_ascii_digit()) {
        return word.to_owned();
    }
    if is_cyrillic(word) {
        // reflexive first ("переключись" → "переключи" → "переключ")
        let mut w = word.to_owned();
        for r in ["ся", "сь"] {
            if w.ends_with(r) && w.chars().count() > 5 {
                w.truncate(w.len() - r.len());
                break;
            }
        }
        for suf in RU_SUFFIXES {
            if w.ends_with(suf) && w.chars().count() - suf.chars().count() >= 3 {
                w.truncate(w.len() - suf.len());
                return w;
            }
        }
        return w;
    }
    for suf in ["ing", "ed", "es", "s"] {
        if word.ends_with(suf) && n - suf.len() >= 3 && !word.ends_with("ss") {
            return word[..word.len() - suf.len()].to_owned();
        }
    }
    word.to_owned()
}

/// Concept of a stem, if any (exact or prefix ≥ 4 chars in either direction).
#[must_use]
pub fn concept(word: &str, stemmed: &str) -> Option<&'static str> {
    for (c, list) in CONCEPTS {
        for s in *list {
            if *s == word || *s == stemmed {
                return Some(c);
            }
        }
    }
    for (c, list) in CONCEPTS {
        for s in *list {
            let sl = s.chars().count();
            if sl >= 4 && (stemmed.starts_with(s) || word.starts_with(s)) {
                return Some(c);
            }
        }
    }
    None
}

/// True for words that carry no meaning.
#[must_use]
pub fn is_filler(word: &str) -> bool {
    FILLERS.contains(&word)
}

/// Tokenizes `text` (fillers dropped; keys = concepts or stems).
#[must_use]
pub fn tokenize(text: &str) -> Vec<Token> {
    words(text)
        .into_iter()
        .filter(|(w, _, _)| !is_filler(w))
        .map(|(w, start, end)| {
            let s = stem(&w);
            let key = concept(&w, &s).map_or(s, str::to_owned);
            Token { key, word: w, start, end }
        })
        .collect()
}

/// Levenshtein distance on chars.
#[must_use]
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Similarity 0..1 (1 = equal).
#[must_use]
pub fn similarity(a: &str, b: &str) -> f32 {
    let n = a.chars().count().max(b.chars().count());
    if n == 0 {
        return 1.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let s = 1.0 - levenshtein(a, b) as f32 / n as f32;
    s
}

/// Token-level match cost: 0 exact, small for prefix / fuzzy, `None` = no match.
#[must_use]
pub fn token_cost(pattern: &str, input: &Token) -> Option<f32> {
    if pattern == input.key || pattern == input.word {
        return Some(0.0);
    }
    if pattern.starts_with('@') || input.key.starts_with('@') {
        return None;
    }
    let (p, k) = (pattern, input.key.as_str());
    let (pl, kl) = (p.chars().count(), k.chars().count());
    if pl >= 4 && kl >= 4 && (p.starts_with(k) || k.starts_with(p)) {
        return Some(0.05);
    }
    if pl >= 4 && kl >= 4 {
        let s = similarity(p, k).max(similarity(p, &input.word));
        if s >= 0.75 {
            return Some((1.0 - s) * 0.6);
        }
    }
    None
}

const RU_UNITS: &[(&str, u32)] = &[
    ("ноль", 0),
    ("один", 1),
    ("одну", 1),
    ("одна", 1),
    ("раз", 1),
    ("два", 2),
    ("две", 2),
    ("три", 3),
    ("четыре", 4),
    ("пять", 5),
    ("шесть", 6),
    ("семь", 7),
    ("восемь", 8),
    ("девять", 9),
    ("десять", 10),
    ("одиннадцать", 11),
    ("двенадцать", 12),
    ("тринадцать", 13),
    ("четырнадцать", 14),
    ("пятнадцать", 15),
    ("шестнадцать", 16),
    ("семнадцать", 17),
    ("восемнадцать", 18),
    ("девятнадцать", 19),
    ("двадцать", 20),
    ("тридцать", 30),
    ("сорок", 40),
    ("пятьдесят", 50),
    ("шестьдесят", 60),
    ("семьдесят", 70),
    ("восемьдесят", 80),
    ("девяносто", 90),
    ("сто", 100),
    ("полминуты", 30),
    // oblique forms («на десяти», «до двадцати», «из ста сорока четырех»)
    ("одного", 1),
    ("двух", 2),
    ("трех", 3),
    ("четырех", 4),
    ("пяти", 5),
    ("шести", 6),
    ("семи", 7),
    ("восьми", 8),
    ("девяти", 9),
    ("десяти", 10),
    ("пятнадцати", 15),
    ("двадцати", 20),
    ("тридцати", 30),
    ("сорока", 40),
    ("пятидесяти", 50),
    ("шестидесяти", 60),
    ("семидесяти", 70),
    ("восьмидесяти", 80),
    ("девяноста", 90),
    ("ста", 100),
    ("двести", 200),
    ("триста", 300),
    ("четыреста", 400),
    ("пятьсот", 500),
    ("тысяча", 1000),
    ("тысячу", 1000),
    // levels said as words («громкость на полную», «на минимум»)
    ("полную", 100),
    ("полной", 100),
    ("максимум", 100),
    ("максимальную", 100),
    ("макс", 100),
    ("половину", 50),
    ("минимум", 0),
    ("full", 100),
    ("max", 100),
    ("maximum", 100),
    ("half", 50),
    ("minimum", 0),
    ("zero", 0),
    ("one", 1),
    ("a", 1),
    ("two", 2),
    ("three", 3),
    ("four", 4),
    ("five", 5),
    ("six", 6),
    ("seven", 7),
    ("eight", 8),
    ("nine", 9),
    ("ten", 10),
    ("eleven", 11),
    ("twelve", 12),
    ("fifteen", 15),
    ("twenty", 20),
    ("thirty", 30),
    ("forty", 40),
    ("fifty", 50),
    ("sixty", 60),
    ("seventy", 70),
    ("eighty", 80),
    ("ninety", 90),
    ("hundred", 100),
];

/// Parses `"5"`, `"пять"`, `"двадцать пять"`, `"twenty five"`, `"5%"`.
#[must_use]
pub fn parse_number(text: &str) -> Option<u32> {
    let t = text.trim().trim_end_matches('%').trim();
    if let Ok(n) = t.parse::<u32>() {
        return Some(n);
    }
    let mut total = 0u32;
    let mut any = false;
    for w in t.split(|c: char| c.is_whitespace() || c == '-') {
        if w.is_empty()
            || matches!(w, "и" | "and" | "процентов" | "процента" | "процент" | "проц" | "percent" | "percents" | "%")
        {
            continue;
        }
        if let Ok(n) = w.trim_end_matches('%').parse::<u32>() {
            total = total.saturating_add(n);
            any = true;
            continue;
        }
        let w = w.replace('ё', "е");
        let v = RU_UNITS.iter().find(|(k, _)| *k == w).map(|(_, v)| *v)?;
        total = total.saturating_add(v);
        any = true;
    }
    any.then_some(total)
}

/// Time of day from `"7:30"`, `"07.30"`, `"19 00"`, `"семь тридцать"`,
/// `"7 утра"`, `"8 вечера"`, `"в 7 часов 15 минут"`, `"полдень"`, `"7 pm"`.
#[must_use]
pub fn parse_clock(text: &str) -> Option<(u32, u32)> {
    let t = text.trim().to_lowercase().replace('ё', "е");
    let t = t.trim_start_matches("в ").trim_start_matches("на ").trim_start_matches("at ").trim();
    match t {
        "полдень" | "noon" => return Some((12, 0)),
        "полночь" | "midnight" => return Some((0, 0)),
        _ => {}
    }
    let pm = ["вечера", "дня", "pm", "p.m."].iter().any(|w| t.ends_with(w));
    let am_night = ["ночи"].iter().any(|w| t.ends_with(w));
    let mut words: Vec<String> = t
        .split(|c: char| c.is_whitespace() || c == ':' || c == '.' || c == '-')
        .filter(|w| !w.is_empty())
        .filter(|w| {
            !matches!(
                *w,
                "утра"
                    | "вечера"
                    | "дня"
                    | "ночи"
                    | "am"
                    | "pm"
                    | "p"
                    | "m"
                    | "a"
                    | "часов"
                    | "час"
                    | "часа"
                    | "минут"
                    | "минуты"
                    | "минута"
                    | "o'clock"
                    | "oclock"
                    | "h"
            )
        })
        .map(str::to_owned)
        .collect();
    if words.is_empty() || words.len() > 4 {
        return None;
    }
    // "семь тридцать" / "двадцать один пятнадцать": hours = the first number
    // word group that stays < 24, minutes = the rest
    let mut hour = None;
    if words[0].chars().all(|c| c.is_ascii_digit()) {
        hour = words[0].parse::<u32>().ok().filter(|h| *h < 24).map(|h| (h, 1));
    }
    for split in (1..=words.len()).rev() {
        if hour.is_some() {
            break;
        }
        let h = parse_number(&words[..split].join(" "))?;
        if h < 24 {
            hour = Some((h, split));
            break;
        }
    }
    let (mut h, split) = hour?;
    let rest = words.split_off(split);
    let m = if rest.is_empty() { 0 } else { parse_number(&rest.join(" "))? };
    if m > 59 {
        return None;
    }
    if pm && h < 12 {
        h += 12;
    }
    if am_night && h == 12 {
        h = 0;
    }
    Some((h, m))
}

/// Phrase-level fixes for common speech-recognition confusions (Whisper /
/// Vosk, Russian): «громко с 10%» → «громкость 10 процентов», «гром кость»,
/// «на полную» → «на 100», «вдвое» → «в два раза», split words. The
/// matcher tries the raw text first and this form only when nothing matched.
#[must_use]
pub fn stt_normalize(text: &str) -> String {
    let mut s = format!(" {} ", text.to_lowercase().replace('ё', "е"));
    // "10%" / "10 %" → "10 процентов"
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        if c == '%' {
            out.push_str(" процентов ");
        } else if matches!(c, ',' | '!' | '?' | ';' | '«' | '»' | '"') {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    s = out;
    const FIXES: &[(&str, &str)] = &[
        (" громко с ", " громкость "),
        (" громко сть ", " громкость "),
        (" гром кость ", " громкость "),
        (" громкостью ", " громкость "),
        (" громкости ", " громкость "),
        (" громкасть ", " громкость "),
        (" громкасти ", " громкость "),
        (" грамкость ", " громкость "),
        (" громкоть ", " громкость "),
        (" громкозть ", " громкость "),
        (" громкостъ ", " громкость "),
        (" громкость звука ", " громкость "),
        (" уровень звука ", " громкость "),
        (" громкость на уровень ", " громкость "),
        (" звук на уровне ", " громкость "),
        (" по громче ", " погромче "),
        (" по тише ", " потише "),
        (" по громче", " погромче"),
        (" по тише", " потише"),
        (" громчe ", " громче "),
        (" вдвое ", " в два раза "),
        (" в 2 раза ", " в два раза "),
        (" в двое ", " в два раза "),
        (" на полную катушку ", " на 100 "),
        (" на всю катушку ", " на 100 "),
        (" на полную мощность ", " на 100 "),
        (" на полную ", " на 100 "),
        (" на максимум ", " на 100 "),
        (" на максимальную ", " на 100 "),
        (" на всю ", " на 100 "),
        (" до упора ", " на 100 "),
        (" на минимум ", " на 0 "),
        (" микро фон ", " микрофон "),
        (" микрофона ", " микрофон "),
        (" курсо р ", " курсор "),
        (" курсер ", " курсор "),
        (" кур сор ", " курсор "),
        (" мышку ", " курсор "),
        (" мышь ", " курсор "),
        (" мышкой ", " курсор "),
        (" скрин шот ", " скриншот "),
        (" скрин ", " скриншот "),
        (" ютюб ", " ютуб "),
        (" ю туб ", " ютуб "),
        (" you tube ", " youtube "),
        (" блю туз ", " блютуз "),
        (" блютус ", " блютуз "),
        (" вай фай ", " вайфай "),
        (" wi fi ", " wifi "),
        (" вай-фай ", " вайфай "),
        (" рабочей стол ", " рабочий стол "),
        (" окошко ", " окно "),
        (" прокрутить ", " прокрути "),
        (" прокрути те ", " прокрути "),
        (" сделать ", " сделай "),
        (" сделайте ", " сделай "),
        (" поставить ", " поставь "),
        (" выключить ", " выключи "),
        (" включить ", " включи "),
        (" открыть ", " открой "),
        (" закрыть ", " закрой "),
        (" нажать ", " нажми "),
        (" свернуть ", " сверни "),
        (" развернуть ", " разверни "),
        (" прибавить ", " прибавь "),
        (" убавить ", " убавь "),
    ];
    for _ in 0..2 {
        for (a, b) in FIXES {
            if s.contains(a) {
                s = s.replace(a, b);
            }
        }
    }
    s.split_whitespace().map(|w| w.trim_end_matches('.')).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Duration in seconds from `"5 минут"`, `"30 секунд"`, `"2 hours"`, `"полчаса"`.
#[must_use]
pub fn parse_duration_secs(text: &str) -> Option<u32> {
    let t = text.trim().to_lowercase().replace('ё', "е");
    if t == "полчаса" || t == "half an hour" {
        return Some(1800);
    }
    if t == "минуту" || t == "minute" || t == "a minute" {
        return Some(60);
    }
    if t == "час" || t == "hour" || t == "an hour" {
        return Some(3600);
    }
    let unit_of = |w: &str| -> Option<u32> {
        if w.starts_with("сек") || w.starts_with("sec") {
            Some(1)
        } else if w.starts_with("мин") || w.starts_with("min") {
            Some(60)
        } else if w.starts_with("час") || w.starts_with("hour") || w == "h" {
            Some(3600)
        } else {
            None
        }
    };
    let ws: Vec<&str> = t.split_whitespace().collect();
    if let Some(last) = ws.last() {
        if let Some(mult) = unit_of(last) {
            let n = if ws.len() == 1 { 1 } else { parse_number(&ws[..ws.len() - 1].join(" "))? };
            return Some(n.saturating_mul(mult));
        }
    }
    // bare number = minutes
    parse_number(&t).map(|n| n.saturating_mul(60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_keep_domains_and_drop_punctuation() {
        let w: Vec<String> = words("Открой ya.ru, пожалуйста!").into_iter().map(|w| w.0).collect();
        assert_eq!(w, ["открой", "ya.ru", "пожалуйста"]);
        let w: Vec<String> = words("Ёлка.").into_iter().map(|w| w.0).collect();
        assert_eq!(w, ["елка"]);
    }

    #[test]
    fn stems_fold_inflections() {
        assert_eq!(stem("блокнот"), "блокнот");
        assert_eq!(stem("блокнота"), "блокнот");
        assert_eq!(stem("браузере"), "браузер");
        assert_eq!(stem("music"), "music");
        assert_eq!(stem("windows"), "window");
        assert_eq!(stem("переключись"), "переключ");
    }

    #[test]
    fn verbs_fold_into_concepts() {
        for v in ["открой", "откройте", "открыть", "запусти", "запустить", "open", "launch", "start"]
        {
            assert_eq!(tokenize(v)[0].key, "@open", "{v}");
        }
        for v in ["закрой", "закрыть", "close", "quit"] {
            assert_eq!(tokenize(v)[0].key, "@close", "{v}");
        }
        assert_eq!(tokenize("прокрути вниз").iter().map(|t| t.key.as_str()).collect::<Vec<_>>(), ["@scroll", "@down"]);
        assert!(tokenize("пожалуйста").is_empty());
    }

    #[test]
    fn clock_times() {
        assert_eq!(parse_clock("7:30"), Some((7, 30)));
        assert_eq!(parse_clock("07.05"), Some((7, 5)));
        assert_eq!(parse_clock("19 00"), Some((19, 0)));
        assert_eq!(parse_clock("семь тридцать"), Some((7, 30)));
        assert_eq!(parse_clock("в семь утра"), Some((7, 0)));
        assert_eq!(parse_clock("8 вечера"), Some((20, 0)));
        assert_eq!(parse_clock("двадцать один пятнадцать"), Some((21, 15)));
        assert_eq!(parse_clock("7 часов 15 минут"), Some((7, 15)));
        assert_eq!(parse_clock("полдень"), Some((12, 0)));
        assert_eq!(parse_clock("7 pm"), Some((19, 0)));
        assert_eq!(parse_clock("25:00"), None);
        assert_eq!(parse_clock("завтра"), None);
    }

    #[test]
    fn speech_recognition_fixes() {
        assert_eq!(stt_normalize("Громко с 10%."), "громкость 10 процентов");
        assert_eq!(stt_normalize("сделай по громче"), "сделай погромче");
        assert_eq!(stt_normalize("звук на полную"), "звук на 100");
        assert_eq!(stt_normalize("вдвое тише"), "в два раза тише");
        assert_eq!(stt_normalize("выключи микро фон"), "выключи микрофон");
        assert_eq!(stt_normalize("открой ютюб"), "открой ютуб");
        assert_eq!(parse_number("десяти"), Some(10));
        assert_eq!(parse_number("полную"), Some(100));
        assert_eq!(parse_number("сорока четырех"), Some(44));
    }

    #[test]
    fn numbers_and_durations() {
        assert_eq!(parse_number("5"), Some(5));
        assert_eq!(parse_number("пять"), Some(5));
        assert_eq!(parse_number("двадцать пять"), Some(25));
        assert_eq!(parse_number("twenty five"), Some(25));
        assert_eq!(parse_number("50%"), Some(50));
        assert_eq!(parse_number("много"), None);
        assert_eq!(parse_duration_secs("5 минут"), Some(300));
        assert_eq!(parse_duration_secs("30 секунд"), Some(30));
        assert_eq!(parse_duration_secs("2 hours"), Some(7200));
        assert_eq!(parse_duration_secs("полчаса"), Some(1800));
        assert_eq!(parse_duration_secs("десять"), Some(600));
    }

    #[test]
    fn fuzzy_token_costs() {
        let t = |w: &str| tokenize(w).remove(0);
        assert_eq!(token_cost("блокнот", &t("блокнота")), Some(0.0));
        assert!(token_cost("калькулятор", &t("калкулятор")).is_some(), "typo tolerated");
        assert!(token_cost("блокнот", &t("браузер")).is_none());
        assert!(token_cost("@open", &t("закрой")).is_none());
    }
}
