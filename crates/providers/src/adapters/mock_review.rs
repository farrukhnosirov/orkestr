//! Scripted Mock behaviours for code review, adversarial review (+ its
//! verification pass) and Consensus Mode (Phase 2, review-consensus).
//!
//! The orchestrator's prompts carry one of the role markers below in the
//! system prompt; the Mock then answers with a deterministic, schema-shaped
//! JSON document derived from the request (the diff, the acceptance criteria,
//! the findings to verify, the proposals to synthesize). This keeps the
//! review / consensus flows demoable offline and testable.
//!
//! Review heuristics over the *added* lines of the diff:
//! * `eval(`, hard-coded `password = "…"` / `api_key = "…"` / private keys →
//!   critical `security` finding;
//! * `.unwrap()` → medium `correctness`; `TODO` / `FIXME` → low `maintainability`;
//!   `console.log(` → info `style`;
//! * empty diff → high `completeness` ("no changes");
//! * `MOCK_REJECT` anywhere in the diff → verdict `Reject`.
//!
//! The adversary always finds at least one edge case (it must not
//! auto-agree); the author concedes critical/high findings with concrete
//! evidence and rebuts the rest.

use serde_json::{json, Value};

use super::mock::{reply_lang, MockLang, MockScript};
use crate::types::{ChatRequest, Role};

/// System-prompt marker of the automatic code reviewer.
pub const REVIEW_MARKER: &str = "[orkestr:code-review]";
/// System-prompt marker of the adversarial reviewer.
pub const ADVERSARIAL_MARKER: &str = "[orkestr:adversarial-review]";
/// System-prompt marker of the adversarial verification pass (author answers).
pub const VERIFY_MARKER: &str = "[orkestr:review-verification]";
/// System-prompt marker of a consensus participant.
pub const PROPOSAL_MARKER: &str = "[orkestr:consensus-proposal]";
/// System-prompt marker of the consensus synthesizer.
pub const SYNTHESIS_MARKER: &str = "[orkestr:consensus-synthesis]";

/// The scripted answer when the request carries one of the markers.
#[must_use]
pub fn script(req: &ChatRequest) -> Option<MockScript> {
    let system = req.system_prompt().unwrap_or_default();
    let user = last_user(req);
    let lang = reply_lang(req);
    let doc = if system.contains(VERIFY_MARKER) {
        verification(&user, lang)
    } else if system.contains(ADVERSARIAL_MARKER) {
        adversarial(&user, lang)
    } else if system.contains(REVIEW_MARKER) {
        review(&user, lang)
    } else if system.contains(SYNTHESIS_MARKER) {
        synthesis(&user, lang)
    } else if system.contains(PROPOSAL_MARKER) {
        proposal(&system, &user, lang)
    } else {
        return None;
    };
    Some(MockScript::Text(doc.to_string()))
}

fn last_user(req: &ChatRequest) -> String {
    req.messages.iter().rev().find(|m| m.role == Role::User).map(|m| m.content.clone()).unwrap_or_default()
}

fn pick(lang: MockLang, en: &str, ru: &str) -> String {
    match lang {
        MockLang::En => en.to_owned(),
        MockLang::Ru => ru.to_owned(),
    }
}

fn short(s: &str, n: usize) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > n {
        format!("{}…", t.chars().take(n).collect::<String>())
    } else {
        t
    }
}

/// One added line of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedLine {
    /// File (new path).
    pub file: String,
    /// Line number in the new file.
    pub line: u32,
    /// Text without the leading `+`.
    pub text: String,
}

