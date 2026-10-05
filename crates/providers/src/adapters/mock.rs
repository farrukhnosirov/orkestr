//! "Mock (offline demo)" — deterministic, offline, clearly labelled provider
//! (spec §10). Its output is a pure function of (model id, conversation, tool
//! list, response format), so the Manager and agents can be demoed and tested
//! without keys or network.
//!
//! Scripted models:
//! * `mock-smart` (default) — planning request (schema `orkestr_plan` or a
//!   planning system prompt) → a goal-aware 4-task plan with dependencies;
//!   AI Manager chat (`propose_plan` tool) → plans build requests, reads
//!   state for status questions; agent loop (`submit_result` tool) → writes
//!   plausible role-specific files (design note, `src/<x>.mjs`, docs,
//!   `node:test` tests), commits on its branch, QA runs `node --test`, then
//!   `submit_result`; tool result → summary; JSON schema → a schema-valid
//!   document; a `write_file` tool → calls it, then finishes; otherwise a
//!   short reply.
//! * `mock-echo` — echoes the last user message.
//! * `mock-tool-roundtrip` — calls the first declared tool once with
//!   schema-valid arguments, then finishes.
//! * `mock-slow` — like smart, slow stream.
//! * `mock-rate-limit` / `mock-quota` / `mock-server-error` — fail with the
//!   corresponding classified error (fallback demos/tests).
//!
//! Self-Healing demo scenarios (Phase 2 quality pipeline), keyed by a marker
//! in the task title:
//! * [`HEAL_ONCE_MARKER`] — the first implementation fails its tests (a
//!   `package.json` with `npm test`, a `node:test` suite and a greeting
//!   without punctuation); the fix attempt (first message carries
//!   [`HEALING_BRIEF_HEADING`]) writes the correct implementation.
//! * [`HEAL_NEVER_MARKER`] — every attempt writes the same bug, so the
//!   failure signature repeats and the pipeline escalates to the user.

use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{Model, ModelCapabilities, ProviderHealth, ProviderId, ProviderKind};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use crate::adapter::ProviderAdapter;
use crate::catalog::{default_models, MOCK_DISPLAY_NAME};
use crate::http::drive;
use crate::types::{
    AuthStatus, ChatEvent, ChatMessage, ChatRequest, ChatStream, CostModelKind, HealthReport, Locality,
    ProviderDescriptor, ProviderError, ResponseFormat, Role, StopReason, ToolSpec, Usage,
};

/// Task-title marker: the first implementation fails its tests; the
/// Self-Healing fix attempt corrects it.
pub const HEAL_ONCE_MARKER: &str = "[mock:heal-once]";
/// Task-title marker: no fix attempt ever corrects the failing tests.
pub const HEAL_NEVER_MARKER: &str = "[mock:heal-never]";
/// Heading of the quality pipeline's fix briefing in a fix agent's first
/// message (`# Self-healing fix (attempt N of M)`).
pub const HEALING_BRIEF_HEADING: &str = "# Self-healing fix";
/// Comment marking the scripted bug of the healing scenarios.
pub const HEAL_BUG_COMMENT: &str = "BUG (mock self-healing demo)";

/// Mock tuning.
#[derive(Debug, Clone, Copy)]
pub struct MockOptions {
    /// Delay between streamed chunks.
    pub chunk_delay: Duration,
}

impl Default for MockOptions {
    fn default() -> Self {
        Self { chunk_delay: Duration::from_millis(15) }
    }
}

/// The mock adapter.
#[derive(Debug)]
pub struct MockAdapter {
    id: ProviderId,
    name: String,
    options: MockOptions,
}

impl MockAdapter {
    /// Creates the adapter.
    #[must_use]
    pub fn new(id: ProviderId, name: impl Into<String>, options: MockOptions) -> Self {
        let name = name.into();
        // The label must always identify the provider as a mock.
        let name = if name.contains("Mock") { name } else { format!("{name} — {MOCK_DISPLAY_NAME}") };
        Self { id, name, options }
    }
}

/// What the mock decided to do.
#[derive(Debug, Clone, PartialEq)]
pub enum MockScript {
    /// Stream this text.
    Text(String),
    /// Call a tool.
    Tool {
        /// Tool name.
        name: String,
        /// Arguments.
        arguments: Value,
    },
    /// Fail.
    Fail(ProviderError),
}

/// Language of the Mock's own sentences: Russian when the UI language is
/// Russian (system prompt marker) or the user writes in Cyrillic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockLang {
    /// English.
    En,
    /// Russian.
    Ru,
}

impl MockLang {
    fn pick(self, en: String, ru: String) -> String {
        match self {
            Self::En => en,
            Self::Ru => ru,
        }
    }
}

/// Reply language of a request (see [`MockLang`]).
#[must_use]
pub fn reply_lang(req: &ChatRequest) -> MockLang {
    let marker = format!("{} ru", crate::UI_LANGUAGE_MARKER);
    let ui_ru = req.system_prompt().is_some_and(|s| s.contains(&marker));
    let cyrillic = last_user(req).chars().any(|c| matches!(c, '\u{0400}'..='\u{04FF}'));
    if ui_ru || cyrillic {
        MockLang::Ru
    } else {
        MockLang::En
    }
}

fn last_user(req: &ChatRequest) -> String {
    req.messages.iter().rev().find(|m| m.role == Role::User).map(|m| m.content.clone()).unwrap_or_default()
}

fn short(s: &str, n: usize) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > n {
        format!("{}…", t.chars().take(n).collect::<String>())
    } else {
        t
    }
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 32 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() {
        "demo".into()
    } else {
        out
    }
}

/// Deterministic schema-valid sample (supports the JSON-schema subset used
/// for tools and structured output).
#[must_use]
pub fn sample_from_schema(schema: &Value, key: &str, hint: &str, index: usize) -> Value {
    sample(schema, key, hint, index, 0)
}

