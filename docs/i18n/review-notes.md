# Russian localization — editorial and visual review

Review of every `apps/desktop/src/locales/ru/*.json` file (and the English
source for consistency), plus a visual pass of the running app in Russian at
the minimum window size 960 × 600. Glossary: `docs/i18n/glossary.md`.
Screenshots: `docs/screenshots/i18n-ru/` (`manifest.json` lists, per view,
clipped text, Latin-only fragments and console errors found by the capture
script `scripts/smoke/i18n-ru-screenshots.mjs`).

## Overall

The translators' work was already of good quality: no straight quotes, no
`...`, no hyphen-as-dash, «ё» used consistently, non-breaking spaces before
units, full plural sets, correct gender agreement (task statuses feminine,
agent states and plan/run statuses masculine, decisions neuter). An automated
lint over all values (quotes, ellipsis, dashes, «ё» in stressed short
participles, capital «Вы», unit spacing, unclosed «») found no violations.
The fixes below are about consistency between namespaces, natural phrasing
and text that was still English on screen.

## Terminology and consistency

| Where | Before | After | Why |
|---|---|---|---|
| shell, security, logs, tray, glossary | «Экстренно остановить весь AI», «Весь AI остановлен», scope «весь AI» | «Остановить всю работу AI», «Работа AI остановлена», «вся работа AI» | «весь AI» is a calque of "all AI"; one phrase now for the command, tray item, button tooltip, dialog and flags. |
| manager, events | mixed «менеджер» / «Менеджер» | «Менеджер» everywhere it is the short name of the AI Manager | One spelling per concept (onboarding, settings, costs already used the capital). Rule added to the glossary. |
| placeholders | «Процессор, память, GPU…» | «ЦП, ОЗУ, GPU…» | Same hardware terms as Settings → System; «память» clashes with project memory. |
| agents | Builtin source «Официальный» (same as Official) | «Встроенный» | Two different sources had the same label. |
| git | «8 МиБ» | «8 МБ» | Units match the byte formatter used everywhere else. |
| notifications, resume | dismiss «Закрыть» / «Закрыть уведомление» | «Скрыть» / «Скрыть уведомление» | Dismissing a card or notice is «Скрыть» across the app (terminal, code, manager, security). |
| shell, code | palette hints «выбор» / «выполнить» / «поиск проектов» | «выбрать» / «выполнить» / «искать проекты» | Same part of speech in one hint row. |
| logs | deployment status «успешно», «не удалось» | «выполнено», «завершилось сбоем» | Agreement with «развёртывание» (neuter), like the other statuses. |
| events | «Сборка запущена: …», «Сборка не удалась: …», «Сборка прошла успешно за …» | «Запущена сборка: …», «Ошибка сборки: …», «Сборка завершена за …» | Same word order as the other event lines («Создан проект…»), shorter. |
| shell | «Восстановить размер» | «Свернуть в окно» | The Windows wording for the restore button. |
| shell | «Приложение запущено не в оболочке Orkestr…» | «Страница открыта вне настольного приложения Orkestr…» | "desktop shell" is not a «оболочка» here; says what actually happened. |
| terminal, files, agents | «Закрыть {{title}}», «Агент {{name}} остановлен.» | «Закрыть «{{title}}»», «Агент «{{name}}» остановлен.» | User-named things in running text are quoted, as in shell / tasks. |
| several | placeholders «например, …» | «Например, …» | Placeholders start with a capital letter, like the rest. |
| placeholders (en + ru) | «… (§21)» | removed | Internal spec references are not user-facing (other translators had already dropped them). |
| tasks | critical path «0 мс · 0 задач» | «Критического пути нет» / «Критический путь: 3 задачи (без оценки времени)» | Empty and unestimated paths no longer show a meaningless zero. |
| agents | «6,6 тыс. токен», «10,3 тыс. токена» | «6,6 тыс. токенов» | After a compact number the noun agrees with «тысяча / миллион» (genitive plural). |
| en common | "not initialised" | "not initialized" | US spelling, as in the rest of the English UI. |

