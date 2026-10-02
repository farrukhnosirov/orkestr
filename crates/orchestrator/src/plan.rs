//! Structured plans (§1 steps 2–4, §9 lite): the JSON schema given to the
//! model, robust JSON extraction for models without structured output,
//! normalisation (roles, priorities, keys, dependencies), validation (unknown
//! dependencies, cycles, limits) and estimates (cost, parallelism).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use orkestr_domain::{AgentRole, ModelRef, TaskPriority};
use orkestr_protocol::methods::manager::{PlanTask, MAX_PLAN_TASKS};
use orkestr_protocol::methods::tasks::{MAX_CRITERION_LEN, MAX_DESCRIPTION_LEN, MAX_TITLE_LEN};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{OrchResult, OrchestratorError};

/// Name of the structured-output schema (`ResponseFormat::JsonSchema`).
pub const PLAN_SCHEMA_NAME: &str = "orkestr_plan";

/// Role names offered to the planning model.
pub const PLAN_ROLES: [&str; 11] = [
    "Architect",
    "Backend",
    "Frontend",
    "Database",
    "QA",
    "Security",
    "DevOps",
    "UI/UX",
    "Product Manager",
    "Reviewer",
    "Researcher",
];

/// JSON schema of a plan document.
#[must_use]
pub fn plan_schema() -> Value {
    json!({
        "type": "object",
        "required": ["summary", "tasks"],
        "properties": {
            "goal": {"type": "string", "description": "The user goal, restated"},
            "summary": {"type": "string", "description": "2-4 sentences: approach and order of work"},
            "assumptions": {"type": "array", "items": {"type": "string"}},
            "risks": {"type": "array", "items": {"type": "string"}},
            "tasks": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_PLAN_TASKS,
                "items": {
                    "type": "object",
                    "required": ["key", "title", "role", "dependsOn"],
                    "properties": {
                        "key": {"type": "string", "description": "Short unique key, e.g. t1"},
                        "title": {"type": "string", "description": "Imperative title, at most 200 characters"},
                        "description": {"type": "string"},
                        "acceptanceCriteria": {"type": "array", "items": {"type": "string"}},
                        "role": {"type": "string", "enum": PLAN_ROLES},
                        "dependsOn": {"type": "array", "items": {"type": "string"}, "description": "Keys of tasks that must finish first"},
                        "priority": {"type": "string", "enum": ["Critical", "High", "Normal", "Low", "Background"]},
                        "epic": {"type": "string"},
                        "feature": {"type": "string"},
                        "files": {"type": "array", "items": {"type": "string"}, "description": "Relative paths the task will likely touch"},
                        "estimatedTokens": {"type": "integer", "minimum": 0}
                    }
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// JSON extraction
// ---------------------------------------------------------------------------

/// Finds the end (exclusive byte index) of the balanced JSON object starting
/// at `start` (`text[start] == '{'`), honouring strings and escapes.
fn balanced_end(text: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, ch) in text[start..].char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(start + i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Extracts the plan JSON object from model text: the whole text, a fenced
/// ```json block, or the first balanced `{…}` that parses and has `tasks`.
#[must_use]
pub fn extract_json(text: &str) -> Option<Value> {
    let is_plan = |v: &Value| v.get("tasks").is_some_and(Value::is_array);
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        if is_plan(&v) {
            return Some(v);
        }
    }
    // Fenced blocks.
    let mut rest = trimmed;
    while let Some(open) = rest.find("```") {
        let after = &rest[open + 3..];
        let body_start = after.find('\n').map_or(0, |n| n + 1);
        let body = &after[body_start..];
        let Some(close) = body.find("```") else { break };
        if let Ok(v) = serde_json::from_str::<Value>(body[..close].trim()) {
            if is_plan(&v) {
                return Some(v);
            }
        }
        rest = &body[close + 3..];
    }
    // Balanced objects.
    let mut from = 0;
    while let Some(off) = trimmed[from..].find('{') {
        let start = from + off;
        if let Some(end) = balanced_end(trimmed, start) {
            if let Ok(v) = serde_json::from_str::<Value>(&trimmed[start..end]) {
                if is_plan(&v) {
                    return Some(v);
                }
            }
        }
        from = start + 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Raw (model) shape
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawPlan {
    goal: Option<String>,
    summary: Option<String>,
    assumptions: Vec<Value>,
    risks: Vec<Value>,
    tasks: Vec<RawTask>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RawTask {
    #[serde(alias = "id")]
    key: Option<Value>,
    #[serde(alias = "name")]
    title: Option<String>,
    description: Option<String>,
    #[serde(alias = "acceptance_criteria", alias = "criteria")]
    acceptance_criteria: Vec<Value>,
    #[serde(alias = "agent", alias = "agent_role")]
    role: Option<String>,
    #[serde(alias = "depends_on", alias = "dependencies", alias = "deps")]
    depends_on: Vec<Value>,
    priority: Option<String>,
    epic: Option<String>,
    feature: Option<String>,
    files: Vec<Value>,
    #[serde(alias = "estimated_tokens", alias = "tokens")]
    estimated_tokens: Option<Value>,
}

fn value_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        Value::Object(o) => o.get("text").or_else(|| o.get("title")).and_then(value_str),
        _ => None,
    }
}

fn cap(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Maps a free-form role name onto a built-in role (`None` = unknown).
#[must_use]
pub fn parse_role(s: &str) -> Option<AgentRole> {
    let k: String = s.to_ascii_lowercase().chars().filter(char::is_ascii_alphanumeric).collect();
    Some(match k.as_str() {
        "architect" | "architecture" | "solutionarchitect" => AgentRole::Architect,
        "backend" | "backenddeveloper" | "backendengineer" | "developer" | "engineer" | "coder" | "implementer"
        | "fullstack" => AgentRole::Backend,
        "frontend" | "frontenddeveloper" | "frontendengineer" | "web" => AgentRole::Frontend,
        "database" | "db" | "dba" | "data" => AgentRole::Database,
        "qa" | "tester" | "test" | "testing" | "qaengineer" | "quality" => AgentRole::Qa,
        "security" | "sec" | "securityreviewer" | "appsec" => AgentRole::Security,
        "devops" | "ops" | "sre" | "infrastructure" | "infra" => AgentRole::DevOps,
        "uiux" | "ux" | "ui" | "designer" | "design" => AgentRole::UiUx,
        "productmanager" | "pm" | "product" | "productowner" | "writer" | "docs" | "documentation" => {
            AgentRole::ProductManager
        }
        "reviewer" | "review" | "codereviewer" => AgentRole::Reviewer,
        "researcher" | "research" | "analyst" => AgentRole::Researcher,
        _ => return None,
    })
}

/// Parses a priority name (case-insensitive; unknown = `Normal`).
#[must_use]
pub fn parse_priority(s: Option<&str>) -> TaskPriority {
    match s.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("critical" | "urgent" | "p0") => TaskPriority::Critical,
        Some("high" | "p1") => TaskPriority::High,
        Some("low" | "p3") => TaskPriority::Low,
        Some("background" | "p4") => TaskPriority::Background,
        _ => TaskPriority::Normal,
    }
}

fn clean_file(f: &str) -> Option<String> {
    let f = f.trim().replace('\\', "/");
    let f = f.trim_start_matches("./").to_owned();
    let bad = f.is_empty()
        || f.len() > 500
        || f.starts_with('/')
        || f.contains(':')
        || f.split('/').any(|seg| seg == "..")
        || f.starts_with(".git/")
        || f == ".git";
    (!bad).then_some(f)
}

/// Normalised plan content (before ids/status are attached).
#[derive(Debug, Clone, PartialEq)]
pub struct PlanDraft {
    /// Restated goal (falls back to the user's goal).
    pub goal: String,
    /// Summary.
    pub summary: String,
    /// Assumptions.
    pub assumptions: Vec<String>,
    /// Risks.
    pub risks: Vec<String>,
    /// Tasks.
    pub tasks: Vec<PlanTask>,
    /// Normalisation notes.
    pub warnings: Vec<String>,
}

/// Normalises and validates a model plan document.
pub fn normalize(doc: &Value, user_goal: &str) -> OrchResult<PlanDraft> {
    let raw: RawPlan = serde_json::from_value(doc.clone())
        .map_err(|e| OrchestratorError::Planning(format!("the plan does not match the schema: {e}")))?;
    if raw.tasks.is_empty() {
        return Err(OrchestratorError::Planning("the plan has no tasks".into()));
    }
    if raw.tasks.len() > MAX_PLAN_TASKS {
        return Err(OrchestratorError::Planning(format!(
            "the plan has {} tasks; at most {MAX_PLAN_TASKS} are allowed — split the goal",
            raw.tasks.len()
        )));
    }
    let mut warnings = Vec::new();
    // Keys: unique, non-empty.
    let mut seen: HashSet<String> = HashSet::new();
    let mut keys: Vec<String> = Vec::with_capacity(raw.tasks.len());
    let mut raw_keys: Vec<Option<String>> = Vec::with_capacity(raw.tasks.len());
    for (i, t) in raw.tasks.iter().enumerate() {
        let original = t.key.as_ref().and_then(value_str);
        let mut key: String =
            original.as_deref().map(|k| cap(k, 40)).filter(|k| !k.is_empty()).unwrap_or_else(|| format!("t{}", i + 1));
        if seen.contains(&key) {
            let mut n = 2;
            while seen.contains(&format!("{key}-{n}")) {
                n += 1;
            }
            warnings.push(format!("duplicate task key `{key}` renamed to `{key}-{n}`"));
            key = format!("{key}-{n}");
        }
        seen.insert(key.clone());
        keys.push(key);
        raw_keys.push(original);
    }
    // Original key → normalised key (first occurrence wins).
    let mut alias: HashMap<String, String> = HashMap::new();
    for (orig, key) in raw_keys.iter().zip(&keys) {
        if let Some(o) = orig {
            alias.entry(o.clone()).or_insert_with(|| key.clone());
        }
        alias.entry(key.clone()).or_insert_with(|| key.clone());
    }

    let mut tasks = Vec::with_capacity(raw.tasks.len());
    for (i, (t, key)) in raw.tasks.iter().zip(&keys).enumerate() {
        let title = t.title.as_deref().map(str::trim).filter(|s| !s.is_empty()).map_or_else(
            || {
                warnings.push(format!("task {key} had no title"));
                format!("Task {}", i + 1)
            },
            |s| cap(s, MAX_TITLE_LEN),
        );
        let role = match t.role.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            Some(r) => parse_role(r).unwrap_or_else(|| {
                warnings.push(format!("task {key}: unknown role `{r}` mapped to Backend"));
                AgentRole::Backend
            }),
            None => {
                warnings.push(format!("task {key}: no role given, using Backend"));
                AgentRole::Backend
            }
        };
        let mut depends_on: Vec<String> = Vec::new();
        for d in t.depends_on.iter().filter_map(value_str) {
            match alias.get(&d) {
                Some(k) if k == key => warnings.push(format!("task {key}: dependency on itself dropped")),
                Some(k) => {
                    if !depends_on.contains(k) {
                        depends_on.push(k.clone());
                    }
                }
                None => warnings.push(format!("task {key}: unknown dependency `{d}` dropped")),
            }
        }
        let files: Vec<String> = {
            let mut out: Vec<String> = Vec::new();
            for f in t.files.iter().filter_map(value_str) {
                match clean_file(&f) {
                    Some(c) if !out.contains(&c) && out.len() < 200 => out.push(c),
                    Some(_) => {}
                    None => warnings.push(format!("task {key}: file `{}` ignored (not a relative path)", cap(&f, 80))),
                }
            }
            out
        };
        let estimated_tokens = t.estimated_tokens.as_ref().and_then(|v| match v {
            Value::Number(n) => n.as_u64().or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)),
            Value::String(s) => s.trim().parse::<u64>().ok(),
            _ => None,
        });
        tasks.push(PlanTask {
            key: key.clone(),
            title,
            description: cap(t.description.as_deref().unwrap_or("").trim(), MAX_DESCRIPTION_LEN),
            acceptance_criteria: t
                .acceptance_criteria
                .iter()
                .filter_map(value_str)
                .map(|c| cap(&c, MAX_CRITERION_LEN))
                .take(30)
                .collect(),
            role,
            depends_on,
            priority: parse_priority(t.priority.as_deref()),
            epic: t.epic.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| cap(s, 120)),
            feature: t.feature.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| cap(s, 120)),
            files,
            estimated_tokens: estimated_tokens.map(|n| n.min(10_000_000)),
            estimated_cost_usd: None,
            task_id: None,
        });
    }
    validate_tasks(&tasks)?;
    let strings = |v: &[Value]| v.iter().filter_map(value_str).map(|s| cap(&s, 500)).take(20).collect::<Vec<_>>();
    let goal = raw
        .goal
        .as_deref()
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map_or_else(|| cap(user_goal, 2000), |g| cap(g, 2000));
    Ok(PlanDraft {
        goal,
        summary: cap(raw.summary.as_deref().unwrap_or("").trim(), 4000),
        assumptions: strings(&raw.assumptions),
        risks: strings(&raw.risks),
        tasks,
        warnings,
    })
}

/// Validates keys, dependencies and acyclicity.
pub fn validate_tasks(tasks: &[PlanTask]) -> OrchResult<()> {
    let keys: HashSet<&str> = tasks.iter().map(|t| t.key.as_str()).collect();
    if keys.len() != tasks.len() {
        return Err(OrchestratorError::Planning("duplicate task keys".into()));
    }
    for t in tasks {
        if let Some(missing) = t.depends_on.iter().find(|d| !keys.contains(d.as_str())) {
            return Err(OrchestratorError::Planning(format!("{} depends on unknown {missing}", t.key)));
        }
    }
    if levels(tasks).is_none() {
        return Err(OrchestratorError::Planning("the task dependencies contain a cycle".into()));
    }
    Ok(())
}

/// Level of every task (0 = no dependencies; else 1 + max level of its
/// dependencies). `None` when the dependencies contain a cycle.
#[must_use]
pub fn levels(tasks: &[PlanTask]) -> Option<BTreeMap<String, usize>> {
    let deps: HashMap<&str, &[String]> = tasks.iter().map(|t| (t.key.as_str(), t.depends_on.as_slice())).collect();
    let mut level: BTreeMap<String, usize> = BTreeMap::new();
    let mut remaining: BTreeSet<&str> = tasks.iter().map(|t| t.key.as_str()).collect();
    while !remaining.is_empty() {
        let ready: Vec<&str> = remaining
            .iter()
            .copied()
            .filter(|k| deps.get(k).is_none_or(|ds| ds.iter().all(|d| level.contains_key(d.as_str()))))
            .collect();
        if ready.is_empty() {
            return None;
        }
        for k in ready {
            let l = deps
                .get(k)
                .map_or(0, |ds| ds.iter().filter_map(|d| level.get(d.as_str())).map(|l| l + 1).max().unwrap_or(0));
            level.insert(k.to_owned(), l);
            remaining.remove(k);
        }
    }
    Some(level)
}

/// Widest dependency level (how many tasks can run at once, by level).
#[must_use]
pub fn max_parallelism(tasks: &[PlanTask]) -> u32 {
    let Some(levels) = levels(tasks) else { return 1 };
    let mut counts: BTreeMap<usize, u32> = BTreeMap::new();
    for l in levels.values() {
        *counts.entry(*l).or_default() += 1;
    }
    counts.values().copied().max().unwrap_or(1).max(1)
}

/// Fills per-task cost estimates for `model` (70 % input / 30 % output
/// tokens at list price) and returns the total (`None` when the price of the
/// model is unknown or no task has a token estimate).
pub fn estimate_costs(tasks: &mut [PlanTask], model: Option<&ModelRef>) -> Option<f64> {
    let pricing = model.and_then(|m| orkestr_providers::pricing_for(&m.model))?;
    let mut total = 0.0;
    let mut any = false;
    for t in tasks.iter_mut() {
        let Some(tokens) = t.estimated_tokens else { continue };
        let usage = orkestr_providers::Usage {
            input_tokens: tokens * 7 / 10,
            output_tokens: tokens - tokens * 7 / 10,
            estimated: true,
            ..orkestr_providers::Usage::default()
        };
        let c = orkestr_providers::pricing::cost_with(&pricing, &usage);
        t.estimated_cost_usd = Some((c * 10_000.0).round() / 10_000.0);
        total += c;
        any = true;
    }
    any.then_some((total * 10_000.0).round() / 10_000.0)
}

/// Applies plan-card edits: removed tasks (and dependencies on them) and new
/// titles. Fails when nothing would remain.
pub fn apply_edits(
    tasks: &mut Vec<PlanTask>,
    edits: &orkestr_protocol::methods::manager::PlanEdits,
) -> OrchResult<Vec<String>> {
    let mut notes = Vec::new();
    let remove: HashSet<&str> = edits.remove_tasks.iter().map(String::as_str).collect();
    if let Some(unknown) = remove.iter().find(|k| !tasks.iter().any(|t| t.key == **k)) {
        return Err(OrchestratorError::Invalid(format!("unknown task key `{unknown}`")));
    }
    tasks.retain(|t| !remove.contains(t.key.as_str()));
    if tasks.is_empty() {
        return Err(OrchestratorError::Invalid("the plan would have no tasks left".into()));
    }
    for t in tasks.iter_mut() {
        let before = t.depends_on.len();
        t.depends_on.retain(|d| !remove.contains(d.as_str()));
        if t.depends_on.len() != before {
            notes.push(format!("{}: dependencies on removed tasks dropped", t.key));
        }
    }
    for e in &edits.titles {
        let title = e.title.trim();
        if title.is_empty() || title.chars().count() > MAX_TITLE_LEN {
            return Err(OrchestratorError::Invalid(format!(
                "title of {} must be 1..={MAX_TITLE_LEN} characters",
                e.key
            )));
        }
        let t = tasks
            .iter_mut()
            .find(|t| t.key == e.key)
            .ok_or_else(|| OrchestratorError::Invalid(format!("unknown task key `{}`", e.key)))?;
        t.title = title.to_owned();
    }
    if !remove.is_empty() {
        notes.push(format!("removed by the user: {}", edits.remove_tasks.join(", ")));
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(key: &str, deps: &[&str]) -> PlanTask {
        PlanTask {
            key: key.into(),
            title: key.into(),
            description: String::new(),
            acceptance_criteria: vec![],
            depends_on: deps.iter().map(|d| (*d).to_owned()).collect(),
            role: AgentRole::Backend,
            priority: TaskPriority::Normal,
            epic: None,
            feature: None,
            files: vec![],
            estimated_tokens: Some(10_000),
            estimated_cost_usd: None,
            task_id: None,
        }
    }

    #[test]
    fn validation_and_parallelism() {
        validate_tasks(&[t("a", &[]), t("b", &["a"]), t("c", &["a", "b"])]).unwrap();
        assert!(validate_tasks(&[t("a", &["b"]), t("b", &["a"])]).is_err());
        assert!(validate_tasks(&[t("a", &["zzz"])]).is_err());
        assert!(validate_tasks(&[t("a", &[]), t("a", &[])]).is_err());
        assert_eq!(max_parallelism(&[t("a", &[]), t("b", &["a"]), t("c", &["a"]), t("d", &["b"])]), 2);
        assert_eq!(max_parallelism(&[t("a", &[]), t("b", &[]), t("c", &[])]), 3);
    }

    #[test]
    fn extracts_json_from_prose_and_fences() {
        let plan = r#"{"summary":"s","tasks":[{"key":"t1","title":"x","role":"Backend","dependsOn":[]}]}"#;
        assert!(extract_json(plan).is_some());
        assert!(extract_json(&format!("Sure! Here is the plan:\n```json\n{plan}\n```\nDone.")).is_some());
        assert!(extract_json(&format!("Plan follows {plan} — let me know {{not json}}")).is_some());
        let tricky = r#"text {"note": "a } brace"} then {"summary":"s \"q\" }","tasks":[]}"#;
        assert_eq!(extract_json(tricky).unwrap()["tasks"], json!([]));
        assert!(extract_json("no json here").is_none());
        assert!(extract_json("{\"a\": 1}").is_none());
    }

    #[test]
    fn normalizes_roles_keys_and_dependencies() {
        let doc = json!({
            "summary": "s",
            "tasks": [
                {"id": 1, "title": "Design", "role": "architect", "depends_on": [], "priority": "HIGH", "files": ["./docs/a.md", "../etc/passwd", "C:/x"]},
                {"id": 1, "title": "Build", "role": "Software Engineer", "dependencies": ["1", "zzz", "1-2"]},
                {"key": "t3", "name": "Test", "role": "tester", "dependsOn": ["1-2", "t3"], "acceptance_criteria": ["passes", {"text": "covers edge cases"}], "estimatedTokens": "5000"}
            ]
        });
        let d = normalize(&doc, "goal").unwrap();
        assert_eq!(d.goal, "goal");
        let keys: Vec<&str> = d.tasks.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys, ["1", "1-2", "t3"]);
        assert_eq!(d.tasks[0].role, AgentRole::Architect);
        assert_eq!(d.tasks[0].priority, TaskPriority::High);
        assert_eq!(d.tasks[0].files, ["docs/a.md"]);
        assert_eq!(d.tasks[1].role, AgentRole::Backend, "unknown role mapped");
        assert_eq!(d.tasks[1].depends_on, ["1"]);
        assert_eq!(d.tasks[2].role, AgentRole::Qa);
        assert_eq!(d.tasks[2].depends_on, ["1-2"]);
        assert_eq!(d.tasks[2].acceptance_criteria, ["passes", "covers edge cases"]);
        assert_eq!(d.tasks[2].estimated_tokens, Some(5000));
        assert!(d.warnings.iter().any(|w| w.contains("zzz")));
        assert!(d.warnings.iter().any(|w| w.contains("itself")));
        assert!(normalize(&json!({"tasks": []}), "g").is_err());
        let cyclic = json!({"tasks": [{"key":"a","title":"a","role":"QA","dependsOn":["b"]},{"key":"b","title":"b","role":"QA","dependsOn":["a"]}]});
        assert!(matches!(normalize(&cyclic, "g"), Err(OrchestratorError::Planning(m)) if m.contains("cycle")));
    }

    #[test]
    fn costs_and_edits() {
        let mut tasks = vec![t("a", &[]), t("b", &["a"]), t("c", &["b"])];
        let m = ModelRef { provider_id: orkestr_domain::ProviderId::new(), model: "gpt-4o".into() };
        let total = estimate_costs(&mut tasks, Some(&m)).unwrap();
        assert!(total > 0.0 && tasks[0].estimated_cost_usd.is_some());
        let mock = ModelRef { provider_id: orkestr_domain::ProviderId::new(), model: "mock-smart".into() };
        assert!(estimate_costs(&mut tasks, Some(&mock)).is_none());
        let edits = orkestr_protocol::methods::manager::PlanEdits {
            remove_tasks: vec!["b".into()],
            titles: vec![orkestr_protocol::methods::manager::PlanTaskTitleEdit {
                key: "c".into(),
                title: "New".into(),
            }],
        };
        apply_edits(&mut tasks, &edits).unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks[1].depends_on.is_empty());
        assert_eq!(tasks[1].title, "New");
        let all = orkestr_protocol::methods::manager::PlanEdits {
            remove_tasks: vec!["a".into(), "c".into()],
            titles: vec![],
        };
        assert!(apply_edits(&mut tasks, &all).is_err());
    }
}
