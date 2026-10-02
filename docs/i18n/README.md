# Internationalisation (i18n)

Orkestr ships in **Russian and English**. English is the source language and
the source of keys; Russian is a first-class language, written to the
standard of `docs/i18n/glossary.md` (binding).

Stack: [i18next](https://www.i18next.com/) + react-i18next in
`apps/desktop`. Everything lives in two places:

```
apps/desktop/src/locales/<lang>/<namespace>.json   translations (en, ru)
apps/desktop/src/i18n/                             runtime, types, formatters, guard tests
  index.ts              init, useT, t, useFormat, language switching
  format.ts             Intl-based formatters
  errors.ts             localized RPC errors (describeError)
  resources.ts          auto-discovery of locale files (import.meta.glob)
  namespaces/<ns>.ts    one type file per namespace (type-safe keys)
  hardcoded.ts          TSX scanner used by the hardcoded-string guard
  hardcoded-allowlist.json  temporary list of unconverted files + allowed literals
apps/desktop/src/lib/language.ts   switchLanguage(), <html lang>, tray, design-system strings
```

## Adding strings

1. Pick the namespace of the feature (create one if needed — see below).
2. Add the key to `locales/en/<ns>.json` **and** `locales/ru/<ns>.json`
   (nested objects, camelCase keys: `"createProject": { "title": "…" }`).
3. Use it:

```tsx
import { useT } from "../../i18n";

export function Example({ name }: { name: string }) {
  const { t } = useT("tasks");               // or useT(["tasks", "common"])
  return (
    <button title={t("create.tooltip")} aria-label={t("common:actions.close")}>
      {t("create.button", { name })}
    </button>
  );
}
```

Keys are type-checked against the English file: a missing or misspelled key
is a `tsc` error. Prefix another namespace with `ns:` (`t("common:actions.cancel")`).

Outside React (stores, notifications, command handlers) import the global
`t` from `src/i18n` — it reads the current language at call time:

```ts
import { t } from "../../i18n";
notify("Error", t("tasks:errors.createFailed"), describeError(e));
```

**Registries** (commands, panels, settings sections, onboarding steps) must
not capture a translated string at module load. Use getters, so palette,
tabs and menus follow a live language switch:

```ts
{
  id: "tasks.create",
  get title() { return t("tasks:commands.create"); },
  get group() { return t("shell:commandGroups.projects"); },
  keywords: ["task", "new", "задача", "создать"],   // both languages, for search
  run: …,
}
```

Rich text (bold, links) uses `<Trans>` with named components:

```tsx
<Trans t={t} i18nKey="shell:phasePlaceholder.project" values={{ name }} components={{ strong: <strong /> }} />
// "Будет работать с проектом <strong>{{name}}</strong>."
```

Never concatenate translated fragments, never put user-visible English in
TSX, and never translate inside the core: the core sends codes and data, the
UI renders text.

## Namespaces

A namespace = one JSON file per language. Present:

| Namespace | Owner / content |
|---|---|
| `common` | shared words: actions (Отмена, Сохранить…), states, statuses of tasks/agents/approvals/providers, counted nouns (`count.agents`…), error-code labels, language names, units |
| `shell` | app shell: title bar, status bar, project rail, command palette, workspace, panel titles/descriptions, core commands, settings frame |
| `events` | one-line descriptions of core events (Logs, notifications) |
| `placeholders` | "arrives in phase N" panels |

Adding a namespace `foo`: create `locales/en/foo.json`, `locales/ru/foo.json`
and `i18n/namespaces/foo.ts`:

```ts
import type en from "../../locales/en/foo.json";
declare module "i18next" {
  interface ResourceNamespaceMap { foo: typeof en; }
}
```

Nothing else: the files are discovered by `import.meta.glob`, and the parity
test fails if the type file is missing. Prefer one namespace per feature or
panel (`tasks`, `agents`, `git`, `memory`, `providers`, `security`,
`settings`, `onboarding`, …) so parallel work never edits the same file.

## Plurals, interpolation, context

- Plurals use i18next suffixes. English needs `_one` + `_other`; Russian needs
  **all of** `_one`, `_few`, `_many`, `_other` (`_other` is used for
  fractions: «1,5 агента»). `_zero` is optional in both.

  ```json
  "agents_one": "{{count}} агент",
  "agents_few": "{{count}} агента",
  "agents_many": "{{count}} агентов",
  "agents_other": "{{count}} агента"
  ```
  Call with `t("common:count.agents", { count: n })`.
- Interpolation: `{{name}}`. Every language must use the same placeholders
  (checked by the parity test). Values are not HTML-escaped (React escapes).
- Formatters inside strings: `{{size, bytes}}`, `{{ms, duration}}`,
  `{{amount, usd}}`, `{{ts, relative}}`, plus i18next built-ins
  (`{{n, number}}`, `{{d, datetime}}`, `{{items, list}}`).
- Context (e.g. gender): `key_male` / `key_female`, call with
  `t("key", { context: "female" })`. Prefer neutral wording when possible.

## Formatting helpers

Never format numbers, money, dates or sizes by hand. In components use
`const fmt = useFormat()` (re-renders on language change); elsewhere import
the functions from `src/i18n`:

| Helper | en | ru |
|---|---|---|
| `fmt.number(12345.6)` | 12,345.6 | 12 345,6 |
| `fmt.currency(1.23)` (USD) | $1.23 | 1,23 $ |
| `fmt.percent(0.25)` | 25% | 25 % |
| `fmt.date(ts, "date")` | Oct 2, 2026 | 2 окт. 2026 г. |
| `fmt.relative(ts)` | 5 minutes ago | 5 минут назад |
| `fmt.bytes(1572864)` | 1.5 MB | 1,5 МБ |
| `fmt.duration(185000)` | 3 min 5 sec | 3 мин 5 с |
| `fmt.list(["a","b","c"])` | a, b, and c | a, b и c |

Spaces before units and thousands separators are non-breaking.

## Errors from the core

The core answers with structured codes (`not_found`, `forbidden`,
`budget_exceeded`, …; contract §4). Show errors with
`describeError(e)` (`src/i18n/errors.ts`) or `errorMessage(e)` from
`lib/rpcQuery`: in Russian the translated code label leads and the core's
technical detail follows («Не найдено: project 42 not found»). Labels live in
`common:errors.codes.*`.

## Language selection

- `AppSettings.language` stores `ru`, `en` or `system` (default for new
  installs). `system` resolves to Russian when the OS/browser prefers Russian,
  otherwise English.
- Startup: i18next starts with the last preference cached in `localStorage`
  (`orkestr.language`) or the system language, then follows `settings.get`
  and live `SettingsChanged` events — no reload.
- Switchers: Settings → General, onboarding step 1, command
  «Switch Language / Сменить язык». They call `switchLanguage(pref)` from
  `lib/language.ts` (applies live + `settings.update`).
- Side effects of a switch: `<html lang>`, design-system defaults
  (`setUiStrings`), the native tray (`set_ui_language` Tauri command), dock tab
  titles.
- AI: `manager.chat` sends the resolved language in `options.language`; the
  core keeps it in a shared `UiLanguage` handle (also set from explicit
  settings) and the Manager, planner and agents get a "reply in <language>"
  section in their system prompts. The Mock provider answers in Russian when
  the UI language is Russian or the user writes in Cyrillic.

## Checks

`npm test -w @orkestr/desktop` runs:

- `src/i18n/parity.test.ts` — same namespaces and keys in en/ru, a type file
  per namespace, no empty values, complete plural sets (ru: one/few/many/other),
  identical placeholders and tags per key.
- `src/i18n/hardcoded.test.ts` — scans every `src/**/*.tsx` (tests excluded)
  with the TypeScript compiler API for JSX text with letters and literal
  strings in `title`, `placeholder`, `aria-label`, `alt`, `label`,
  `description`, `tooltip`, `hint`, `subtitle` and JSX children
  (`{ok ? "On" : "Off"}`). Exceptions:
  - `literals` in `hardcoded-allowlist.json` — texts identical in every
    language (brand, keys). Keep it short.
  - `files` — **temporary** list of files not converted yet. Remove your file
    in the same change that converts it; the test fails if a listed file is
    already clean. The list must end up empty.
- `src/i18n/i18n.test.ts` — language resolution, Russian plurals, formatters,
  error localization.

Before you finish a UI change: switch to Russian (Ctrl+K → «Сменить язык»),
walk through the screen at 960 × 600 and make sure nothing is truncated or
left in English.

## Text that comes from the core

The core (Rust) stores and sends some fixed English strings as data: built-in
agent template names and descriptions (and agent names like "QA Engineer #1"),
agent activity lines and transcript notes, automatic snapshot labels, task
role tags, plan warnings, tool-call summaries, `system.checks` results and the
Manager's resume headline. They stay English in the database (other clients
read them); the UI translates their **display**:

- `src/lib/coreText.ts` — `agentName`, `templateName`, `templateDescription`,
  `agentActivity`, `agentNote`, `agentToolSummary`, `managerToolSummary`,
  `snapshotLabel`, `taskTag`, `planWarning`, `terminalTitle` (namespace
  `coreText`).
- `src/features/settings/sections/checkText.ts` — system checks by stable id.
- The Manager resume banner is built from its counters (`resumeHeadline`,
  `resumeLines`, `resumeIssue` in `panels/Manager/parts.tsx`).

Each helper matches the exact English shape the core produces and returns
anything else unchanged, so user-entered names and unknown text pass through.
When you change one of these strings in Rust, update the matching pattern and
`src/lib/coreText.test.ts`. Prefer codes in the protocol for new messages.

Free text written by a model (plans, replies, summaries) and core log lines
(`Log` events) are not translated. The Mock provider answers in the UI
language (including its demo plan).

## Pitfalls

- `context` is a reserved i18next option — never use it as a placeholder
  name (`{{context}}`); use e.g. `{{window}}`.
- Third-party components with their own English strings must be localized
  through their hooks (dockview: `messages` in `components/Workspace.tsx`).
- `scripts/smoke/i18n-ru-screenshots.mjs` drives the real app in Russian at
  960×600, captures every panel, dialog, settings section and onboarding step
  (webview only), reports clipped text and Latin-only fragments, and checks the
  live switch to English. Screenshots: `docs/screenshots/i18n-ru/`.
