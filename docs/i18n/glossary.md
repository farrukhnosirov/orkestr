# Orkestr — Russian glossary and style guide

Binding for every Russian string in the UI, the native shell (tray), docs
shown in the app and AI-facing instructions that produce user-visible text.
English is the source language; Russian must read like a native product
(JetBrains / Yandex quality), never like a literal translation.

When a term is missing here, pick the natural Russian word, use it
consistently and add it to this table in the same change.

## 1. Terminology

| English | Russian (noun) | Russian (action / button) | Notes |
|---|---|---|---|
| Orkestr | Orkestr | — | Brand. Never translated or transliterated. |
| AI Development OS | операционная система для AI-разработки | — | Tagline. |
| AI | AI | — | Latin «AI» everywhere (as in «AI-менеджер»), not «ИИ». Hyphenated compounds: «AI-агент», «AI-провайдер». |
| Agent | агент | — | «AI-агент» when the context is not obvious. |
| AI Manager | AI-менеджер | — | Short form in panel titles and chat: «Менеджер». |
| Project | проект | Создать проект | |
| Task | задача | Создать задачу | |
| Plan | план | Подтвердить план / Отклонить | |
| Run (of a plan) | запуск | Запустить / Приостановить / Возобновить | |
| Approval | подтверждение | Подтвердить / Отклонить | «Запрос на подтверждение» for a pending request. |
| Approval mode: Ask / Semi-Auto / Full Auto | «Спрашивать» / «Полуавтомат» / «Полный автомат» | — | Mode names in « ». |
| Permission | разрешение | Разрешить / Запретить | |
| Snapshot | снимок | Сделать снимок | |
| Rollback | откат | Откатить | |
| Worktree | рабочая копия (worktree) | — | Add «(worktree)» on first mention in a screen / tooltip. |
| Branch | ветка | Создать ветку | |
| Commit | коммит | Сделать коммит / Закоммитить (only in dev-heavy UI) | |
| Merge | слияние | Выполнить слияние / Слить | |
| Diff / changes | изменения, сравнение | Показать изменения | Not «дифф» in UI text. |
| Repository | репозиторий | | |
| Provider | провайдер | Подключить провайдера | «AI-провайдер». |
| Model | модель | Выбрать модель | |
| Budget | бюджет | Задать бюджет | |
| Usage | расход | — | «Расход токенов», «расход за месяц». |
| Cost(s) | расходы, стоимость | — | Panel title: «Расходы». |
| Token | токен | — | |
| Limit | лимит, ограничение | — | «Лимит запросов» for rate limit. |
| Rate limited | превышен лимит запросов | — | |
| Emergency Stop | экстренная остановка | Остановить всю работу AI | Command and tray item «Stop All AI». State: «Работа AI остановлена». |
| Mission Control | Центр управления | — | Proper name, capitalised. |
| Memory | память | — | «Память проекта». |
| Decision Log | журнал решений | — | |
| Knowledge Base | база знаний | — | |
| Logs (panel) | журнал | Открыть журнал | «Журнал событий»; not «логи» in UI text. |
| Event | событие | — | |
| Settings | настройки | Открыть настройки | Section names: «Общие», «Система», «О программе». |
| Terminal | терминал | — | «Сеанс терминала». |
| Quality gates | критерии качества | — | |
| Self-healing | самовосстановление | — | |
| Quality pipeline | конвейер качества | Запустить конвейер | Stages: «Автор», «Ревью», «Сборка», «Тесты», «Безопасность», «Слияние». |
| Review (of code / task) | ревью | Отправить на ревью / Принять | Status «На ревью». «Проверка» for automatic checks. |
| Test(s) | тесты | Запустить тесты | |
| Build | сборка | Собрать | |
| Deployment | развёртывание | Развернуть | |
| Environment | окружение | — | |
| Workspace (docking area) | рабочая область | — | |
| Layout | раскладка | Сбросить раскладку | Preset «Разработка» (Coding). |
| Panel | панель | Открыть панель | |
| Command palette | палитра команд | — | |
| Onboarding / Setup wizard | мастер настройки | Запустить мастер настройки | |
| Do Not Disturb | «Не беспокоить» | Включить «Не беспокоить» | |
| Tray | системный трей / область уведомлений | — | «Трей» is fine in settings descriptions. |
| Core (orkestr-core) | ядро | — | «Встроенное ядро» (embedded core). |
| Notification | уведомление | — | |
| Secret / secret vault | секрет / хранилище секретов | — | |
| Template | шаблон | — | |
| Instance | экземпляр | — | |
| Handoff | передача контекста | — | |
| Phase N (roadmap) | этап N | — | «Появится на этапе 2». |
| Mock (offline demo) | Mock (офлайн-демо) | — | Provider label. |
| Memory item | запись (памяти) | Добавить запись | Feminine: «Закреплена». |
| Pin / Unpin | — | Закрепить / Открепить | |
| Knowledge source | источник знаний | Добавить источник | Index states: «В очереди», «Индексируется», «Готов», «Ошибка». |
| Chunk (search) | фрагмент | — | |
| Call (model / tool call) | вызов | — | «1 вызов, 3 вызова, 5 вызовов». |
| Estimated (cost) | оценка, оценочный | — | «оценка — 1,20 $»; badge «С учётом оценок». |
| Unattributed (cost) | без привязки | — | |
| Architecture Map | карта архитектуры | Обновить карту | Nodes: «сервис», «модуль», «API», «база данных», «внешние зависимости». Architecture snapshot — «снимок». |
| Route (HTTP) | маршрут | — | «3 маршрута». |
| Execution Replay | воспроизведение | Воспроизвести | Tab in the Logs panel. Timeline — «хронология», step — «шаг», phase — «этап». |
| Delegation | делегирование | — | Replay phase. |
| Decision status: Proposed / Accepted / Rejected / Superseded | Предложено / Принято / Отклонено / Заменено | Принять / Отклонить | Neuter — agrees with «решение». |
| AI Manager (short form) | Менеджер | Спросить Менеджера | Capitalised everywhere as the name of the feature («чат с Менеджером», «модель Менеджера»); lowercase only inside «AI-менеджер» and job titles («Продакт-менеджер»). |
| Acceptance criteria | критерии приёмки | Добавить критерий | Per task. Not the same as quality gates («критерии качества»). |
| Transcript (agent) | журнал работы | — | |
| Turn (agent loop) | ход | — | «Лимит ходов», «30 ходов». |
| System prompt | системный промпт | — | |
| Staged / Unstaged / Untracked | подготовлено к коммиту / изменения / неотслеживаемые | Добавить в коммит / Убрать из коммита | |
| Safety snapshot | страховочный снимок | — | Taken automatically before discarding, rollback, restore. |
| Trash (deleted files) | корзина Orkestr | Восстановить / Заменить | Copies of files deleted in the Files panel. Not the OS «Корзина». |
| Task rollback | откат задачи | Откатить задачу | Undoes only one task's changes; differs from «Откатить рабочую область». Conflicts — «конфликты». |
| Upstream branch | вышестоящая ветка | — | |
| Shell (terminal) | оболочка | Выбрать оболочку | |
| Session (terminal / app) | сеанс | — | «Сводка сеанса» (session resume). |
| Binary file | двоичный файл | — | |
| Capability (permissions) | тип действия | — | Column header in permission rules. |
| Effect (permission rule) | решение | Разрешить / Спрашивать / Запретить | Check result: «Разрешено / Нужно подтверждение / Запрещено». |
| Emergency controls | экстренные ограничения | — | |
| Re-arm (after emergency stop) | — | Снять остановку | |
| Circuit open (provider) | пауза после сбоев | — | |
| Fallback chain / routing | резервная цепочка / маршрутизация | Добавить в цепочку | «Переход на резервную модель» for a fallback event. |
| Reasoning effort / context policy | глубина рассуждений / объём контекста | — | |
| CPU / RAM | ЦП / ОЗУ («Оперативная память» in labels) | — | Not «Память» (clashes with project memory); a CPU core is «ядро ЦП» (not the Orkestr core). |
| Wake word | фраза активации | — | |
| Push-to-talk | режим рации | Удерживайте клавиши, пока говорите | Voice settings / HUD. |
| Voice notifications | голосовые уведомления | — | |
| Quiet hours | тихие часы | — | |
| Sound set (preset) | набор звуков | — | Presets: «Как в теме», «Туманность», «Кристалл», «Пульс», «Минимум». |
| Built-in agent templates | Архитектор ПО, Бэкенд-разработчик, Фронтенд-разработчик, Инженер баз данных, Тестировщик, Аудитор безопасности, DevOps-инженер, UI/UX-дизайнер, Продакт-менеджер, Ревьюер кода, Исследователь | — | Display names only; the core stores the English names (`lib/coreText.ts`). |
| Epic (task tag) | эпик | — | Tag shown as «эпик: Название». |
| Finding (review) | замечание | Отметить исправленным / Отклонить | Neuter: severities «Критическое / Серьёзное / Среднее / Незначительное / Информационное», statuses «Открыто / Подтверждено / Опровергнуто / Исправлено / Отклонено». |
| Review verdict: Approve / Request changes / Reject | Одобрено / Нужны доработки / Отклонено | — | |
| Adversarial review | ревью оппонента | Запросить ревью оппонента | Badge «Оппонент». Not «состязательное ревью». |
| Consensus (mode) | консенсус, режим консенсуса | Запустить консенсус | |
| Proposal (consensus) | предложение | — | |
| Synthesizer (consensus) | арбитр | — | The model that turns proposals into one decision. |
| Agent team | команда (агентов) | Назначить проекту / Снять с проекта | Built-in names: «ERP-команда», «Веб-команда», «Мобильная команда», «Команда безопасности», «Команда быстрых прототипов». |
| Team lead | лид (команды) | — | Hierarchy levels: Менеджер → Лид → Исполнитель → Ревьюер. Not «тимлид». |
| Executor (team member) | исполнитель | — | |
| Hierarchy (team policy) | иерархия | — | |
| Sub-task | подзадача | — | «до 4 подзадач на лида». |
| AI Product Manager | AI-продакт-менеджер | — | Composer toggle «Режим продакт-менеджера». |
| Feature (product breakdown) | функция | Добавить функцию | Epic → функции → истории → критерии приёмки → задачи. |
| User story | пользовательская история, история | Добавить историю | «Как пользователь, я могу …». |
| Product draft | черновик продукта | Подтвердить / Отклонить | Statuses: Черновик / Подтверждён / Отклонён. |
| Extension (VS Code) | расширение | Установить / Удалить / Включить / Отключить | «Установить из .vsix…». Registry name «Open VSX» as is. |
| Language server | языковой сервер | Разрешить / Остановить / Отозвать разрешение | |
| Color theme (editor) | цветовая тема редактора | — | Separate from the Orkestr UI theme. |
| File icon theme | тема значков файлов | — | Default: «Значки Orkestr». |
| Grammar (TextMate) | грамматика подсветки синтаксиса | — | |
| Snippet | сниппет | — | |
| Verified publisher | подтверждённый издатель | — | Badge «Подтверждён». |

