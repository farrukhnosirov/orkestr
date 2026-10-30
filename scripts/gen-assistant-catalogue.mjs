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
win("snapleft", "Snap left", "Окно влево", ["окно влево", "окно слева", "окно налево", "окно в левую половину", "прикрепи окно влево", "сдвинь окно влево", "snap left", "snap window left", "move window left"], [{ type: "Window", op: "SnapLeft" }]);
win("snapright", "Snap right", "Окно вправо", ["окно вправо", "окно справа", "окно направо", "окно в правую половину", "прикрепи окно вправо", "сдвинь окно вправо", "snap right", "snap window right", "move window right"], [{ type: "Window", op: "SnapRight" }]);
win("nextmonitor", "Move to the next monitor", "Перенести на следующий монитор", ["перенеси окно на другой монитор", "перенеси на следующий монитор", "окно на второй монитор", "move window to the next monitor", "move to other monitor", "next monitor"], [{ type: "Window", op: "NextMonitor" }]);
win("prevmonitor", "Move to the previous monitor", "Перенести на предыдущий монитор", ["перенеси окно на предыдущий монитор", "окно на первый монитор", "move window to the previous monitor", "previous monitor"], [{ type: "Window", op: "PreviousMonitor" }]);
win("switchnext", "Switch window", "Переключить окно", ["переключи окно", "следующее окно", "другое окно", "смени окно", "switch window", "next window", "alt tab", "альт таб"], [{ type: "Window", op: "SwitchNext" }]);
win("desktop", "Show the desktop", "Показать рабочий стол", ["покажи рабочий стол", "сверни все окна", "сверни всё", "show desktop", "minimize all windows", "minimize everything"], [{ type: "Window", op: "ShowDesktop" }]);
win("taskview", "Task view", "Представление задач", ["представление задач", "покажи все окна", "покажи все рабочие столы", "task view", "show all windows", "show all desktops"], [{ type: "Desktop", op: "TaskView" }]);
win("vdnew", "New virtual desktop", "Новый рабочий стол", ["новый рабочий стол", "создай рабочий стол", "new desktop", "new virtual desktop"], [{ type: "Desktop", op: "New" }]);
win("vdclose", "Close virtual desktop", "Закрыть рабочий стол", ["закрой рабочий стол", "close desktop", "close virtual desktop"], [{ type: "Desktop", op: "Close" }]);
win("vdnext", "Next virtual desktop", "Следующий рабочий стол", ["следующий рабочий стол", "рабочий стол вправо", "переключись на следующий рабочий стол", "next desktop", "next virtual desktop", "switch desktop right"], [{ type: "Desktop", op: "Next" }]);
win("vdprev", "Previous virtual desktop", "Предыдущий рабочий стол", ["предыдущий рабочий стол", "рабочий стол влево", "вернись на предыдущий рабочий стол", "previous desktop", "previous virtual desktop", "switch desktop left"], [{ type: "Desktop", op: "Previous" }]);
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
sc("element", "Click an element", "Нажать на элемент", ["нажми на кнопку {name}", "нажми на {name}", "нажми кнопку {name}", "кликни {name}", "кликни по {name}", "кликни по кнопке {name}", "щелкни {name}", "щелкни по {name}", "щелкни по кнопке {name}", "щелкни на {name}", "ткни в {name}", "click {name}", "click on {name}", "press button {name}", "click the {name} button"], { type: "ClickElement", name: "{name}" });
sc("click", "Click", "Клик", ["кликни", "клик", "щелкни", "click", "left click"], { type: "MouseClick", button: "Left" });
sc("rightclick", "Right click", "Правый клик", ["правый клик", "кликни правой кнопкой", "right click"], { type: "MouseClick", button: "Right" });
sc("doubleclick", "Double click", "Двойной клик", ["двойной клик", "дважды кликни", "double click"], { type: "MouseClick", button: "Double" });

// ── media / volume / brightness ───────────────────────────────────────────
const md = (id, en, ru, phrases, steps) => cmd("media", `media.${id}`, "media", en, ru, phrases, steps);
md("playpause", "Play / pause", "Пауза / продолжить", ["пауза", "поставь на паузу", "продолжи воспроизведение", "играй", "play", "pause", "resume", "play pause"], [{ type: "Media", key: "PlayPause" }]);
md("next", "Next track", "Следующий трек", ["следующий трек", "следующая песня", "дальше", "next track", "next song", "skip"], [{ type: "Media", key: "Next" }]);
md("prev", "Previous track", "Предыдущий трек", ["предыдущий трек", "предыдущая песня", "previous track", "previous song"], [{ type: "Media", key: "Previous" }]);
md("stop", "Stop playback", "Остановить воспроизведение", ["останови музыку", "стоп музыка", "останови воспроизведение", "stop music", "stop playback"], [{ type: "Media", key: "Stop" }]);
md("volup", "Volume up", "Громче", ["громче", "сделай громче", "погромче", "сделай погромче", "прибавь звук", "прибавь громкость", "увеличь громкость", "увеличь звук", "звук громче", "громкость больше", "louder", "volume up", "turn it up", "turn the volume up", "make it louder"], [{ type: "Volume", op: "Up", value: 10 }]);
md("voldown", "Volume down", "Тише", ["тише", "сделай тише", "потише", "сделай потише", "убавь звук", "убавь громкость", "уменьши громкость", "уменьши звук", "звук тише", "громкость меньше", "quieter", "volume down", "turn it down", "turn the volume down", "make it quieter"], [{ type: "Volume", op: "Down", value: 10 }]);
md("volupn", "Volume up by N%", "Громче на N%", ["громче на {n}", "прибавь звук на {n}", "volume up by {n}", "louder by {n}"], [{ type: "Volume", op: "Up", value: "{n}" }]);
md("voldownn", "Volume down by N%", "Тише на N%", ["тише на {n}", "убавь звук на {n}", "volume down by {n}", "quieter by {n}"], [{ type: "Volume", op: "Down", value: "{n}" }]);
md("volset", "Set volume", "Установить громкость", ["громкость {n}", "громкость на {n}", "громкость {n} процентов", "громкость на {n} процентов", "поставь громкость {n}", "поставь громкость на {n}", "сделай громкость {n}", "установи громкость {n}", "звук {n}", "звук на {n}", "звук на {n} процентов", "поставь звук на {n}", "set volume to {n}", "volume {n}", "volume to {n} percent", "set the volume at {n}"], [{ type: "Volume", op: "Set", value: "{n}" }]);
md("mute", "Mute / unmute", "Выключить / включить звук", ["звук вкл выкл", "переключи беззвучный режим", "mute toggle"], [{ type: "Volume", op: "Mute" }]);
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

