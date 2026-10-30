//! The action registry the AI sees (ADR-0053): every `ComputerAction` with
//! its parameters, what it does and request → step examples. The planner
//! prompt and the Jarvis capability summary are generated from it; the enum
//! values in the signatures come from the protocol types themselves
//! (ts-rs), and tests make sure every action of the protocol has an entry
//! and every example parses — so the AI is never told less than Orkestr can
//! do (the «у меня нет управления звуком» bug).

use ts_rs::{Config, TS};

use orkestr_protocol::methods::computer::{
    AudioDeviceKind, DesktopOp, DisplayOp, LevelOp, MediaKey, MouseButton, QueryKind, RadioKind, ScreenPoint,
    ScreenshotScope, ScrollDirection, ScrollUnit, SystemOp, WindowOp,
};

/// One action of the vocabulary.
#[derive(Debug, Clone, Copy)]
pub struct ActionSpec {
    /// `type` value.
    pub name: &'static str,
    /// Family (sound, windows, mouse…).
    pub family: &'static str,
    /// Fields; `<Enum>` placeholders are filled with the protocol's values.
    pub params: &'static str,
    /// What it does.
    pub doc: &'static str,
    /// `(request, step JSON)` examples.
    pub examples: &'static [(&'static str, &'static str)],
}

const fn a(
    name: &'static str,
    family: &'static str,
    params: &'static str,
    doc: &'static str,
    examples: &'static [(&'static str, &'static str)],
) -> ActionSpec {
    ActionSpec { name, family, params, doc, examples }
}