fn sample(schema: &Value, key: &str, hint: &str, index: usize, depth: usize) -> Value {
    if depth > 8 {
        return Value::Null;
    }
    if let Some(c) = schema.get("const") {
        return c.clone();
    }
    if let Some(e) = schema.get("enum").and_then(Value::as_array).and_then(|a| a.get(index % a.len().max(1))) {
        return e.clone();
    }
    if let Some(d) = schema.get("default") {
        return d.clone();
    }
    for comb in ["anyOf", "oneOf", "allOf"] {
        if let Some(first) = schema
            .get(comb)
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|s| s.get("type") != Some(&json!("null"))))
        {
            return sample(first, key, hint, index, depth + 1);
        }
    }
    let ty = match schema.get("type") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).find(|t| *t != "null").unwrap_or("null").to_owned(),
        _ if schema.get("properties").is_some() => "object".into(),
        _ if schema.get("items").is_some() => "array".into(),
        _ => "string".into(),
    };
    let k = key.to_ascii_lowercase();
    match ty.as_str() {
        "object" => {
            let mut map = Map::new();
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                for (name, sub) in props {
                    map.insert(name.clone(), sample(sub, name, hint, index, depth + 1));
                }
            }
            Value::Object(map)
        }
        "array" => {
            let min = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
            let max = schema.get("maxItems").and_then(Value::as_u64).map(|m| m as usize);
            let plural = ["tasks", "steps", "subtasks", "items"].iter().any(|p| k.contains(p));
            let mut n = min.max(if plural { 3 } else { 1 });
            if let Some(max) = max {
                n = n.min(max);
            }
            let items = schema.get("items").cloned().unwrap_or_else(|| json!({"type": "string"}));
            let is_dep = k.contains("depend") || k.contains("after") || k.contains("blocked");
            if is_dep {
                // Dependencies point at the previous item (a simple chain).
                return if index == 0 { json!([]) } else { json!([format!("t{index}")]) };
            }
            Value::Array((0..n).map(|i| sample(&items, key.trim_end_matches('s'), hint, i, depth + 1)).collect())
        }
        "integer" => json!(schema.get("minimum").and_then(Value::as_i64).unwrap_or(1).max(if k.contains("priority") {
            1
        } else {
            0
        })),
        "number" => json!(schema.get("minimum").and_then(Value::as_f64).unwrap_or(1.0)),
        "boolean" => json!(true),
        "null" => Value::Null,
        _ => {
            let n = index + 1;
            let s = if k == "id" || k.ends_with("_id") || k.ends_with("id") {
                format!("t{n}")
            } else if k.contains("path") || k.contains("file") {
                format!("orkestr-demo/{}.md", slug(hint))
            } else if k.contains("content") || k.contains("body") || k.contains("text") {
                format!("# {}\n\nGenerated by Mock (offline demo).\n", short(hint, 80))
            } else if k.contains("title") || k.contains("name") {
                const TITLES: [&str; 4] =
                    ["Design the approach", "Implement the change", "Write tests", "Review and document"];
                TITLES[index % TITLES.len()].to_owned()
            } else if k.contains("role") || k.contains("agent") {
                const ROLES: [&str; 4] = ["Architect", "Backend", "QA", "Reviewer"];
                ROLES[index % ROLES.len()].to_owned()
            } else if k.contains("summary") || k.contains("description") || k.contains("goal") || k.contains("reason") {
                format!("Mock (offline demo) step {n} for: {}", short(hint, 80))
            } else {
                format!("mock {key}")
            };
            let min_len = schema.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize;
            let mut s = s;
            while s.chars().count() < min_len {
                s.push('.');
            }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
                s = s.chars().take(max as usize).collect();
            }
            json!(s)
        }
    }
}

/// Words ignored when looking for the subject of a request.
const STOP_WORDS: [&str; 34] = [
    "a",
    "an",
    "the",
    "create",
    "build",
    "implement",
    "add",
    "make",
    "write",
    "new",
    "simple",
    "small",
    "with",
    "and",
    "for",
    "of",
    "to",
    "please",
    "some",
    "tests",
    "test",
    "design",
    "document",
    "review",
    "module",
    "service",
    "feature",
    "plan",
    "me",
    "my",
    "our",
    "us",
    "it",
    "that",
];

/// Nouns after which the previous word names the thing to build
/// ("a **hello** module").
const KIND_WORDS: [&str; 10] =
    ["module", "service", "component", "library", "feature", "endpoint", "page", "api", "function", "helper"];

/// The subject of a request, e.g. `hello` for "Create a hello module with
/// tests" (lowercase `[a-z0-9-]`, never empty).
#[must_use]
pub fn subject_of(text: &str) -> String {
    let words: Vec<String> = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let clean = |w: &str| -> String {
        let s: String = w.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(24).collect();
        s.trim_matches('-').to_owned()
    };
    for (i, w) in words.iter().enumerate() {
        if KIND_WORDS.contains(&w.as_str()) && i > 0 {
            let prev = &words[i - 1];
            if !STOP_WORDS.contains(&prev.as_str()) {
                let c = clean(prev);
                if !c.is_empty() {
                    return c;
                }
            }
        }
    }
    words
        .iter()
        .find(|w| w.len() >= 3 && !STOP_WORDS.contains(&w.as_str()) && w.chars().any(|c| c.is_ascii_alphabetic()))
        .map(|w| clean(w))
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| "demo".into())
}

