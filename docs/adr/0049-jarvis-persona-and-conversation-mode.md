# ADR-0049 — Jarvis persona and conversation mode

Status: accepted — implemented (wave 9, package B: requests 2 and 3)

## Context

The owner asked for (2) a conversation mode with an AI — a full assistant —
where template phrases are answered without spending tokens and everything
else is understood and answered by the AI, and (3) ~10 000 Russian template
question–answer pairs plus a JARVIS-from-Iron-Man style everywhere the
assistant speaks: «Здравствуйте, сэр.», «Открываю YouTube, сэр.»,
«Прокручиваю вниз, босс.».

## Decision

### Persona

* One i18next default variable, `{{addr}}`, is the form of address in suffix
  form (", сэр" / ", boss" / ", Фаррух" / ""). Every speaking string carries
  it (computer-command replies `assistant:say.*`, `assistant:reply.*`,
  `voice:reply.*`, project-status templates `voice:natural.*`, spoken
  notifications `voice:notify.phrases.*`, `jarvis:say.*`). `applyPersona()`
  refreshes it on settings and language changes; persona off → "". No call
  site had to pass a new variable.
* `jarvisSay(kind, vars)` returns a persona phrase; variants rotate and never
  repeat the previous one. Time-of-day greetings («Доброе утро, сэр»), the
  start-of-app greeting and the first-request-of-the-day greeting (once per
  calendar day, setting «Приветствие при запуске»).
* Settings (`jarvis.getConfig/setConfig`, one JSON document in
  `core_settings`): persona on/off, address (Sir / Boss / Name / None),
  user name, greeting on start, local templates on/off, conversation model,
  memory length (2–60 messages), project context on/off.

### Template bank (no tokens)

* `scripts/gen-jarvis-dialog.mjs` generates `crates/computer/data/dialog.json`
  from curated categories (greetings, courtesy, mood, about Jarvis/Orkestr,
  help, conversation control, confirmations, compliments/insults, jokes &
  facts, time/date, weather, coding small talk, Stark banter, refusals): 108
  intents, 15 400+ unique question phrasings (+~800 typo variants), 336
  answer variants with runtime slots (`{address}`, `{time}`, `{date_full}`,
  `{weekday}`, `{days_to_ny}`, `{weekend}`, `{greeting}`, `{coin}`, …). The
  counting rule (normalized forms, fillers dropped, typos separate) is in
  `docs/jarvis-dialog.md`. 8 % of the generated phrasings are held out into
  `dialog-heldout.json` and never enter the bank.
* Matching (`orkestr_computer::dialog`) reuses the command matcher's text
  layer (normalization, light Russian stemming, concepts, fuzzy tokens) but
  not its aligner: small talk is order-free and its frames («хочу спросить»,
  «если честно») carry little meaning, so tokens are weighted by inverse
  intent frequency and scored with a weighted Dice coefficient against the
  best phrasing; unknown words weigh the most, so real questions fall
  through to the AI. Threshold 0.70. Measured: 98.4 % of held-out
  paraphrases → the right intent, 76.7 % with typos, 0 false positives on
  626 catalogue command phrases + 34 unrelated questions; ~3 ms per query in
  a debug build.
* Intents may carry a client action: `mode.chat`, `mode.commands`,
  `memory.forget`, `speech.stop`, `repeat`, `address.sir|boss|none`.

### Routing (client, `features/jarvis/router.ts`)

For an utterance that is not an Orkestr intent (voice) or anything typed in
the Assistant surface: (0) Jarvis actions; (1) a locally matched computer
command → executed, Jarvis reply; (2) a template answer; (3) conversation
mode → AI conversation; (4) command mode → the AI planner (Assistant UI) or
back to the Orkestr pipeline / Manager (Orchestra UI). Orkestr intents and
pending confirmations / plans keep their priority (unchanged
`routeUtterance` front part).

### AI conversation (`jarvis.chat`)

* Jarvis system prompt (persona, address, language, spoken vs typed length,
  local date and time, a short project summary when enabled) + the rolling
  context (last *N* messages of the persisted history, alternating roles).
* The model is called in exactly one function, `call_model`
  (`crates/core/src/rpc/jarvis.rs`): configured conversation model → the AI
  Manager's model → routing, through `ProviderRegistry::chat` (the existing
  fallback chain: a failing subscription / key moves on to the next
  provider). The offline Mock is used only when chosen explicitly. This is
  the single place to wire a central provider chain later.
* Deltas stream as an ephemeral `JarvisMessage` event (global topic); the
  client shows them and speaks complete sentences while the rest streams.
* Hand-off: the model may end with `<<run: команда>>`; the client strips it
  and runs it as a computer command (risky steps still need confirmation).
* Memory: `jarvis.history` (≤ 200 messages, `core_settings`), `jarvis.record`
  for template / command exchanges, `jarvis.forget` («забудь разговор»).
* The whole `jarvis.*` area is local-only (policy area default).

## Consequences

* Replies now end with a period and carry the address; tests updated.
* The i18n parity guard accepts arrays of variants and exempts the
  language-specific `assistant:say.verbs` table (the guard failed on HEAD
  since the wave-8 natural replies).
* The template bank is Russian; English utterances go to the AI.
* Public client API for other packages: `features/jarvis/index.ts`
  (`jarvisSay`, `jarvisSession`, `useJarvisStore`, `greeting`).
