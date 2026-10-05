//! Scripted Mock behaviours for Agent Teams and the AI Product Manager
//! (Phase 2, §6, §9). Keyed by distinctive system-prompt markers so they
//! never interfere with the other Mock scripts:
//!
//! * [`TEAM_LEAD_MARKER`] — a team lead decomposes a delegated task into
//!   executor sub-tasks (implement → test, one per executor role named in
//!   the `Executor roles:` line of the request).
//! * [`PM_MARKER`] — the AI Product Manager turns an idea into an epic with
//!   two features, three stories with acceptance criteria and four tasks
//!   with dependencies (one crossing features).
//!
//! Output is a pure function of the request (deterministic, offline).

use serde_json::{json, Value};

use super::mock::{reply_lang, subject_of, MockLang, MockScript};
use crate::types::{ChatRequest, Role};

/// System-prompt marker of a team lead decomposition request.
pub const TEAM_LEAD_MARKER: &str = "[orkestr:team-lead]";
/// System-prompt marker of an AI Product Manager request.
pub const PM_MARKER: &str = "[orkestr:product-manager]";

fn last_user(req: &ChatRequest) -> String {
    req.messages.iter().rev().find(|m| m.role == Role::User).map(|m| m.content.clone()).unwrap_or_default()
}

fn line_value(text: &str, prefix: &str) -> Option<String> {
    text.lines().find_map(|l| l.trim().strip_prefix(prefix).map(|v| v.trim().to_owned())).filter(|v| !v.is_empty())
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The scripted reply for a teams / PM request, `None` for other requests.
#[must_use]
pub fn script(req: &ChatRequest) -> Option<MockScript> {
    let system = req.system_prompt().unwrap_or_default();
    if system.contains(TEAM_LEAD_MARKER) {
        return Some(MockScript::Text(lead_plan(req).to_string()));
    }
    if system.contains(PM_MARKER) {
        return Some(MockScript::Text(pm_draft(req).to_string()));
    }
    None
}

/// Lead decomposition: `implement` (first executor role) → `verify` (a QA
/// role when the team has one, otherwise the same role).
#[must_use]
pub fn lead_plan(req: &ChatRequest) -> Value {
    let user = last_user(req);
    let title = line_value(&user, "Task:").unwrap_or_else(|| "the delegated task".into());
    let roles: Vec<String> = line_value(&user, "Executor roles:")
        .map(|l| l.split(',').map(|r| r.trim().to_owned()).filter(|r| !r.is_empty()).collect())
        .unwrap_or_default();
    let first = roles.first().cloned().unwrap_or_else(|| "Backend".into());
    let qa = roles.iter().find(|r| r.eq_ignore_ascii_case("qa")).cloned().unwrap_or_else(|| first.clone());
    let subject = subject_of(&title);
    let ru = reply_lang(req) == MockLang::Ru;
    let (impl_t, test_t, summary) = if ru {
        (
            format!("Реализовать: {title}"),
            format!("Проверить и покрыть тестами: {title}"),
            format!("Разбил задачу «{title}» на реализацию и проверку — Mock (офлайн-демо)."),
        )
    } else {
        (
            format!("Implement: {title}"),
            format!("Verify and test: {title}"),
            format!("Split \"{title}\" into implementation and verification — Mock (offline demo)."),
        )
    };
    json!({
        "summary": summary,
        "subtasks": [
            {
                "key": "s1",
                "title": impl_t,
                "description": format!("Implement the {subject} part of the delegated task."),
                "role": first,
                "acceptanceCriteria": [format!("src/{subject}.mjs exists and exports the feature")],
                "dependsOn": [],
                "files": [format!("src/{subject}.mjs")]
            },
            {
                "key": "s2",
                "title": test_t,
                "description": format!("Write and run tests for {subject}."),
                "role": qa,
                "acceptanceCriteria": ["tests pass"],
                "dependsOn": ["s1"],
                "files": [format!("test/{subject}.test.mjs")]
            }
        ]
    })
}

/// PM breakdown of the idea in the `Idea:` line.
#[must_use]
pub fn pm_draft(req: &ChatRequest) -> Value {
    let user = last_user(req);
    let idea = line_value(&user, "Idea:").unwrap_or_else(|| user.trim().to_owned());
    let subject = subject_of(&idea);
    let s = title_case(&subject);
    let ru = reply_lang(req) == MockLang::Ru;
    let t = |en: String, ru_text: String| if ru { ru_text } else { en };
    json!({
        "epic": {
            "title": t(format!("{s}: first release"), format!("{s}: первый выпуск")),
            "description": t(format!("Deliver {idea} — Mock (offline demo)."), format!("Реализовать: {idea} — Mock (офлайн-демо).")),
            "features": [
                {
                    "key": "f1",
                    "title": t(format!("{s} core"), format!("Ядро {s}")),
                    "description": t("Data model and service logic.".into(), "Модель данных и логика сервиса.".into()),
                    "stories": [
                        {
                            "key": "s1",
                            "title": t(format!("As a user I can store {subject} records"), format!("Как пользователь, я могу сохранять записи {subject}")),
                            "acceptanceCriteria": [
                                t("A record can be created and read back".into(), "Запись можно создать и прочитать".into()),
                                t("Invalid input is rejected with a clear error".into(), "Некорректный ввод отклоняется с понятной ошибкой".into())
                            ],
                            "tasks": [
                                {"key": "t1", "title": t(format!("Design the {subject} data model"), format!("Спроектировать модель данных {subject}")), "role": "Architect", "priority": "High", "dependsOn": [], "estimateMinutes": 30},
                                {"key": "t2", "title": t(format!("Implement the {subject} service"), format!("Реализовать сервис {subject}")), "role": "Backend", "priority": "High", "dependsOn": ["t1"], "estimateMinutes": 60}
                            ]
                        },
                        {
                            "key": "s2",
                            "title": t(format!("As a maintainer I trust {subject} changes"), format!("Как сопровождающий, я доверяю изменениям {subject}")),
                            "acceptanceCriteria": [t("Automated tests cover the service".into(), "Сервис покрыт автоматическими тестами".into())],
                            "tasks": [
                                {"key": "t3", "title": t(format!("Test the {subject} service"), format!("Протестировать сервис {subject}")), "role": "QA", "priority": "Normal", "dependsOn": ["t2"], "estimateMinutes": 30}
                            ]
                        }
                    ]
                },
                {
                    "key": "f2",
                    "title": t(format!("{s} screen"), format!("Экран {s}")),
                    "description": t("User-facing view.".into(), "Пользовательский интерфейс.".into()),
                    "stories": [
                        {
                            "key": "s3",
                            "title": t(format!("As a user I see my {subject} records"), format!("Как пользователь, я вижу свои записи {subject}")),
                            "acceptanceCriteria": [t("The list shows every stored record".into(), "Список показывает все сохранённые записи".into())],
                            "tasks": [
                                {"key": "t4", "title": t(format!("Build the {subject} list view"), format!("Сделать список {subject}")), "role": "Frontend", "priority": "Normal", "dependsOn": ["t2"], "estimateMinutes": 45}
                            ]
                        }
                    ]
                }
            ]
        }
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

    #[test]
    fn lead_plan_uses_the_executor_roles() {
        let r =
            req(&format!("{TEAM_LEAD_MARKER} lead"), "Task: Build the invoice module\nExecutor roles: Frontend, QA");
        let MockScript::Text(t) = super::super::mock::script(&r) else { panic!("text expected") };
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["subtasks"][0]["role"], "Frontend");
        assert_eq!(v["subtasks"][1]["role"], "QA");
        assert_eq!(v["subtasks"][1]["dependsOn"][0], "s1");
        assert!(v["subtasks"][0]["title"].as_str().unwrap().contains("invoice"));
    }

    #[test]
    fn pm_draft_has_a_full_hierarchy() {
        let r = req(&format!("{PM_MARKER} pm"), "Idea: a payroll module for small shops");
        let MockScript::Text(t) = super::super::mock::script(&r) else { panic!("text expected") };
        let v: Value = serde_json::from_str(&t).unwrap();
        let features = v["epic"]["features"].as_array().unwrap();
        assert_eq!(features.len(), 2);
        assert_eq!(features[1]["stories"][0]["tasks"][0]["dependsOn"][0], "t2", "cross-feature dependency");
        assert!(v["epic"]["title"].as_str().unwrap().contains("Payroll"));
    }

    #[test]
    fn other_requests_are_untouched() {
        assert!(script(&req("You are an agent", "hi")).is_none());
    }
}