/// A JavaScript identifier for the subject (`hello-world` → `helloWorld`).
#[must_use]
pub fn ident_of(subject: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in subject.chars() {
        if c == '-' || c == '_' {
            upper = !out.is_empty();
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'm');
    }
    out
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

/// The plan returned for planning requests (a JSON schema named
/// `orkestr_plan`, or a planning system prompt), in English. See [`demo_plan_in`].
#[must_use]
pub fn demo_plan(goal: &str) -> Value {
    demo_plan_in(goal, MockLang::En)
}

/// The demo plan in the reply language. Four tasks with dependencies and a
/// parallel branch: design → (implement ∥ document), implement → test.
/// Every task names its role, acceptance criteria and expected files; roles,
/// keys, priorities and file paths do not depend on the language.
#[must_use]
pub fn demo_plan_in(goal: &str, lang: MockLang) -> Value {
    let g = short(goal, 120);
    let s = subject_of(goal);
    let name = title_case(&s);
    let f = ident_of(&s);
    let ru = lang == MockLang::Ru;
    let tr = |en: String, ru_text: String| if ru { ru_text } else { en };
    let epic = tr(format!("{name} module"), format!("Модуль {s}"));
    json!({
        "goal": g,
        "summary": tr(
            format!("Demo plan (Mock, offline) for: {g}. An architect designs the {s} module first; then a backend agent implements it while a product manager writes the usage docs in parallel; finally QA adds tests and runs them."),
            format!("Демо-план (Mock, офлайн) для цели: {g}. Сначала архитектор проектирует модуль {s}, затем бэкенд-разработчик реализует его, а продакт-менеджер параллельно пишет документацию; в конце тестировщик добавляет и запускает тесты."),
        ),
        "assumptions": [
            tr("The project is a JavaScript (ES module) code base or an empty folder.".into(), "Проект — кодовая база на JavaScript (ES-модули) или пустая папка.".into()),
            tr("Tests run with Node's built-in test runner (`node --test`).".into(), "Тесты запускаются встроенным средством Node (`node --test`).".into())
        ],
        "risks": [tr(
            "Mock (offline demo) output is scripted — connect a real provider for real work.".into(),
            "Ответы Mock (офлайн-демо) заранее заданы — для настоящей работы подключите реального провайдера.".into(),
        )],
        "tasks": [
            {
                "key": "t1",
                "title": tr(format!("Design the {s} module API"), format!("Спроектировать API модуля {s}")),
                "description": tr(
                    format!("Write a short design note for the {s} module: its public function `{f}(name)`, file layout and test strategy."),
                    format!("Написать краткую проектную заметку о модуле {s}: публичная функция `{f}(name)`, структура файлов и стратегия тестирования."),
                ),
                "acceptanceCriteria": [
                    tr(format!("docs/{s}-design.md describes the API of `{f}`"), format!("В docs/{s}-design.md описан API `{f}`")),
                    tr("The file layout and test strategy are listed".to_owned(), "Перечислены структура файлов и стратегия тестирования".to_owned())
                ],
                "role": "Architect",
                "dependsOn": [],
                "priority": "High",
                "epic": epic,
                "feature": tr("Design".into(), "Проектирование".into()),
                "files": [format!("docs/{s}-design.md")],
                "estimatedTokens": 6000
            },
            {
                "key": "t2",
                "title": tr(format!("Implement the {s} module"), format!("Реализовать модуль {s}")),
                "description": tr(
                    format!("Implement `{f}(name = \"world\")` in src/{s}.mjs returning `Hello, <name>!` and commit it."),
                    format!("Реализовать в src/{s}.mjs функцию `{f}(name = \"world\")`, которая возвращает `Hello, <name>!`, и закоммитить её."),
                ),
                "acceptanceCriteria": [
                    tr(format!("src/{s}.mjs exports `{f}`"), format!("src/{s}.mjs экспортирует `{f}`")),
                    tr(format!("`{f}(\"Orkestr\")` returns \"Hello, Orkestr!\""), format!("`{f}(\"Orkestr\")` возвращает \"Hello, Orkestr!\""))
                ],
                "role": "Backend",
                "dependsOn": ["t1"],
                "priority": "High",
                "epic": epic,
                "feature": tr("Implementation".into(), "Реализация".into()),
                "files": [format!("src/{s}.mjs")],
                "estimatedTokens": 12000
            },
            {
                "key": "t3",
                "title": tr(format!("Document the {s} module"), format!("Задокументировать модуль {s}")),
                "description": tr(
                    format!("Write usage documentation for the {s} module in docs/{s}.md."),
                    format!("Написать документацию по использованию модуля {s} в docs/{s}.md."),
                ),
                "acceptanceCriteria": [tr(
                    format!("docs/{s}.md shows how to import and call `{f}`"),
                    format!("В docs/{s}.md показано, как импортировать и вызвать `{f}`"),
                )],
                "role": "Product Manager",
                "dependsOn": ["t1"],
                "priority": "Normal",
                "epic": epic,
                "feature": tr("Documentation".into(), "Документация".into()),
                "files": [format!("docs/{s}.md")],
                "estimatedTokens": 4000
            },
            {
                "key": "t4",
                "title": tr(format!("Test the {s} module"), format!("Протестировать модуль {s}")),
                "description": tr(
                    format!("Add tests/{s}.test.mjs (node:test) for `{f}` and run them with `node --test`."),
                    format!("Добавить tests/{s}.test.mjs (node:test) для `{f}` и запустить их командой `node --test`."),
                ),
                "acceptanceCriteria": [
                    tr(format!("tests/{s}.test.mjs covers `{f}`"), format!("tests/{s}.test.mjs покрывает `{f}`")),
                    tr("`node --test` passes".to_owned(), "`node --test` проходит без ошибок".to_owned())
                ],
                "role": "QA",
                "dependsOn": ["t2"],
                "priority": "Normal",
                "epic": epic,
                "feature": tr("Quality".into(), "Качество".into()),
                "files": [format!("tests/{s}.test.mjs")],
                "estimatedTokens": 8000
            }
        ]
    })
}

fn has_tool(req: &ChatRequest, name: &str) -> bool {
    req.tools.iter().any(|t| t.name == name)
}

/// Text between `(role ` and `)` in the agent system prompt.
fn agent_role(system: &str) -> String {
    system
        .split_once("(role ")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(r, _)| r.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The `# Task: …` line of the agent's first message.
fn agent_task_title(req: &ChatRequest) -> String {
    req.messages
        .iter()
        .filter(|m| m.role == Role::User)
        .flat_map(|m| m.content.lines())
        .find_map(|l| l.strip_prefix("# Task:"))
        .map(|t| t.trim().to_owned())
        .unwrap_or_else(|| short(&last_user(req), 80))
}

/// One scripted agent step.
struct Step {
    tool: &'static str,
    args: Value,
}

fn write_step(path: String, content: String) -> Step {
    Step { tool: "write_file", args: json!({"path": path, "content": content}) }
}

/// Attempt number of a Self-Healing fix request (`None` for a first attempt).
fn healing_attempt(req: &ChatRequest) -> Option<u32> {
    let first = req.messages.iter().find(|m| m.role == Role::User)?;
    let line = first.content.lines().find(|l| l.trim_start().starts_with(HEALING_BRIEF_HEADING))?;
    Some(
        line.split("attempt ")
            .nth(1)
            .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(1),
    )
}

/// Files of the Self-Healing demo scenarios (implementation, tests, npm
/// manifest) — buggy unless this is a fix attempt of a heal-once task.
fn healing_files(title: &str, req: &ChatRequest) -> Option<Vec<(String, String)>> {
    let never = title.contains(HEAL_NEVER_MARKER);
    if !never && !title.contains(HEAL_ONCE_MARKER) {
        return None;
    }
    let clean = title.replace(HEAL_NEVER_MARKER, "").replace(HEAL_ONCE_MARKER, "");
    let s = subject_of(clean.trim());
    let f = ident_of(&s);
    let name = title_case(&s);
    let attempt = healing_attempt(req);
    let fixed = attempt.is_some() && !never;
    let body = if fixed {
        format!(
            "// {name} module — generated by Mock (offline demo); fixed by self-healing.

export function {f}(name = \"world\") {{
  return `Hello, ${{name}}!`;
}}
"
        )
    } else {
        let note = attempt
            .map(|n| {
                format!(
                    "
// Self-healing attempt {n} kept the bug."
                )
            })
            .unwrap_or_default();
        format!(
            "// {name} module — generated by Mock (offline demo).
// {HEAL_BUG_COMMENT}: the greeting misses its punctuation.{note}

export function {f}(name = \"world\") {{
  return `Hello, ${{name}}`;
}}
"
        )
    };
    let mut files = vec![(format!("src/{s}.mjs"), body)];
    if attempt.is_none() {
        files.push((
            format!("tests/{s}.test.mjs"),
            format!(
                "// Tests for the {s} module — generated by Mock (offline demo).
import {{ test }} from \"node:test\";
import assert from \"node:assert/strict\";

import {{ {f} }} from \"../src/{s}.mjs\";

test(\"{f} greets by name\", () => {{
  assert.equal({f}(\"Orkestr\"), \"Hello, Orkestr!\");
}});
"
            ),
        ));
        files.push((
            "package.json".to_owned(),
            "{
  \"name\": \"mock-healing-demo\",
  \"private\": true,
  \"type\": \"module\",
  \"scripts\": { \"test\": \"node --test\" }
}
"
            .to_owned(),
        ));
    }
    Some(files)
}

