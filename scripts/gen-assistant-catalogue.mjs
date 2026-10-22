#!/usr/bin/env node
// Generates the built-in computer-assistant command catalogue (ADR-0035):
// phrase templates (ru + en, with {slots}) × the source tables in
// crates/computer/data/{apps,settings,sites,folders}.json + hand-written
// key / window / media / system / file commands → crates/computer/data/catalogue.json
// (embedded by the `orkestr-computer` crate; a Rust test checks ≥ 1000
// unique commands load and parse).
//
// Usage: node scripts/gen-assistant-catalogue.mjs [--check]
//   --check: exit 1 when the committed catalogue.json is out of date.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const data = join(root, "crates", "computer", "data");
const load = (f) => JSON.parse(readFileSync(join(data, f), "utf8"));
const { apps } = load("apps.json");
const { pages } = load("settings.json");
const { sites } = load("sites.json");
const { folders } = load("folders.json");

const sets = new Map();
const ids = new Set();
function set(id, name, description, context) {
  if (!sets.has(id)) sets.set(id, { id, name, description, enabled: true, builtin: true, ...(context ? { context } : {}), commands: [] });
  return sets.get(id);
}
/** Adds a command; phrases are de-duplicated, ids must be unique. */
function cmd(setId, id, category, en, ru, phrases, steps, safety = "Safe", context) {
  if (ids.has(id)) throw new Error(`duplicate command id ${id}`);
  ids.add(id);
  const s = sets.get(setId);
  if (!s) throw new Error(`unknown set ${setId}`);
  s.commands.push({ id, category, title: { en, ru }, phrases: [...new Set(phrases.map((p) => p.trim()).filter(Boolean))], steps, safety, ...(context ? { context } : {}) });
}
const cap = (s) => s.charAt(0).toUpperCase() + s.slice(1);
const each = (names, tpls) => names.flatMap((n) => tpls.map((t) => t.replace("%", n)));

// ── sets ───────────────────────────────────────────────────────────────────
const BROWSERS = { processes: ["chrome", "msedge", "firefox", "opera", "browser", "brave", "vivaldi"] };
set("apps", "Apps", "Open, close, switch to, minimize, maximize, restart and show apps.");
set("windows", "Windows", "Window management: focus, minimize, maximize, snap, monitors, desktops.");
set("keys", "Keyboard", "Hotkeys and keys: copy, paste, undo, save, find, tabs, zoom, navigation.");
set("scroll", "Scroll & mouse", "Scrolling, clicks and UI elements by name.");
set("media", "Media & sound", "Playback, volume and brightness.");
set("system", "System", "Lock, sleep, power, screenshots, clipboard, timers, processes.");
set("settings", "Windows Settings", "Opens Windows Settings pages (ms-settings:).");
set("web", "Web", "Sites and web search.");
set("files", "Files & folders", "Known folders, create / move / delete files (with confirmation).");
set("assistant", "Assistant", "Assistant modes and helpers.");
set("browser", "Browser (context)", "Only while a browser window is active.", BROWSERS);
set("office", "Office (context)", "Only while Word, Excel or PowerPoint is active.", { processes: ["winword", "excel", "powerpnt"] });
set("explorer", "File Explorer (context)", "Only while File Explorer is active.", { processes: ["explorer"] });
set("media-player", "Media players (context)", "Only while a media player is active.", { processes: ["vlc", "potplayermini64", "aimp", "spotify", "microsoft.media.player"] });

// ── apps: open / close / switch / minimize / maximize / restart / show ────
for (const a of apps) {
  const en = a.en, ru = a.ru;
  const e0 = en[0], r0 = ru[0];
  const app = a.key;
  cmd("apps", `app.${app}.open`, "apps", `Open ${e0}`, `Открыть ${r0}`,
    [...each(ru, ["открой %", "запусти %", "включи %"]), ...each(en, ["open %", "launch %", "start %", "run %"])],
    [{ type: "Launch", target: app }]);
  cmd("apps", `app.${app}.close`, "apps", `Close ${e0}`, `Закрыть ${r0}`,
    [...each(ru, ["закрой %", "выйди из %", "выключи %"]), ...each(en, ["close %", "quit %", "exit %"])],
    [{ type: "CloseApp", app, discard: false }]);
  cmd("apps", `app.${app}.switch`, "apps", `Switch to ${e0}`, `Переключиться на ${r0}`,
    [...each(ru, ["переключись на %", "перейди в %", "перейди к %", "активируй %"]), ...each(en, ["switch to %", "focus %", "go to %"])],
    [{ type: "FocusApp", app }]);
  cmd("apps", `app.${app}.minimize`, "apps", `Minimize ${e0}`, `Свернуть ${r0}`,
    [...each(ru, ["сверни %"]), ...each(en, ["minimize %"])], [{ type: "Window", op: "Minimize", app }]);
  cmd("apps", `app.${app}.maximize`, "apps", `Maximize ${e0}`, `Развернуть ${r0}`,
    [...each(ru, ["разверни %", "разверни % на весь экран"]), ...each(en, ["maximize %"])], [{ type: "Window", op: "Maximize", app }]);
  cmd("apps", `app.${app}.show`, "apps", `Show ${e0} (open if needed)`, `Показать ${r0} (открыть при необходимости)`,
    [...each(ru, ["покажи %", "где %"]), ...each(en, ["show %", "bring up %"])],
    [{ type: "If", condition: { type: "WindowExists", app }, then: [{ type: "FocusApp", app }], otherwise: [{ type: "Launch", target: app }] }]);
  if (!a.noKill) {
    cmd("apps", `app.${app}.restart`, "apps", `Restart ${e0}`, `Перезапустить ${r0}`,
      [...each(ru, ["перезапусти %"]), ...each(en, ["restart %", "relaunch %"])],
      [{ type: "CloseApp", app, discard: false }, { type: "Wait", ms: 1500 }, { type: "Launch", target: app }]);
  }
}
// generic app commands (any Start-menu app / exe by name)
cmd("apps", "app.any.open", "apps", "Open an app", "Открыть приложение",
  ["открой {app}", "запусти {app}", "открой программу {app}", "запусти приложение {app}", "open {app}", "launch {app}", "start {app}", "run {app}"],
  [{ type: "Launch", target: "{app}" }]);