// ── wave 10: full PC control (ADR-0055) ───────────────────────────────────
// Sound (system, apps, microphone, devices), media session, window geometry,
// virtual desktops, cursor & mouse, keyboard layouts, displays, radios,
// reports (time, battery, disk, IP, CPU / RAM, what is playing), calculator,
// alarms, files. Phrases include common speech-recognition variants
// («громкасть», «по громче», «звук на») — the matcher also normalizes them.
set("sound", "Sound", "Volume of the system and of apps, microphone, audio devices, what is playing.");
set("mouse", "Cursor & mouse", "Cursor movement, positions, UI elements, holding and dragging, pointer speed.");
set("display", "Display", "Brightness, projection, screen off, screenshots, screen recording.");
set("info", "Information", "Time and date, battery, disk space, IP and network, CPU / RAM, calculator.");
const so = (id, en, ru, phrases, steps, safety) => cmd("sound", `sound.${id}`, "sound", en, ru, phrases, steps, safety);
const vol = (op, value, app) => ({ type: "Volume", op, ...(value !== undefined ? { value } : {}), ...(app ? { app } : {}) });

// system volume
so("max", "Volume to maximum", "Громкость на максимум",
  ["громкость на полную", "звук на полную", "на полную громкость", "максимальная громкость", "громкость на максимум", "звук на максимум", "сделай звук на полную", "выкрути звук на полную", "громкость сто", "volume max", "max volume", "full volume", "volume all the way up", "crank it up"], [vol("Set", 100)]);
so("min", "Volume to minimum", "Громкость на минимум",
  ["громкость на минимум", "минимальная громкость", "звук на минимум", "еле слышно", "volume min", "minimum volume", "barely audible"], [vol("Set", 5)]);
so("zero", "Volume to zero", "Громкость на ноль", ["громкость на ноль", "громкость ноль", "звук на ноль", "volume zero", "volume to zero"], [vol("Set", 0)]);
so("half", "Half the volume", "В два раза тише",
  ["в два раза тише", "вдвое тише", "сделай в два раза тише", "уменьши громкость вдвое", "половина громкости", "half the volume", "halve the volume", "twice as quiet"], [vol("Scale", 50)]);
so("double", "Double the volume", "В два раза громче",
  ["в два раза громче", "вдвое громче", "сделай в два раза громче", "увеличь громкость вдвое", "double the volume", "twice as loud"], [vol("Scale", 200)]);
so("bitup", "A little louder", "Чуть громче",
  ["чуть громче", "немного громче", "чуть-чуть громче", "капельку громче", "чуть погромче", "немного погромче", "a bit louder", "slightly louder", "a little louder"], [vol("Up", 5)]);
so("bitdown", "A little quieter", "Чуть тише",
  ["чуть тише", "немного тише", "чуть-чуть тише", "капельку тише", "чуть потише", "немного потише", "a bit quieter", "slightly quieter", "a little quieter"], [vol("Down", 5)]);
so("muchup", "Much louder", "Намного громче",
  ["намного громче", "гораздо громче", "сильно громче", "ещё громче", "еще громче", "much louder", "way louder", "a lot louder"], [vol("Up", 25)]);
so("muchdown", "Much quieter", "Намного тише",
  ["намного тише", "гораздо тише", "сильно тише", "ещё тише", "еще тише", "much quieter", "way quieter", "a lot quieter"], [vol("Down", 25)]);
so("muteon", "Mute", "Выключить звук",
  ["выключи звук", "отключи звук", "без звука", "заглуши звук", "звук выключи", "убери звук", "тишина", "вырубай звук", "mute", "mute the sound", "turn off the sound", "mute audio", "sound off"], [vol("MuteOn")]);
so("muteoff", "Unmute", "Включить звук",
  ["включи звук", "верни звук", "звук включи", "сними беззвучный", "unmute", "sound on", "turn the sound on", "unmute audio"], [vol("MuteOff")]);
so("mutetoggle", "Toggle mute", "Переключить звук", ["переключи звук вкл выкл", "toggle mute"], [vol("Mute")]);
so("get", "Current volume", "Какая громкость", ["какая громкость", "какая сейчас громкость", "сколько громкость", "на сколько громкость", "what's the volume", "what is the volume", "current volume"], [{ type: "Query", what: "Volume" }]);
for (const v of [5, 15, 25, 35, 45, 55, 65, 75, 85, 95]) {
  so(`set${v}`, `Volume ${v}%`, `Громкость ${v}%`, [`громкость ${v} процентов`, `громкость на ${v} процентов`, `volume ${v} percent`], [vol("Set", v)]);
}
// app volume (the app's own slider in the mixer)
so("app.set", "App volume", "Громкость приложения",
  ["громкость {app} {n}", "громкость {app} на {n}", "поставь громкость {app} на {n}", "громкость в {app} на {n}", "{app} громкость {n}", "set {app} volume to {n}", "{app} volume {n}", "{app} volume to {n} percent"], [vol("Set", "{n}", "{app}")]);
so("app.up", "App louder", "Приложение громче",
  ["{app} громче", "сделай {app} громче", "громче в {app}", "прибавь звук в {app}", "make {app} louder", "{app} louder", "turn {app} up"], [vol("Up", 20, "{app}")]);
so("app.down", "App quieter", "Приложение тише",
  ["{app} тише", "сделай {app} тише", "тише в {app}", "убавь звук в {app}", "make {app} quieter", "{app} quieter", "turn {app} down"], [vol("Down", 20, "{app}")]);
so("app.mute", "Mute an app", "Выключить звук приложения",
  ["выключи звук в {app}", "выключи звук {app}", "заглуши {app}", "отключи звук в {app}", "mute {app}", "silence {app}"], [vol("MuteOn", undefined, "{app}")]);
so("app.unmute", "Unmute an app", "Включить звук приложения",
  ["включи звук в {app}", "верни звук в {app}", "unmute {app}"], [vol("MuteOff", undefined, "{app}")]);