fn agent_steps(role: &str, title: &str, req: &ChatRequest) -> Vec<Step> {
    if let Some(files) = healing_files(title, req) {
        let mut steps: Vec<Step> = files.into_iter().map(|(p, c)| write_step(p, c)).collect();
        if has_tool(req, "git_commit") {
            let msg = match healing_attempt(req) {
                Some(n) => format!("{title}: self-healing attempt {n} (Mock offline demo)"),
                None => format!("{title} (Mock offline demo)"),
            };
            steps.push(Step { tool: "git_commit", args: json!({ "message": msg }) });
        }
        steps.retain(|st| has_tool(req, st.tool));
        return steps;
    }
    let s = subject_of(title);
    let f = ident_of(&s);
    let name = title_case(&s);
    let lower = title.to_ascii_lowercase();
    let mut steps = Vec::new();
    match role {
        "qa" => {
            steps.push(write_step(
                format!("tests/{s}.test.mjs"),
                format!(
                    "// Tests for the {s} module — generated by Mock (offline demo).\nimport {{ test }} from \"node:test\";\nimport assert from \"node:assert/strict\";\n\nimport {{ {f} }} from \"../src/{s}.mjs\";\n\ntest(\"{f} greets by name\", () => {{\n  assert.equal({f}(\"Orkestr\"), \"Hello, Orkestr!\");\n}});\n\ntest(\"{f} has a default\", () => {{\n  assert.equal({f}(), \"Hello, world!\");\n}});\n"
                ),
            ));
        }
        "reviewer" | "security" | "researcher" => {
            steps.push(Step { tool: "list_dir", args: json!({"path": ".", "depth": 2}) });
            steps.push(Step { tool: "git_diff", args: json!({}) });
        }
        "architect" | "product manager" | "ui/ux" => {
            if lower.contains("doc") || title.to_lowercase().contains("документ") {
                steps.push(write_step(
                    format!("docs/{s}.md"),
                    format!("# {name}\n\nUsage of the {s} module.\n\n```js\nimport {{ {f} }} from \"../src/{s}.mjs\";\n\n{f}(\"Orkestr\"); // \"Hello, Orkestr!\"\n{f}();          // \"Hello, world!\"\n```\n\n_Generated by Mock (offline demo)._\n"),
                ));
            } else {
                steps.push(write_step(
                    format!("docs/{s}-design.md"),
                    format!("# {name} module — design\n\nTask: {title}\n\n## API\n\n- `{f}(name = \"world\"): string` — returns `Hello, <name>!`.\n\n## Files\n\n- `src/{s}.mjs` — implementation (ES module)\n- `tests/{s}.test.mjs` — `node:test` suite\n- `docs/{s}.md` — usage\n\n## Test strategy\n\nUnit tests with Node's built-in runner: `node --test tests/{s}.test.mjs`.\n\n_Generated by Mock (offline demo)._\n"),
                ));
            }
        }
        _ => {
            steps.push(write_step(
                format!("src/{s}.mjs"),
                format!("// {name} module — generated by Mock (offline demo).\n\n/**\n * Returns a greeting.\n * @param {{string}} [name]\n * @returns {{string}}\n */\nexport function {f}(name = \"world\") {{\n  return `Hello, ${{name}}!`;\n}}\n"),
            ));
        }
    }
    let writes = steps.iter().any(|st| st.tool == "write_file");
    if writes && has_tool(req, "git_commit") {
        steps.push(Step { tool: "git_commit", args: json!({"message": format!("{title} (Mock offline demo)")}) });
    }
    if role == "qa" {
        steps.push(Step { tool: "run_command", args: json!({"command": format!("node --test tests/{s}.test.mjs")}) });
    }
    for command in requested_commands(req) {
        steps.push(Step { tool: "run_command", args: json!({ "command": command }) });
    }
    steps.retain(|st| has_tool(req, st.tool));
    steps
}

/// Commands the task explicitly asks for, one per line in the form
/// ``Run: `<command>` `` in the task message (the first user message). They
/// run after the role's own steps, so a demo task can exercise the command
/// gate (e.g. a dangerous command that needs approval).
fn requested_commands(req: &ChatRequest) -> Vec<String> {
    let Some(task) = req.messages.iter().find(|m| m.role == Role::User) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    // The context bundle after the task repeats the task text: stop there.
    for l in task.content.lines().map(str::trim).take_while(|l| !l.starts_with("# Context")) {
        let Some(rest) = l.get(..4).filter(|p| p.eq_ignore_ascii_case("run:")).map(|_| l[4..].trim()) else {
            continue;
        };
        let Some(cmd) = rest.strip_prefix('`').and_then(|r| r.strip_suffix('`')).map(str::trim) else {
            continue;
        };
        if !cmd.is_empty() && !cmd.contains('`') && !out.iter().any(|c| c == cmd) && out.len() < 4 {
            out.push(cmd.to_owned());
        }
    }
    out
}

/// Whether a tool result reports a failure (error flag or non-zero exit).
fn result_failed(m: &ChatMessage) -> bool {
    if m.is_error {
        return true;
    }
    let c = m.content.to_ascii_lowercase();
    ["exit code: ", "exit_code: ", "exit code ", "exited with "].iter().any(|p| {
        c.split(p)
            .skip(1)
            .any(|rest| rest.trim_start().chars().next().is_some_and(|ch| ch.is_ascii_digit() && ch != '0'))
    })
}

/// Scripted agent loop: the role's steps, one tool call per turn, then
/// `submit_result` (status `partial` when a step failed).
fn agent_script(req: &ChatRequest) -> MockScript {
    let system = req.system_prompt().unwrap_or_default();
    let role = agent_role(&system);
    let title = agent_task_title(req);
    let steps = agent_steps(&role, &title, req);
    let results: Vec<&ChatMessage> = req.messages.iter().filter(|m| m.role == Role::Tool).collect();
    if let Some(step) = steps.get(results.len()) {
        return MockScript::Tool { name: step.tool.to_owned(), arguments: step.args.clone() };
    }
    let failed: Vec<(usize, String)> = results
        .iter()
        .enumerate()
        .filter(|(_, m)| result_failed(m))
        .map(|(i, m)| (i, short(&m.content, 160)))
        .collect();
    let files: Vec<String> = steps
        .iter()
        .filter(|s| s.tool == "write_file")
        .filter_map(|s| s.args.get("path").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let tests: Vec<Value> = steps
        .iter()
        .enumerate()
        .filter(|(_, s)| s.tool == "run_command")
        .map(|(i, s)| json!({ "command": s.args["command"], "passed": !failed.iter().any(|(j, _)| *j == i) }))
        .collect();
    let status = if failed.is_empty() { "done" } else { "partial" };
    let lang = reply_lang(req);
    let summary = if files.is_empty() {
        lang.pick(
            format!("{title}: reviewed the workspace ({} step(s)). Mock (offline demo).", steps.len()),
            format!("{title}: рабочая копия проверена (шагов: {}). Mock (офлайн-демо).", steps.len()),
        )
    } else {
        lang.pick(
            format!("{title}: wrote {}. Mock (offline demo).", files.join(", ")),
            format!("{title}: записаны файлы {}. Mock (офлайн-демо).", files.join(", ")),
        )
    };
    MockScript::Tool {
        name: "submit_result".into(),
        arguments: json!({
            "status": status,
            "summary": summary,
            "files_changed": files,
            "tests_run": tests,
            "open_issues": failed.iter().map(|(_, m)| format!("step failed: {m}")).collect::<Vec<_>>(),
        }),
    }
}

const BUILD_WORDS: [&str; 18] = [
    "create",
    "build",
    "implement",
    "add ",
    "make",
    "write",
    "fix",
    "refactor",
    "develop",
    "generate",
    "set up",
    "setup",
    "plan",
    "создай",
    "сделай",
    "напиши",
    "добавь",
    "реализуй",
];
const STATUS_WORDS: [&str; 9] =
    ["status", "progress", "how are", "what's going", "overview", "tasks", "agents", "статус", "прогресс"];

/// Scripted AI Manager chat: build requests → `propose_plan`; status
/// questions → `project_overview`; tool results → a short summary.
fn manager_script(req: &ChatRequest) -> MockScript {
    let last = req.messages.iter().rev().find(|m| m.role != Role::System);
    if let Some(m) = last.filter(|m| m.role == Role::Tool) {
        let tool = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant && !m.tool_calls.is_empty())
            .and_then(|m| m.tool_calls.first())
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let result = short(&m.content, 300);
        let lang = reply_lang(req);
        return MockScript::Text(if m.is_error {
            lang.pick(
                format!("I could not complete that: {result}\n\n_— Mock (offline demo)_"),
                format!("Не получилось это выполнить: {result}\n\n_— Mock (офлайн-демо)_"),
            )
        } else if tool == "propose_plan" {
            lang.pick(
                format!("I studied the project and drafted a plan: {result}\n\nReview the **plan card** below — approve it to start the agents (each works in its own worktree), edit task titles or remove tasks first, or reject it.\n\n_— Mock (offline demo)_"),
                format!("Я изучил проект и подготовил план: {result}\n\nПосмотрите **карточку плана** ниже: подтвердите её, чтобы запустить агентов (каждый работает в своей рабочей копии), при необходимости сначала измените названия задач или уберите лишние — или отклоните план.\n\n_— Mock (офлайн-демо)_"),
            )
        } else {
            lang.pick(
                format!("Here is the current state (`{tool}`):\n\n{result}\n\n_— Mock (offline demo)_"),
                format!("Текущее состояние (`{tool}`):\n\n{result}\n\n_— Mock (офлайн-демо)_"),
            )
        });
    }
    let user = last_user(req);
    let request = user.split("\n\n[Attached").next().unwrap_or_default().trim().to_owned();
    let lower = request.to_lowercase();
    if has_tool(req, "propose_plan") && BUILD_WORDS.iter().any(|w| lower.contains(w)) {
        return MockScript::Tool { name: "propose_plan".into(), arguments: json!({ "goal": request }) };
    }
    if has_tool(req, "project_overview") && STATUS_WORDS.iter().any(|w| lower.contains(w)) {
        return MockScript::Tool { name: "project_overview".into(), arguments: json!({}) };
    }
    let quoted = short(&request, 200);
    MockScript::Text(reply_lang(req).pick(
        format!(
            "Hello from the AI Manager — **Mock (offline demo)**. You said: \"{quoted}\".\n\nAsk me to build something (for example *\"Create a hello module with tests\"*) and I will plan it, create tasks and run agents. Connect a real provider in Settings → AI Providers for real answers."
        ),
        format!(
            "Здравствуйте! Это AI-менеджер — **Mock (офлайн-демо)**. Вы написали: «{quoted}».\n\nПопросите меня что-нибудь сделать (например, *«Создай модуль hello с тестами»*) — я составлю план, создам задачи и запущу агентов. Чтобы получать настоящие ответы, подключите реального провайдера в разделе «Настройки → AI-провайдеры»."
        ),
    ))
}