cmd("apps", "app.any.close", "apps", "Close an app", "Закрыть приложение",
  ["закрой {app}", "закрой программу {app}", "выйди из {app}", "close {app}", "quit {app}", "exit {app}"], [{ type: "CloseApp", app: "{app}", discard: false }]);
cmd("apps", "app.any.discard", "apps", "Close an app without saving", "Закрыть приложение без сохранения",
  ["закрой {app} без сохранения", "закрой {app} не сохраняя", "выйди из {app} без сохранения", "close {app} without saving", "close {app} and discard changes", "quit {app} without saving"],
  [{ type: "CloseApp", app: "{app}", discard: true }], "Confirm");
cmd("apps", "app.any.switch", "apps", "Switch to an app", "Переключиться на приложение",
  ["переключись на {app}", "перейди в {app}", "перейди к {app}", "switch to {app}", "focus {app}", "go to {app}"], [{ type: "FocusApp", app: "{app}" }]);
cmd("apps", "app.any.minimize", "apps", "Minimize an app", "Свернуть приложение", ["сверни {app}", "minimize {app}"], [{ type: "Window", op: "Minimize", app: "{app}" }]);
cmd("apps", "app.any.maximize", "apps", "Maximize an app", "Развернуть приложение", ["разверни {app}", "maximize {app}"], [{ type: "Window", op: "Maximize", app: "{app}" }]);
cmd("apps", "app.any.kill", "apps", "Kill a process", "Завершить процесс",
  ["убей процесс {name}", "заверши процесс {name}", "убей {name}", "kill process {name}", "kill {name}", "terminate {name}"], [{ type: "KillProcess", name: "{name}" }], "Dangerous");
cmd("apps", "app.any.openfile", "apps", "Open a file or folder", "Открыть файл или папку",
  ["открой файл {path}", "открой папку {path}", "open file {path}", "open folder {path}"], [{ type: "OpenPath", path: "{path}" }]);

// ── windows ────────────────────────────────────────────────────────────────
const win = (id, en, ru, phrases, steps) => cmd("windows", `win.${id}`, "windows", en, ru, phrases, steps);
win("minimize", "Minimize the window", "Свернуть окно", ["сверни окно", "сверни это окно", "сверни", "minimize window", "minimize this window", "minimize"], [{ type: "Window", op: "Minimize" }]);
win("maximize", "Maximize the window", "Развернуть окно", ["разверни окно", "разверни на весь экран", "разверни", "maximize window", "maximize", "make it full screen"], [{ type: "Window", op: "Maximize" }]);
win("restore", "Restore the window", "Восстановить окно", ["восстанови окно", "верни размер окна", "restore window", "restore"], [{ type: "Window", op: "Restore" }]);
win("close", "Close the window", "Закрыть окно", ["закрой окно", "закрой это окно", "close window", "close this window"], [{ type: "Window", op: "Close" }]);
win("snapleft", "Snap left", "Окно влево", ["окно влево", "прикрепи окно влево", "сдвинь окно влево", "snap left", "snap window left", "move window left"], [{ type: "Window", op: "SnapLeft" }]);
win("snapright", "Snap right", "Окно вправо", ["окно вправо", "прикрепи окно вправо", "сдвинь окно вправо", "snap right", "snap window right", "move window right"], [{ type: "Window", op: "SnapRight" }]);
win("nextmonitor", "Move to the next monitor", "Перенести на следующий монитор", ["перенеси окно на другой монитор", "перенеси на следующий монитор", "окно на второй монитор", "move window to the next monitor", "move to other monitor", "next monitor"], [{ type: "Window", op: "NextMonitor" }]);
win("prevmonitor", "Move to the previous monitor", "Перенести на предыдущий монитор", ["перенеси окно на предыдущий монитор", "окно на первый монитор", "move window to the previous monitor", "previous monitor"], [{ type: "Window", op: "PreviousMonitor" }]);
win("switchnext", "Switch window", "Переключить окно", ["переключи окно", "следующее окно", "другое окно", "switch window", "next window", "alt tab"], [{ type: "Window", op: "SwitchNext" }]);
win("desktop", "Show the desktop", "Показать рабочий стол", ["покажи рабочий стол", "сверни все окна", "сверни всё", "show desktop", "minimize all windows", "minimize everything"], [{ type: "Window", op: "ShowDesktop" }]);
win("taskview", "Task view", "Представление задач", ["представление задач", "покажи все окна", "task view", "show all windows"], [{ type: "Keys", combo: "win+tab" }]);
win("vdnew", "New virtual desktop", "Новый рабочий стол", ["новый рабочий стол", "создай рабочий стол", "new desktop", "new virtual desktop"], [{ type: "Keys", combo: "win+ctrl+d" }]);
win("vdclose", "Close virtual desktop", "Закрыть рабочий стол", ["закрой рабочий стол", "close desktop", "close virtual desktop"], [{ type: "Keys", combo: "win+ctrl+f4" }]);
win("vdnext", "Next virtual desktop", "Следующий рабочий стол", ["следующий рабочий стол", "рабочий стол вправо", "next desktop"], [{ type: "Keys", combo: "win+ctrl+right" }]);
win("vdprev", "Previous virtual desktop", "Предыдущий рабочий стол", ["предыдущий рабочий стол", "рабочий стол влево", "previous desktop"], [{ type: "Keys", combo: "win+ctrl+left" }]);
win("list", "List windows", "Список окон", ["какие окна открыты", "список окон", "покажи список окон", "list windows", "what windows are open"], [{ type: "ListWindows" }]);
win("fullscreen", "Toggle full screen", "Полноэкранный режим", ["полноэкранный режим", "во весь экран", "full screen", "toggle fullscreen"], [{ type: "Keys", combo: "f11" }]);

