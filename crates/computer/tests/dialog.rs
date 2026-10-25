//! Jarvis template bank (ADR-0049): size, held-out accuracy, no false
//! positives on unrelated questions or computer commands.

use std::time::Instant;

use orkestr_computer::dialog::{DialogBank, RenderContext};
use orkestr_protocol::methods::jarvis::JarvisAddress;
use serde_json::Value;

fn heldout() -> Vec<(String, String, bool)> {
    let v: Value = serde_json::from_str(include_str!("../data/dialog-heldout.json")).unwrap();
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["text"].as_str().unwrap().to_owned(),
                i["intent"].as_str().unwrap().to_owned(),
                i["typo"].as_bool().unwrap_or(false),
            )
        })
        .collect()
}

#[test]
fn bank_has_ten_thousand_phrasings() {
    let bank = DialogBank::builtin();
    let s = bank.stats();
    assert!(s.intents >= 100, "{s:?}");
    assert!(s.questions >= 10_000, "{s:?}");
    assert!(s.answers >= 300, "{s:?}");
    for i in bank.intents() {
        assert!(!i.answers.is_empty(), "{}", i.id);
        for a in &i.answers {
            assert!(!a.contains("{{"), "{}: i18next-style slot in {a}", i.id);
        }
    }
}

#[test]
fn held_out_paraphrases_match_their_intent() {
    let bank = DialogBank::builtin();
    let items = heldout();
    assert!(items.len() > 1_000);
    let (mut ok, mut ok_typo, mut n_typo) = (0usize, 0usize, 0usize);
    let mut misses = Vec::new();
    for (text, intent, typo) in &items {
        let hit = bank.find(text).is_some_and(|m| &m.intent == intent);
        if *typo {
            n_typo += 1;
            ok_typo += usize::from(hit);
        } else {
            ok += usize::from(hit);
            if !hit && misses.len() < 40 {
                misses.push(format!(
                    "{text} → {intent} (got {:?})",
                    bank.find_with(text, 0.0).map(|m| (m.intent, m.score))
                ));
            }
        }
    }
    let clean = items.len() - n_typo;
    #[allow(clippy::cast_precision_loss)]
    let (acc, acc_typo) = (ok as f64 / clean as f64, ok_typo as f64 / n_typo.max(1) as f64);
    println!("held-out accuracy: {acc:.4} ({ok}/{clean}); with typos: {acc_typo:.4} ({ok_typo}/{n_typo})");
    assert!(acc >= 0.95, "held-out accuracy {acc:.3} < 0.95; misses:\n{}", misses.join("\n"));
    assert!(acc_typo >= 0.7, "typo accuracy {acc_typo:.3} < 0.7");
}

/// Real questions for the AI and commands for the computer: never small talk.
const UNRELATED: &[&str] = &[
    "объясни квантовую механику простыми словами",
    "напиши функцию сортировки на питоне",
    "какая погода была в Москве в 1812 году во время пожара",
    "кто такой Илон Маск",
    "сколько будет 17 умножить на 23",
    "переведи на английский фразу я люблю программировать",
    "почему небо голубое",
    "как работает протокол TCP",
    "составь план поездки в Санкт-Петербург на три дня",
    "что такое замыкание в JavaScript",
    "в чем разница между процессом и потоком",
    "расскажи историю Римской империи",
    "как приготовить борщ",
    "какой самый высокий небоскреб в мире",
    "посчитай сколько дней между первым мая и первым сентября",
    "напиши письмо начальнику об отпуске",
    "какие тесты упали в проекте schoolify и почему",
    "который час сейчас в Нью-Йорке и в Токио",
    "сравни rust и go по производительности для веб серверов",
    "придумай название для стартапа про доставку еды",
    "открой блокнот",
    "закрой хром",
    "сделай скриншот экрана",
    "прокрути вниз",
    "громкость на 30 процентов",
    "найди в интернете рецепт пиццы",
    "открой ютуб",
    "поставь таймер на 5 минут",
    "сверни все окна",
    "запусти калькулятор",
    "останови проект schoolify",
    "статус проекта ai office",
    "какие агенты сейчас работают над задачей авторизации",
    "создай новую задачу исправить вход",
];

#[test]
fn unrelated_questions_and_commands_fall_through() {
    let bank = DialogBank::builtin();
    let hits: Vec<String> = UNRELATED
        .iter()
        .filter_map(|t| bank.find(t).map(|m| format!("{t} → {} ({:.2}, «{}»)", m.intent, m.score, m.phrase)))
        .collect();
    assert!(hits.is_empty(), "false positives:\n{}", hits.join("\n"));
}

#[test]
fn catalogue_command_phrases_are_not_small_talk() {
    let bank = DialogBank::builtin();
    let cat: Value = serde_json::from_str(include_str!("../data/catalogue.json")).unwrap();
    let mut phrases = Vec::new();
    for set in cat["sets"].as_array().unwrap() {
        for c in set["commands"].as_array().unwrap() {
            for p in c["phrases"].as_array().unwrap() {
                let p = p.as_str().unwrap();
                if !p.contains('{') && p.chars().any(|c| ('а'..='я').contains(&c)) {
                    phrases.push(p.to_owned());
                }
            }
        }
    }
    let sample: Vec<&String> = phrases.iter().step_by((phrases.len() / 600).max(1)).collect();
    let hits: Vec<String> =
        sample.iter().filter_map(|p| bank.find(p).map(|m| format!("{p} → {} ({:.2})", m.intent, m.score))).collect();
    #[allow(clippy::cast_precision_loss)]
    let rate = hits.len() as f64 / sample.len() as f64;
    println!("command phrases matched as small talk: {}/{}", hits.len(), sample.len());
    assert!(rate <= 0.01, "{rate:.3} of command phrases look like small talk:\n{}", hits.join("\n"));
}