const AUDIO_APPS = ["spotify", "chrome", "firefox", "edge", "yandexbrowser", "opera", "brave", "discord", "telegram", "zoom", "teams", "skype", "vlc", "aimp", "potplayer", "steam", "obs", "whatsapp", "mediaplayer", "viber"];
for (const a of apps.filter((x) => AUDIO_APPS.includes(x.key))) {
  const key = a.key, e0 = a.en[0], r0 = a.ru[0];
  so(`app.${key}.mute`, `Mute ${e0}`, `Выключить звук: ${r0}`, [...each(a.ru, ["выключи звук в %", "заглуши %"]), ...each(a.en, ["mute %"])], [vol("MuteOn", undefined, key)]);
  so(`app.${key}.unmute`, `Unmute ${e0}`, `Включить звук: ${r0}`, [...each(a.ru, ["включи звук в %"]), ...each(a.en, ["unmute %"])], [vol("MuteOff", undefined, key)]);
  so(`app.${key}.up`, `${e0} louder`, `${r0}: громче`, [...each(a.ru, ["% громче", "громче в %"]), ...each(a.en, ["% louder"])], [vol("Up", 20, key)]);
  so(`app.${key}.down`, `${e0} quieter`, `${r0}: тише`, [...each(a.ru, ["% тише", "тише в %"]), ...each(a.en, ["% quieter"])], [vol("Down", 20, key)]);
}
// microphone
so("mic.mute", "Mute the microphone", "Выключить микрофон",
  ["выключи микрофон", "отключи микрофон", "замьють микрофон", "заглуши микрофон", "микрофон выключи", "mute the mic", "mute microphone", "mic off", "turn off the microphone"], [{ type: "Microphone", op: "MuteOn" }]);
so("mic.unmute", "Unmute the microphone", "Включить микрофон",
  ["включи микрофон", "верни микрофон", "микрофон включи", "unmute the mic", "unmute microphone", "mic on", "turn on the microphone"], [{ type: "Microphone", op: "MuteOff" }]);
so("mic.set", "Microphone level", "Громкость микрофона",
  ["громкость микрофона {n}", "громкость микрофона на {n}", "микрофон на {n}", "чувствительность микрофона {n}", "mic volume {n}", "microphone level {n}", "set the microphone to {n}"], [{ type: "Microphone", op: "Set", value: "{n}" }]);
so("mic.up", "Microphone louder", "Микрофон громче", ["микрофон громче", "сделай микрофон громче", "прибавь микрофон", "mic louder", "microphone up"], [{ type: "Microphone", op: "Up", value: 10 }]);
so("mic.down", "Microphone quieter", "Микрофон тише", ["микрофон тише", "сделай микрофон тише", "убавь микрофон", "mic quieter", "microphone down"], [{ type: "Microphone", op: "Down", value: 10 }]);
// devices
so("dev.to", "Switch the audio output", "Переключить вывод звука",
  ["переключи звук на {name}", "выведи звук на {name}", "звук через {name}", "звук в {name}", "переключи вывод звука на {name}", "switch audio to {name}", "output sound to {name}", "play sound through {name}", "switch the output to {name}"], [{ type: "AudioDevice", kind: "Output", name: "{name}" }]);
for (const [id, name, ru, en] of [
  ["headphones", "наушники", ["звук в наушники", "переключи на наушники", "включи наушники", "звук на наушники", "переключи звук в наушники"], ["switch to headphones", "sound to headphones", "use headphones"]],
  ["speakers", "динамики", ["звук на колонки", "переключи на динамики", "звук в колонки", "включи колонки", "звук на динамики"], ["switch to speakers", "sound to speakers", "use speakers"]],
  ["monitor", "монитор", ["звук на монитор", "звук через hdmi", "звук на телевизор"], ["sound to the monitor", "audio over hdmi", "sound to the tv"]],
]) so(`dev.${id}`, `Audio to ${en[0].replace("switch to ", "")}`, `Звук: ${name}`, [...ru, ...en], [{ type: "AudioDevice", kind: "Output", name }]);
so("dev.next", "Next audio output", "Следующее устройство звука",
  ["следующее устройство звука", "переключи устройство вывода", "другое устройство звука", "смени устройство звука", "next audio device", "next output device", "switch audio device"], [{ type: "AudioDevice", kind: "Output" }]);
so("dev.mic", "Switch the microphone", "Переключить микрофон",
  ["переключи микрофон на {name}", "используй микрофон {name}", "switch microphone to {name}", "use the {name} microphone"], [{ type: "AudioDevice", kind: "Input", name: "{name}" }]);
so("dev.micnext", "Next microphone", "Следующий микрофон", ["следующий микрофон", "другой микрофон", "next microphone"], [{ type: "AudioDevice", kind: "Input" }]);
so("dev.list", "Audio devices", "Устройства звука", ["какие устройства звука", "список аудиоустройств", "какие есть колонки и микрофоны", "audio devices", "list audio devices", "what audio devices are there"], [{ type: "Query", what: "AudioDevices" }]);
so("sessions", "Apps playing sound", "Какие программы играют звук", ["какие программы играют звук", "что издаёт звук", "что издает звук", "кто шумит", "what apps are playing sound", "which apps make sound"], [{ type: "Query", what: "AudioSessions" }]);
so("mixer", "Volume mixer", "Микшер громкости", ["открой микшер громкости", "микшер громкости", "микшер звука", "volume mixer", "open the volume mixer"], [{ type: "OpenSettings", page: "apps-volume" }]);
// media session
so("nowplaying", "What is playing", "Что сейчас играет",
  ["что сейчас играет", "что играет", "какая песня", "что за песня", "какой трек играет", "кто поёт", "кто поет", "как называется песня", "what's playing", "what is playing", "what song is this", "who is singing", "now playing"], [{ type: "Query", what: "NowPlaying" }]);
so("seekfwd", "Seek forward", "Перемотать вперёд",
  ["перемотай вперёд на {duration}", "перемотай вперед на {duration}", "вперёд на {duration}", "вперед на {duration}", "промотай на {duration} вперед", "skip forward {duration}", "fast forward {duration}", "seek forward {duration}"], [{ type: "MediaSeek", forward: true, seconds: "{duration}" }]);
so("seekback", "Seek back", "Перемотать назад",
  ["перемотай назад на {duration}", "назад на {duration}", "отмотай на {duration}", "отмотай назад на {duration}", "rewind {duration}", "skip back {duration}", "go back {duration}"], [{ type: "MediaSeek", forward: false, seconds: "{duration}" }]);