// ── keys ───────────────────────────────────────────────────────────────────
const KEYS = [
  ["copy", "ctrl+c", "Copy", "Копировать", ["скопируй", "копировать", "копируй", "copy", "copy that"]],
  ["paste", "ctrl+v", "Paste", "Вставить", ["вставь", "вставить", "paste", "paste it"]],
  ["cut", "ctrl+x", "Cut", "Вырезать", ["вырежи", "вырезать", "cut", "cut it"]],
  ["undo", "ctrl+z", "Undo", "Отменить", ["отмени", "отменить", "отмени действие", "undo", "undo that"]],
  ["redo", "ctrl+y", "Redo", "Повторить", ["повтори действие", "верни отмену", "redo"]],
  ["selectall", "ctrl+a", "Select all", "Выделить всё", ["выдели всё", "выделить всё", "выдели весь текст", "select all", "select everything"]],
  ["save", "ctrl+s", "Save", "Сохранить", ["сохрани", "сохранить", "сохрани файл", "save", "save file", "save it"]],
  ["saveas", "ctrl+shift+s", "Save as", "Сохранить как", ["сохрани как", "сохранить как", "save as"]],
  ["new", "ctrl+n", "New", "Создать", ["новый документ", "создай новый документ", "new document", "new file"]],
  ["openfile", "ctrl+o", "Open file dialog", "Диалог открытия", ["диалог открытия", "открой файл диалог", "open file dialog"]],
  ["print", "ctrl+p", "Print", "Печать", ["распечатай", "печать", "напечатай документ", "print", "print this"]],
  ["find", "ctrl+f", "Find", "Найти", ["поиск по странице", "найди на странице", "найти в тексте", "find on page", "find in page"]],
  ["replace", "ctrl+h", "Replace", "Заменить", ["замени текст", "найти и заменить", "find and replace", "replace text"]],
  ["closetab", "ctrl+w", "Close tab", "Закрыть вкладку", ["закрой вкладку", "закрыть вкладку", "close tab", "close this tab"]],
  ["newtab", "ctrl+t", "New tab", "Новая вкладка", ["новая вкладка", "открой новую вкладку", "new tab", "open new tab"]],
  ["reopentab", "ctrl+shift+t", "Reopen closed tab", "Вернуть закрытую вкладку", ["верни вкладку", "открой закрытую вкладку", "восстанови вкладку", "reopen tab", "reopen closed tab"]],
  ["nexttab", "ctrl+tab", "Next tab", "Следующая вкладка", ["следующая вкладка", "вкладка вправо", "next tab"]],
  ["prevtab", "ctrl+shift+tab", "Previous tab", "Предыдущая вкладка", ["предыдущая вкладка", "вкладка влево", "previous tab"]],
  ["refresh", "f5", "Refresh", "Обновить", ["обнови", "обнови страницу", "перезагрузи страницу", "refresh", "reload", "reload page"]],
  ["hardrefresh", "ctrl+f5", "Hard refresh", "Обновить без кэша", ["обнови без кэша", "жесткая перезагрузка", "hard refresh", "hard reload"]],
  ["zoomin", "ctrl+plus", "Zoom in", "Увеличить масштаб", ["увеличь масштаб", "приблизь", "крупнее", "zoom in", "make it bigger"]],
  ["zoomout", "ctrl+minus", "Zoom out", "Уменьшить масштаб", ["уменьши масштаб", "отдали", "мельче", "zoom out", "make it smaller"]],
  ["zoomreset", "ctrl+0", "Reset zoom", "Сбросить масштаб", ["сбрось масштаб", "обычный масштаб", "reset zoom", "actual size"]],
  ["bold", "ctrl+b", "Bold", "Жирный", ["жирный шрифт", "сделай жирным", "bold", "make it bold"]],
  ["italic", "ctrl+i", "Italic", "Курсив", ["курсив", "сделай курсивом", "italic", "make it italic"]],
  ["underline", "ctrl+u", "Underline", "Подчеркнуть", ["подчеркни", "подчеркнуть", "underline"]],
  ["escape", "esc", "Escape", "Escape", ["нажми эскейп", "эскейп", "отмена окна", "press escape", "escape"]],
  ["enter", "enter", "Enter", "Enter", ["нажми энтер", "нажми ввод", "энтер", "подтверди", "press enter", "hit enter", "enter"]],
  ["tab", "tab", "Tab", "Tab", ["нажми таб", "следующее поле", "press tab", "next field"]],
  ["shifttab", "shift+tab", "Previous field", "Предыдущее поле", ["предыдущее поле", "previous field"]],
  ["backspace", "backspace", "Backspace", "Стереть символ", ["сотри символ", "нажми бэкспейс", "backspace", "press backspace"]],
  ["deleteword", "ctrl+backspace", "Delete word", "Стереть слово", ["сотри слово", "удали слово", "delete word", "delete last word"]],
  ["delete", "delete", "Delete key", "Клавиша Delete", ["нажми делит", "нажми delete", "press delete"]],
  ["space", "space", "Space", "Пробел", ["нажми пробел", "пробел", "press space", "space"]],
  ["home", "home", "Home", "В начало строки", ["в начало строки", "home", "go to line start"]],
  ["end", "end", "End", "В конец строки", ["в конец строки", "end key", "go to line end"]],
  ["docstart", "ctrl+home", "Start of document", "В начало документа", ["в начало документа", "в самое начало", "go to the beginning", "start of document"]],
  ["docend", "ctrl+end", "End of document", "В конец документа", ["в конец документа", "в самый конец", "go to the end", "end of document"]],
  ["up", "up", "Arrow up", "Стрелка вверх", ["стрелка вверх", "arrow up"]],
  ["down", "down", "Arrow down", "Стрелка вниз", ["стрелка вниз", "arrow down"]],
  ["left", "left", "Arrow left", "Стрелка влево", ["стрелка влево", "arrow left"]],
  ["right", "right", "Arrow right", "Стрелка вправо", ["стрелка вправо", "arrow right"]],
  ["wordleft", "ctrl+left", "Previous word", "Слово назад", ["на слово назад", "previous word"]],
  ["wordright", "ctrl+right", "Next word", "Слово вперёд", ["на слово вперед", "next word"]],
  ["start", "win", "Start menu", "Меню Пуск", ["открой пуск", "меню пуск", "пуск", "open start", "start menu"]],
  ["winsearch", "win+s", "Windows search", "Поиск Windows", ["поиск windows", "открой поиск", "windows search", "open search"]],
  ["run", "win+r", "Run dialog", "Окно Выполнить", ["окно выполнить", "открой выполнить", "run dialog", "open run"]],
  ["quicksettings", "win+a", "Quick settings", "Быстрые настройки", ["быстрые настройки", "центр управления", "quick settings", "action center"]],
  ["notificationcenter", "win+n", "Notification center", "Центр уведомлений", ["центр уведомлений", "покажи уведомления", "notification center", "show notifications"]],
  ["emoji", "win+period", "Emoji panel", "Панель эмодзи", ["эмодзи", "смайлики", "панель эмодзи", "emoji", "emoji panel"]],
  ["clipboardhistory", "win+v", "Clipboard history", "Журнал буфера обмена", ["история буфера обмена", "журнал буфера", "clipboard history"]],
  ["winsettings", "win+i", "Windows Settings", "Параметры Windows", ["открой параметры", "открой настройки windows", "open windows settings"]],
  ["winexplorer", "win+e", "File Explorer", "Проводник", ["новое окно проводника", "new explorer window"]],
  ["project", "win+p", "Project to a screen", "Проецировать", ["проецировать", "второй экран режим", "project screen", "display mode"]],
  ["snip", "win+shift+s", "Snip a region", "Вырезать фрагмент экрана", ["вырежи фрагмент экрана", "фрагмент экрана", "snip", "snip screen", "screen snip"]],
  ["widgets", "win+w", "Widgets", "Виджеты", ["виджеты", "открой виджеты", "widgets", "open widgets"]],
  ["quicklink", "win+x", "Quick link menu", "Меню быстрых ссылок", ["меню быстрых ссылок", "quick link menu"]],
  ["voicetyping", "win+h", "Voice typing", "Голосовой ввод", ["голосовой ввод", "диктовка windows", "voice typing", "windows dictation"]],
  ["gamebar", "win+g", "Game Bar", "Игровая панель", ["игровая панель", "game bar", "xbox game bar"]],
  ["taskmgrkeys", "ctrl+shift+esc", "Task Manager (hotkey)", "Диспетчер задач (клавиши)", ["вызови диспетчер задач", "task manager hotkey"]],
  ["rename", "f2", "Rename", "Переименовать", ["переименуй", "переименовать", "rename", "rename it"]],
  ["properties", "alt+enter", "Properties", "Свойства", ["свойства", "покажи свойства", "properties", "show properties"]],
  ["contextmenu", "shift+f10", "Context menu", "Контекстное меню", ["контекстное меню", "правое меню", "context menu"]],
  ["altf4", "alt+f4", "Alt+F4", "Alt+F4", ["альт ф4", "alt f4"]],
  ["help", "f1", "Help", "Справка", ["справка", "открой справку", "help", "open help"]],
  ["lockkeys", "win+l", "Lock (hotkey)", "Блокировка (клавиши)", ["win l", "вин л"]],
  ["magup", "win+plus", "Magnifier zoom in", "Лупа крупнее", ["лупа крупнее", "magnifier zoom in"]],
  ["magdown", "win+minus", "Magnifier zoom out", "Лупа мельче", ["лупа мельче", "magnifier zoom out"]],
  ["magoff", "win+esc", "Magnifier off", "Выключить лупу", ["выключи лупу", "magnifier off"]],
  ["langswitch", "win+space", "Switch keyboard layout", "Сменить раскладку", ["смени раскладку", "переключи язык", "другой язык ввода", "switch layout", "change keyboard language"]],
  ["screenshotfile", "win+printscreen", "Screenshot to Pictures", "Снимок экрана в Изображения", ["снимок экрана в файл", "win print screen"]],
  ["printscreen", "printscreen", "Print Screen", "Print Screen", ["принтскрин", "print screen"]],
];
for (const [id, combo, en, ru, phrases] of KEYS) cmd("keys", `key.${id}`, "keys", en, ru, phrases, [{ type: "Keys", combo }]);
cmd("keys", "key.press", "keys", "Press a key", "Нажать клавишу", ["нажми {key}", "нажми клавишу {key}", "нажми кнопки {key}", "press {key}", "hit {key}", "press key {key}"], [{ type: "Keys", combo: "{key}" }]);
cmd("keys", "key.pressn", "keys", "Press a key N times", "Нажать клавишу N раз",
  ["нажми {key} {n} раз", "нажми {key} {n} раза", "press {key} {n} times"], [{ type: "Keys", combo: "{key}", times: "{n}" }]);