Task statuses: Запланирована, Готова к работе, Выполняется, Ожидает,
Заблокирована, На ревью, Тестируется, Выполнена, Сбой, Отменена, Прервана,
Восстанавливается (feminine — agrees with «задача»).
Agent states: Свободен, Слушает, Думает, Пишет код, Тестирует, Проводит ревью,
Ожидает, Ошибка, Завершил работу, Остановлен (masculine — «агент»).
The canonical strings live in `common:status.*`; reuse them, do not re-translate.

Keep as is (established dev terms / proper names): API, CLI, JSON, Git, GitHub,
PTY, DAG, SSH, Docker, URL, Monaco, Ollama, Claude Code, Codex, keyboard keys
(Ctrl, Shift, Alt, Enter, Esc).

## 2. Tone of voice

- Address the user with **«вы»**, lowercase (UI convention; capital «Вы» only in
  formal letters). Polite and direct, no bureaucratese: «Не удалось сохранить
  настройки», not «Произошла ошибка при попытке осуществления сохранения».
- **Buttons and commands: imperative verb**, sentence case: «Создать проект»,
  «Откатить», «Подтвердить», «Открыть настройки». Not nouns («Создание проекта»)
  and not «Нажмите, чтобы…».
- Status texts: short participles/adjectives agreeing in gender with the
  subject: «Задача выполнена», «Агент остановлен», «Ядро подключено».
