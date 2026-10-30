//! Speech-recognition robustness (ADR-0053): ≥ 100 noisy utterances the way
//! Whisper / Vosk hear them («громко с 10%», «по громче», «звук на десять»)
//! must reach the right catalogue command with the right slots. Accuracy is
//! printed (`cargo test -p orkestr-computer --test noisy -- --nocapture`).

use std::collections::BTreeMap;

use orkestr_computer::catalogue::builtin_sets;
use orkestr_computer::matcher::{Entry, Matcher, MATCH_THRESHOLD, NEAR_MISS_THRESHOLD};

fn index() -> (Matcher, Vec<String>) {
    let mut entries = Vec::new();
    let mut ids = Vec::new();
    for s in builtin_sets().unwrap() {
        for c in s.commands {
            entries.push(Entry {
                phrases: c.phrases.clone(),
                context: c.context.clone().or(s.context.clone()),
                bonus: 0.0,
            });
            ids.push(c.id.clone());
        }
    }
    (Matcher::new(entries), ids)
}

/// Expected slot values.
type Slots = &'static [(&'static str, &'static str)];

/// `(utterance, command id, expected slots)`.
const CASES: &[(&str, &str, Slots)] = &[
    // ── the bug from the screenshot and its family ──
    ("громко с 10%", "media.volset", &[("n", "10")]),
    ("Громко с 10%.", "media.volset", &[("n", "10")]),
    ("громкость 10%", "media.volset", &[("n", "10")]),
    ("громкость десять процентов", "media.volset", &[("n", "10")]),
    ("громкостью 10", "media.volset", &[("n", "10")]),
    ("громкости 30 процентов", "media.volset", &[("n", "30")]),
    ("гром кость 50", "media.volset", &[("n", "50")]),
    ("громкасть двадцать", "media.volset", &[("n", "20")]),
    ("звук на десять", "media.volset", &[("n", "10")]),
    ("звук на 10", "media.volset", &[("n", "10")]),
    ("звук на двадцать пять процентов", "media.volset", &[("n", "25")]),
    ("поставь громкость на сорок", "media.volset", &[("n", "40")]),
    ("сделай громкость 15", "media.volset", &[("n", "15")]),
    ("громкость на 70 процента", "media.volset", &[("n", "70")]),
    ("джарвис громкость 60", "media.volset", &[("n", "60")]),
    ("set volume to 35", "media.volset", &[("n", "35")]),
    ("volume 80 percent", "media.volset", &[("n", "80")]),
    ("громкость на полную", "sound.max", &[]),
    ("звук на полную", "sound.max", &[]),
    ("выкрути звук на полную", "sound.max", &[]),
    ("громкость на максимум", "sound.max", &[]),
    ("в два раза тише", "sound.half", &[]),
    ("вдвое тише", "sound.half", &[]),
    ("сделай в два раза тише", "sound.half", &[]),
    ("в два раза громче", "sound.double", &[]),
    ("сделай погромче", "media.volup", &[]),
    ("сделай по громче", "media.volup", &[]),
    ("погромче пожалуйста", "media.volup", &[]),
    ("прибавь звук", "media.volup", &[]),
    ("прибавь громкость", "media.volup", &[]),
    ("сделай потише", "media.voldown", &[]),
    ("по тише", "media.voldown", &[]),
    ("убавь громкость", "media.voldown", &[]),
    ("чуть громче", "sound.bitup", &[]),
    ("немного тише", "sound.bitdown", &[]),
    ("намного громче", "sound.muchup", &[]),
    ("громче на 20", "media.volupn", &[("n", "20")]),
    ("тише на десять", "media.voldownn", &[("n", "10")]),
    ("выключи звук", "sound.muteon", &[]),
    ("выключить звук", "sound.muteon", &[]),
    ("отключи звук", "sound.muteon", &[]),
    ("включи звук", "sound.muteoff", &[]),
    ("верни звук", "sound.muteoff", &[]),
    ("какая громкость", "sound.get", &[]),
    ("спотифай тише", "sound.app.spotify.down", &[]),
    ("выключи звук в дискорде", "sound.app.discord.mute", &[]),
    ("громкость хрома на 30", "sound.app.set", &[("n", "30")]),
    ("выключи микрофон", "sound.mic.mute", &[]),
    ("выключи микро фон", "sound.mic.mute", &[]),
    ("включи микрофон", "sound.mic.unmute", &[]),
    ("громкость микрофона 80", "sound.mic.set", &[("n", "80")]),
    ("звук в наушники", "sound.dev.headphones", &[]),
    ("переключи на наушники", "sound.dev.headphones", &[]),
    ("звук на колонки", "sound.dev.speakers", &[]),
    ("переключи звук на гарнитуру", "sound.dev.to", &[("name", "гарнитуру")]),
    ("следующее устройство звука", "sound.dev.next", &[]),
    ("что сейчас играет", "sound.nowplaying", &[]),
    ("что за песня", "sound.nowplaying", &[]),
    ("перемотай вперед на 30 секунд", "sound.seekfwd", &[("duration", "30")]),
    ("перемотай назад", "sound.seekback10", &[]),
    ("следующий трек", "media.next", &[]),
    ("пауза", "media.playpause", &[]),
    // ── windows ──
    ("окно слева", "win.snapleft", &[]),
    ("окно влево", "win.snapleft", &[]),
    ("окно справа", "win.snapright", &[]),
    ("окно в левый верхний угол", "win.snaptl", &[]),
    ("окно в правый нижний угол", "win.snapbr", &[]),
    ("окно по центру", "win.center", &[]),
    ("отцентрируй окно", "win.center", &[]),
    ("закрепи окно поверх всех", "win.ontop", &[]),
    ("поверх всех окон", "win.ontop", &[]),
    ("открепи окно", "win.notontop", &[]),
    ("разверни все окна", "win.restoreall", &[]),
    ("сверни окно", "win.minimize", &[]),
    ("разверни окно", "win.maximize", &[]),
    ("свернуть окно", "win.minimize", &[]),
    ("окно на 70 процентов", "win.resize", &[("n", "70")]),
    ("окно 1280 на 720", "win.resizepx", &[("w", "1280"), ("h", "720")]),
    ("сделай окно прозрачным", "win.transparent", &[]),
    ("убери прозрачность", "win.opaque", &[]),
    ("хром влево", "win.chrome.left", &[]),
    ("телеграм справа", "win.telegram.right", &[]),
    ("следующий рабочий стол", "win.vdnext", &[]),
    ("новый рабочий стол", "win.vdnew", &[]),
    ("предыдущее окно", "win.previous", &[]),
    ("закрой все окна хрома", "win.closeall", &[("app", "хрома")]),
    // ── cursor & mouse ──
    ("курсор вверх на 200", "mouse.moveupn", &[("n", "200")]),
    ("курсор вниз на сто пикселей", "mouse.movedownn", &[("n", "100")]),
    ("курсор влево", "mouse.moveleft", &[]),
    ("курсер вправо", "mouse.moveright", &[]),
    ("курсор немного вверх", "mouse.moveupbit", &[]),
    ("курсор сильно вниз", "mouse.movedownfar", &[]),
    ("курсор в центр", "mouse.tocenter", &[]),
    ("мышку в центр экрана", "mouse.tocenter", &[]),
    ("курсор в правый верхний угол", "mouse.totopright", &[]),
    ("где курсор", "mouse.locate", &[]),
    ("где мышка", "mouse.locate", &[]),
    ("щёлкни по кнопке Сохранить", "scroll.element", &[("name", "Сохранить")]),
    ("щелкни по кнопке отправить", "scroll.element", &[("name", "отправить")]),
    ("наведи на кнопку Далее", "mouse.hover", &[("name", "Далее")]),
    ("правый клик", "scroll.rightclick", &[]),
    ("двойной клик", "scroll.doubleclick", &[]),
    ("средний клик", "mouse.middleclick", &[]),
    ("зажми левую кнопку", "mouse.hold", &[]),
    ("отпусти кнопку мыши", "mouse.release", &[]),
    ("перетащи вправо на 300", "mouse.dragrightn", &[("n", "300")]),
    ("курсор быстрее", "mouse.faster", &[]),
    ("прокрути вниз", "scroll.down", &[]),
    ("прокрутить вниз на 5", "scroll.downn", &[("n", "5")]),
    // ── keyboard ──
    ("переключи на английский", "layout.en", &[]),
    ("русская раскладка", "layout.ru", &[]),
    ("включи капс лок", "key.capslock", &[]),
    ("нажми контрл шифт эс", "key.press", &[("key", "контрл шифт эс")]),
    ("скопируй", "key.copy", &[]),
    ("вставь", "key.paste", &[]),
    ("отмени", "key.undo", &[]),
    ("сохрани", "key.save", &[]),
    ("выдели всё", "key.selectall", &[]),
    ("напиши привет мир", "type.text", &[("text", "привет мир")]),
    // ── display ──
    ("яркость 70", "media.brset", &[("n", "70")]),
    ("яркость на полную", "display.brmax", &[]),
    ("выключи экран", "display.off", &[]),
    ("расширь экран", "display.extend", &[]),
    ("скриншот окна", "display.shotwindow", &[]),
    ("скриншот в буфер", "display.shotclip", &[]),
    ("скрин шот области", "display.shotregion", &[]),
    ("включи ночной свет", "display.nightlight", &[]),
    ("запиши экран", "display.recstart", &[]),
    // ── system & info ──
    ("выключи вайфай", "sys.wifioff", &[]),
    ("выключи вай фай", "sys.wifioff", &[]),
    ("включи блютуз", "sys.bton", &[]),
    ("включи блю туз", "sys.bton", &[]),
    ("режим полета", "sys.airplaneon", &[]),
    ("который час", "info.time", &[]),
    ("сколько времени", "info.time", &[]),
    ("какое сегодня число", "info.date", &[]),
    ("сколько заряда", "info.battery", &[]),
    ("сколько места на диске", "info.disk", &[]),
    ("какой у меня ip", "info.ip", &[]),
    ("загрузка процессора", "info.sysinfo", &[]),
    ("сколько будет два плюс два", "info.calc", &[("expr", "два плюс два")]),
    ("разбуди меня в 7:30", "info.alarm", &[("time", "07:30")]),
    ("будильник на семь утра", "info.alarm", &[("time", "07:00")]),
    ("поставь таймер на 5 минут", "sys.timer", &[("duration", "300")]),
    ("заблокируй компьютер", "sys.lock", &[]),
    ("не беспокоить", "sys.dnd", &[]),
    ("прочитай заголовок страницы", "info.title", &[]),
    // ── files & browser ──
    ("найди файл отчет", "file.search", &[("name", "отчет")]),
    ("очисти корзину", "file.emptybin", &[]),
    ("открой загрузки", "folder.downloads", &[]),
    ("последние файлы", "file.recent", &[]),
    ("включи на ютубе lofi", "yt.play", &[("query", "lofi")]),
    ("открой ютюб", "site.youtube", &[]),
    ("открой блокнот", "app.notepad.open", &[]),
    ("открыть калькулятор", "app.calculator.open", &[]),
];