for (let i = 1; i <= 9; i++) {
  cmd("keys", `key.tab${i}`, "keys", `Go to tab ${i}`, `Перейти на вкладку ${i}`, [`вкладка ${i}`, `перейди на вкладку ${i}`, `tab ${i}`, `go to tab ${i}`], [{ type: "Keys", combo: `ctrl+${i}` }]);
}

// ── typing / clipboard / clicks / scroll ─────────────────────────────────
cmd("keys", "type.text", "typing", "Type text", "Напечатать текст",
  ["напечатай {text}", "напиши {text}", "введи {text}", "набери {text}", "впиши {text}", "type {text}", "write {text}", "enter text {text}"], [{ type: "TypeText", text: "{text}" }]);
cmd("keys", "type.line", "typing", "Type a line and press Enter", "Напечатать строку и Enter",
  ["напиши и отправь {text}", "напечатай и нажми энтер {text}", "type and send {text}", "send message {text}"], [{ type: "TypeText", text: "{text}" }, { type: "Keys", combo: "enter" }]);
cmd("keys", "type.newline", "typing", "New line", "Новая строка", ["новая строка", "с новой строки", "new line", "newline"], [{ type: "Keys", combo: "enter" }]);
const sc = (id, en, ru, phrases, step) => cmd("scroll", `scroll.${id}`, "scroll", en, ru, phrases, [step]);
sc("down", "Scroll down", "Прокрутить вниз", ["прокрути вниз", "листай вниз", "вниз", "ниже", "scroll down", "down"], { type: "Scroll", direction: "Down", amount: 5 });
sc("up", "Scroll up", "Прокрутить вверх", ["прокрути вверх", "листай вверх", "вверх", "выше", "scroll up", "up"], { type: "Scroll", direction: "Up", amount: 5 });
sc("downn", "Scroll down N", "Прокрутить вниз на N", ["прокрути вниз на {n}", "прокрути на {n} вниз", "scroll down {n}", "scroll down by {n}"], { type: "Scroll", direction: "Down", amount: "{n}" });
sc("upn", "Scroll up N", "Прокрутить вверх на N", ["прокрути вверх на {n}", "прокрути на {n} вверх", "scroll up {n}", "scroll up by {n}"], { type: "Scroll", direction: "Up", amount: "{n}" });
sc("left", "Scroll left", "Прокрутить влево", ["прокрути влево", "scroll left"], { type: "Scroll", direction: "Left", amount: 5 });
sc("right", "Scroll right", "Прокрутить вправо", ["прокрути вправо", "scroll right"], { type: "Scroll", direction: "Right", amount: 5 });
sc("pagedown", "Page down", "Страница вниз", ["страница вниз", "следующая страница", "листни страницу", "page down", "next page"], { type: "Scroll", direction: "Down", amount: 1, unit: "Pages" });
sc("pageup", "Page up", "Страница вверх", ["страница вверх", "предыдущая страница", "page up", "previous page"], { type: "Scroll", direction: "Up", amount: 1, unit: "Pages" });
sc("pagesdown", "N pages down", "N страниц вниз", ["прокрути {n} страниц вниз", "вниз на {n} страниц", "scroll down {n} pages", "{n} pages down"], { type: "Scroll", direction: "Down", amount: "{n}", unit: "Pages" });
sc("pagesup", "N pages up", "N страниц вверх", ["прокрути {n} страниц вверх", "вверх на {n} страниц", "scroll up {n} pages", "{n} pages up"], { type: "Scroll", direction: "Up", amount: "{n}", unit: "Pages" });
sc("top", "Scroll to the top", "В самый верх", ["прокрути в самый верх", "в начало страницы", "наверх страницы", "scroll to top", "go to top"], { type: "Scroll", direction: "Up", unit: "Edge" });
sc("bottom", "Scroll to the bottom", "В самый низ", ["прокрути в самый низ", "в конец страницы", "scroll to bottom", "go to bottom"], { type: "Scroll", direction: "Down", unit: "Edge" });
sc("element", "Click an element", "Нажать на элемент", ["нажми на кнопку {name}", "нажми на {name}", "кликни {name}", "кликни по {name}", "щелкни {name}", "click {name}", "click on {name}", "press button {name}"], { type: "ClickElement", name: "{name}" });
sc("click", "Click", "Клик", ["кликни", "клик", "щелкни", "click", "left click"], { type: "MouseClick", button: "Left" });
sc("rightclick", "Right click", "Правый клик", ["правый клик", "кликни правой кнопкой", "right click"], { type: "MouseClick", button: "Right" });
sc("doubleclick", "Double click", "Двойной клик", ["двойной клик", "дважды кликни", "double click"], { type: "MouseClick", button: "Double" });