/// The fenced diff of a review prompt (between "```diff" and the closing fence).
fn diff_block(user: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for l in user.lines() {
        if !inside && l.trim_start().starts_with("```diff") {
            inside = true;
            continue;
        }
        if inside && l.trim() == "```" {
            break;
        }
        if inside {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

/// Added lines of a unified diff with their new-file line numbers.
#[must_use]
pub fn added_lines(diff: &str) -> Vec<AddedLine> {
    let mut out = Vec::new();
    let mut file = String::new();
    let mut line: u32 = 0;
    for l in diff.lines() {
        if let Some(p) = l.strip_prefix("+++ ") {
            file = p.trim().trim_start_matches("b/").to_owned();
            continue;
        }
        if l.starts_with("--- ") || l.starts_with("diff --git") || l.starts_with("index ") {
            continue;
        }
        if let Some(rest) = l.strip_prefix("@@") {
            // @@ -a,b +c,d @@
            line = rest
                .split_whitespace()
                .find_map(|t| t.strip_prefix('+'))
                .and_then(|t| t.split(',').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            continue;
        }
        if let Some(text) = l.strip_prefix('+') {
            out.push(AddedLine { file: file.clone(), line, text: text.to_owned() });
            line += 1;
        } else if l.starts_with(' ') {
            line += 1;
        }
    }
    out
}

fn changed_files(diff: &str) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for l in diff.lines() {
        if let Some(p) = l.strip_prefix("+++ ") {
            let f = p.trim().trim_start_matches("b/").to_owned();
            if f != "/dev/null" && !files.contains(&f) {
                files.push(f);
            }
        }
    }
    files
}

/// Acceptance criteria listed under "## Acceptance criteria" (`1. text`).
fn criteria(user: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for l in user.lines() {
        if l.starts_with("## ") {
            inside = l.to_ascii_lowercase().contains("acceptance criteria");
            continue;
        }
        if inside {
            let t = l.trim();
            if let Some((num, rest)) = t.split_once(". ") {
                if num.chars().all(|c| c.is_ascii_digit()) && !rest.trim().is_empty() {
                    out.push(rest.trim().to_owned());
                }
            }
        }
    }
    out
}

struct Hit {
    severity: &'static str,
    category: &'static str,
    title: String,
    detail: String,
    suggestion: String,
    at: AddedLine,
}

fn security_hit(a: &AddedLine, lang: MockLang) -> Option<Hit> {
    let lower = a.text.to_ascii_lowercase();
    let secret = ["password", "api_key", "apikey", "secret", "token"]
        .iter()
        .any(|k| lower.contains(k) && (lower.contains("= \"") || lower.contains("=\"") || lower.contains(": \"")));
    if lower.contains("eval(") {
        return Some(Hit {
            severity: "critical",
            category: "security",
            title: pick(lang, "Dynamic code execution with eval()", "Динамическое выполнение кода через eval()"),
            detail: pick(
                lang,
                "eval() executes arbitrary strings; any attacker-controlled input becomes code execution.",
                "eval() выполняет произвольные строки: любые данные злоумышленника превращаются в выполнение кода.",
            ),
            suggestion: pick(
                lang,
                "Parse the input explicitly instead of evaluating it.",
                "Разбирайте входные данные явно вместо eval().",
            ),
            at: a.clone(),
        });
    }
    if secret || lower.contains("private key-----") {
        return Some(Hit {
            severity: "critical",
            category: "security",
            title: pick(lang, "Hard-coded secret", "Секрет записан прямо в коде"),
            detail: pick(
                lang,
                "A credential is committed in plain text and will leak through git history.",
                "Учётные данные записаны открытым текстом и утекут через историю git.",
            ),
            suggestion: pick(
                lang,
                "Load it from the secret store or an environment variable.",
                "Загружайте его из хранилища секретов или переменной окружения.",
            ),
            at: a.clone(),
        });
    }
    None
}

fn review_hits(lines: &[AddedLine], lang: MockLang) -> Vec<Hit> {
    let mut hits = Vec::new();
    for a in lines {
        if let Some(h) = security_hit(a, lang) {
            hits.push(h);
            continue;
        }
        let t = &a.text;
        if t.contains(".unwrap()") {
            hits.push(Hit {
                severity: "medium",
                category: "correctness",
                title: pick(lang, "unwrap() can panic", "unwrap() может вызвать панику"),
                detail: pick(lang, "The error case is not handled.", "Случай ошибки не обработан."),
                suggestion: pick(
                    lang,
                    "Propagate the error with `?` or handle it.",
                    "Передайте ошибку через `?` или обработайте её.",
                ),
                at: a.clone(),
            });
        } else if t.contains("TODO") || t.contains("FIXME") {
            hits.push(Hit {
                severity: "low",
                category: "maintainability",
                title: pick(lang, "Unresolved TODO", "Незакрытый TODO"),
                detail: pick(lang, "The change leaves work unfinished.", "Изменение оставляет незавершённую работу."),
                suggestion: pick(lang, "Finish it or track it as a task.", "Завершите её или заведите задачу."),
                at: a.clone(),
            });
        } else if t.contains("console.log(") {
            hits.push(Hit {
                severity: "info",
                category: "style",
                title: pick(lang, "Debug logging left in", "Оставлен отладочный вывод"),
                detail: pick(lang, "console.log output in production code.", "Вывод console.log в рабочем коде."),
                suggestion: pick(
                    lang,
                    "Remove it or use the project logger.",
                    "Удалите его или используйте логгер проекта.",
                ),
                at: a.clone(),
            });
        }
        if hits.len() >= 12 {
            break;
        }
    }
    hits
}

fn finding_json(h: &Hit) -> Value {
    json!({
        "severity": h.severity,
        "category": h.category,
        "file": h.at.file,
        "line": h.at.line,
        "title": h.title,
        "detail": h.detail,
        "suggestion": h.suggestion,
        "evidence": format!("{}:{}: {}", h.at.file, h.at.line, short(h.at.text.trim(), 160)),
    })
}

fn review(user: &str, lang: MockLang) -> Value {
    let diff = diff_block(user);
    let lines = added_lines(&diff);
    let files = changed_files(&diff);
    let mut findings: Vec<Value> = review_hits(&lines, lang).iter().map(finding_json).collect();
    let empty = lines.is_empty() && files.is_empty();
    if empty {
        findings.push(json!({
            "severity": "high",
            "category": "completeness",
            "file": null,
            "line": null,
            "title": pick(lang, "No changes to review", "Нет изменений для проверки"),
            "detail": pick(lang, "The task's branch has no changes against its base.", "В ветке задачи нет изменений относительно базовой ветки."),
            "suggestion": pick(lang, "Implement the task and commit the changes.", "Выполните задачу и закоммитьте изменения."),
            "evidence": "",
        }));
    }
    let serious = findings.iter().any(|f| matches!(f["severity"].as_str(), Some("critical" | "high")));
    let verdict = if diff.contains("MOCK_REJECT") {
        "reject"
    } else if serious {
        "request_changes"
    } else {
        "approve"
    };
    let file_list = if files.is_empty() { "—".to_owned() } else { files.join(", ") };
    let ac: Vec<Value> = criteria(user)
        .into_iter()
        .map(|c| {
            json!({
                "criterion": c,
                "met": !empty && verdict != "reject",
                "evidence": if empty {
                    pick(lang, "no changes", "изменений нет")
                } else {
                    format!("{}: {file_list}", pick(lang, "changed", "изменено"))
                },
            })
        })
        .collect();
    let summary = match (verdict, lang) {
        ("approve", MockLang::En) => {
            format!("Looks good: {} file(s) changed, {} finding(s). — Mock (offline demo)", files.len(), findings.len())
        }
        ("approve", MockLang::Ru) => format!(
            "Изменения в порядке: файлов — {}, замечаний — {}. — Mock (офлайн-демо)",
            files.len(),
            findings.len()
        ),
        ("reject", MockLang::En) => "The approach is rejected. — Mock (offline demo)".to_owned(),
        ("reject", MockLang::Ru) => "Подход отклонён. — Mock (офлайн-демо)".to_owned(),
        (_, MockLang::En) => {
            format!("Changes requested: {} finding(s) must be addressed. — Mock (offline demo)", findings.len())
        }
        (_, MockLang::Ru) => format!("Нужны доработки: замечаний — {}. — Mock (офлайн-демо)", findings.len()),
    };
    json!({ "verdict": verdict, "summary": summary, "findings": findings, "acceptanceCriteria": ac })
}

fn adversarial(user: &str, lang: MockLang) -> Value {
    let diff = diff_block(user);
    let lines = added_lines(&diff);
    let mut findings: Vec<Value> = Vec::new();
    for a in &lines {
        if let Some(h) = security_hit(a, lang) {
            findings.push(finding_json(&h));
            continue;
        }
        let lower = a.text.to_ascii_lowercase();
        if ["spawn(", "thread", "settimeout(", "promise.all(", "static mut"].iter().any(|k| lower.contains(k)) {
            findings.push(finding_json(&Hit {
                severity: "high",
                category: "concurrency",
                title: pick(lang, "Possible race condition", "Возможное состояние гонки"),
                detail: pick(
                    lang,
                    "Concurrent work touches shared state without synchronisation.",
                    "Параллельная работа меняет общее состояние без синхронизации.",
                ),
                suggestion: pick(
                    lang,
                    "Guard the shared state or make the operation atomic.",
                    "Защитите общее состояние или сделайте операцию атомарной.",
                ),
                at: a.clone(),
            }));
        } else if ["drop table", "rm -rf", "fs.rm", "unlink(", "delete from", "remove_dir_all"]
            .iter()
            .any(|k| lower.contains(k))
        {
            findings.push(finding_json(&Hit {
                severity: "high",
                category: "data-loss",
                title: pick(lang, "Destructive operation without a safeguard", "Разрушающая операция без защиты"),
                detail: pick(
                    lang,
                    "Data is removed without a backup, confirmation or dry run.",
                    "Данные удаляются без резервной копии, подтверждения или пробного запуска.",
                ),
                suggestion: pick(
                    lang,
                    "Add a confirmation, snapshot or soft delete.",
                    "Добавьте подтверждение, снимок или мягкое удаление.",
                ),
                at: a.clone(),
            }));
        }
        if findings.len() >= 10 {
            break;
        }
    }
    // The adversary never simply agrees: it always challenges an edge case.
    match lines.iter().find(|a| !a.text.trim().is_empty()) {
        Some(first) => findings.push(finding_json(&Hit {
            severity: "medium",
            category: "edge-case",
            title: pick(lang, "Empty or invalid input is not handled", "Не обработан пустой или некорректный ввод"),
            detail: pick(
                lang,
                "Nothing shows what happens with empty, very large or malformed input here.",
                "Не видно, что происходит при пустом, очень большом или некорректном вводе.",
            ),
            suggestion: pick(lang, "Validate the input and add a test for the edge case.", "Проверяйте ввод и добавьте тест на граничный случай."),
            at: first.clone(),
        })),
        None => findings.push(json!({
            "severity": "high",
            "category": "assumption",
            "file": null,
            "line": null,
            "title": pick(lang, "The change is empty", "Изменение пустое"),
            "detail": pick(lang, "There is nothing to verify; the task's claim of success is unsupported.", "Проверять нечего: заявленный успех задачи ничем не подтверждён."),
            "suggestion": pick(lang, "Commit the implementation.", "Закоммитьте реализацию."),
            "evidence": pick(lang, "the diff is empty", "изменений нет"),
        })),
    }
    let serious = findings.iter().any(|f| matches!(f["severity"].as_str(), Some("critical" | "high")));
    json!({
        "verdict": if serious { "request_changes" } else { "approve" },
        "summary": pick(
            lang,
            &format!("Adversarial review: {} attack(s) on the change. — Mock (offline demo)", findings.len()),
            &format!("Состязательное ревью: найдено слабых мест — {}. — Mock (офлайн-демо)", findings.len()),
        ),
        "findings": findings,
        "acceptanceCriteria": [],
    })
}

/// The JSON array after "## Findings to verify".
fn findings_to_verify(user: &str) -> Vec<Value> {
    let Some(start) = user.find("## Findings to verify") else { return Vec::new() };
    let rest = &user[start..];
    let (Some(a), Some(b)) = (rest.find('['), rest.rfind(']')) else { return Vec::new() };
    serde_json::from_str::<Value>(&rest[a..=b]).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default()
}

fn verification(user: &str, lang: MockLang) -> Value {
    let responses: Vec<Value> = findings_to_verify(user)
        .iter()
        .map(|f| {
            let severity = f["severity"].as_str().unwrap_or("medium").to_ascii_lowercase();
            let has_evidence = f["evidence"].as_str().is_some_and(|e| !e.trim().is_empty());
            let concede = matches!(severity.as_str(), "critical" | "high") && has_evidence;
            json!({
                "id": f["id"],
                "verdict": if concede { "concede" } else { "rebut" },
                "evidence": if concede {
                    pick(lang, "Confirmed: the quoted line does exactly that.", "Подтверждаю: процитированная строка делает именно это.")
                } else {
                    pick(
                        lang,
                        "Out of scope for this task: inputs are validated by the caller and covered by the existing tests.",
                        "Вне рамок задачи: ввод проверяет вызывающий код, и это покрыто существующими тестами.",
                    )
                },
            })
        })
        .collect();
    json!({ "responses": responses })
}

const APPROACHES: [(&str, &str, f64); 5] = [
    ("Incremental refactor", "Пошаговый рефакторинг", 0.72),
    ("Dedicated service module", "Отдельный сервисный модуль", 0.81),
    ("Event-driven pipeline", "Событийный конвейер", 0.64),
    ("Adopt a proven library", "Готовая проверенная библиотека", 0.58),
    ("Minimal targeted patch", "Минимальная точечная правка", 0.5),
];

/// `Participant N of M` → N (1-based).
fn participant(system: &str) -> usize {
    system
        .lines()
        .find_map(|l| l.split("Participant ").nth(1))
        .and_then(|r| r.split_whitespace().next())
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1)
}

fn question_of(user: &str) -> String {
    user.lines()
        .find_map(|l| l.strip_prefix("Question:").map(|q| q.trim().to_owned()))
        .filter(|q| !q.is_empty())
        .unwrap_or_else(|| short(user, 160))
}

fn proposal(system: &str, user: &str, lang: MockLang) -> Value {
    let n = participant(system);
    let (en, ru, confidence) = APPROACHES[(n - 1) % APPROACHES.len()];
    let title = pick(lang, en, ru);
    let q = short(&question_of(user), 140);
    json!({
        "title": title,
        "summary": pick(
            lang,
            &format!("{en} for: {q} — Mock (offline demo)"),
            &format!("{ru} для задачи: {q} — Mock (офлайн-демо)"),
        ),
        "approach": pick(
            lang,
            &format!("1. Isolate the affected code.\n2. Apply the {} approach.\n3. Cover it with tests and document the decision.", en.to_lowercase()),
            &format!("1. Изолировать затронутый код.\n2. Применить подход «{}».\n3. Покрыть тестами и задокументировать решение.", ru.to_lowercase()),
        ),
        "pros": [pick(lang, "Clear ownership of the change", "Понятная зона ответственности"), pick(lang, "Easy to test", "Легко тестировать")],
        "cons": [pick(lang, "Takes extra effort up front", "Требует дополнительных усилий в начале")],
        "risks": [pick(lang, "Hidden coupling with existing code", "Скрытые связи с существующим кодом")],
        "confidence": confidence,
    })
}

/// `### Proposal N — title (confidence 0.81)` lines → (N, confidence).
fn proposals_in(user: &str) -> Vec<(u32, f64)> {
    user.lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("### Proposal ")?;
            let n: u32 = rest.split_whitespace().next()?.parse().ok()?;
            let conf = l
                .rsplit("confidence ")
                .next()
                .and_then(|c| c.trim_end_matches(')').trim().parse::<f64>().ok())
                .unwrap_or(0.5);
            Some((n, conf))
        })
        .collect()
}