so("seekfwd10", "Seek forward 10 s", "Вперёд на 10 секунд", ["перемотай вперёд", "перемотай вперед", "промотай вперед", "skip ahead", "fast forward"], [{ type: "MediaSeek", forward: true, seconds: 10 }]);
so("seekback10", "Seek back 10 s", "Назад на 10 секунд", ["перемотай назад", "отмотай назад", "отмотай немного", "rewind", "rewind a bit"], [{ type: "MediaSeek", forward: false, seconds: 10 }]);

// ── windows (geometry) ─────────────────────────────────────────────────────
const W = (op, app) => ({ type: "Window", op, ...(app ? { app } : {}) });
for (const [op, id, en, ru, ruPh, enPh] of [
  ["SnapTop", "snaptop", "Window to the top half", "Окно в верхнюю половину", ["окно вверх", "окно наверх", "окно в верхнюю половину", "окно сверху"], ["snap window to the top", "window to the top half"]],
  ["SnapBottom", "snapbottom", "Window to the bottom half", "Окно в нижнюю половину", ["окно в нижнюю половину", "окно снизу", "окно вниз экрана"], ["snap window to the bottom", "window to the bottom half"]],
  ["SnapTopLeft", "snaptl", "Window to the top-left quarter", "Окно в левый верхний угол", ["окно в левый верхний угол", "окно влево вверх", "окно в верхний левый угол"], ["window to the top left", "snap window top left"]],
  ["SnapTopRight", "snaptr", "Window to the top-right quarter", "Окно в правый верхний угол", ["окно в правый верхний угол", "окно вправо вверх", "окно в верхний правый угол"], ["window to the top right", "snap window top right"]],
  ["SnapBottomLeft", "snapbl", "Window to the bottom-left quarter", "Окно в левый нижний угол", ["окно в левый нижний угол", "окно влево вниз", "окно в нижний левый угол"], ["window to the bottom left", "snap window bottom left"]],
  ["SnapBottomRight", "snapbr", "Window to the bottom-right quarter", "Окно в правый нижний угол", ["окно в правый нижний угол", "окно вправо вниз", "окно в нижний правый угол"], ["window to the bottom right", "snap window bottom right"]],
  ["Center", "center", "Center the window", "Окно по центру", ["окно по центру", "отцентрируй окно", "окно в центр", "окно в центр экрана", "поставь окно по центру"], ["center window", "center the window", "window to the center"]],
  ["AlwaysOnTop", "ontop", "Always on top", "Поверх всех окон", ["закрепи окно поверх всех", "поверх всех окон", "окно поверх других", "закрепи окно сверху", "окно всегда сверху"], ["always on top", "pin window on top", "keep window on top"]],
  ["NotOnTop", "notontop", "Not on top", "Не поверх всех", ["открепи окно", "убери поверх всех", "окно не поверх всех", "сними закрепление окна"], ["not on top", "unpin window", "stop keeping on top"]],
  ["MinimizeAll", "minall", "Minimize all windows", "Свернуть все окна", ["сверни все окна сразу", "сверни все программы", "убери все окна"], ["minimize all", "minimize every window"]],
  ["RestoreAll", "restoreall", "Restore all windows", "Вернуть все окна", ["разверни все окна", "верни все окна", "восстанови все окна", "верни свернутые окна"], ["restore all windows", "bring back all windows", "unminimize all"]],
]) cmd("windows", `win.${id}`, "windows", en, ru, [...ruPh, ...enPh], [W(op)]);
for (const [op, ruW, enW] of [
  ["SnapLeft", ["влево", "налево", "слева", "в левую половину"], ["left", "to the left"]],
  ["SnapRight", ["вправо", "направо", "справа", "в правую половину"], ["right", "to the right"]],
  ["SnapTop", ["наверх", "в верхнюю половину"], ["to the top"]],
  ["SnapBottom", ["вниз экрана", "в нижнюю половину"], ["to the bottom"]],
  ["SnapTopLeft", ["в левый верхний угол"], ["to the top left"]],
  ["SnapTopRight", ["в правый верхний угол"], ["to the top right"]],
  ["SnapBottomLeft", ["в левый нижний угол"], ["to the bottom left"]],
  ["SnapBottomRight", ["в правый нижний угол"], ["to the bottom right"]],
  ["Center", ["по центру", "в центр"], ["to the center"]],
  ["AlwaysOnTop", ["поверх всех"], ["on top"]],
]) {
  cmd("windows", `win.app.${op.toLowerCase()}`, "windows", `App window: ${op}`, `Окно приложения: ${ruW[0]}`,
    [...ruW.flatMap((w) => [`{app} ${w}`, `перемести {app} ${w}`, `окно {app} ${w}`]), ...enW.flatMap((w) => [`move {app} ${w}`, `snap {app} ${w}`, `put {app} ${w}`])], [W(op, "{app}")]);
}
cmd("windows", "win.resize", "windows", "Resize the window (percent)", "Размер окна в процентах",
  ["окно на {n} процентов", "размер окна {n} процентов", "сделай окно {n} процентов экрана", "окно {n} процентов экрана", "resize window to {n} percent", "window size {n} percent"], [{ type: "WindowResize", percent: "{n}" }]);
cmd("windows", "win.resizepx", "windows", "Resize the window (pixels)", "Размер окна в пикселях",
  ["окно {w} на {h}", "размер окна {w} на {h}", "сделай окно {w} на {h}", "resize window to {w} by {h}", "window size {w} by {h}"], [{ type: "WindowResize", width: "{w}", height: "{h}" }]);
cmd("windows", "win.smaller", "windows", "Smaller window", "Окно поменьше", ["сделай окно поменьше", "окно поменьше", "уменьши окно", "make the window smaller", "smaller window"], [{ type: "WindowResize", percent: 60 }]);
cmd("windows", "win.bigger", "windows", "Bigger window", "Окно побольше", ["сделай окно побольше", "окно побольше", "увеличь окно", "make the window bigger", "bigger window"], [{ type: "WindowResize", percent: 85 }]);
for (const [id, p, ru, en] of [
  ["transparent", 70, ["сделай окно прозрачным", "окно полупрозрачное", "прозрачное окно"], ["make the window transparent", "transparent window"]],
  ["verytransparent", 40, ["сделай окно очень прозрачным", "окно сильно прозрачное"], ["make the window very transparent"]],
  ["slightlytransparent", 85, ["сделай окно немного прозрачным", "окно чуть прозрачное"], ["make the window slightly transparent"]],
  ["opaque", 100, ["убери прозрачность", "окно непрозрачное", "сделай окно непрозрачным"], ["make the window opaque", "remove transparency"]],
]) cmd("windows", `win.${id}`, "windows", `Window opacity ${p}%`, `Непрозрачность окна ${p}%`, [...ru, ...en], [{ type: "WindowOpacity", percent: p }]);
cmd("windows", "win.opacity", "windows", "Window opacity", "Непрозрачность окна", ["непрозрачность окна {n}", "непрозрачность {n}", "window opacity {n}", "set window opacity to {n}"], [{ type: "WindowOpacity", percent: "{n}" }]);
cmd("windows", "win.closeall", "windows", "Close all windows of an app", "Закрыть все окна приложения",
  ["закрой все окна {app}", "закрой все окна приложения {app}", "закрой все {app}", "close all {app} windows", "close every {app} window"], [{ type: "CloseApp", app: "{app}", discard: false }], "Confirm");