#[test]
fn noisy_speech_reaches_the_right_command() {
    let (m, ids) = index();
    assert!(CASES.len() >= 100, "{} cases", CASES.len());
    let mut ok = 0usize;
    let mut misses = Vec::new();
    for (text, want, slots) in CASES {
        let r = m.find(text, None, 3, 0.0);
        let got = r.first().filter(|x| x.score >= MATCH_THRESHOLD);
        let good = got.is_some_and(|g| {
            let id = ids[g.owner].as_str();
            let exact = id == *want && slots.iter().all(|(k, v)| g.slots.get(*k).map(String::as_str) == Some(*v));
            // a fixed-value command that does the same («громкость 10%» → media.vol10)
            let fixed = *want == "media.volset"
                && slots.first().is_some_and(|(_, n)| id == format!("media.vol{n}") || id == format!("sound.set{n}"));
            exact || fixed
        });
        if good {
            ok += 1;
        } else {
            let detail = r.first().map_or_else(
                || "nothing".to_owned(),
                |g| format!("{} {:.2} {:?}", ids[g.owner], g.score, g.slots.iter().collect::<BTreeMap<_, _>>()),
            );
            misses.push(format!("«{text}» → want {want}, got {detail}"));
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let accuracy = ok as f64 * 100.0 / CASES.len() as f64;
    println!("noisy set: {ok}/{} = {accuracy:.1}%", CASES.len());
    for miss in &misses {
        println!("  miss: {miss}");
    }
    assert!(accuracy >= 97.0, "accuracy {accuracy:.1}% < 97%:\n{}", misses.join("\n"));
}

#[test]
fn the_screenshot_bug_matches_exactly() {
    let (m, ids) = index();
    for text in ["громко с 10%", "сделай погромче", "звук на десять"] {
        let r = m.find(text, None, 1, MATCH_THRESHOLD);
        let id = r.first().map(|g| ids[g.owner].as_str());
        assert!(
            matches!(id, Some("media.volset" | "media.volup" | "media.vol10")),
            "«{text}» must be a volume command, got {id:?}"
        );
    }
}

#[test]
fn near_misses_are_offered_not_run() {
    let (m, ids) = index();
    // too garbled to run, close enough to ask «Вы имели в виду …?»
    let r = m.find("гомкст десят працентв", None, 3, 0.0);
    let top = r.first().expect("a suggestion");
    assert!(top.score < MATCH_THRESHOLD, "{} {}", ids[top.owner], top.score);
    // unrelated chatter is neither run nor offered
    let r = m.find("расскажи анекдот про программистов", None, 3, 0.0);
    assert!(
        r.first().is_none_or(|t| t.score < NEAR_MISS_THRESHOLD),
        "{:?}",
        r.first().map(|t| (&ids[t.owner], t.score))
    );
}