/// One visible sentence describing a scripted agent tool call.
fn narrate(name: &str, args: &Value, lang: MockLang) -> String {
    let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    let ru = lang == MockLang::Ru;
    let what = match (name, ru) {
        ("write_file", false) => format!("Writing `{}`", arg("path")),
        ("write_file", true) => format!("Записываю `{}`", arg("path")),
        ("run_command", false) => format!("Running `{}`", arg("command")),
        ("run_command", true) => format!("Выполняю `{}`", arg("command")),
        ("git_commit", false) => "Committing my changes on my branch".to_owned(),
        ("git_commit", true) => "Делаю коммит изменений в своей ветке".to_owned(),
        ("list_dir", false) => "Looking at the files".to_owned(),
        ("list_dir", true) => "Просматриваю файлы".to_owned(),
        ("git_diff", false) => "Reviewing the diff".to_owned(),
        ("git_diff", true) => "Проверяю изменения".to_owned(),
        ("submit_result", false) => format!("Submitting the result: {}", short(&arg("summary"), 160)),
        ("submit_result", true) => format!("Сдаю результат: {}", short(&arg("summary"), 160)),
        (other, false) => format!("Calling `{other}`"),
        (other, true) => format!("Вызываю `{other}`"),
    };
    if ru {
        format!("{what} — Mock (офлайн-демо).")
    } else {
        format!("{what} — Mock (offline demo).")
    }
}

fn is_plan_request(req: &ChatRequest) -> bool {
    if let ResponseFormat::JsonSchema { name, .. } = &req.response_format {
        return name == "orkestr_plan";
    }
    let system = req.system_prompt().unwrap_or_default().to_ascii_lowercase();
    let planning = system.contains("plan") || system.contains("decompose");
    planning && (req.response_format != ResponseFormat::Text || system.contains("json") || req.tools.is_empty())
}

/// The goal of a planning request: the `Goal:` line when present.
fn plan_goal(req: &ChatRequest) -> String {
    let user = last_user(req);
    user.lines()
        .find_map(|l| l.strip_prefix("Goal:").map(|g| g.trim().to_owned()))
        .filter(|g| !g.is_empty())
        .unwrap_or(user)
}

fn tool_args(tool: &ToolSpec, hint: &str) -> Value {
    sample(&tool.parameters, "", hint, 0, 0)
}

/// Decides the scripted response (pure; unit-tested).
#[must_use]
pub fn script(req: &ChatRequest) -> MockScript {
    let model = req.model.as_str();
    match model {
        "mock-rate-limit" => return MockScript::Fail(ProviderError::RateLimited { retry_after_secs: Some(1) }),
        "mock-quota" => {
            return MockScript::Fail(ProviderError::QuotaExceeded("mock quota exhausted (offline demo)".into()))
        }
        "mock-server-error" => {
            return MockScript::Fail(ProviderError::Server("mock upstream failure (offline demo)".into()))
        }
        _ => {}
    }
    // Phase 2 extension point: handoff summaries (context optimizer).
    if let Some(s) = super::mock_summarize::script(req) {
        return s;
    }
    // Phase 2 extension point: teams / AI Product Manager scripts (marker-keyed).
    if let Some(s) = super::mock_teams_pm::script(req) {
        return s;
    }
    let user = last_user(req);
    let last = req.messages.iter().rev().find(|m| m.role != Role::System);
    let after_tool = last.is_some_and(|m| m.role == Role::Tool);
    if model == "mock-echo" {
        return MockScript::Text(if user.is_empty() { "(empty)".into() } else { user });
    }
    if model == "mock-tool-roundtrip" {
        if let (false, Some(tool)) = (after_tool, req.tools.first()) {
            return MockScript::Tool { name: tool.name.clone(), arguments: tool_args(tool, &user) };
        }
        return MockScript::Text("Tool round-trip complete.".into());
    }
    // mock-smart / mock-slow / anything else.
    // Phase 2 review / adversarial / consensus prompts (role markers).
    if let Some(s) = super::mock_review::script(req) {
        return s;
    }
    // Extension scripts keyed by a system-prompt marker (Phase 2 areas).
    if let Some(s) = super::mock_merge::script(req) {
        return s;
    }
    if is_plan_request(req) {
        return MockScript::Text(demo_plan_in(&plan_goal(req), reply_lang(req)).to_string());
    }
    if has_tool(req, "propose_plan") {
        return manager_script(req);
    }
    if has_tool(req, "submit_result") {
        return agent_script(req);
    }
    if after_tool {
        let result = last.map(|m| short(&m.content, 120)).unwrap_or_default();
        let failed = last.is_some_and(|m| m.is_error);
        let lang = reply_lang(req);
        return MockScript::Text(if failed {
            lang.pick(
                format!("The tool reported an error ({result}). Stopping here — Mock (offline demo)."),
                format!("Инструмент сообщил об ошибке ({result}). Останавливаюсь — Mock (офлайн-демо)."),
            )
        } else {
            lang.pick(
                format!("Done. The tool returned: {result}\n\nTask complete — Mock (offline demo)."),
                format!("Готово. Инструмент вернул: {result}\n\nЗадача выполнена — Mock (офлайн-демо)."),
            )
        });
    }
    if let ResponseFormat::JsonSchema { schema, .. } = &req.response_format {
        return MockScript::Text(sample(schema, "", &user, 0, 0).to_string());
    }
    if let Some(tool) = req.tools.iter().find(|t| t.name == "write_file") {
        return MockScript::Tool { name: tool.name.clone(), arguments: tool_args(tool, &user) };
    }
    if req.response_format == ResponseFormat::JsonObject {
        return MockScript::Text(
            json!({"answer": format!("Mock (offline demo) answer to: {}", short(&user, 120))}).to_string(),
        );
    }
    let quoted = short(&user, 200);
    MockScript::Text(reply_lang(req).pick(
        format!(
            "Hello from Mock (offline demo). You said: \"{quoted}\". I'm a scripted offline provider — connect a real provider in Settings → AI Providers for real answers."
        ),
        format!(
            "Здравствуйте! Это Mock (офлайн-демо). Вы написали: «{quoted}». Я офлайн-провайдер с заранее заданными ответами — чтобы получать настоящие ответы, подключите реального провайдера в разделе «Настройки → AI-провайдеры»."
        ),
    ))
}