cmd("windows", "win.previous", "windows", "Previous window", "Предыдущее окно", ["предыдущее окно", "вернись в предыдущее окно", "верни прошлое окно", "previous window", "last window", "go back to the previous window"], [W("SwitchNext")]);

// ── cursor & mouse ─────────────────────────────────────────────────────────
const mo = (id, en, ru, phrases, steps) => cmd("mouse", `mouse.${id}`, "mouse", en, ru, phrases, steps);
const DIRS = [
  ["Up", ["вверх", "наверх", "выше"], ["up"]],
  ["Down", ["вниз", "ниже"], ["down"]],
  ["Left", ["влево", "налево", "левее"], ["left"]],
  ["Right", ["вправо", "направо", "правее"], ["right"]],
];
for (const [d, ruW, enW] of DIRS) {
  const lo = d.toLowerCase();
  mo(`move${lo}n`, `Cursor ${lo} by N px`, `Курсор ${ruW[0]} на N пикселей`,
    [...ruW.flatMap((w) => [`курсор ${w} на {n}`, `курсор ${w} на {n} пикселей`, `сдвинь курсор ${w} на {n}`, `мышь ${w} на {n}`, `мышку ${w} на {n}`, `передвинь курсор ${w} на {n}`]), ...enW.flatMap((w) => [`move the cursor ${w} {n}`, `move mouse ${w} by {n} pixels`, `cursor ${w} {n}`])],
    [{ type: "MouseMove", direction: d, distance: "{n}" }]);
  mo(`move${lo}`, `Cursor ${lo}`, `Курсор ${ruW[0]}`,
    [...ruW.flatMap((w) => [`курсор ${w}`, `мышь ${w}`, `мышку ${w}`, `сдвинь курсор ${w}`, `подвинь курсор ${w}`]), ...enW.flatMap((w) => [`move the cursor ${w}`, `cursor ${w}`, `mouse ${w}`])],
    [{ type: "MouseMove", direction: d, distance: 100 }]);
  mo(`move${lo}bit`, `Cursor a bit ${lo}`, `Курсор немного ${ruW[0]}`,
    [...ruW.flatMap((w) => [`курсор немного ${w}`, `курсор чуть ${w}`, `чуть-чуть курсор ${w}`]), ...enW.flatMap((w) => [`cursor a bit ${w}`, `nudge the cursor ${w}`])],
    [{ type: "MouseMove", direction: d, distance: 30 }]);
  mo(`move${lo}far`, `Cursor far ${lo}`, `Курсор сильно ${ruW[0]}`,
    [...ruW.flatMap((w) => [`курсор сильно ${w}`, `курсор далеко ${w}`, `курсор намного ${w}`]), ...enW.flatMap((w) => [`cursor far ${w}`, `move the cursor a lot ${w}`])],
    [{ type: "MouseMove", direction: d, distance: 400 }]);
}
for (const [p, id, ru, en] of [
  ["Center", "center", ["курсор в центр", "курсор в центр экрана", "мышь в центр", "мышку в центр экрана", "курсор по центру", "поставь курсор в центр"], ["cursor to the center", "move the mouse to the center", "center the cursor"]],
  ["TopLeft", "topleft", ["курсор в левый верхний угол", "мышь в левый верхний угол"], ["cursor to the top left corner"]],
  ["TopRight", "topright", ["курсор в правый верхний угол", "мышь в правый верхний угол"], ["cursor to the top right corner"]],
  ["BottomLeft", "bottomleft", ["курсор в левый нижний угол", "мышь в левый нижний угол"], ["cursor to the bottom left corner"]],
  ["BottomRight", "bottomright", ["курсор в правый нижний угол", "мышь в правый нижний угол"], ["cursor to the bottom right corner"]],
  ["Top", "top", ["курсор к верхнему краю", "курсор наверх экрана"], ["cursor to the top edge"]],
  ["Bottom", "bottom", ["курсор к нижнему краю", "курсор вниз экрана"], ["cursor to the bottom edge"]],
  ["Left", "left", ["курсор к левому краю", "курсор на левый край"], ["cursor to the left edge"]],
  ["Right", "right", ["курсор к правому краю", "курсор на правый край"], ["cursor to the right edge"]],
]) mo(`to${id}`, `Cursor: ${p}`, `Курсор: ${ru[0].replace("курсор ", "")}`, [...ru, ...en], [{ type: "MouseTo", point: p }]);
mo("hover", "Cursor onto an element", "Навести курсор на элемент",
  ["наведи на {name}", "наведи курсор на {name}", "курсор на кнопку {name}", "наведи мышь на {name}", "наведи на кнопку {name}", "hover over {name}", "move the cursor to {name}", "point at {name}"], [{ type: "MouseToElement", name: "{name}" }]);
mo("middleclick", "Middle click", "Средний клик", ["средний клик", "кликни колёсиком", "кликни колесиком", "нажми колесико", "middle click", "wheel click"], [{ type: "MouseClick", button: "Middle" }]);
mo("leftclick", "Left click", "Левый клик", ["левый клик", "кликни левой", "нажми левой кнопкой мыши", "щёлкни левой кнопкой", "left mouse click"], [{ type: "MouseClick", button: "Left" }]);
mo("dbl", "Double click", "Двойной щелчок", ["двойной щелчок", "щёлкни дважды", "щелкни два раза", "кликни два раза", "double-click"], [{ type: "MouseClick", button: "Double" }]);
mo("hold", "Hold the left button", "Зажать левую кнопку",
  ["зажми левую кнопку", "зажми кнопку мыши", "удерживай кнопку мыши", "зажми мышку", "hold the mouse button", "press and hold", "hold left click"], [{ type: "MouseHold", button: "Left", release: false }]);