// ── media / volume / brightness ───────────────────────────────────────────
const md = (id, en, ru, phrases, steps) => cmd("media", `media.${id}`, "media", en, ru, phrases, steps);
md("playpause", "Play / pause", "Пауза / продолжить", ["пауза", "поставь на паузу", "продолжи воспроизведение", "играй", "play", "pause", "resume", "play pause"], [{ type: "Media", key: "PlayPause" }]);
md("next", "Next track", "Следующий трек", ["следующий трек", "следующая песня", "дальше", "next track", "next song", "skip"], [{ type: "Media", key: "Next" }]);
md("prev", "Previous track", "Предыдущий трек", ["предыдущий трек", "предыдущая песня", "previous track", "previous song"], [{ type: "Media", key: "Previous" }]);
md("stop", "Stop playback", "Остановить воспроизведение", ["останови музыку", "стоп музыка", "останови воспроизведение", "stop music", "stop playback"], [{ type: "Media", key: "Stop" }]);
md("volup", "Volume up", "Громче", ["громче", "сделай громче", "прибавь звук", "увеличь громкость", "louder", "volume up", "turn it up"], [{ type: "Volume", op: "Up", value: 10 }]);
md("voldown", "Volume down", "Тише", ["тише", "сделай тише", "убавь звук", "уменьши громкость", "quieter", "volume down", "turn it down"], [{ type: "Volume", op: "Down", value: 10 }]);
md("volupn", "Volume up by N%", "Громче на N%", ["громче на {n}", "прибавь звук на {n}", "volume up by {n}", "louder by {n}"], [{ type: "Volume", op: "Up", value: "{n}" }]);
md("voldownn", "Volume down by N%", "Тише на N%", ["тише на {n}", "убавь звук на {n}", "volume down by {n}", "quieter by {n}"], [{ type: "Volume", op: "Down", value: "{n}" }]);
md("volset", "Set volume", "Установить громкость", ["громкость {n}", "громкость на {n}", "поставь громкость {n}", "set volume to {n}", "volume {n}"], [{ type: "Volume", op: "Set", value: "{n}" }]);
md("mute", "Mute / unmute", "Выключить / включить звук", ["выключи звук", "включи звук", "без звука", "mute", "unmute", "mute sound"], [{ type: "Volume", op: "Mute" }]);
md("brup", "Brightness up", "Ярче", ["ярче", "сделай ярче", "прибавь яркость", "brighter", "brightness up"], [{ type: "Brightness", op: "Up", value: 10 }]);
md("brdown", "Brightness down", "Темнее", ["темнее", "сделай темнее", "убавь яркость", "dimmer", "brightness down"], [{ type: "Brightness", op: "Down", value: 10 }]);
md("brset", "Set brightness", "Установить яркость", ["яркость {n}", "яркость на {n}", "поставь яркость {n}", "set brightness to {n}", "brightness {n}"], [{ type: "Brightness", op: "Set", value: "{n}" }]);
for (const v of [0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100]) {
  md(`vol${v}`, `Volume ${v}%`, `Громкость ${v}%`, [`громкость ${v} процентов`, `volume ${v} percent`], [{ type: "Volume", op: "Set", value: v }]);
  if (v > 0) md(`br${v}`, `Brightness ${v}%`, `Яркость ${v}%`, [`яркость ${v} процентов`, `brightness ${v} percent`], [{ type: "Brightness", op: "Set", value: v }]);
}