- Errors: what happened + what to do. «Не удалось подключиться к ядру Orkestr.
  Повторите попытку.» Avoid blaming the user.
- Progress: noun phrase with an ellipsis: «Загрузка…», «Подключение к ядру…»,
  «Идёт поиск…».
- Empty states: neutral and helpful: «Проектов пока нет — создайте первый…».
- No anglicisms where a natural Russian word exists (настройки, not «сеттинги»;
  журнал, not «логи»; изменения, not «диффы»), but keep established dev terms:
  коммит, ветка, токен, ревью, API, CLI.
- The AI Manager and agents speak in first person, masculine past tense
  («Я изучил проект и подготовил план»).

## 3. Typography

- Quotes: «ёлочки»; nested „лапки“. Never "straight quotes" in Russian text.
- Dash: em dash with spaces « — » between parts of a sentence; en dash without
  spaces for ranges «5–40»; hyphen in compounds «AI-агент».
- Non-breaking space (U+00A0) between a number and its unit: «5 мин», «1,5 МБ»,
  «12 345» (thousands separator), «1,23 $» — use the formatting helpers
  (`useFormat()` / `src/i18n/format.ts`), never hand-built strings.
- Decimal comma: «1,5». Currency after the amount: «1,23 $» (en: «$1.23»).
- Ellipsis is one character «…».
- **Use «ё»** everywhere it is pronounced: «ещё», «всё», «развёртывание», «отменён».
- «№» with a space before the number: «событие № 42».
- Nested quotes: «Перед откатом к „Снимок вручную“».