/// Upper bound on streamed chunks per reply.
const MAX_CHUNKS: usize = 48;

/// Chunk size: 12 characters, larger for long replies so that at most
/// [`MAX_CHUNKS`] chunks are emitted.
fn chunk_size(chars: usize) -> usize {
    12.max(chars.div_ceil(MAX_CHUNKS))
}

fn chunks(text: &str, size: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars.chunks(size.max(1)).map(|c| c.iter().collect()).collect()
}

#[async_trait]
impl ProviderAdapter for MockAdapter {
    fn id(&self) -> ProviderId {
        self.id
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            kind: ProviderKind::Mock,
            display_name: self.name.clone(),
            is_local: true,
            locality: Locality::Local,
            capabilities: ModelCapabilities {
                tools: true,
                vision: false,
                structured_output: true,
                streaming: true,
                reasoning: false,
                coding_agent: false,
            },
            max_context_tokens: 128_000,
            coding_agent: false,
            cost_model: CostModelKind::Local,
            max_concurrency: 8,
        }
    }

    async fn auth_state(&self) -> AuthStatus {
        AuthStatus::Ready { label: "no credentials needed (offline demo)".into() }
    }

    async fn list_models(&self) -> Result<Vec<Model>, ProviderError> {
        Ok(default_models(ProviderKind::Mock, self.id, true))
    }

    async fn health(&self) -> HealthReport {
        HealthReport {
            health: ProviderHealth::Healthy,
            latency_ms: Some(0),
            detail: Some("offline demo provider".into()),
            version: None,
        }
    }

    async fn chat(&self, request: ChatRequest, cancel: CancellationToken) -> Result<ChatStream, ProviderError> {
        let plan = script(&request);
        // Agents narrate their tool calls like real models do.
        let narration = match &plan {
            MockScript::Tool { name, arguments } if has_tool(&request, "submit_result") => {
                Some(narrate(name, arguments, reply_lang(&request)))
            }
            _ => None,
        };
        let delay = if request.model == "mock-slow" {
            Duration::from_millis(250).max(self.options.chunk_delay)
        } else {
            self.options.chunk_delay
        };
        let prompt_chars = request.prompt_chars();
        Ok(drive(cancel, move |em| async move {
            let output_chars;
            let stop = match plan {
                MockScript::Fail(e) => return Err(e),
                MockScript::Text(text) => {
                    output_chars = text.len();
                    // Chat-sized chunks, but never more than MAX_CHUNKS of
                    // them: a long document (a plan, a JSON reply) streams
                    // in about a second instead of tens of seconds.
                    for c in chunks(&text, chunk_size(text.chars().count())) {
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        em.emit(ChatEvent::TextDelta(c)).await?;
                    }
                    StopReason::EndTurn
                }
                MockScript::Tool { name, arguments } => {
                    output_chars = arguments.to_string().len();
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    if let Some(n) = narration {
                        em.emit(ChatEvent::TextDelta(n)).await?;
                    }
                    em.emit(ChatEvent::ToolCall { id: format!("mock_call_{}", slug(&name)), name, arguments }).await?;
                    StopReason::ToolUse
                }
            };
            em.emit(ChatEvent::Usage(Usage::estimate(prompt_chars, output_chars))).await?;
            em.emit(ChatEvent::Done { stop_reason: stop }).await
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[test]
    fn long_replies_stream_in_a_bounded_number_of_chunks() {
        assert_eq!(chunk_size(10), 12);
        assert_eq!(chunk_size(12 * MAX_CHUNKS), 12);
        let long = "x".repeat(5000);
        assert!(chunks(&long, chunk_size(5000)).len() <= MAX_CHUNKS);
        assert_eq!(chunks(&long, chunk_size(5000)).concat(), long);
    }

    #[test]
    fn subject_and_identifier() {
        assert_eq!(subject_of("Create a hello module with tests"), "hello");
        assert_eq!(subject_of("Build the payroll service"), "payroll");
        assert_eq!(subject_of("Add caching"), "caching");
        assert_eq!(subject_of("!!!"), "demo");
        assert_eq!(ident_of("hello-world"), "helloWorld");
        assert_eq!(ident_of("2fa"), "m2fa");
    }

    #[test]
    fn demo_plan_follows_the_reply_language() {
        let ru = demo_plan_in("Создай модуль hello с тестами", MockLang::Ru);
        let en = demo_plan_in("Create a hello module with tests", MockLang::En);
        assert_eq!(ru["tasks"][1]["title"], "Реализовать модуль hello");
        assert_eq!(en["tasks"][1]["title"], "Implement the hello module");
        // Structure is language-independent.
        for i in 0..4 {
            assert_eq!(ru["tasks"][i]["role"], en["tasks"][i]["role"]);
            assert_eq!(ru["tasks"][i]["dependsOn"], en["tasks"][i]["dependsOn"]);
            assert_eq!(ru["tasks"][i]["files"], en["tasks"][i]["files"]);
        }
    }

    #[test]
    fn demo_plan_has_dependencies_and_a_parallel_branch() {
        let v = demo_plan("Create a hello module with tests");
        let tasks = v["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 4);
        assert_eq!(tasks[0]["dependsOn"], json!([]));
        assert_eq!(tasks[1]["dependsOn"], json!(["t1"]));
        assert_eq!(tasks[2]["dependsOn"], json!(["t1"]));
        assert_eq!(tasks[3]["dependsOn"], json!(["t2"]));
        assert_eq!(tasks[1]["files"], json!(["src/hello.mjs"]));
        assert_eq!(tasks[3]["role"], "QA");
        let mut r = ChatRequest::new(
            "mock-smart",
            vec![ChatMessage::user(
                "Goal: Create a hello module

Context…",
            )],
        );
        r.response_format =
            ResponseFormat::JsonSchema { name: "orkestr_plan".into(), schema: json!({"type":"object"}) };
        let MockScript::Text(t) = script(&r) else { panic!() };
        assert_eq!(serde_json::from_str::<Value>(&t).unwrap()["tasks"][1]["title"], "Implement the hello module");
    }

    fn agent_req(role: &str, title: &str, tools: &[&str]) -> ChatRequest {
        let mut r = ChatRequest::new(
            "mock-smart",
            vec![ChatMessage::user(format!(
                "# Task: {title}

Do it."
            ))],
        );
        r.system = Some(format!("You are agent \"A\" (role {role}) running inside Orkestr."));
        r.tools = tools.iter().map(|t| tool(t, json!({"type": "object"}))).collect();
        r
    }

    #[test]
    fn qa_agent_writes_tests_commits_runs_them_and_submits() {
        let tools = ["write_file", "git_commit", "run_command", "submit_result"];
        let mut r = agent_req("QA", "Test the hello module", &tools);
        let MockScript::Tool { name, arguments } = script(&r) else { panic!() };
        assert_eq!(name, "write_file");
        assert_eq!(arguments["path"], "tests/hello.test.mjs");
        assert!(arguments["content"].as_str().unwrap().contains("from \"../src/hello.mjs\""));
        r.messages.push(ChatMessage::tool_result("1", "wrote", false));
        let MockScript::Tool { name, .. } = script(&r) else { panic!() };
        assert_eq!(name, "git_commit");
        r.messages.push(ChatMessage::tool_result("2", "committed", false));
        let MockScript::Tool { name, arguments } = script(&r) else { panic!() };
        assert_eq!(name, "run_command");
        assert_eq!(arguments["command"], "node --test tests/hello.test.mjs");
        r.messages.push(ChatMessage::tool_result(
            "3",
            "exit code: 1
failed",
            false,
        ));
        let MockScript::Tool { name, arguments } = script(&r) else { panic!() };
        assert_eq!(name, "submit_result");
        assert_eq!(arguments["status"], "partial");
        assert_eq!(arguments["tests_run"][0]["passed"], false);
    }

    #[test]
    fn task_requested_commands_run_after_the_role_steps() {
        let tools = ["write_file", "run_command", "submit_result"];
        let mut r = ChatRequest::new(
            "mock-smart",
            vec![ChatMessage::user(
                "# Task: Clean the hello module\n\nTidy up.\nRun: `git clean -fd`\nrun: `echo ok`\nRun: `echo ok`\nRun: not quoted\n\n# Context (selected by Orkestr)\nRun: `git clean -fd`\nRun: `rm -rf ctx`\n",
            )],
        );
        r.system = Some("You are agent \"A\" (role Backend) running inside Orkestr.".into());
        r.tools = tools.iter().map(|t| tool(t, json!({"type": "object"}))).collect();
        assert_eq!(requested_commands(&r), vec!["git clean -fd".to_owned(), "echo ok".to_owned()]);
        let MockScript::Tool { name, .. } = script(&r) else { panic!() };
        assert_eq!(name, "write_file");
        r.messages.push(ChatMessage::tool_result("1", "wrote", false));
        let MockScript::Tool { name, arguments } = script(&r) else { panic!() };
        assert_eq!(name, "run_command");
        assert_eq!(arguments["command"], "git clean -fd");
        r.messages.push(ChatMessage::tool_result("2", "denied by the user", true));
        r.messages.push(ChatMessage::tool_result("3", "exit code: 0", false));
        let MockScript::Tool { name, arguments } = script(&r) else { panic!() };
        assert_eq!(name, "submit_result");
        assert_eq!(arguments["status"], "partial");
        // No request in the task → no extra commands.
        assert!(requested_commands(&agent_req("QA", "Test the hello module", &tools)).is_empty());
    }

    #[test]
    fn healing_scenarios_fail_first_and_fix_on_the_fix_attempt() {
        let tools = ["write_file", "git_commit", "submit_result"];
        let first = agent_req("Backend", "Implement the greeter module [mock:heal-once]", &tools);
        let MockScript::Tool { arguments, .. } = script(&first) else { panic!() };
        assert_eq!(arguments["path"], "src/greeter.mjs");
        assert!(arguments["content"].as_str().unwrap().contains(HEAL_BUG_COMMENT));
        let steps = agent_steps("backend", "Implement the greeter module [mock:heal-once]", &first);
        assert_eq!(steps.len(), 4, "implementation, tests, package.json, commit");
        let mut fix = first.clone();
        fix.messages[0].content.push_str(
            "
# Self-healing fix (attempt 1 of 3)
Fix the failing test.
",
        );
        assert_eq!(healing_attempt(&fix), Some(1));
        let MockScript::Tool { arguments, .. } = script(&fix) else { panic!() };
        let body = arguments["content"].as_str().unwrap();
        assert!(!body.contains(HEAL_BUG_COMMENT) && body.contains("!`"), "{body}");
        assert_eq!(agent_steps("backend", "Implement the greeter module [mock:heal-once]", &fix).len(), 2);
        let mut never = agent_req("Backend", "Implement the greeter module [mock:heal-never]", &tools);
        never.messages[0].content.push_str(
            "
# Self-healing fix (attempt 2 of 3)
",
        );
        let MockScript::Tool { arguments, .. } = script(&never) else { panic!() };
        let body = arguments["content"].as_str().unwrap();
        assert!(body.contains(HEAL_BUG_COMMENT) && body.contains("attempt 2"), "{body}");
    }

    #[test]
    fn backend_and_reviewer_agents() {
        let mut r = agent_req("Backend", "Implement the hello module", &["write_file", "submit_result"]);
        let MockScript::Tool { arguments, .. } = script(&r) else { panic!() };
        assert_eq!(arguments["path"], "src/hello.mjs");
        assert!(arguments["content"].as_str().unwrap().contains("export function hello("));
        r.messages.push(ChatMessage::tool_result("1", "wrote", false));
        let MockScript::Tool { name, arguments } = script(&r) else { panic!() };
        assert_eq!(name, "submit_result");
        assert_eq!(arguments["status"], "done");
        assert_eq!(arguments["files_changed"], json!(["src/hello.mjs"]));
        let r = agent_req("Reviewer", "Review the hello module", &["list_dir", "git_diff", "submit_result"]);
        let MockScript::Tool { name, .. } = script(&r) else { panic!() };
        assert_eq!(name, "list_dir");
    }

    #[test]
    fn manager_chat_plans_build_requests_and_reads_state() {
        let tools = ["project_overview", "propose_plan", "answer_user"];
        let mut r = ChatRequest::new("mock-smart", vec![ChatMessage::user("Create a hello module with tests")]);
        r.system = Some("You are the Orkestr AI Manager.".into());
        r.tools = tools.iter().map(|t| tool(t, json!({"type": "object"}))).collect();
        let MockScript::Tool { name, arguments } = script(&r) else { panic!() };
        assert_eq!(name, "propose_plan");
        assert_eq!(arguments["goal"], "Create a hello module with tests");
        r.messages.push(ChatMessage::assistant_tool_calls(
            "",
            vec![crate::types::ToolCallPart { id: "c".into(), name: "propose_plan".into(), arguments }],
        ));
        r.messages.push(ChatMessage::tool_result("c", "plan with 4 tasks", false));
        assert!(matches!(script(&r), MockScript::Text(t) if t.contains("plan card")));
        let mut s = ChatRequest::new("mock-smart", vec![ChatMessage::user("What's the status?")]);
        s.tools = r.tools.clone();
        assert!(matches!(script(&s), MockScript::Tool { name, .. } if name == "project_overview"));
        let mut h = ChatRequest::new("mock-smart", vec![ChatMessage::user("hi")]);
        h.tools = r.tools.clone();
        assert!(matches!(script(&h), MockScript::Text(t) if t.contains("Mock (offline demo)")));
    }

    #[test]
    fn replies_in_russian_for_cyrillic_input_or_russian_ui() {
        let tools: Vec<ToolSpec> =
            ["project_overview", "propose_plan"].iter().map(|t| tool(t, json!({"type": "object"}))).collect();
        // Cyrillic input → Russian reply, plan requests still recognised.
        let mut ru = ChatRequest::new("mock-smart", vec![ChatMessage::user("привет")]);
        ru.tools = tools.clone();
        assert_eq!(reply_lang(&ru), MockLang::Ru);
        assert!(matches!(script(&ru), MockScript::Text(t) if t.contains("Здравствуйте") && t.contains("офлайн-демо")));
        let mut build = ChatRequest::new("mock-smart", vec![ChatMessage::user("Создай модуль hello с тестами")]);
        build.tools = tools.clone();
        assert!(matches!(script(&build), MockScript::Tool { name, .. } if name == "propose_plan"));
        // Russian UI (system-prompt marker) → Russian even for Latin input.
        let mut ui = ChatRequest::new("mock-smart", vec![ChatMessage::user("hi")]);
        ui.tools = tools.clone();
        ui.system = Some(format!("You are the AI Manager.\n{} ru (Russian).", crate::UI_LANGUAGE_MARKER));
        assert_eq!(reply_lang(&ui), MockLang::Ru);
        assert!(matches!(script(&ui), MockScript::Text(t) if t.contains("Вы написали: «hi»")));
        // English UI and Latin input → English.
        let mut en = ChatRequest::new("mock-smart", vec![ChatMessage::user("hi")]);
        en.tools = tools;
        en.system = Some(format!("{} en (English).", crate::UI_LANGUAGE_MARKER));
        assert_eq!(reply_lang(&en), MockLang::En);
        assert!(matches!(script(&en), MockScript::Text(t) if t.contains("You said")));
        // Plain chat without tools follows the same rule.
        let plain = ChatRequest::new("mock-smart", vec![ChatMessage::user("как дела?")]);
        assert!(matches!(script(&plain), MockScript::Text(t) if t.starts_with("Здравствуйте")));
        assert_eq!(narrate("list_dir", &json!({}), MockLang::Ru), "Просматриваю файлы — Mock (офлайн-демо).");
    }

    fn tool(name: &str, schema: Value) -> ToolSpec {
        ToolSpec { name: name.into(), description: "d".into(), parameters: schema }
    }

    #[test]
    fn planning_prompt_returns_plan_json() {
        let mut r = ChatRequest::new("mock-smart", vec![ChatMessage::user("Build a todo app")]);
        r.system = Some("You are the AI Manager. Produce a plan as JSON.".into());
        let MockScript::Text(t) = script(&r) else { panic!() };
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["tasks"].as_array().unwrap().len(), 4);
        assert_eq!(v["tasks"][1]["dependsOn"][0], "t1");
    }

    #[test]
    fn schema_output_is_valid_shape() {
        let schema = json!({"type":"object","required":["summary","tasks"],"properties":{
            "summary":{"type":"string"},
            "tasks":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"title":{"type":"string"},"role":{"type":"string","enum":["Backend","QA"]},"dependsOn":{"type":"array","items":{"type":"string"}},"estimate":{"type":"integer","minimum":2}}}}}});
        let mut r = ChatRequest::new("mock-smart", vec![ChatMessage::user("x")]);
        r.response_format = ResponseFormat::JsonSchema { name: "plan".into(), schema };
        let MockScript::Text(t) = script(&r) else { panic!() };
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["tasks"].as_array().unwrap().len(), 3);
        assert_eq!(v["tasks"][0]["role"], "Backend");
        assert_eq!(v["tasks"][1]["role"], "QA");
        assert_eq!(v["tasks"][2]["id"], "t3");
        assert_eq!(v["tasks"][2]["dependsOn"], json!(["t2"]));
        assert_eq!(v["tasks"][0]["estimate"], 2);
    }

    #[test]
    fn agent_task_writes_file_then_finishes() {
        let wf = tool(
            "write_file",
            json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        );
        let mut r = ChatRequest::new(
            "mock-smart",
            vec![ChatMessage::system("You are a Backend agent."), ChatMessage::user("Add a README")],
        );
        r.tools = vec![tool("read_file", json!({"type":"object"})), wf];
        let MockScript::Tool { name, arguments } = script(&r) else { panic!("expected tool call") };
        assert_eq!(name, "write_file");
        assert_eq!(arguments["path"], "orkestr-demo/add-a-readme.md");
        assert!(arguments["content"].as_str().unwrap().contains("Add a README"));
        r.messages.push(ChatMessage::assistant_tool_calls("", vec![]));
        r.messages.push(ChatMessage::tool_result("mock_call_write-file", "wrote 40 bytes", false));
        assert!(matches!(script(&r), MockScript::Text(t) if t.contains("Task complete")));
    }

    #[test]
    fn deterministic() {
        let r = ChatRequest::new("mock-smart", vec![ChatMessage::user("hello")]);
        assert_eq!(script(&r), script(&r));
    }

    #[tokio::test]
    async fn stream_shape_and_errors() {
        let m = MockAdapter::new(ProviderId::new(), "Mock (offline demo)", MockOptions { chunk_delay: Duration::ZERO });
        let s = m
            .chat(
                ChatRequest::new("mock-echo", vec![ChatMessage::user("ping pong ping pong")]),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let items: Vec<_> = s.collect().await;
        let text: String = items
            .iter()
            .filter_map(|e| if let ChatEvent::TextDelta(t) = e { Some(t.as_str()) } else { None })
            .collect();
        assert_eq!(text, "ping pong ping pong");
        assert!(matches!(items[items.len() - 2], ChatEvent::Usage(Usage { estimated: true, .. })));
        assert_eq!(items.last(), Some(&ChatEvent::Done { stop_reason: StopReason::EndTurn }));

        let s = m.chat(ChatRequest::new("mock-rate-limit", vec![]), CancellationToken::new()).await.unwrap();
        let items: Vec<_> = s.collect().await;
        assert_eq!(items, vec![ChatEvent::Error(ProviderError::RateLimited { retry_after_secs: Some(1) })]);
        assert!(m.descriptor().display_name.contains("Mock (offline demo)"));
        let named = MockAdapter::new(ProviderId::new(), "Demo", MockOptions::default());
        assert!(named.descriptor().display_name.contains("Mock (offline demo)"));
    }
}