// ── system ─────────────────────────────────────────────────────────────────
const sy = (id, en, ru, phrases, steps, safety) => cmd("system", `sys.${id}`, "system", en, ru, phrases, steps, safety);
sy("lock", "Lock the computer", "Заблокировать компьютер", ["заблокируй компьютер", "заблокируй экран", "блокировка", "lock computer", "lock the screen", "lock"], [{ type: "System", op: "Lock" }]);
sy("sleep", "Sleep", "Спящий режим", ["спящий режим", "усыпи компьютер", "в сон", "sleep", "go to sleep", "put computer to sleep"], [{ type: "System", op: "Sleep" }], "Confirm");
sy("shutdown", "Shut down", "Выключить компьютер", ["выключи компьютер", "завершение работы", "shut down", "shutdown computer", "turn off computer"], [{ type: "System", op: "Shutdown" }], "Dangerous");
sy("restart", "Restart", "Перезагрузить компьютер", ["перезагрузи компьютер", "перезагрузка", "restart computer", "reboot"], [{ type: "System", op: "Restart" }], "Dangerous");
sy("signout", "Sign out", "Выйти из системы", ["выйди из системы", "выход из учетной записи", "sign out", "log off", "log out"], [{ type: "System", op: "SignOut" }], "Dangerous");
sy("screenshot", "Take a screenshot", "Сделать скриншот", ["сделай скриншот", "сделай снимок экрана", "скриншот", "take a screenshot", "screenshot", "capture screen"], [{ type: "Screenshot" }]);
sy("clipread", "Read the clipboard", "Что в буфере обмена", ["что в буфере обмена", "прочитай буфер обмена", "покажи буфер обмена", "read clipboard", "what is in the clipboard"], [{ type: "ClipboardGet" }]);
sy("clipset", "Copy text to the clipboard", "Скопировать текст в буфер", ["скопируй в буфер {text}", "положи в буфер {text}", "copy to clipboard {text}"], [{ type: "ClipboardSet", text: "{text}" }]);
sy("processes", "List running apps", "Список запущенных программ", ["какие программы запущены", "список процессов", "покажи процессы", "list processes", "what is running", "show running apps"], [{ type: "ListProcesses" }]);
sy("timer", "Set a timer", "Поставить таймер", ["поставь таймер на {duration}", "таймер на {duration}", "засеки {duration}", "set a timer for {duration}", "timer for {duration}", "start a {duration} timer"], [{ type: "Timer", seconds: "{duration}", message: "" }]);
sy("remind", "Remind me", "Напомнить", ["напомни через {duration} {text}", "напомни мне через {duration} {text}", "remind me in {duration} to {text}", "remind me in {duration} {text}"], [{ type: "Timer", seconds: "{duration}", message: "{text}" }]);
sy("notify", "Show a notification", "Показать уведомление", ["покажи уведомление {text}", "уведомление {text}", "show notification {text}", "notify me {text}"], [{ type: "Notify", title: "Orkestr", body: "{text}" }]);
sy("say", "Say something", "Сказать", ["скажи {text}", "произнеси {text}", "say {text}", "speak {text}"], [{ type: "Speak", text: "{text}" }]);
sy("taskmgr", "Open Task Manager", "Открыть диспетчер задач", ["покажи диспетчер задач", "show task manager"], [{ type: "Launch", target: "taskmgr" }]);
for (const m of [1, 2, 3, 5, 10, 15, 20, 25, 30, 45, 60]) {
  sy(`timer${m}`, `${m}-minute timer`, `Таймер на ${m} мин`, [`таймер ${m} минут`, `timer ${m} minutes`, `${m} minute timer`], [{ type: "Timer", seconds: m * 60, message: "" }]);
}

// ── settings pages ─────────────────────────────────────────────────────────
for (const p of pages) {
  cmd("settings", `settings.${p.page}`, "settings", `${cap(p.en[0])} settings`, `Настройки: ${p.ru[0]}`,
    [...each(p.ru, ["открой настройки %", "настройки %", "параметры %", "открой параметры %"]), ...each(p.en, ["open % settings", "% settings", "settings %", "go to % settings"])],
    [{ type: "OpenSettings", page: p.page }]);
}
cmd("settings", "settings.home", "settings", "Windows Settings", "Параметры Windows", ["открой настройки", "открой параметры", "настройки windows", "open settings", "windows settings"], [{ type: "OpenSettings", page: "" }]);
cmd("settings", "settings.any", "settings", "Open a Settings page", "Открыть страницу настроек", ["открой настройки {page}", "настройки {page}", "open {page} settings", "{page} settings"], [{ type: "OpenSettings", page: "{page}" }]);