fn synthesis(user: &str, lang: MockLang) -> Value {
    let props = proposals_in(user);
    let best = props.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0))).unwrap_or((1, 0.5));
    let (lo, hi) = props.iter().fold((1.0_f64, 0.0_f64), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
    let agreement = if props.len() < 2 { 1.0 } else { ((1.0 - (hi - lo)) * 100.0).round() / 100.0 };
    let scores: Vec<Value> = props
        .iter()
        .map(|(n, c)| {
            json!({
                "proposal": n,
                "score": ((c * 10.0) * 10.0).round() / 10.0,
                "note": if *n == best.0 {
                    pick(lang, "Best balance of risk and effort", "Лучший баланс риска и трудозатрат")
                } else {
                    pick(lang, "Viable, but weaker on risk", "Рабочий вариант, но рискованнее")
                },
            })
        })
        .collect();
    let q = short(&question_of(user), 120);
    json!({
        "title": pick(lang, &format!("Decision: {q}"), &format!("Решение: {q}")),
        "decision": pick(
            lang,
            &format!("Adopt proposal {} and fold in the test plan of the others. — Mock (offline demo)", best.0),
            &format!("Принять предложение {} и добавить план тестирования из остальных. — Mock (офлайн-демо)", best.0),
        ),
        "reasoningSummary": pick(
            lang,
            &format!("Proposal {} had the highest confidence and the clearest risks; the others were weaker on risk.", best.0),
            &format!("У предложения {} самая высокая уверенность и самые понятные риски; остальные рискованнее.", best.0),
        ),
        "consequences": pick(lang, "Document the decision and schedule the follow-up tasks.", "Задокументировать решение и запланировать следующие задачи."),
        "chosenProposal": best.0,
        "agreement": agreement,
        "scores": scores,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;

    fn req(system: &str, user: &str) -> ChatRequest {
        let mut r = ChatRequest::new("mock-smart", vec![ChatMessage::user(user)]);
        r.system = Some(system.into());
        r
    }

    fn json_of(s: Option<MockScript>) -> Value {
        match s {
            Some(MockScript::Text(t)) => serde_json::from_str(&t).unwrap(),
            other => panic!("unexpected {other:?}"),
        }
    }

    const DIFF: &str = "## Acceptance criteria\n1. It greets\n2. It is tested\n\n## Diff\n```diff\ndiff --git a/src/a.js b/src/a.js\n--- a/src/a.js\n+++ b/src/a.js\n@@ -1,2 +10,3 @@\n context\n+const password = \"hunter2\";\n+// TODO later\n```\n";

    #[test]
    fn added_lines_track_files_and_numbers() {
        let lines = added_lines(&diff_block(DIFF));
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            AddedLine { file: "src/a.js".into(), line: 11, text: "const password = \"hunter2\";".into() }
        );
        assert_eq!(lines[1].line, 12);
    }

    #[test]
    fn reviewer_flags_secrets_and_checks_criteria() {
        let v = json_of(script(&req(REVIEW_MARKER, DIFF)));
        assert_eq!(v["verdict"], "request_changes");
        assert_eq!(v["findings"][0]["severity"], "critical");
        assert_eq!(v["findings"][0]["line"], 11);
        assert_eq!(v["acceptanceCriteria"].as_array().unwrap().len(), 2);
        let clean = DIFF.replace("const password = \"hunter2\";", "export const x = 1;").replace("// TODO later", "");
        assert_eq!(json_of(script(&req(REVIEW_MARKER, &clean)))["verdict"], "approve");
    }

    #[test]
    fn adversary_never_agrees_and_author_verifies() {
        let v = json_of(script(&req(ADVERSARIAL_MARKER, DIFF)));
        let f = v["findings"].as_array().unwrap();
        assert!(f.len() >= 2 && f.iter().any(|x| x["category"] == "edge-case"));
        let mut list = Vec::new();
        for (i, x) in f.iter().enumerate() {
            let mut x = x.clone();
            x["id"] = json!(format!("f{}", i + 1));
            list.push(x);
        }
        let user = format!("## Findings to verify\n{}\n", Value::Array(list));
        let r = json_of(script(&req(VERIFY_MARKER, &user)));
        let resp = r["responses"].as_array().unwrap();
        assert_eq!(resp[0]["verdict"], "concede");
        assert_eq!(resp.last().unwrap()["verdict"], "rebut");
    }

    #[test]
    fn consensus_proposals_differ_and_synthesis_picks_the_most_confident() {
        let a = json_of(script(&req(&format!("{PROPOSAL_MARKER}\nParticipant 1 of 2"), "Question: cache?")));
        let b = json_of(script(&req(&format!("{PROPOSAL_MARKER}\nParticipant 2 of 2"), "Question: cache?")));
        assert_ne!(a["title"], b["title"]);
        let user = "Question: cache?\n### Proposal 1 — A (confidence 0.72)\n### Proposal 2 — B (confidence 0.81)\n";
        let s = json_of(script(&req(SYNTHESIS_MARKER, user)));
        assert_eq!(s["chosenProposal"], 2);
        assert_eq!(s["scores"].as_array().unwrap().len(), 2);
        assert!(script(&req("plain", "hi")).is_none());
    }
}