## 4. Capitalisation

- **Sentence case** for everything: titles, buttons, menu items, tabs,
  section names («Новый проект», «Режим производительности»). Title Case is
  English-only.
- Proper names keep their capitals: Orkestr, Центр управления, Git, Claude Code.
- No trailing period in buttons, titles, labels and single-line hints; full
  sentences in descriptions end with a period.

## 5. Grammar

- Numbers with nouns always go through i18next plurals (`_one`, `_few`,
  `_many`, `_other`): 1 агент, 2 агента, 5 агентов, 21 агент, 1,5 агента.
  Never «агент(ов)» or «агентов: 3» as a shortcut unless the label form is
  deliberate («Пройдено — 12»).
- Agreement: participles and verbs agree with the noun's gender
  («остановлен 1 агент» / «остановлено 3 агента»). If a sentence would need
  several genders, rephrase into the «Метка: значение» form.
- Do not concatenate translated fragments into sentences; put the whole
  sentence into one key with `{{placeholders}}`.
- Gender/context: when the subject can vary, use i18next context keys
  (`key_male`, `key_female`) or rephrase neutrally.

## 6. Length

Russian is ~30 % longer than English (single words up to 50–100 %).
Layouts must not truncate: let buttons and tabs grow, wrap descriptions, and
use `text-overflow: ellipsis` with a `title` tooltip only for user data (names,
paths), never for UI labels. Check every new screen in Russian at the minimum
window size (960 × 600).