// ── web ────────────────────────────────────────────────────────────────────
for (const s of sites) {
  cmd("web", `site.${s.key}`, "web", `Open ${s.en[0]}`, `Открыть ${s.ru[0]}`,
    [...each(s.ru, ["открой %", "зайди на %", "открой сайт %", "перейди на %"]), ...each(s.en, ["open %", "go to %", "open site %", "visit %"])],
    [{ type: "OpenUrl", url: s.url }]);
  if (s.search) {
    cmd("web", `search.${s.key}`, "web", `Search ${s.en[0]}`, `Поиск: ${s.ru[0]}`,
      [...each(s.ru, ["найди на % {query}", "поищи на % {query}", "поиск на % {query}", "найди в % {query}", "% найди {query}"]), ...each(s.en, ["search % for {query}", "search on % {query}", "find on % {query}", "% search {query}"])],
      [{ type: "WebSearch", query: "{query}", engine: s.key }]);
  }
}
cmd("web", "search.default", "web", "Web search", "Поиск в интернете",
  ["найди в интернете {query}", "поищи в интернете {query}", "загугли {query}", "найди {query}", "поищи {query}", "search the web for {query}", "search for {query}", "google {query}", "look up {query}"],
  [{ type: "WebSearch", query: "{query}" }]);
cmd("web", "site.any", "web", "Open a web site", "Открыть сайт", ["открой сайт {site}", "зайди на сайт {site}", "зайди на {site}", "open website {site}", "go to website {site}", "visit {site}"], [{ type: "OpenUrl", url: "{site}" }]);

// ── files ──────────────────────────────────────────────────────────────────
for (const f of folders) {
  cmd("files", `folder.${f.key}`, "files", `Open ${f.en[0]}`, `Открыть ${f.ru[0]}`,
    [...each(f.ru, ["открой %", "покажи %", "открой папку %"]), ...each(f.en, ["open %", "show %", "open folder %"])],
    [{ type: "OpenPath", path: f.key }]);
}
cmd("files", "file.create", "files", "Create a file", "Создать файл", ["создай файл {path}", "новый файл {path}", "create file {path}", "new file {path}"], [{ type: "FileCreate", path: "{path}", content: "" }]);
cmd("files", "file.mkdir", "files", "Create a folder", "Создать папку", ["создай папку {path}", "новая папка {path}", "create folder {path}", "new folder {path}"], [{ type: "FolderCreate", path: "{path}" }]);
cmd("files", "file.move", "files", "Move a file", "Переместить файл", ["перемести {from} в {to}", "переименуй файл {from} в {to}", "move {from} to {to}", "rename file {from} to {to}"], [{ type: "FileMove", from: "{from}", to: "{to}" }], "Confirm");
cmd("files", "file.delete", "files", "Delete a file", "Удалить файл", ["удали файл {path}", "удали папку {path}", "delete file {path}", "delete folder {path}"], [{ type: "FileDelete", path: "{path}" }], "Dangerous");

// ── assistant ──────────────────────────────────────────────────────────────
cmd("assistant", "mode.assistant", "assistant", "Assistant mode", "Режим ассистента", ["режим ассистента", "включи режим ассистента", "assistant mode", "switch to assistant mode"], [{ type: "SetMode", mode: "Assistant" }]);
cmd("assistant", "mode.orchestra", "assistant", "Orchestra mode", "Режим оркестра", ["режим оркестра", "включи режим оркестра", "вернись в оркестр", "orchestra mode", "switch to orchestra mode"], [{ type: "SetMode", mode: "Orchestra" }]);