#[test]
fn matching_is_fast_and_answers_render() {
    let bank = DialogBank::builtin();
    let started = Instant::now();
    for t in ["привет джарвис", "который час", "расскажи анекдот", "что ты умеешь", "объясни теорию струн"]
    {
        let _ = bank.find(t);
    }
    let per = started.elapsed() / 5;
    // generous: debug builds are ~20x slower than release
    assert!(per.as_millis() < 400, "{per:?} per match");
    let m = bank.find("который час").unwrap();
    assert_eq!(m.intent, "time.now");
    let ctx = RenderContext {
        address: JarvisAddress::Sir,
        user_name: String::new(),
        persona: true,
        now: chrono::NaiveDate::from_ymd_opt(2026, 10, 26).unwrap().and_hms_opt(14, 30, 0).unwrap(),
        last_reply: None,
        seed: 42,
    };
    let a = bank.answer(&m, &ctx);
    assert!(a.contains("14:30") && a.contains("сэр"), "{a}");
    let m = bank.find("Добрый вечер, Джарвис!").unwrap();
    assert_eq!(m.intent, "greet.evening");
    assert!(bank
        .find("режим разговора")
        .and_then(|m| bank.intent(&m).action.clone())
        .is_some_and(|a| a == "mode.chat"));
    assert_eq!(bank.find("называй меня босс").map(|m| m.intent), Some("address.boss".into()));
    assert_eq!(bank.find("не называй меня сэр").map(|m| m.intent), Some("address.none".into()));
}

#[test]
#[ignore = "tuning helper"]
fn tune_thresholds() {
    let bank = DialogBank::builtin();
    let items = heldout();
    let res: Vec<(bool, Option<(String, f32)>)> =
        items.iter().map(|(t, _, typo)| (*typo, bank.find_with(t, 0.5).map(|m| (m.intent, m.score)))).collect();
    let cat: Value = serde_json::from_str(include_str!("../data/catalogue.json")).unwrap();
    let mut phrases = Vec::new();
    for set in cat["sets"].as_array().unwrap() {
        for c in set["commands"].as_array().unwrap() {
            for p in c["phrases"].as_array().unwrap() {
                let p = p.as_str().unwrap();
                if !p.contains('{') && p.chars().any(|c| ('а'..='я').contains(&c)) {
                    phrases.push(p.to_owned());
                }
            }
        }
    }
    let neg: Vec<Option<(String, f32)>> = phrases
        .iter()
        .step_by(5)
        .map(String::as_str)
        .chain(UNRELATED.iter().copied())
        .map(|t| bank.find_with(t, 0.5).map(|m| (format!("{t} → {}", m.intent), m.score)))
        .collect();
    for th in [0.6_f32, 0.65, 0.7, 0.72, 0.75, 0.78, 0.8, 0.85] {
        let mut ok = [0usize; 2];
        let mut n = [0usize; 2];
        for ((_, intent, _), (typo, r)) in items.iter().zip(&res) {
            let k = usize::from(*typo);
            n[k] += 1;
            if r.as_ref().is_some_and(|(i, s)| i == intent && *s >= th) {
                ok[k] += 1;
            }
        }
        let fp: Vec<&String> = neg.iter().flatten().filter(|(_, s)| *s >= th).map(|(t, _)| t).collect();
        println!("th {th}: clean {}/{} typo {}/{} fp {} of {}", ok[0], n[0], ok[1], n[1], fp.len(), neg.len());
        if (0.7..0.76).contains(&th) {
            for f in fp.iter().take(25) {
                println!("   fp {f}");
            }
        }
    }
}

#[test]
#[ignore = "prints the demo transcript of docs/jarvis-dialog.md"]
fn demo_transcript() {
    let bank = DialogBank::builtin();
    let ctx = |seed: u64| RenderContext {
        address: JarvisAddress::Sir,
        user_name: String::new(),
        persona: true,
        now: chrono::NaiveDate::from_ymd_opt(2026, 10, 26).unwrap().and_hms_opt(9, 12, 0).unwrap(),
        last_reply: Some("Открываю YouTube, сэр.".into()),
        seed,
    };
    for (i, t) in [
        "Джарвис, привет!",
        "как дела?",
        "который час",
        "какое сегодня число",
        "кто ты такой",
        "что ты умеешь",
        "расскажи анекдот про программистов",
        "какая погода сегодня",
        "у меня опять баг",
        "я устал",
        "спасибо, дружище",
        "называй меня босс",
        "ты захватишь мир?",
        "режим разговора",
        "повтори",
        "подбрось монетку",
        "сколько дней до нового года",
        "режим команд",
        "спокойной ночи",
        "кто такой Илон Маск",
    ]
    .iter()
    .enumerate()
    {
        match bank.find(t) {
            Some(m) => println!("{t} => [{} {:.2}] {}", m.intent, m.score, bank.answer(&m, &ctx(i as u64 * 7919))),
            None => println!("{t} => (AI)"),
        }
    }
}
