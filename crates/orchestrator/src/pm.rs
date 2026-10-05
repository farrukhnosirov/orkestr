//! AI Product Manager (§9, §68.47): idea → Epic → Features → Stories →
//! Acceptance Criteria → Tasks + Dependencies.
//!
//! * `pm.draft` asks the Manager's model (structured output, validated, one
//!   repair round) for the breakdown and stores it as an editable draft.
//! * `pm.update` replaces the breakdown of a draft after strict validation
//!   (unique keys, limits, known acyclic task dependencies).
//! * `pm.approve` turns the draft into task-engine tasks — the epic, its
//!   features, their stories (with acceptance criteria) and the executable
//!   tasks as a parent hierarchy, dependencies between tasks — wraps the
//!   executable tasks in an approved Manager plan and can start a run (or
//!   delegate it to an Agent Team).
//! * `pm.get` reports progress and completes container tasks (story,
//!   feature, epic) once all their children completed.

use std::collections::{BTreeMap, HashMap, HashSet};

use orkestr_domain::{now, ManagerPlanId, ProjectId, TaskActor, TaskId, TaskStatus};
use orkestr_protocol::event::{EventKind, PmDraftPayload};
use orkestr_protocol::methods::manager::{
    ManagerMessageRole, ManagerPlan, ManagerRequestOptions, ManagerRunParams, PlanSource, PlanStatus, PlanTask,
};
use orkestr_protocol::methods::pm::{
    PmApproveParams, PmApproveResult, PmDraft, PmDraftParams, PmDraftStatus, PmEpic, PmFeature, PmListParams,
    PmListResult, PmProgress, PmStory, PmTask, PmUpdateParams, MAX_PM_CRITERIA, MAX_PM_FEATURES, MAX_PM_IDEA,
    MAX_PM_STORIES, MAX_PM_TASKS,
};
use orkestr_protocol::methods::tasks::{CreateTaskParams, MAX_CRITERION_LEN, MAX_DESCRIPTION_LEN, MAX_TITLE_LEN};
use orkestr_protocol::NewEvent;
use orkestr_providers::adapters::mock_teams_pm::PM_MARKER;
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, StorageError};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::error::{OrchResult, OrchestratorError};
use crate::plan::{parse_priority, parse_role, PLAN_ROLES};
use crate::service::{Inner, Manager, MessageExtras};
use crate::structured::StructuredCall;
use crate::team_lead::has_cycle;
use crate::util::{cap, is_ru, one_line, role_name, ru_plural};

/// Structured-output schema name of a PM breakdown.
pub const PM_SCHEMA_NAME: &str = "orkestr_pm_draft";