## English that was still on screen (fixed)

The `.tsx` guard only sees literals in JSX. The visual pass found English
coming from other places; all of it is now localized:

- **Dockview screen-reader announcements** ("Git opened", "Tasks maximized",
  "Close X", keyboard-docking narration): passed through dockview's
  `messages` hook (`shell:dock.*`), live-switchable.
- **System checks** (Settings → System, onboarding Git / Terminal steps): the
  core sends English titles, details and hints. `checkText.ts` translates them
  by stable check id and known message shape; OS error text is kept as is.
- **Manager "Last session" banner**: the English headline and lines from the
  core are replaced by text built from the counters (`resumeHeadline`,
  `resumeLines`, `resumeIssue`). The protocol gained
  `ManagerResumeSummary.tasksCancelled` so the headline uses the same total
  as the percentage.
- **Built-in agent templates** (names, descriptions) and the agent names made
  from them («Тестировщик #1»), agent activity («Работа завершена»),
  transcript notes, agent and Manager tool summaries, automatic snapshot
  labels («Перед изменениями: Тестировщик #1», «Перед откатом к „…“»),
  task role / epic tags, plan warnings, terminal command-view titles,
  default provider instance names («Mock (офлайн-демо)»): display-only
  translation in `lib/coreText.ts`; the core keeps storing English.
- **Resume card**: 23 event kinds that fell back to "30 × agent state
  changed" now have Russian plural labels.
- **Mock provider demo plan**: task titles, descriptions, criteria, epic,
  feature, summary, assumptions and risks follow the reply language (the
  Mock already answered chat in Russian). Task descriptions written by the
  Manager use «Функция:» instead of "Feature:" when the UI is Russian.
- **Tray**: «Остановить всю работу AI».

## Layout fixes found at 960 × 600

| Screen | Problem | Fix |
|---|---|---|
| Title bar (every screen) | The centred search box covered the approvals button and the «Стоп» button (also overlapped «Остановлено»). | Below 1240 px the search box joins the flex flow and shrinks; its label ellipsizes (`styles/app.css`). |
| Git → Changes | «К коммиту: 0 · автор берётся из настроек Git» wrapped word by word into a narrow column and pushed «Сделать коммит» out of view. | The commit row wraps: info on its own line, buttons below (`git.css`). |
| Git → Snapshots / Branches | Reason badges («Вручную», «Задача · после») were clipped. | Names wrap, badges never shrink. |
| Agents list | The state chip «Завершил работу» squeezed agent names to «Продакт-мен…». | The row header wraps; the chip moves under the name when needed. |

Checked and fine: command palette, create-project dialog, notification
centre, approvals inbox, emergency-stop dialog, all seven settings sections,
all twelve onboarding steps, every panel (Manager, Agents + templates +
editor + spawn dialog, Tasks list / board / graph / drawer / create dialog,
Code + quick open + search, Files, Terminal, Git five tabs + rollback dialog,
Memory four tabs + editor, Costs + budget editor, Logs, phase placeholders).
No horizontal page scroll, no console errors in any view.

Live switch: «Сменить язык» switches to English without a reload
(`performance.timeOrigin` unchanged, `<html lang="en">`), and back.

## Remaining (not translated on purpose or out of scope)

- Free text written by a model (plans, replies, summaries, review notes) —
  it follows the reply-language instruction, not the UI catalogue.
- Core `Log` events in the Logs panel ("file watcher started", "task
  transition") — developer log lines, shown as the core writes them; the
  event-type badges (`TaskUpdated`) are technical identifiers.
- Data typed by users or tools: commit messages and authors, branch names,
  file paths, OS error messages inside check details.
- Generic fallbacks: an unknown core string is shown unchanged. When the core
  changes one of the matched English shapes, the matching pattern in
  `lib/coreText.ts` must change too (covered by `lib/coreText.test.ts`).
- Notifications already in the bell keep the language they arrived in.
- If the language setting is `system`, agents started directly from the
  Agents panel reply in English until the first Manager chat reports the
  resolved language (foundation limitation).