// ── context sets ───────────────────────────────────────────────────────────
const ctx = (setId, id, en, ru, phrases, combo) => cmd(setId, `${setId}.${id}`, setId === "media-player" ? "media" : setId, en, ru, phrases, [{ type: "Keys", combo }]);
for (const [id, combo, en, ru, phrases] of [
  ["back", "alt+left", "Back", "Назад", ["назад", "вернись назад", "предыдущая страница браузера", "go back", "back"]],
  ["forward", "alt+right", "Forward", "Вперёд", ["вперед", "go forward", "forward"]],
  ["home", "alt+home", "Home page", "Домашняя страница", ["домашняя страница", "home page"]],
  ["address", "ctrl+l", "Address bar", "Адресная строка", ["адресная строка", "фокус на адрес", "address bar"]],
  ["history", "ctrl+h", "History", "История", ["история", "история браузера", "history", "browser history"]],
  ["downloads", "ctrl+j", "Downloads", "Загрузки браузера", ["загрузки браузера", "покажи загрузки", "browser downloads", "show downloads"]],
  ["bookmark", "ctrl+d", "Bookmark the page", "Добавить в закладки", ["добавь в закладки", "в закладки", "bookmark this page", "bookmark"]],
  ["bookmarks", "ctrl+shift+o", "Bookmarks", "Закладки", ["закладки", "покажи закладки", "bookmarks", "show bookmarks"]],
  ["private", "ctrl+shift+n", "Private window", "Приватное окно", ["приватное окно", "окно инкогнито", "инкогнито", "private window", "incognito window"]],
  ["newwindow", "ctrl+n", "New browser window", "Новое окно браузера", ["новое окно браузера", "new browser window"]],
  ["devtools", "f12", "Developer tools", "Инструменты разработчика", ["инструменты разработчика", "девтулз", "developer tools", "dev tools"]],
  ["source", "ctrl+u", "View source", "Исходный код страницы", ["исходный код страницы", "view source", "page source"]],
  ["stop", "esc", "Stop loading", "Остановить загрузку", ["останови загрузку", "stop loading"]],
  ["lasttab", "ctrl+9", "Last tab", "Последняя вкладка", ["последняя вкладка", "last tab"]],
  ["pin", "ctrl+shift+b", "Toggle bookmarks bar", "Панель закладок", ["панель закладок", "bookmarks bar"]],
  ["find", "ctrl+f", "Find on page", "Найти на странице", ["найди на этой странице", "find on this page"]],
  ["savepage", "ctrl+s", "Save page", "Сохранить страницу", ["сохрани страницу", "save page"]],
  ["printpage", "ctrl+p", "Print page", "Печать страницы", ["распечатай страницу", "print page"]],
  ["clear", "ctrl+shift+delete", "Clear browsing data", "Очистить историю", ["очисти историю", "очистить данные браузера", "clear browsing data"]],
  ["mutetab", "ctrl+m", "Mute tab (Firefox)", "Выключить звук вкладки", ["выключи звук вкладки", "mute tab"]],
]) ctx("browser", id, en, ru, phrases, combo);
for (const [id, combo, en, ru, phrases] of [
  ["newslide", "ctrl+m", "New slide", "Новый слайд", ["новый слайд", "добавь слайд", "new slide", "add slide"]],
  ["slideshow", "f5", "Start slide show", "Начать показ", ["начни показ", "запусти презентацию", "start slideshow", "start presentation"]],
  ["slideshowcurrent", "shift+f5", "Slide show from current", "Показ с текущего слайда", ["показ с текущего слайда", "present from current slide"]],
  ["center", "ctrl+e", "Center text", "По центру", ["выровняй по центру", "по центру", "center text", "align center"]],
  ["left", "ctrl+l", "Align left", "По левому краю", ["выровняй по левому краю", "align left"]],
  ["right", "ctrl+r", "Align right", "По правому краю", ["выровняй по правому краю", "align right"]],
  ["justify", "ctrl+j", "Justify", "По ширине", ["выровняй по ширине", "justify"]],
  ["bigger", "ctrl+shift+period", "Bigger font", "Шрифт крупнее", ["шрифт крупнее", "увеличь шрифт", "bigger font", "increase font"]],
  ["smaller", "ctrl+shift+comma", "Smaller font", "Шрифт мельче", ["шрифт мельче", "уменьши шрифт", "smaller font", "decrease font"]],
  ["pagebreak", "ctrl+enter", "Page break", "Разрыв страницы", ["разрыв страницы", "page break"]],
  ["link", "ctrl+k", "Insert link", "Вставить ссылку", ["вставь ссылку", "insert link"]],
  ["sum", "alt+equals", "AutoSum", "Автосумма", ["автосумма", "сумма столбца", "autosum"]],
  ["newsheet", "shift+f11", "New sheet", "Новый лист", ["новый лист", "new sheet", "add sheet"]],
  ["editcell", "f2", "Edit cell", "Редактировать ячейку", ["редактировать ячейку", "edit cell"]],
  ["comment", "ctrl+alt+m", "New comment", "Новый комментарий", ["добавь комментарий", "new comment"]],
  ["spelling", "f7", "Spelling", "Проверка орфографии", ["проверь орфографию", "проверка правописания", "spell check", "check spelling"]],
]) ctx("office", id, en, ru, phrases, combo);
for (const [id, combo, en, ru, phrases] of [
  ["newfolder", "ctrl+shift+n", "New folder", "Новая папка", ["создай новую папку здесь", "новая папка здесь", "new folder here"]],
  ["up", "alt+up", "Parent folder", "На уровень вверх", ["на уровень вверх", "в родительскую папку", "parent folder", "go up a folder"]],
  ["address", "alt+d", "Address bar", "Адресная строка", ["адресная строка проводника", "explorer address bar"]],
  ["preview", "alt+p", "Preview pane", "Область просмотра", ["область просмотра", "preview pane"]],
  ["details", "alt+shift+p", "Details pane", "Область сведений", ["область сведений", "details pane"]],
  ["hidden", "alt+v", "View menu", "Меню вид", ["меню вид", "view menu"]],
  ["search", "ctrl+e", "Search in folder", "Поиск в папке", ["поиск в папке", "search in folder"]],
  ["newwindow", "ctrl+n", "New Explorer window", "Новое окно проводника здесь", ["новое окно здесь", "new window here"]],
  ["newtab", "ctrl+t", "New Explorer tab", "Новая вкладка проводника", ["новая вкладка проводника", "new explorer tab"]],
  ["largeicons", "ctrl+shift+2", "Large icons", "Крупные значки", ["крупные значки", "large icons"]],
  ["detailsview", "ctrl+shift+6", "Details view", "Таблица", ["вид таблица", "details view"]],
  ["listview", "ctrl+shift+5", "List view", "Список", ["вид список", "list view"]],
]) ctx("explorer", id, en, ru, phrases, combo);
for (const [id, combo, en, ru, phrases] of [
  ["fullscreen", "f", "Player full screen", "Плеер на весь экран", ["плеер на весь экран", "player full screen"]],
  ["forward", "right", "Seek forward", "Перемотать вперёд", ["перемотай вперед", "вперед на несколько секунд", "seek forward", "skip forward"]],
  ["backward", "left", "Seek backward", "Перемотать назад", ["перемотай назад", "назад на несколько секунд", "seek backward", "skip back"]],
  ["space", "space", "Play / pause (player)", "Пауза в плеере", ["пауза в плеере", "player pause"]],
  ["volup", "up", "Player volume up", "Громче в плеере", ["громче в плеере", "player louder"]],
  ["voldown", "down", "Player volume down", "Тише в плеере", ["тише в плеере", "player quieter"]],
]) ctx("media-player", id, en, ru, phrases, combo);

// ── output ─────────────────────────────────────────────────────────────────
const out = { format: "orkestr-command-catalogue", version: 1, generatedBy: "scripts/gen-assistant-catalogue.mjs", sets: [...sets.values()] };
const json = `${JSON.stringify(out, null, 1)}\n`;
const target = join(data, "catalogue.json");
const total = out.sets.reduce((n, s) => n + s.commands.length, 0);
const phrases = out.sets.reduce((n, s) => n + s.commands.reduce((m, c) => m + c.phrases.length, 0), 0);
if (process.argv.includes("--check")) {
  const cur = readFileSync(target, "utf8");
  if (cur !== json) {
    console.error("catalogue.json is out of date: run node scripts/gen-assistant-catalogue.mjs");
    process.exit(1);
  }
  console.log(`catalogue.json up to date (${total} commands)`);
} else {
  writeFileSync(target, json);
  console.log(`wrote ${target}: ${out.sets.length} sets, ${total} commands, ${phrases} phrases`);
}