mo("holdright", "Hold the right button", "Зажать правую кнопку", ["зажми правую кнопку", "hold the right button"], [{ type: "MouseHold", button: "Right", release: false }]);
mo("release", "Release the button", "Отпустить кнопку", ["отпусти кнопку", "отпусти мышь", "отпусти кнопку мыши", "отпусти", "release the mouse button", "let go", "release"], [{ type: "MouseHold", button: "Left", release: true }]);
for (const [d, ruW, enW] of DIRS) {
  const lo = d.toLowerCase();
  mo(`drag${lo}n`, `Drag ${lo} N px`, `Перетащить ${ruW[0]} на N`, [...ruW.flatMap((w) => [`перетащи ${w} на {n}`, `тяни ${w} на {n}`]), ...enW.flatMap((w) => [`drag ${w} {n}`, `drag ${w} by {n} pixels`])], [{ type: "MouseDrag", direction: d, distance: "{n}" }]);
  mo(`drag${lo}`, `Drag ${lo}`, `Перетащить ${ruW[0]}`, [...ruW.flatMap((w) => [`перетащи ${w}`, `тяни ${w}`, `потяни ${w}`]), ...enW.flatMap((w) => [`drag ${w}`])], [{ type: "MouseDrag", direction: d, distance: 200 }]);
}
mo("dragto", "Drag onto an element", "Перетащить на элемент", ["перетащи на {name}", "перетащи это на {name}", "перетяни на {name}", "drag to {name}", "drag it onto {name}"], [{ type: "MouseDrag", element: "{name}" }]);
for (const [d, ruW, enW] of [["Left", "влево", "left"], ["Right", "вправо", "right"]]) {
  mo(`scroll${d.toLowerCase()}n`, `Scroll ${enW} N`, `Прокрутить ${ruW} на N`, [`прокрути ${ruW} на {n}`, `листай ${ruW} на {n}`, `scroll ${enW} {n}`, `scroll ${enW} by {n}`], [{ type: "Scroll", direction: d, amount: "{n}" }]);
}
mo("faster", "Faster pointer", "Курсор быстрее", ["увеличь скорость курсора", "курсор быстрее", "мышь быстрее", "ускорь курсор", "скорость мыши выше", "faster pointer", "increase mouse speed", "faster mouse"], [{ type: "MouseSpeed", op: "Up", value: 2 }]);
mo("slower", "Slower pointer", "Курсор медленнее", ["уменьши скорость курсора", "курсор медленнее", "мышь медленнее", "замедли курсор", "скорость мыши ниже", "slower pointer", "decrease mouse speed", "slower mouse"], [{ type: "MouseSpeed", op: "Down", value: 2 }]);
mo("speed", "Pointer speed", "Скорость курсора", ["скорость курсора {n}", "скорость мыши {n}", "поставь скорость курсора {n}", "pointer speed {n}", "mouse speed {n}"], [{ type: "MouseSpeed", op: "Set", value: "{n}" }]);
mo("size", "Pointer size", "Размер курсора", ["увеличь курсор", "размер курсора", "сделай курсор больше", "сделай курсор меньше", "cursor size", "bigger cursor", "pointer size"], [{ type: "OpenSettings", page: "easeofaccess-mousepointer" }]);
mo("locate", "Where is the cursor", "Где курсор", ["где курсор", "покажи курсор", "найди курсор", "где мышка", "где указатель", "where is the cursor", "show me the cursor", "find the cursor"], [{ type: "LocateCursor" }]);
mo("pos", "Cursor position", "Координаты курсора", ["координаты курсора", "позиция курсора", "cursor position", "cursor coordinates"], [{ type: "Query", what: "CursorPosition" }]);

// ── keyboard ───────────────────────────────────────────────────────────────
for (const [lang, id, ru, en] of [
  ["en", "en", ["переключи на английский", "английская раскладка", "раскладка английский", "английский язык ввода", "включи английский"], ["switch to english keyboard", "english layout", "english keyboard"]],
  ["ru", "ru", ["переключи на русский", "русская раскладка", "раскладка русский", "русский язык ввода", "включи русский"], ["switch to russian keyboard", "russian layout", "russian keyboard"]],
  ["uk", "uk", ["переключи на украинский", "украинская раскладка"], ["ukrainian layout", "switch to ukrainian keyboard"]],
  ["de", "de", ["переключи на немецкий", "немецкая раскладка"], ["german layout", "switch to german keyboard"]],
]) cmd("keys", `layout.${id}`, "keys", `Keyboard: ${en[2] ?? en[0]}`, `Раскладка: ${ru[1]}`, [...ru, ...en], [{ type: "KeyboardLayout", lang }]);
cmd("keys", "layout.next", "keys", "Next keyboard layout", "Следующая раскладка", ["следующая раскладка", "другая раскладка", "переключи язык клавиатуры", "next keyboard layout", "next input language"], [{ type: "KeyboardLayout" }]);
cmd("keys", "layout.get", "keys", "Current keyboard layout", "Какая раскладка", ["какая раскладка", "какой язык ввода", "what keyboard layout", "which input language"], [{ type: "Query", what: "KeyboardLayout" }]);
cmd("keys", "key.capslock", "keys", "Caps Lock", "Caps Lock", ["включи капс лок", "выключи капс лок", "капс лок", "капслок", "caps lock", "toggle caps lock"], [{ type: "Keys", combo: "capslock" }]);
cmd("keys", "key.numlock", "keys", "Num Lock", "Num Lock", ["нам лок", "num lock"], [{ type: "Keys", combo: "numlock" }]);
cmd("keys", "type.dictate", "typing", "Dictate text", "Диктовка текста", ["надиктуй {text}", "диктую {text}", "запиши текст {text}", "введи текст {text}", "dictate {text}"], [{ type: "TypeText", text: "{text}" }]);