/// Every action (order = prompt order).
pub const ACTIONS: &[ActionSpec] = &[
    // apps & web
    a("Launch", "apps", "{target}", "start an app (name / alias / exe), a URL or a file", &[("открой блокнот", r#"{"type":"Launch","target":"notepad"}"#)]),
    a("FocusApp", "apps", "{app}", "bring an app's window to the front", &[("переключись на телеграм", r#"{"type":"FocusApp","app":"telegram"}"#)]),
    a("CloseApp", "apps", "{app, discard}", "close every window of an app (empty app = active window); discard = without saving", &[("закрой все окна хрома", r#"{"type":"CloseApp","app":"chrome","discard":false}"#)]),
    a("KillProcess", "apps", "{name}", "terminate processes by name (confirmed)", &[("убей процесс notepad", r#"{"type":"KillProcess","name":"notepad"}"#)]),
    a("ListProcesses", "apps", "{}", "show running apps", &[("какие программы запущены", r#"{"type":"ListProcesses"}"#)]),
    a("OpenUrl", "web", "{url}", "open a web page", &[("открой github", r#"{"type":"OpenUrl","url":"https://github.com"}"#)]),
    a("WebSearch", "web", "{query, engine?}", "web search; engine: google, yandex, bing, duckduckgo, youtube, wikipedia, github, maps…", &[("найди на ютубе lofi", r#"{"type":"WebSearch","query":"lofi","engine":"youtube"}"#)]),
    a("OpenSettings", "system", "{page}", "open a Windows Settings page (ms-settings id: display, sound, bluetooth, network-wifi, nightlight, powersleep, mousetouchpad, easeofaccess-mousepointer, notifications, quiethours, dateandtime, apps-volume…)", &[("открой настройки ночного света", r#"{"type":"OpenSettings","page":"nightlight"}"#)]),
    a("OpenPath", "files", "{path}", "open a file or folder (known: downloads, documents, desktop, pictures, music, videos, recent, recycle bin)", &[("открой загрузки", r#"{"type":"OpenPath","path":"downloads"}"#)]),
    // windows
    a("Window", "windows", "{op: <WindowOp>, app?}", "window of an app (absent = the active window): snap halves / quarters, center, always on top, minimize all…", &[
        ("окно слева", r#"{"type":"Window","op":"SnapLeft"}"#),
        ("хром в правый верхний угол", r#"{"type":"Window","op":"SnapTopRight","app":"chrome"}"#),
        ("закрепи окно поверх всех", r#"{"type":"Window","op":"AlwaysOnTop"}"#),
        ("сверни все окна", r#"{"type":"Window","op":"MinimizeAll"}"#),
    ]),
    a("WindowResize", "windows", "{app?, percent? | width?, height?}", "resize (centered): percent of the screen or exact px", &[("сделай окно 70 процентов экрана", r#"{"type":"WindowResize","percent":70}"#), ("окно 1280 на 720", r#"{"type":"WindowResize","width":1280,"height":720}"#)]),
    a("WindowOpacity", "windows", "{app?, percent}", "window transparency (100 = opaque, min 20)", &[("сделай окно прозрачным на 80", r#"{"type":"WindowOpacity","percent":80}"#)]),
    a("Desktop", "windows", "{op: <DesktopOp>}", "virtual desktops (switch, create, close, task view). Moving a window to another desktop has NO public API: say so", &[("следующий рабочий стол", r#"{"type":"Desktop","op":"Next"}"#)]),
    a("ListWindows", "windows", "{}", "show the open windows", &[("какие окна открыты", r#"{"type":"ListWindows"}"#)]),
    // keyboard
    a("Keys", "keyboard", "{combo, times?}", "press keys: ctrl, shift, alt, win, enter, esc, tab, space, f1-f24, arrows, letters, capslock, printscreen, volumeup…; Russian names ok (контрл шифт эс)", &[("нажми контрл шифт эс", r#"{"type":"Keys","combo":"ctrl+shift+s"}"#), ("новая вкладка", r#"{"type":"Keys","combo":"ctrl+t"}"#), ("запиши экран", r#"{"type":"Keys","combo":"win+alt+r"}"#)]),
    a("TypeText", "keyboard", "{text}", "type text into the focused field (never into password fields)", &[("напиши привет", r#"{"type":"TypeText","text":"привет"}"#)]),
    a("KeyboardLayout", "keyboard", "{lang?}", "switch the keyboard layout (absent = next; en, ru, uk, de…)", &[("переключи на английский", r#"{"type":"KeyboardLayout","lang":"en"}"#)]),
    // mouse
    a("Scroll", "mouse", "{direction: <ScrollDirection>, amount?, unit?: <ScrollUnit>}", "scroll (wheel notches, pages or to the edge)", &[("прокрути вниз на 3 страницы", r#"{"type":"Scroll","direction":"Down","amount":3,"unit":"Pages"}"#)]),
    a("ClickElement", "mouse", "{name}", "click a button / link / menu item of the active window by its visible name (UI Automation)", &[("щёлкни по кнопке Сохранить", r#"{"type":"ClickElement","name":"Сохранить"}"#)]),
    a("MouseClick", "mouse", "{button?: <MouseButton>, x?, y?}", "click at the cursor (or at x, y)", &[("правый клик", r#"{"type":"MouseClick","button":"Right"}"#)]),
    a("MouseMove", "mouse", "{direction: <ScrollDirection>, distance? (px, default 100)}", "move the cursor (немного = 50, сильно = 400)", &[("курсор вверх на 200", r#"{"type":"MouseMove","direction":"Up","distance":200}"#)]),
    a("MouseTo", "mouse", "{point: <ScreenPoint>}", "cursor to the center / a corner / an edge of its screen", &[("курсор в центр", r#"{"type":"MouseTo","point":"Center"}"#)]),
    a("MouseToElement", "mouse", "{name}", "cursor onto a named UI element", &[("наведи на кнопку Отправить", r#"{"type":"MouseToElement","name":"Отправить"}"#)]),
    a("MouseHold", "mouse", "{button?, release}", "press and hold / release a mouse button", &[("зажми левую кнопку", r#"{"type":"MouseHold","button":"Left","release":false}"#)]),
    a("MouseDrag", "mouse", "{direction?, distance?, element?}", "drag with the left button from the cursor (direction + px, or onto an element)", &[("перетащи вправо на 300", r#"{"type":"MouseDrag","direction":"Right","distance":300}"#)]),
    a("MouseSpeed", "mouse", "{op: <LevelOp>, value?}", "pointer speed on the Windows 1..20 scale", &[("курсор быстрее", r#"{"type":"MouseSpeed","op":"Up","value":2}"#)]),
    a("LocateCursor", "mouse", "{}", "show where the cursor is", &[("где курсор", r#"{"type":"LocateCursor"}"#)]),
    // sound & media
    a("Volume", "sound", "{op: <LevelOp>, value?, app?}", "system volume percent; with app = that app's own volume in the mixer. Scale: value = percent of the current level (50 = twice quieter)", &[
        ("громкость 10%", r#"{"type":"Volume","op":"Set","value":10}"#),
        ("громко с 10%", r#"{"type":"Volume","op":"Set","value":10}"#),
        ("сделай погромче", r#"{"type":"Volume","op":"Up","value":10}"#),
        ("звук на десять", r#"{"type":"Volume","op":"Set","value":10}"#),
        ("в два раза тише", r#"{"type":"Volume","op":"Scale","value":50}"#),
        ("на полную", r#"{"type":"Volume","op":"Set","value":100}"#),
        ("выключи звук", r#"{"type":"Volume","op":"MuteOn"}"#),
        ("спотифай потише", r#"{"type":"Volume","op":"Down","value":20,"app":"spotify"}"#),
    ]),
    a("Microphone", "sound", "{op: <LevelOp>, value?}", "default microphone level / mute", &[("выключи микрофон", r#"{"type":"Microphone","op":"MuteOn"}"#)]),
    a("AudioDevice", "sound", "{kind: <AudioDeviceKind>, name?}", "switch the default output / input device (absent name = next)", &[("переключи звук на наушники", r#"{"type":"AudioDevice","kind":"Output","name":"наушники"}"#)]),
    a("Media", "media", "{key: <MediaKey>}", "media keys", &[("следующий трек", r#"{"type":"Media","key":"Next"}"#)]),
    a("MediaSeek", "media", "{forward, seconds?}", "seek the playing media", &[("перемотай на 30 секунд вперёд", r#"{"type":"MediaSeek","forward":true,"seconds":30}"#)]),
    // display
    a("Brightness", "display", "{op: <LevelOp>, value?}", "display brightness (laptop panel or DDC/CI monitor)", &[("яркость 70", r#"{"type":"Brightness","op":"Set","value":70}"#)]),
    a("Display", "display", "{op: <DisplayOp>}", "monitors off, projection mode (Win+P). Resolution / scale / night light: OpenSettings display / nightlight", &[("выключи экран", r#"{"type":"Display","op":"ScreenOff"}"#), ("расширь экран", r#"{"type":"Display","op":"Extend"}"#)]),
    a("Screenshot", "display", "{path?, scope?: <ScreenshotScope>, clipboard?}", "screenshot of the screen, the active window or a selected region; to a file and / or the clipboard", &[("скриншот окна в буфер", r#"{"type":"Screenshot","scope":"Window","clipboard":true}"#)]),
    // system
    a("System", "system", "{op: <SystemOp>}", "lock / sleep / shutdown / restart / sign out (confirmed)", &[("заблокируй компьютер", r#"{"type":"System","op":"Lock"}"#)]),
    a("Radio", "system", "{kind: <RadioKind>, on}", "Wi-Fi / Bluetooth / all radios (airplane mode) on or off (confirmed)", &[("выключи блютуз", r#"{"type":"Radio","kind":"Bluetooth","on":false}"#)]),
    a("Query", "system", "{what: <QueryKind>}", "report time, battery, disk space, IP, network, CPU / RAM, volume, what is playing, the active window / page title, audio devices…", &[("сколько заряда", r#"{"type":"Query","what":"Battery"}"#), ("что сейчас играет", r#"{"type":"Query","what":"NowPlaying"}"#)]),
    a("Calculate", "system", "{expression}", "calculator (digits or spoken words)", &[("сколько будет 25 умножить на 4", r#"{"type":"Calculate","expression":"25*4"}"#)]),
    a("Timer", "system", "{seconds, message}", "timer / reminder after seconds", &[("таймер на 5 минут", r#"{"type":"Timer","seconds":300,"message":""}"#)]),
    a("Alarm", "system", "{at, message}", "alarm at a time of day", &[("разбуди меня в 7:30", r#"{"type":"Alarm","at":"07:30","message":""}"#)]),
    a("Notify", "system", "{title, body}", "desktop notification", &[("уведомление привет", r#"{"type":"Notify","title":"Orkestr","body":"привет"}"#)]),
    a("ClipboardSet", "system", "{text}", "put text on the clipboard", &[("скопируй в буфер привет", r#"{"type":"ClipboardSet","text":"привет"}"#)]),
    a("ClipboardGet", "system", "{}", "read the clipboard", &[("что в буфере", r#"{"type":"ClipboardGet"}"#)]),
    // files
    a("FileCreate", "files", "{path, content}", "create a file (relative = Documents)", &[("создай файл заметки.txt", r#"{"type":"FileCreate","path":"заметки.txt","content":""}"#)]),
    a("FolderCreate", "files", "{path}", "create a folder", &[("создай папку проекты", r#"{"type":"FolderCreate","path":"проекты"}"#)]),
    a("FileMove", "files", "{from, to}", "move / rename (confirmed)", &[("переименуй отчет.txt в итог.txt", r#"{"type":"FileMove","from":"отчет.txt","to":"итог.txt"}"#)]),
    a("FileCopy", "files", "{from, to}", "copy a file or folder", &[("скопируй отчет.txt на рабочий стол", r#"{"type":"FileCopy","from":"отчет.txt","to":"desktop"}"#)]),
    a("FileDelete", "files", "{path}", "to the recycle bin (confirmed)", &[("удали файл старое.txt", r#"{"type":"FileDelete","path":"старое.txt"}"#)]),
    a("FileSearch", "files", "{name, folder?}", "find files by name", &[("найди файл отчет", r#"{"type":"FileSearch","name":"отчет"}"#)]),
    a("EmptyRecycleBin", "files", "{}", "empty the recycle bin (confirmed)", &[("очисти корзину", r#"{"type":"EmptyRecycleBin"}"#)]),
    // flow
    a("Wait", "flow", "{ms}", "pause", &[("подожди секунду", r#"{"type":"Wait","ms":1000}"#)]),
    a("Speak", "flow", "{text}", "say something", &[("скажи привет", r#"{"type":"Speak","text":"привет"}"#)]),
    a("SetMode", "flow", "{mode: Orchestra|Assistant}", "switch Orkestr's UI mode", &[("режим ассистента", r#"{"type":"SetMode","mode":"Assistant"}"#)]),
    a("If", "flow", "{condition: {type: WindowActive|WindowExists, app} | {type: ProcessRunning, name}, then: [...], otherwise: [...]}", "conditional steps", &[("если хром открыт переключись", r#"{"type":"If","condition":{"type":"WindowExists","app":"chrome"},"then":[{"type":"FocusApp","app":"chrome"}],"otherwise":[]}"#)]),
];

/// Quoted values of a string-union TS type (`"A" | "B"` → `A|B`).
fn union_of<T: TS>() -> String {
    let inline = T::inline(&Config::default());
    let mut vals = Vec::new();
    let mut rest = inline.as_str();
    while let Some(s) = rest.find('"') {
        let after = &rest[s + 1..];
        let Some(e) = after.find('"') else { break };
        vals.push(&after[..e]);
        rest = &after[e + 1..];
    }
    vals.join("|")
}

/// `<Enum>` → its values from the protocol.
fn fill(params: &str) -> String {
    type Values = fn() -> String;
    let table: [(&str, Values); 13] = [
        ("<WindowOp>", union_of::<WindowOp>),
        ("<LevelOp>", union_of::<LevelOp>),
        ("<DesktopOp>", union_of::<DesktopOp>),
        ("<ScrollDirection>", union_of::<ScrollDirection>),
        ("<ScrollUnit>", union_of::<ScrollUnit>),
        ("<MouseButton>", union_of::<MouseButton>),
        ("<ScreenPoint>", union_of::<ScreenPoint>),
        ("<AudioDeviceKind>", union_of::<AudioDeviceKind>),
        ("<MediaKey>", union_of::<MediaKey>),
        ("<DisplayOp>", union_of::<DisplayOp>),
        ("<ScreenshotScope>", union_of::<ScreenshotScope>),
        ("<SystemOp>", union_of::<SystemOp>),
        ("<QueryKind>", union_of::<QueryKind>),
    ];
    let mut s = params.to_owned();
    for (k, f) in table {
        if s.contains(k) {
            s = s.replace(k, &f());
        }
    }
    s.replace("<RadioKind>", &union_of::<RadioKind>())
}

/// The vocabulary section of the planner prompt (one line per action + examples).
#[must_use]
pub fn prompt_vocabulary() -> String {
    let mut out = String::new();
    for s in ACTIONS {
        out.push_str(&format!("- {} {} — {}", s.name, fill(s.params), s.doc));
        for (req, json) in s.examples.iter().take(3) {
            out.push_str(&format!("\n    «{req}» → {json}"));
        }
        out.push('\n');
    }
    out
}

/// Family names in the order of the registry.
#[must_use]
pub fn families() -> Vec<&'static str> {
    let mut f: Vec<&'static str> = Vec::new();
    for s in ACTIONS {
        if !f.contains(&s.family) {
            f.push(s.family);
        }
    }
    f
}

/// Short capability summary for the conversation (Jarvis) prompt: what
/// the computer assistant can do, generated from the registry.
#[must_use]
pub fn capability_summary() -> String {
    let mut out = String::new();
    for fam in families() {
        let names: Vec<&str> = ACTIONS.iter().filter(|s| s.family == fam && fam != "flow").map(|s| s.doc).collect();
        if names.is_empty() {
            continue;
        }
        out.push_str(&format!("- {fam}: {}\n", names.join("; ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_protocol::methods::computer::ComputerAction;
    use std::collections::BTreeSet;

    #[test]
    fn every_protocol_action_is_in_the_registry() {
        let decl = ComputerAction::decl(&Config::default());
        let mut in_protocol = BTreeSet::new();
        let mut rest = decl.as_str();
        while let Some(i) = rest.find("\"type\": \"") {
            let after = &rest[i + 9..];
            let e = after.find('"').unwrap();
            in_protocol.insert(after[..e].to_owned());
            rest = &after[e..];
        }
        let in_registry: BTreeSet<String> = ACTIONS.iter().map(|s| s.name.to_owned()).collect();
        assert!(in_protocol.len() >= 50, "{in_protocol:?}");
        assert_eq!(in_protocol, in_registry, "the planner registry must list every ComputerAction");
    }

    #[test]
    fn examples_parse_as_their_action() {
        for s in ACTIONS {
            assert!(!s.examples.is_empty(), "{} has no example", s.name);
            for (req, json) in s.examples {
                let a: ComputerAction = serde_json::from_str(json).unwrap_or_else(|e| panic!("{req}: {json}: {e}"));
                assert_eq!(a.type_name(), s.name, "{req}");
            }
        }
    }

    #[test]
    fn prompt_lists_enum_values_from_the_protocol() {
        let p = prompt_vocabulary();
        for v in [
            "SnapTopLeft",
            "AlwaysOnTop",
            "MinimizeAll",
            "MuteOn",
            "Scale",
            "NowPlaying",
            "Battery",
            "TaskView",
            "Extend",
        ] {
            assert!(p.contains(v), "{v} missing");
        }
        assert!(!p.contains('<'), "unfilled placeholder");
        assert!(capability_summary().contains("volume"));
    }
}