/// JSON schema of a PM breakdown.
#[must_use]
pub fn pm_schema() -> Value {
    let task = json!({
        "type": "object",
        "required": ["key", "title", "role", "dependsOn"],
        "properties": {
            "key": {"type": "string", "description": "Unique task key, e.g. t1"},
            "title": {"type": "string"},
            "description": {"type": "string"},
            "role": {"type": "string", "enum": PLAN_ROLES},
            "priority": {"type": "string", "enum": ["Critical", "High", "Normal", "Low", "Background"]},
            "dependsOn": {"type": "array", "items": {"type": "string"}, "description": "Task keys (any story) that must finish first"},
            "estimateMinutes": {"type": "integer", "minimum": 1}
        }
    });
    let story = json!({
        "type": "object",
        "required": ["key", "title", "acceptanceCriteria", "tasks"],
        "properties": {
            "key": {"type": "string"},
            "title": {"type": "string", "description": "User story: As a … I want … so that …"},
            "description": {"type": "string"},
            "acceptanceCriteria": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": MAX_PM_CRITERIA},
            "tasks": {"type": "array", "items": task, "minItems": 1}
        }
    });
    json!({
        "type": "object",
        "required": ["epic"],
        "properties": {
            "epic": {
                "type": "object",
                "required": ["title", "features"],
                "properties": {
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "features": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_PM_FEATURES,
                        "items": {
                            "type": "object",
                            "required": ["key", "title", "stories"],
                            "properties": {
                                "key": {"type": "string"},
                                "title": {"type": "string"},
                                "description": {"type": "string"},
                                "stories": {"type": "array", "items": story, "minItems": 1, "maxItems": MAX_PM_STORIES}
                            }
                        }
                    }
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// normalisation (model output) and validation (edits)
// ---------------------------------------------------------------------------

fn s(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(x)) => x.trim().to_owned(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn arr<'a>(v: &'a Value, keys: &[&str]) -> Vec<&'a Value> {
    keys.iter().find_map(|k| v.get(*k).and_then(Value::as_array)).map(|a| a.iter().collect()).unwrap_or_default()
}

fn strings(v: &Value, keys: &[&str]) -> Vec<String> {
    arr(v, keys)
        .into_iter()
        .filter_map(|x| match x {
            Value::String(t) => Some(t.trim().to_owned()),
            Value::Object(o) => o.get("text").and_then(Value::as_str).map(|t| t.trim().to_owned()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .collect()
}

fn unique_key(raw: String, fallback: String, seen: &mut HashSet<String>, warnings: &mut Vec<String>) -> String {
    let mut k = cap(if raw.is_empty() { &fallback } else { &raw }, 40);
    if seen.contains(&k) {
        let mut n = 2;
        while seen.contains(&format!("{k}-{n}")) {
            n += 1;
        }
        warnings.push(format!("duplicate key `{k}` renamed to `{k}-{n}`"));
        k = format!("{k}-{n}");
    }
    seen.insert(k.clone());
    k
}

/// Normalises a model breakdown (lenient: aliases, generated keys, unknown
/// roles mapped, unknown dependencies dropped) and validates it strictly.
pub fn normalize_pm(doc: &Value) -> OrchResult<(PmEpic, Vec<String>)> {
    let e = doc.get("epic").unwrap_or(doc);
    let mut warnings = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut task_alias: HashMap<String, String> = HashMap::new();
    let (mut fi, mut si, mut ti) = (0usize, 0usize, 0usize);
    let mut features = Vec::new();
    for f in arr(e, &["features"]) {
        fi += 1;
        let fkey = unique_key(s(f.get("key").or_else(|| f.get("id"))), format!("f{fi}"), &mut seen, &mut warnings);
        let mut stories = Vec::new();
        for st in arr(f, &["stories", "userStories"]) {
            si += 1;
            let skey = unique_key(s(st.get("key").or_else(|| st.get("id"))), format!("s{si}"), &mut seen, &mut warnings);
            let mut tasks = Vec::new();
            for t in arr(st, &["tasks"]) {
                ti += 1;
                let original = s(t.get("key").or_else(|| t.get("id")));
                let tkey = unique_key(original.clone(), format!("t{ti}"), &mut seen, &mut warnings);
                if !original.is_empty() {
                    task_alias.entry(original).or_insert_with(|| tkey.clone());
                }
                task_alias.entry(tkey.clone()).or_insert_with(|| tkey.clone());
                let role_s = s(t.get("role").or_else(|| t.get("agent")));
                let role = parse_role(&role_s).unwrap_or_else(|| {
                    warnings.push(format!("{tkey}: unknown role `{role_s}` mapped to Backend"));
                    orkestr_domain::AgentRole::Backend
                });
                let estimate = t
                    .get("estimateMinutes")
                    .or_else(|| t.get("estimate_minutes"))
                    .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|x| x.trim().parse().ok())))
                    .map(|n| u32::try_from(n.clamp(1, 10_000)).unwrap_or(10_000));
                tasks.push(PmTask {
                    key: tkey,
                    title: cap(&s(t.get("title").or_else(|| t.get("name"))), MAX_TITLE_LEN),
                    description: cap(&s(t.get("description")), MAX_DESCRIPTION_LEN),
                    role,
                    priority: parse_priority(t.get("priority").and_then(Value::as_str)),
                    depends_on: strings(t, &["dependsOn", "depends_on", "dependencies"]),
                    estimate_minutes: estimate,
                    task_id: None,
                });
            }
            stories.push(PmStory {
                key: skey,
                title: cap(&s(st.get("title").or_else(|| st.get("name"))), MAX_TITLE_LEN),
                description: cap(&s(st.get("description")), MAX_DESCRIPTION_LEN),
                acceptance_criteria: strings(st, &["acceptanceCriteria", "acceptance_criteria", "criteria"])
                    .into_iter()
                    .map(|c| cap(&c, MAX_CRITERION_LEN))
                    .collect(),
                tasks,
                task_id: None,
            });
        }
        features.push(PmFeature {
            key: fkey,
            title: cap(&s(f.get("title").or_else(|| f.get("name"))), MAX_TITLE_LEN),
            description: cap(&s(f.get("description")), MAX_DESCRIPTION_LEN),
            stories,
            task_id: None,
        });
    }
    let mut epic = PmEpic {
        title: cap(&s(e.get("title").or_else(|| e.get("name"))), MAX_TITLE_LEN),
        description: cap(&s(e.get("description")), MAX_DESCRIPTION_LEN),
        features,
        task_id: None,
    };
    // Map dependency aliases; drop unknown ones and self-references.
    for f in &mut epic.features {
        for st in &mut f.stories {
            for t in &mut st.tasks {
                let mut deps = Vec::new();
                for d in &t.depends_on {
                    match task_alias.get(d) {
                        Some(k) if *k != t.key && !deps.contains(k) => deps.push(k.clone()),
                        Some(_) => {}
                        None => warnings.push(format!("{}: unknown dependency `{d}` dropped", t.key)),
                    }
                }
                t.depends_on = deps;
            }
        }
    }
    validate_epic(&epic)?;
    Ok((epic, warnings))
}

fn bad<T>(m: impl Into<String>) -> OrchResult<T> {
    Err(OrchestratorError::Invalid(m.into()))
}

fn check_title(what: &str, t: &str) -> OrchResult<()> {
    if t.trim().is_empty() || t.chars().count() > MAX_TITLE_LEN {
        return bad(format!("{what}: the title must be 1..={MAX_TITLE_LEN} characters"));
    }
    Ok(())
}

/// Strict validation of a breakdown (edits and normalised model output).
pub fn validate_epic(epic: &PmEpic) -> OrchResult<()> {
    check_title("epic", &epic.title)?;
    if epic.features.is_empty() || epic.features.len() > MAX_PM_FEATURES {
        return bad(format!("an epic needs 1..={MAX_PM_FEATURES} features"));
    }
    let mut keys: HashSet<&str> = HashSet::new();
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if epic.description.chars().count() > MAX_DESCRIPTION_LEN {
        return bad("the epic description is too long");
    }
    for f in &epic.features {
        check_title(&format!("feature {}", f.key), &f.title)?;
        if f.key.trim().is_empty() || !keys.insert(&f.key) {
            return bad(format!("feature key `{}` is empty or not unique", f.key));
        }
        if f.stories.is_empty() || f.stories.len() > MAX_PM_STORIES {
            return bad(format!("feature {} needs 1..={MAX_PM_STORIES} stories", f.key));
        }
        if f.description.chars().count() > MAX_DESCRIPTION_LEN {
            return bad(format!("feature {}: the description is too long", f.key));
        }
        for st in &f.stories {
            check_title(&format!("story {}", st.key), &st.title)?;
            if st.key.trim().is_empty() || !keys.insert(&st.key) {
                return bad(format!("story key `{}` is empty or not unique", st.key));
            }
            if st.acceptance_criteria.is_empty() || st.acceptance_criteria.len() > MAX_PM_CRITERIA {
                return bad(format!("story {} needs 1..={MAX_PM_CRITERIA} acceptance criteria", st.key));
            }
            if st.acceptance_criteria.iter().any(|c| c.trim().is_empty() || c.chars().count() > MAX_CRITERION_LEN) {
                return bad(format!("story {}: every acceptance criterion must be 1..={MAX_CRITERION_LEN} characters", st.key));
            }
            if st.tasks.is_empty() {
                return bad(format!("story {} needs at least one task", st.key));
            }
            if st.description.chars().count() > MAX_DESCRIPTION_LEN {
                return bad(format!("story {}: the description is too long", st.key));
            }
            for t in &st.tasks {
                check_title(&format!("task {}", t.key), &t.title)?;
                if t.key.trim().is_empty() || !keys.insert(&t.key) {
                    return bad(format!("task key `{}` is empty or not unique", t.key));
                }
                if t.description.chars().count() > MAX_DESCRIPTION_LEN {
                    return bad(format!("task {}: the description is too long", t.key));
                }
                if t.estimate_minutes.is_some_and(|m| m == 0 || m > 10_000) {
                    return bad(format!("task {}: the estimate must be 1..=10000 minutes", t.key));
                }
                deps.insert(t.key.clone(), t.depends_on.clone());
            }
        }
    }
    if deps.len() > MAX_PM_TASKS {
        return bad(format!("the draft has {} tasks; at most {MAX_PM_TASKS} are allowed", deps.len()));
    }
    for (k, ds) in &deps {
        if let Some(d) = ds.iter().find(|d| !deps.contains_key(*d) || *d == k) {
            return bad(format!("task {k} depends on unknown task `{d}`"));
        }
    }
    if has_cycle(&deps) {
        return bad("the task dependencies contain a cycle");
    }
    Ok(())
}

/// Clears task ids (edited drafts are never linked to tasks).
fn strip_ids(epic: &mut PmEpic) {
    epic.task_id = None;
    for f in &mut epic.features {
        f.task_id = None;
        for st in &mut f.stories {
            st.task_id = None;
            for t in &mut st.tasks {
                t.task_id = None;
            }
        }
    }
}

fn counts(epic: &PmEpic) -> (usize, usize, usize) {
    let stories = epic.features.iter().map(|f| f.stories.len()).sum();
    let tasks = epic.features.iter().flat_map(|f| &f.stories).map(|s| s.tasks.len()).sum();
    (epic.features.len(), stories, tasks)
}

// ---------------------------------------------------------------------------
// store
// ---------------------------------------------------------------------------

/// Persistence of PM drafts.
#[derive(Debug, Clone)]
pub struct PmStore {
    db: Db,
}

impl PmStore {
    /// Wraps a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Inserts or replaces a draft.
    pub async fn save(&self, d: &PmDraft) -> OrchResult<()> {
        let doc = serde_json::to_string(d)?;
        let d = d.clone();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO pm_drafts (id, project_id, status, created_at, updated_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO UPDATE SET status = excluded.status, updated_at = excluded.updated_at,
                        doc = excluded.doc",
                    params![
                        d.id,
                        d.project_id.to_string(),
                        format!("{:?}", d.status),
                        d.created_at.to_rfc3339(),
                        d.updated_at.to_rfc3339(),
                        doc
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// One draft.
    pub async fn get(&self, id: &str) -> OrchResult<PmDraft> {
        let sid = id.to_owned();
        let doc: Option<String> = self
            .db
            .call(move |c| Ok(c.query_row("SELECT doc FROM pm_drafts WHERE id = ?1", [sid], |r| r.get(0)).optional()?))
            .await?;
        let doc = doc.ok_or_else(|| OrchestratorError::NotFound(format!("product draft {id}")))?;
        serde_json::from_str(&doc).map_err(|e| OrchestratorError::Storage(StorageError::Json(e)))
    }

    /// Drafts of a project, newest first.
    pub async fn list(&self, project: ProjectId, include_discarded: bool) -> OrchResult<Vec<PmDraft>> {
        let p = project.to_string();
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM pm_drafts WHERE project_id = ?1 AND (?2 OR status != 'Discarded')
                     ORDER BY created_at DESC LIMIT 100",
                )?;
                let rows = stmt.query_map(params![p, include_discarded], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        docs.iter()
            .map(|d| serde_json::from_str(d).map_err(|e| OrchestratorError::Storage(StorageError::Json(e))))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// service
// ---------------------------------------------------------------------------

fn system_prompt(language: &str, extra: &str) -> String {
    let mut s = format!(
        "{PM_MARKER} You are the AI Product Manager of Orkestr. Turn the user's idea into a product breakdown: ONE epic → \
         features → user stories (\"As a … I want … so that …\") with testable acceptance criteria → concrete engineering \
         tasks for specialised AI agents, with dependencies between tasks (task keys, across stories when needed).\n\n\
         Rules:\n\
         - 1..={MAX_PM_FEATURES} features, 1..={MAX_PM_STORIES} stories per feature, at most {MAX_PM_TASKS} tasks in total.\n\
         - Every story has acceptance criteria and at least one task; every task has one role and a priority.\n\
         - Only add a dependency when a task really needs another task's output.\n\
         - Roles: {}.\n",
        PLAN_ROLES.join(", ")
    );
    if !extra.trim().is_empty() {
        s.push_str(&format!("\nInstructions from the user's AI Manager settings:\n{}\n", extra.trim()));
    }
    s.push('\n');
    s.push_str(&orkestr_agent_runtime::reply_language_instruction(language));
    s.push_str("- Field names and keys stay in English.\n");
    s
}

impl Inner {
    fn pm_store(&self) -> PmStore {
        PmStore::new(self.deps.db.clone())
    }

    async fn save_draft(&self, d: &PmDraft) -> OrchResult<()> {
        self.pm_store().save(d).await?;
        self.deps
            .publish(NewEvent::new(EventKind::PmDraftUpdated(PmDraftPayload { draft: d.clone() })).project(d.project_id))
            .await;
        Ok(())
    }

    /// Progress of an approved draft; completes containers whose children
    /// all completed (story → feature → epic).
    async fn pm_progress(&self, d: &mut PmDraft) {
        if d.status != PmDraftStatus::Approved {
            return;
        }
        let status = |id: Option<TaskId>| {
            let tasks = std::sync::Arc::clone(&self.deps.tasks);
            async move {
                match id {
                    Some(id) => tasks.get(id).await.ok().map(|t| t.status),
                    None => None,
                }
            }
        };
        let mut p = PmProgress::default();
        let mut feature_done = Vec::new();
        for f in &d.epic.features {
            let mut story_done = Vec::new();
            for st in &f.stories {
                let mut all = true;
                for t in &st.tasks {
                    p.total += 1;
                    match status(t.task_id).await {
                        Some(TaskStatus::Completed) => p.completed += 1,
                        Some(TaskStatus::Failed | TaskStatus::Cancelled) => {
                            p.failed += 1;
                            all = false;
                        }
                        _ => all = false,
                    }
                }
                if all {
                    self.complete_container(st.task_id).await;
                }
                story_done.push(all);
            }
            let all = story_done.iter().all(|x| *x);
            if all {
                self.complete_container(f.task_id).await;
            }
            feature_done.push(all);
        }
        if feature_done.iter().all(|x| *x) {
            self.complete_container(d.epic.task_id).await;
        }
        d.progress = Some(p);
    }

    async fn complete_container(&self, id: Option<TaskId>) {
        let Some(id) = id else { return };
        let Ok(t) = self.deps.tasks.get(id).await else { return };
        let path: &[TaskStatus] = match t.status {
            TaskStatus::Planned => &[TaskStatus::Ready, TaskStatus::Running, TaskStatus::Completed],
            TaskStatus::Ready => &[TaskStatus::Running, TaskStatus::Completed],
            TaskStatus::Running => &[TaskStatus::Completed],
            _ => return,
        };
        for to in path {
            if let Err(e) = self
                .deps
                .tasks
                .transition(id, *to, TaskActor::Manager, Some("all child tasks completed".into()), None)
                .await
            {
                tracing::warn!(task = %id, error = %e, "could not complete a product container task");
                return;
            }
        }
    }
}

impl Manager {
    /// `pm.draft`: idea → structured breakdown (stored as an editable draft).
    pub async fn pm_draft(&self, p: PmDraftParams) -> OrchResult<PmDraft> {
        let project = self.inner.project(p.project_id).await?;
        let idea = p.idea.trim().to_owned();
        if idea.is_empty() || idea.chars().count() > MAX_PM_IDEA {
            return Err(OrchestratorError::Invalid(format!("the idea must be 1..={MAX_PM_IDEA} characters")));
        }
        if self.inner.deps.security.emergency.flags().all_stopped {
            return Err(OrchestratorError::Forbidden("Stop All AI is active — reset the emergency controls first".into()));
        }
        let options: ManagerRequestOptions = p.options.unwrap_or_default();
        if let Some(lang) = options.language.as_deref() {
            self.inner.deps.config.ui_language.set(lang);
        }
        let language = self.inner.deps.config.ui_language.get();
        let cfg = self.inner.config().await;
        let ru = is_ru(&language);
        self.inner
            .add_message(
                project.id,
                None,
                ManagerMessageRole::User,
                &if ru { format!("Продакт-менеджер: {idea}") } else { format!("Product Manager: {idea}") },
                MessageExtras::default(),
            )
            .await?;
        let mut user = format!("Idea: {idea}\n\n# Project\nName: {}\n", project.name);
        if !project.description.trim().is_empty() {
            user.push_str(&format!("Description: {}\n", one_line(&project.description, 500)));
        }
        user.push_str("\nProduce the breakdown now.");
        let call = StructuredCall {
            project: &project,
            model: options.model.as_ref(),
            system: system_prompt(&language, &cfg.system_instructions),
            user,
            schema_name: PM_SCHEMA_NAME,
            schema: pm_schema(),
            key: "epic",
        };
        let cancel = CancellationToken::new();
        let out = match self.inner.structured_call(call, &cancel, normalize_pm).await {
            Ok(o) => o,
            Err(e) => {
                let text = if ru {
                    format!("Продакт-менеджер не смог разобрать идею: {e}")
                } else {
                    format!("The Product Manager could not break the idea down: {e}")
                };
                self.inner.system_event(project.id, &text, MessageExtras::default()).await;
                return Err(e);
            }
        };
        let (epic, mut warnings) = out.value;
        if out.repaired {
            warnings.push("the first answer was invalid and was repaired".into());
        }
        let at = now();
        let draft = PmDraft {
            id: uuid::Uuid::now_v7().to_string(),
            project_id: project.id,
            idea,
            status: PmDraftStatus::Draft,
            epic,
            warnings,
            model: out.model.clone(),
            plan_id: None,
            run_id: None,
            team_id: options.team_id,
            progress: None,
            created_at: at,
            updated_at: at,
        };
        self.inner.save_draft(&draft).await?;
        let (f, s, t) = counts(&draft.epic);
        let text = if ru {
            format!(
                "Продакт-менеджер подготовил черновик «{}»: {f} {}, {s} {}, {t} {}. Откройте «Задачи → Продукт», чтобы отредактировать и подтвердить его.",
                draft.epic.title,
                ru_plural(f, "функция", "функции", "функций"),
                ru_plural(s, "история", "истории", "историй"),
                ru_plural(t, "задача", "задачи", "задач"),
            )
        } else {
            format!(
                "The Product Manager drafted \"{}\": {f} feature(s), {s} stor(ies), {t} task(s). Open Tasks → Product to edit and approve it.",
                draft.epic.title
            )
        };
        self.inner
            .add_message(
                project.id,
                None,
                ManagerMessageRole::Manager,
                &text,
                MessageExtras { model: out.model, usage: Some(out.usage), ..MessageExtras::default() },
            )
            .await?;
        tracing::info!(draft = %draft.id, features = f, stories = s, tasks = t, "product draft created");
        Ok(draft)
    }

    /// `pm.get`.
    pub async fn pm_get(&self, id: &str) -> OrchResult<PmDraft> {
        let mut d = self.inner.pm_store().get(id).await?;
        self.inner.pm_progress(&mut d).await;
        Ok(d)
    }

    /// `pm.list`.
    pub async fn pm_list(&self, p: PmListParams) -> OrchResult<PmListResult> {
        self.inner.project(p.project_id).await?;
        let mut drafts = self.inner.pm_store().list(p.project_id, p.include_discarded.unwrap_or(false)).await?;
        for d in &mut drafts {
            self.inner.pm_progress(d).await;
        }
        Ok(PmListResult { drafts })
    }

    /// `pm.update`: replaces the breakdown of a draft (validated).
    pub async fn pm_update(&self, p: PmUpdateParams) -> OrchResult<PmDraft> {
        let _g = self.inner.decide.lock().await;
        let mut d = self.inner.pm_store().get(&p.draft_id).await?;
        if d.status != PmDraftStatus::Draft {
            return Err(OrchestratorError::Conflict(format!("the draft is {:?}; only drafts can be edited", d.status)));
        }
        let mut epic = p.epic;
        strip_ids(&mut epic);
        validate_epic(&epic)?;
        d.epic = epic;
        d.warnings.clear();
        d.updated_at = now();
        self.inner.save_draft(&d).await?;
        Ok(d)
    }

    /// `pm.discard`.
    pub async fn pm_discard(&self, id: &str) -> OrchResult<PmDraft> {
        let _g = self.inner.decide.lock().await;
        let mut d = self.inner.pm_store().get(id).await?;
        if d.status != PmDraftStatus::Draft {
            return Err(OrchestratorError::Conflict(format!("the draft is {:?}; only drafts can be discarded", d.status)));
        }
        d.status = PmDraftStatus::Discarded;
        d.updated_at = now();
        self.inner.save_draft(&d).await?;
        Ok(d)
    }

    /// `pm.approve`: creates the task hierarchy, wraps the executable tasks
    /// in an approved Manager plan and (optionally) starts a run.
    pub async fn pm_approve(&self, p: PmApproveParams) -> OrchResult<PmApproveResult> {
        let (draft, plan) = {
            let _g = self.inner.decide.lock().await;
            self.approve_locked(&p).await?
        };
        let run = if p.start_run.unwrap_or(false) {
            Some(self.start_run(ManagerRunParams { plan_id: Some(plan.id), run_id: None, limits: None }).await?)
        } else {
            None
        };
        let mut draft = draft;
        if let Some(r) = &run {
            draft.run_id = Some(r.id);
            draft.updated_at = now();
            self.inner.save_draft(&draft).await?;
        }
        let plan = self.inner.store.plan(plan.id).await?;
        Ok(PmApproveResult { draft, plan, run })
    }

    async fn approve_locked(&self, p: &PmApproveParams) -> OrchResult<(PmDraft, ManagerPlan)> {
        let inner = &self.inner;
        let mut d = inner.pm_store().get(&p.draft_id).await?;
        if d.status != PmDraftStatus::Draft {
            return Err(OrchestratorError::Conflict(format!("the draft is {:?}; only drafts can be approved", d.status)));
        }
        if inner.deps.security.emergency.flags().all_stopped {
            return Err(OrchestratorError::Forbidden("Stop All AI is active — reset the emergency controls first".into()));
        }
        let project = inner.project(d.project_id).await?;
        validate_epic(&d.epic)?;
        let team = match p.team_id.or(d.team_id) {
            Some(id) => Some(inner.team_store().get(id).await?),
            None => None,
        };
        let ru = is_ru(&inner.deps.config.ui_language.get());
        let mut created: Vec<TaskId> = Vec::new();
        let result = self.create_hierarchy(&mut d, &mut created, ru).await;
        if let Err(e) = result {
            for id in created.iter().rev() {
                if let Err(x) = inner.deps.tasks.delete(*id).await {
                    tracing::warn!(task = %id, error = %x, "rollback of a product task failed");
                }
            }
            strip_ids(&mut d.epic);
            return Err(e);
        }
        // The executable tasks become an approved Manager plan.
        let mut plan_tasks = Vec::new();
        for f in &d.epic.features {
            for st in &f.stories {
                for t in &st.tasks {
                    plan_tasks.push(PlanTask {
                        key: t.key.clone(),
                        title: t.title.clone(),
                        description: t.description.clone(),
                        acceptance_criteria: st.acceptance_criteria.clone(),
                        role: t.role.clone(),
                        depends_on: t.depends_on.clone(),
                        priority: t.priority,
                        epic: Some(cap(&d.epic.title, 120)),
                        feature: Some(cap(&f.title, 120)),
                        files: Vec::new(),
                        estimated_tokens: None,
                        estimated_cost_usd: None,
                        task_id: t.task_id,
                    });
                }
            }
        }
        let at = now();
        let width = crate::plan::max_parallelism(&plan_tasks);
        let cfg = inner.config().await;
        let plan = ManagerPlan {
            id: ManagerPlanId::new(),
            project_id: project.id,
            conversation_id: None,
            goal: d.idea.clone(),
            summary: cap(
                format!("{} — {}", d.epic.title, one_line(&d.epic.description, 600)).trim_end_matches(" — "),
                4000,
            ),
            assumptions: Vec::new(),
            risks: Vec::new(),
            tasks: plan_tasks,
            estimated_cost_usd: None,
            max_parallelism: width,
            parallelism: width.min(cfg.max_parallel_agents.max(1)).max(1),
            status: PlanStatus::Approved,
            source: PlanSource::Structured,
            model: d.model.clone(),
            warnings: vec!["created by the AI Product Manager".into()],
            options: ManagerRequestOptions { team_id: team.as_ref().map(|t| t.id), ..ManagerRequestOptions::default() },
            run_id: None,
            rejection_reason: None,
            created_at: at,
            updated_at: at,
        };
        inner.save_plan(&plan).await?;
        if let Some(t) = &team {
            let mut a = inner.ensure_assignment(t, project.id).await?;
            a.plan_id = Some(plan.id);
            inner.team_store().save_assignment(&a).await?;
        }
        d.status = PmDraftStatus::Approved;
        d.plan_id = Some(plan.id);
        d.team_id = team.as_ref().map(|t| t.id);
        d.updated_at = now();
        inner.pm_progress(&mut d).await;
        inner.save_draft(&d).await?;
        let (_, _, n) = counts(&d.epic);
        let text = if ru {
            format!(
                "Черновик «{}» подтверждён: {n} {} с иерархией эпик → функции → истории.",
                d.epic.title,
                ru_plural(n, "задача создана", "задачи созданы", "задач создано")
            )
        } else {
            format!("Draft \"{}\" approved — {n} task(s) created under the epic → feature → story hierarchy.", d.epic.title)
        };
        inner.system_event(project.id, &text, MessageExtras { plan_id: Some(plan.id), ..MessageExtras::default() }).await;
        Ok((d, plan))
    }

    async fn create_hierarchy(&self, d: &mut PmDraft, created: &mut Vec<TaskId>, ru: bool) -> OrchResult<()> {
        let tasks = &self.inner.deps.tasks;
        let project_id = d.project_id;
        let mk = |title: &str, description: &str, parent: Option<TaskId>, tags: Vec<String>, ac: Option<Vec<String>>| {
            CreateTaskParams {
                project_id,
                title: cap(title, MAX_TITLE_LEN),
                description: Some(cap(description, MAX_DESCRIPTION_LEN)),
                acceptance_criteria: ac,
                parent_id: parent,
                tags: Some(tags),
                ..CreateTaskParams::default()
            }
        };
        let (l_epic, l_feature, l_story) = if ru { ("Эпик", "Функция", "История") } else { ("Epic", "Feature", "Story") };
        let epic_task = tasks
            .create(mk(&d.epic.title, &d.epic.description, None, vec!["pm".into(), "epic".into()], None), TaskActor::Manager)
            .await?;
        created.push(epic_task.id);
        d.epic.task_id = Some(epic_task.id);
        let epic_title = d.epic.title.clone();
        for f in &mut d.epic.features {
            let desc = format!("{}\n\n_{l_epic}: {epic_title}_", f.description).trim().to_owned();
            let ft = tasks
                .create(mk(&f.title, &desc, Some(epic_task.id), vec!["pm".into(), "feature".into()], None), TaskActor::Manager)
                .await?;
            created.push(ft.id);
            f.task_id = Some(ft.id);
            for st in &mut f.stories {
                let desc = format!("{}\n\n_{l_feature}: {}_", st.description, f.title).trim().to_owned();
                let stt = tasks
                    .create(
                        mk(&st.title, &desc, Some(ft.id), vec!["pm".into(), "story".into()], Some(st.acceptance_criteria.clone())),
                        TaskActor::Manager,
                    )
                    .await?;
                created.push(stt.id);
                st.task_id = Some(stt.id);
            }
        }
        // Executable tasks in dependency order.
        let mut ids: HashMap<String, TaskId> = HashMap::new();
        loop {
            let mut progressed = false;
            let mut pending = false;
            for f in &mut d.epic.features {
                for st in &mut f.stories {
                    let story_id = st.task_id;
                    let story_title = st.title.clone();
                    let ac = st.acceptance_criteria.clone();
                    for t in &mut st.tasks {
                        if t.task_id.is_some() {
                            continue;
                        }
                        if !t.depends_on.iter().all(|k| ids.contains_key(k)) {
                            pending = true;
                            continue;
                        }
                        let desc = format!("{}\n\n_{l_story}: {story_title}_", t.description).trim().to_owned();
                        let mut params = mk(
                            &t.title,
                            &desc,
                            story_id,
                            vec!["pm".into(), cap(&role_name(&t.role), 40)],
                            Some(ac.clone()),
                        );
                        params.priority = Some(t.priority);
                        params.estimate_minutes = t.estimate_minutes;
                        params.depends_on = Some(t.depends_on.iter().filter_map(|k| ids.get(k).copied()).collect());
                        let task = tasks.create(params, TaskActor::Manager).await?;
                        created.push(task.id);
                        ids.insert(t.key.clone(), task.id);
                        t.task_id = Some(task.id);
                        progressed = true;
                    }
                }
            }
            if !pending {
                return Ok(());
            }
            if !progressed {
                return Err(OrchestratorError::Invalid("unresolvable task dependencies".into()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Value {
        json!({"epic": {"title": "Payroll", "features": [
            {"title": "Core", "stories": [
                {"title": "Store", "acceptance_criteria": ["works"], "tasks": [
                    {"id": "a", "title": "Model", "role": "architect", "dependencies": []},
                    {"id": "b", "title": "Service", "role": "dev", "dependsOn": ["a", "zzz", "b"], "estimateMinutes": "45"}
                ]}
            ]},
            {"key": "f2", "title": "UI", "stories": [
                {"key": "s9", "title": "List", "acceptanceCriteria": [{"text": "shows rows"}], "tasks": [
                    {"key": "c", "title": "View", "role": "Frontend", "dependsOn": ["b"], "priority": "high"}
                ]}
            ]}
        ]}})
    }

    #[test]
    fn normalizes_a_model_breakdown() {
        let (epic, warnings) = normalize_pm(&doc()).unwrap();
        assert_eq!(epic.features.len(), 2);
        assert_eq!(epic.features[0].key, "f1");
        let t = &epic.features[0].stories[0].tasks;
        assert_eq!(t[0].role, orkestr_domain::AgentRole::Architect);
        assert_eq!(t[1].depends_on, vec!["a".to_owned()]);
        assert_eq!(t[1].estimate_minutes, Some(45));
        assert_eq!(epic.features[1].stories[0].acceptance_criteria, vec!["shows rows".to_owned()]);
        assert_eq!(epic.features[1].stories[0].tasks[0].priority, orkestr_domain::TaskPriority::High);
        assert!(warnings.iter().any(|w| w.contains("zzz")));
        assert_eq!(counts(&epic), (2, 2, 3));
    }

    #[test]
    fn validation_rejects_bad_edits() {
        let (epic, _) = normalize_pm(&doc()).unwrap();
        let mut cyc = epic.clone();
        cyc.features[0].stories[0].tasks[0].depends_on = vec!["c".into()];
        assert!(validate_epic(&cyc).unwrap_err().to_string().contains("cycle"));
        let mut dup = epic.clone();
        dup.features[1].stories[0].tasks[0].key = "a".into();
        assert!(validate_epic(&dup).is_err());
        let mut no_ac = epic.clone();
        no_ac.features[0].stories[0].acceptance_criteria.clear();
        assert!(validate_epic(&no_ac).is_err());
        let mut unknown = epic.clone();
        unknown.features[0].stories[0].tasks[1].depends_on = vec!["nope".into()];
        assert!(validate_epic(&unknown).is_err());
        let mut empty = epic.clone();
        empty.features[0].stories[0].tasks.clear();
        assert!(validate_epic(&empty).is_err());
        let mut no_title = epic;
        no_title.title = "  ".into();
        assert!(validate_epic(&no_title).is_err());
        assert!(normalize_pm(&json!({"epic": {"title": "x", "features": []}})).is_err());
    }
}