// ── display ────────────────────────────────────────────────────────────────
const di = (id, en, ru, phrases, steps, safety) => cmd("display", `display.${id}`, "display", en, ru, phrases, steps, safety);
di("brmax", "Brightness to maximum", "Яркость на максимум", ["яркость на полную", "максимальная яркость", "яркость на максимум", "яркость сто", "brightness max", "full brightness"], [{ type: "Brightness", op: "Set", value: 100 }]);
di("brmin", "Brightness to minimum", "Яркость на минимум", ["яркость на минимум", "минимальная яркость", "brightness min", "minimum brightness"], [{ type: "Brightness", op: "Set", value: 10 }]);
di("brhalf", "Half the brightness", "Яркость вдвое меньше", ["яркость в два раза меньше", "вдвое темнее", "half brightness"], [{ type: "Brightness", op: "Scale", value: 50 }]);
di("brget", "Current brightness", "Какая яркость", ["какая яркость", "какая сейчас яркость", "what's the brightness"], [{ type: "Query", what: "Brightness" }]);
di("nightlight", "Night light", "Ночной свет", ["включи ночной свет", "выключи ночной свет", "ночной свет", "ночной режим экрана", "тёплый экран", "night light", "turn on night light", "turn off night light"], [{ type: "OpenSettings", page: "nightlight" }]);
di("off", "Turn the screen off", "Выключить экран", ["выключи экран", "погаси экран", "выключи монитор", "отключи экран", "turn off the screen", "screen off", "turn off the monitor"], [{ type: "Display", op: "ScreenOff" }]);
for (const [op, id, ru, en] of [
  ["PcScreenOnly", "pconly", ["только экран компьютера", "только этот экран", "только основной экран"], ["pc screen only", "only this screen"]],
  ["Duplicate", "duplicate", ["дублировать экран", "повтори экран", "дублируй экраны", "повторяющийся режим"], ["duplicate displays", "duplicate screen", "mirror the screen"]],
  ["Extend", "extend", ["расширить экран", "расширь экран", "расширь рабочий стол на второй экран", "расширенный режим"], ["extend displays", "extend the screen"]],
  ["SecondScreenOnly", "secondonly", ["только второй экран", "только внешний монитор", "только проектор"], ["second screen only", "projector only"]],
]) di(id, `Projection: ${en[0]}`, `Проецирование: ${ru[0]}`, [...ru, ...en], [{ type: "Display", op }], "Confirm");
di("resolution", "Resolution and scale", "Разрешение и масштаб", ["разрешение экрана", "измени разрешение", "масштаб экрана", "поменяй разрешение", "change resolution", "display scale", "screen resolution"], [{ type: "OpenSettings", page: "display" }]);
di("shotwindow", "Screenshot of the window", "Скриншот окна", ["скриншот окна", "снимок активного окна", "сделай скриншот окна", "screenshot of the window", "window screenshot"], [{ type: "Screenshot", scope: "Window" }]);
di("shotclip", "Screenshot to the clipboard", "Скриншот в буфер", ["скриншот в буфер", "скопируй скриншот в буфер", "снимок экрана в буфер обмена", "screenshot to clipboard", "copy a screenshot"], [{ type: "Screenshot", scope: "Full", clipboard: true }]);
di("shotwindowclip", "Window screenshot to the clipboard", "Скриншот окна в буфер", ["скриншот окна в буфер", "скопируй окно в буфер как картинку", "window screenshot to clipboard"], [{ type: "Screenshot", scope: "Window", clipboard: true }]);
di("shotregion", "Screenshot of a region", "Скриншот области", ["скриншот области", "выдели область для скриншота", "скриншот части экрана", "screenshot of a region", "region screenshot", "screenshot part of the screen"], [{ type: "Screenshot", scope: "Region" }]);
di("recstart", "Start screen recording", "Начать запись экрана", ["начни запись экрана", "запиши экран", "включи запись экрана", "start screen recording", "record the screen"], [{ type: "Keys", combo: "win+alt+r" }]);
di("recstop", "Stop screen recording", "Остановить запись экрана", ["останови запись экрана", "закончи запись экрана", "stop screen recording", "stop recording"], [{ type: "Keys", combo: "win+alt+r" }]);

// ── system ─────────────────────────────────────────────────────────────────
for (const [kind, id, onRu, offRu, onEn, offEn] of [
  ["Wifi", "wifi", ["включи вайфай", "включи wi-fi", "включи wifi", "вайфай включи", "подключи вайфай"], ["выключи вайфай", "выключи wi-fi", "выключи wifi", "отключи вайфай", "вайфай выключи"], ["turn on wifi", "wifi on", "enable wi-fi"], ["turn off wifi", "wifi off", "disable wi-fi"]],
  ["Bluetooth", "bt", ["включи блютуз", "включи bluetooth", "блютуз включи", "включи блютус"], ["выключи блютуз", "выключи bluetooth", "отключи блютуз", "блютуз выключи"], ["turn on bluetooth", "bluetooth on", "enable bluetooth"], ["turn off bluetooth", "bluetooth off", "disable bluetooth"]],
]) {
  sy(`${id}on`, `Turn ${kind} on`, `Включить ${kind === "Wifi" ? "Wi-Fi" : "Bluetooth"}`, [...onRu, ...onEn], [{ type: "Radio", kind, on: true }], "Confirm");
  sy(`${id}off`, `Turn ${kind} off`, `Выключить ${kind === "Wifi" ? "Wi-Fi" : "Bluetooth"}`, [...offRu, ...offEn], [{ type: "Radio", kind, on: false }], "Confirm");
}
sy("airplaneon", "Airplane mode on", "Режим «в самолёте»", ["режим полёта", "режим полета", "включи режим в самолете", "режим в самолёте", "включи авиарежим", "airplane mode on", "turn on airplane mode", "flight mode"], [{ type: "Radio", kind: "All", on: false }], "Confirm");
sy("airplaneoff", "Airplane mode off", "Выключить режим «в самолёте»", ["выключи режим полета", "выключи режим в самолете", "выключи авиарежим", "airplane mode off", "turn off airplane mode"], [{ type: "Radio", kind: "All", on: true }], "Confirm");
sy("dnd", "Do not disturb", "Не беспокоить", ["не беспокоить", "режим не беспокоить", "включи не беспокоить", "фокусировка внимания", "включи фокусировку", "do not disturb", "focus mode", "turn on do not disturb"], [{ type: "OpenSettings", page: "notifications" }]);
sy("powermode", "Power mode", "Режим питания", ["режим питания", "режим энергосбережения", "экономия заряда", "режим производительности", "power mode", "battery saver", "power settings"], [{ type: "OpenSettings", page: "powersleep" }]);
sy("lockc", "Lock (short)", "Заблокируй комп", ["заблокируй комп", "закрой комп", "lock it"], [{ type: "System", op: "Lock" }]);
cmd("system", "sys.killname", "system", "Kill a process by name", "Завершить процесс по имени", ["убей процесс {name}", "заверши процесс {name}", "закрой процесс {name}", "kill process {name}", "end process {name}"], [{ type: "KillProcess", name: "{name}" }], "Dangerous");
const inf = (id, en, ru, phrases, steps) => cmd("info", `info.${id}`, "info", en, ru, phrases, steps);
inf("time", "What time is it", "Который час", ["который час", "сколько времени", "сколько сейчас времени", "скажи время", "время", "what time is it", "what's the time", "tell me the time"], [{ type: "Query", what: "Time" }]);
inf("date", "Today's date", "Какое сегодня число", ["какое сегодня число", "какой сегодня день", "какая сегодня дата", "какое число", "какой день недели", "what's the date", "what day is it", "today's date"], [{ type: "Query", what: "Date" }]);
inf("battery", "Battery level", "Заряд батареи", ["сколько заряда", "уровень заряда", "заряд батареи", "сколько осталось батареи", "сколько процентов батареи", "батарея", "battery level", "how much battery", "battery status"], [{ type: "Query", what: "Battery" }]);
inf("disk", "Free disk space", "Свободное место", ["сколько места на диске", "свободное место", "место на диске", "сколько свободно на диске", "disk space", "free space", "how much disk space"], [{ type: "Query", what: "DiskSpace" }]);
inf("ip", "IP address", "IP-адрес", ["мой ip", "какой у меня ip", "какой у меня ip адрес", "ip адрес", "айпи адрес", "what's my ip", "my ip address", "ip address"], [{ type: "Query", what: "IpAddress" }]);
inf("network", "Network info", "Информация о сети", ["информация о сети", "сетевые подключения", "какая сеть", "сетевые адаптеры", "network info", "network status", "network adapters"], [{ type: "Query", what: "Network" }]);
inf("sysinfo", "CPU and memory", "Загрузка процессора и памяти", ["загрузка процессора", "сколько памяти занято", "нагрузка на систему", "информация о системе", "загрузка системы", "сколько оперативки", "cpu usage", "memory usage", "system info", "system load"], [{ type: "Query", what: "SystemInfo" }]);
inf("uptime", "Uptime", "Время работы", ["сколько работает компьютер", "аптайм", "время работы компьютера", "uptime", "how long has the computer been on"], [{ type: "Query", what: "Uptime" }]);
inf("calc", "Calculate", "Посчитать", ["сколько будет {expr}", "посчитай {expr}", "вычисли {expr}", "реши пример {expr}", "calculate {expr}", "how much is {expr}", "compute {expr}"], [{ type: "Calculate", expression: "{expr}" }]);
inf("alarm", "Alarm", "Будильник", ["разбуди меня в {time}", "будильник на {time}", "поставь будильник на {time}", "заведи будильник на {time}", "set an alarm for {time}", "wake me up at {time}", "alarm at {time}"], [{ type: "Alarm", at: "{time}", message: "" }]);
inf("remindat", "Remind at a time", "Напомнить в время", ["напомни в {time} {text}", "напомни мне в {time} {text}", "remind me at {time} to {text}", "remind me at {time} {text}"], [{ type: "Alarm", at: "{time}", message: "{text}" }]);
inf("title", "Page / window title", "Заголовок страницы", ["прочитай заголовок страницы", "как называется страница", "какая страница открыта", "какое окно активно", "что за окно", "read the page title", "what page is this", "which window is active"], [{ type: "Query", what: "ActiveWindow" }]);

// ── files ──────────────────────────────────────────────────────────────────
cmd("files", "file.copy", "files", "Copy a file", "Скопировать файл", ["скопируй файл {from} в {to}", "скопируй папку {from} в {to}", "сделай копию {from} в {to}", "copy file {from} to {to}", "copy folder {from} to {to}"], [{ type: "FileCopy", from: "{from}", to: "{to}" }]);
cmd("files", "file.rename", "files", "Rename a file", "Переименовать файл", ["переименуй {from} в {to}", "переименуй папку {from} в {to}", "rename {from} to {to}"], [{ type: "FileMove", from: "{from}", to: "{to}" }]);
cmd("files", "file.search", "files", "Find files", "Найти файлы", ["найди файл {name}", "поищи файл {name}", "где файл {name}", "найди файлы {name}", "найди документ {name}", "find file {name}", "search for file {name}", "where is the file {name}"], [{ type: "FileSearch", name: "{name}" }]);
cmd("files", "file.searchin", "files", "Find files in a folder", "Найти файлы в папке", ["найди файл {name} в {folder}", "поищи {name} в папке {folder}", "find file {name} in {folder}"], [{ type: "FileSearch", name: "{name}", folder: "{folder}" }]);
cmd("files", "file.recent", "files", "Recent files", "Недавние файлы", ["открой недавние файлы", "последние файлы", "недавние документы", "покажи последние файлы", "recent files", "open recent files"], [{ type: "OpenPath", path: "recent" }]);
cmd("files", "file.emptybin", "files", "Empty the recycle bin", "Очистить корзину", ["очисти корзину", "опустоши корзину", "очистить корзину", "удали всё из корзины", "empty the recycle bin", "empty trash", "clear the recycle bin"], [{ type: "EmptyRecycleBin" }], "Dangerous");

// per-app window placement (known names match better than the {app} slot)
for (const x of apps) {
  const key = x.key, e0 = x.en[0], r0 = x.ru[0];
  cmd("windows", `win.${key}.left`, "windows", `${e0} to the left`, `${r0}: влево`, [...each(x.ru, ["% влево", "% слева", "перемести % влево"]), ...each(x.en, ["move % left", "% to the left"])], [{ type: "Window", op: "SnapLeft", app: key }]);
  cmd("windows", `win.${key}.right`, "windows", `${e0} to the right`, `${r0}: вправо`, [...each(x.ru, ["% вправо", "% справа", "перемести % вправо"]), ...each(x.en, ["move % right", "% to the right"])], [{ type: "Window", op: "SnapRight", app: key }]);
}

// ── browser helpers ────────────────────────────────────────────────────────
cmd("web", "yt.play", "web", "Play on YouTube", "Включить на YouTube", ["включи на ютубе {query}", "поставь на ютубе {query}", "включи на youtube {query}", "запусти на ютубе {query}", "ютуб {query}", "play {query} on youtube", "youtube {query}"], [{ type: "WebSearch", query: "{query}", engine: "youtube" }]);

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
