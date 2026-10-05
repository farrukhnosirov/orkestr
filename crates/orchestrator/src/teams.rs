//! Agent Teams (§6) and cross-project team agents (§68.27).
//!
//! * Persistent teams: members (template refs × count, model override, at
//!   most one lead) and default policies (hierarchy on/off, fan-out,
//!   parallelism, attempts, review, approval cap, budget, time limit). Five
//!   built-in teams (ERP, Web, Mobile, Security, Rapid Prototype) are seeded
//!   on first use from the built-in agent templates.
//! * Assignment of a team to projects (and optionally to one Manager plan:
//!   the Manager delegates that plan's run to the team). One team may serve
//!   many projects; each assignment carries its own permission scope, which
//!   becomes agent-scope `Deny` rules on every spawn of a team agent in that
//!   project — agents of a team never see secrets of another project, and
//!   only see the secrets of a project whose assignment explicitly allows it.
//! * Routing for the Manager's run loop: a plan delegated to a team spawns
//!   the matching team member for each task, or — with the hierarchy policy
//!   and a lead — hands the whole task to the team lead
//!   ([`crate::team_lead`]).

use orkestr_agent_runtime::SpawnExtras;
use orkestr_domain::{
    now, AgentRole, AgentTeam, AgentTeamId, AgentTemplateId, ApprovalMode, Capability, ModelRef, PermissionEffect,
    PermissionRule, PermissionScope, ProjectId, SecretRefId, SecretScope, TeamKind, TeamMember, TeamPolicies,
    TemplateSource,
};
use orkestr_protocol::event::{EventKind, TeamDeletedPayload, TeamLeadRunPayload, TeamPayload};
use orkestr_protocol::methods::manager::ManagerPlan;
use orkestr_protocol::methods::teams::{
    AssignTeamParams, CreateTeamParams, DeleteTeamResult, ListTeamsParams, ListTeamsResult, TeamAssignment,
    TeamLeadRun, TeamLeadStatus, TeamProjectPermissions, TeamView, UpdateTeamParams, MAX_FAN_OUT_LIMIT,
    MAX_MEMBER_COUNT, MAX_TEAM_DESCRIPTION_LEN, MAX_TEAM_MEMBERS, MAX_TEAM_NAME_LEN,
};
use orkestr_protocol::NewEvent;
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError};

use crate::error::{OrchResult, OrchestratorError};
use crate::service::{Inner, Manager};

/// Tables of teams, assignments, lead runs and AI Product Manager drafts
/// (Phase 2; applied after `orchestrator_0001_manager`).
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "orchestrator_0003_teams_pm",
    owner: "orchestrator",
    up: "CREATE TABLE agent_teams (
            id          TEXT PRIMARY KEY NOT NULL,
            name        TEXT NOT NULL,
            builtin     INTEGER NOT NULL DEFAULT 0,
            project_id  TEXT REFERENCES projects(id) ON DELETE CASCADE,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );
         CREATE TABLE team_assignments (
            team_id     TEXT NOT NULL REFERENCES agent_teams(id) ON DELETE CASCADE,
            project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            plan_id     TEXT,
            assigned_at TEXT NOT NULL,
            doc         TEXT NOT NULL,
            PRIMARY KEY (team_id, project_id)
         );
         CREATE INDEX team_assignments_project ON team_assignments(project_id);
         CREATE INDEX team_assignments_plan ON team_assignments(plan_id);
         CREATE TABLE team_lead_runs (
            id             TEXT PRIMARY KEY NOT NULL,
            team_id        TEXT NOT NULL,
            project_id     TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            parent_task_id TEXT NOT NULL,
            status         TEXT NOT NULL,
            created_at     TEXT NOT NULL,
            updated_at     TEXT NOT NULL,
            doc            TEXT NOT NULL
         );
         CREATE INDEX team_lead_runs_team ON team_lead_runs(team_id, created_at);
         CREATE INDEX team_lead_runs_status ON team_lead_runs(status);
         CREATE TABLE team_meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
         CREATE TABLE pm_drafts (
            id          TEXT PRIMARY KEY NOT NULL,
            project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            status      TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );
         CREATE INDEX pm_drafts_project ON pm_drafts(project_id, created_at);",
    down: "DROP INDEX IF EXISTS pm_drafts_project;
           DROP TABLE IF EXISTS pm_drafts;
           DROP TABLE IF EXISTS team_meta;
           DROP INDEX IF EXISTS team_lead_runs_status;
           DROP INDEX IF EXISTS team_lead_runs_team;
           DROP TABLE IF EXISTS team_lead_runs;
           DROP INDEX IF EXISTS team_assignments_plan;
           DROP INDEX IF EXISTS team_assignments_project;
           DROP TABLE IF EXISTS team_assignments;
           DROP TABLE IF EXISTS agent_teams;",
}];

fn status_str(s: TeamLeadStatus) -> String {
    format!("{s:?}")
}

fn parse_doc<T: serde::de::DeserializeOwned>(doc: &str) -> OrchResult<T> {
    serde_json::from_str(doc).map_err(|e| OrchestratorError::Storage(StorageError::Json(e)))
}

/// Persistence of teams, assignments and lead runs (cheap to clone).
#[derive(Debug, Clone)]
pub struct TeamStore {
    db: Db,
}

impl TeamStore {
    /// Wraps a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Whether the built-in teams were seeded once.
    pub async fn seeded(&self) -> OrchResult<bool> {
        Ok(self
            .db
            .call(|c| {
                Ok(c.query_row("SELECT value FROM team_meta WHERE key = 'seeded'", [], |r| r.get::<_, String>(0))
                    .optional()?)
            })
            .await?
            .is_some())
    }

    /// Stores built-in teams and the seeded flag atomically.
    pub async fn seed(&self, teams: Vec<AgentTeam>) -> OrchResult<bool> {
        let docs =
            teams.iter().map(|t| serde_json::to_string(t).map(|d| (t.clone(), d))).collect::<Result<Vec<_>, _>>()?;
        let inserted = self
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                let done: Option<String> =
                    tx.query_row("SELECT value FROM team_meta WHERE key = 'seeded'", [], |r| r.get(0)).optional()?;
                if done.is_some() {
                    return Ok(false);
                }
                for (t, doc) in &docs {
                    tx.execute(
                        "INSERT OR IGNORE INTO agent_teams (id, name, builtin, project_id, created_at, updated_at, doc)
                         VALUES (?1, ?2, 1, NULL, ?3, ?3, ?4)",
                        params![t.id.to_string(), t.name, t.created_at.to_rfc3339(), doc],
                    )?;
                }
                tx.execute("INSERT INTO team_meta (key, value) VALUES ('seeded', ?1)", [now().to_rfc3339()])?;
                tx.commit()?;
                Ok(true)
            })
            .await?;
        Ok(inserted)
    }

    /// Inserts or replaces a team.
    pub async fn save(&self, t: &AgentTeam) -> OrchResult<()> {
        let doc = serde_json::to_string(t)?;
        let t = t.clone();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO agent_teams (id, name, builtin, project_id, created_at, updated_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(id) DO UPDATE SET name = excluded.name, updated_at = excluded.updated_at,
                        doc = excluded.doc",
                    params![
                        t.id.to_string(),
                        t.name,
                        i64::from(t.builtin),
                        t.project_id.map(|p| p.to_string()),
                        t.created_at.to_rfc3339(),
                        t.updated_at.unwrap_or(t.created_at).to_rfc3339(),
                        doc
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// One team.
    pub async fn get(&self, id: AgentTeamId) -> OrchResult<AgentTeam> {
        let sid = id.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM agent_teams WHERE id = ?1", [sid], |r| r.get(0)).optional()?)
            })
            .await?;
        parse_doc(&doc.ok_or_else(|| OrchestratorError::NotFound(format!("team {id}")))?)
    }

    /// All teams: built-ins first, then by name.
    pub async fn list(&self) -> OrchResult<Vec<AgentTeam>> {
        let docs: Vec<String> = self
            .db
            .call(|c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM agent_teams ORDER BY builtin DESC, created_at ASC, name COLLATE NOCASE",
                )?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        docs.iter().map(|d| parse_doc(d)).collect()
    }

    /// Deletes a team (assignments cascade).
    pub async fn delete(&self, id: AgentTeamId) -> OrchResult<bool> {
        let sid = id.to_string();
        let n = self.db.call(move |c| Ok(c.execute("DELETE FROM agent_teams WHERE id = ?1", [sid])?)).await?;
        Ok(n > 0)
    }

    /// Inserts or replaces an assignment; a plan is executed by one team only.
    pub async fn save_assignment(&self, a: &TeamAssignment) -> OrchResult<()> {
        let doc = serde_json::to_string(a)?;
        let a = a.clone();
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                if let Some(plan) = a.plan_id {
                    // Drop the plan from other teams' assignments.
                    let others: Vec<(String, String, String)> = {
                        let mut stmt = tx.prepare(
                            "SELECT team_id, project_id, doc FROM team_assignments WHERE plan_id = ?1 AND team_id != ?2",
                        )?;
                        let rows = stmt.query_map(params![plan.to_string(), a.team_id.to_string()], |r| {
                            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                        })?;
                        rows.collect::<Result<Vec<_>, _>>()?
                    };
                    for (team, project, doc) in others {
                        let mut other: TeamAssignment = serde_json::from_str(&doc)?;
                        other.plan_id = None;
                        tx.execute(
                            "UPDATE team_assignments SET plan_id = NULL, doc = ?3 WHERE team_id = ?1 AND project_id = ?2",
                            params![team, project, serde_json::to_string(&other)?],
                        )?;
                    }
                }
                tx.execute(
                    "INSERT INTO team_assignments (team_id, project_id, plan_id, assigned_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(team_id, project_id) DO UPDATE SET plan_id = excluded.plan_id, doc = excluded.doc",
                    params![
                        a.team_id.to_string(),
                        a.project_id.to_string(),
                        a.plan_id.map(|p| p.to_string()),
                        a.assigned_at.to_rfc3339(),
                        doc
                    ],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Removes an assignment.
    pub async fn delete_assignment(&self, team: AgentTeamId, project: ProjectId) -> OrchResult<bool> {
        let (t, p) = (team.to_string(), project.to_string());
        let n = self
            .db
            .call(move |c| {
                Ok(c.execute("DELETE FROM team_assignments WHERE team_id = ?1 AND project_id = ?2", params![t, p])?)
            })
            .await?;
        Ok(n > 0)
    }

    async fn assignments_where(&self, sql: &'static str, arg: String) -> OrchResult<Vec<TeamAssignment>> {
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(sql)?;
                let rows = stmt.query_map([arg], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        docs.iter().map(|d| parse_doc(d)).collect()
    }

    /// Assignments of a team.
    pub async fn assignments(&self, team: AgentTeamId) -> OrchResult<Vec<TeamAssignment>> {
        self.assignments_where(
            "SELECT doc FROM team_assignments WHERE team_id = ?1 ORDER BY assigned_at",
            team.to_string(),
        )
        .await
    }

    /// Assignments in a project.
    pub async fn project_assignments(&self, project: ProjectId) -> OrchResult<Vec<TeamAssignment>> {
        self.assignments_where(
            "SELECT doc FROM team_assignments WHERE project_id = ?1 ORDER BY assigned_at",
            project.to_string(),
        )
        .await
    }

    /// The assignment executing a plan.
    pub async fn plan_assignment(&self, plan: orkestr_domain::ManagerPlanId) -> OrchResult<Option<TeamAssignment>> {
        Ok(self
            .assignments_where("SELECT doc FROM team_assignments WHERE plan_id = ?1", plan.to_string())
            .await?
            .into_iter()
            .next())
    }

    /// One assignment.
    pub async fn assignment(&self, team: AgentTeamId, project: ProjectId) -> OrchResult<Option<TeamAssignment>> {
        Ok(self.assignments(team).await?.into_iter().find(|a| a.project_id == project))
    }

    /// Inserts or replaces a lead run.
    pub async fn save_lead(&self, l: &TeamLeadRun) -> OrchResult<()> {
        let doc = serde_json::to_string(l)?;
        let l = l.clone();
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO team_lead_runs (id, team_id, project_id, parent_task_id, status, created_at, updated_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(id) DO UPDATE SET status = excluded.status, updated_at = excluded.updated_at,
                        doc = excluded.doc",
                    params![
                        l.id,
                        l.team_id.to_string(),
                        l.project_id.to_string(),
                        l.parent_task_id.to_string(),
                        status_str(l.status),
                        l.created_at.to_rfc3339(),
                        l.updated_at.to_rfc3339(),
                        doc
                    ],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Lead runs of a team, newest first.
    pub async fn leads(&self, team: AgentTeamId, limit: u32) -> OrchResult<Vec<TeamLeadRun>> {
        let t = team.to_string();
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt =
                    c.prepare("SELECT doc FROM team_lead_runs WHERE team_id = ?1 ORDER BY created_at DESC LIMIT ?2")?;
                let rows = stmt.query_map(params![t, i64::from(limit)], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        docs.iter().map(|d| parse_doc(d)).collect()
    }

    /// Lead runs that are not final (any team).
    pub async fn active_leads(&self) -> OrchResult<Vec<TeamLeadRun>> {
        let docs: Vec<String> = self
            .db
            .call(|c| {
                let mut stmt =
                    c.prepare("SELECT doc FROM team_lead_runs WHERE status IN ('Planning', 'Executing', 'Reviewing')")?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        docs.iter().map(|d| parse_doc(d)).collect()
    }

    /// One lead run.
    pub async fn lead(&self, id: &str) -> OrchResult<TeamLeadRun> {
        let sid = id.to_owned();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM team_lead_runs WHERE id = ?1", [sid], |r| r.get(0)).optional()?)
            })
            .await?;
        parse_doc(&doc.ok_or_else(|| OrchestratorError::NotFound(format!("team lead run {id}")))?)
    }
}

// ---------------------------------------------------------------------------
// validation
// ---------------------------------------------------------------------------

/// Validates policies (every bound is finite and in range).
pub fn validate_policies(p: &TeamPolicies) -> OrchResult<()> {
    let bad = |m: &str| Err(OrchestratorError::Invalid(m.to_owned()));
    if !(1..=MAX_FAN_OUT_LIMIT).contains(&p.max_fan_out) {
        return bad(&format!("maxFanOut must be 1..={MAX_FAN_OUT_LIMIT}"));
    }
    if !(1..=8).contains(&p.max_parallel) {
        return bad("maxParallel must be 1..=8");
    }
    if !(1..=5).contains(&p.max_attempts) {
        return bad("maxAttempts must be 1..=5");
    }
    if !(1..=1440).contains(&p.time_limit_mins) {
        return bad("timeLimitMins must be 1..=1440");
    }
    if p.budget_usd.is_some_and(|b| !b.is_finite() || b < 0.0) {
        return bad("budgetUsd must be a non-negative number");
    }
    Ok(())
}

/// Validates the static shape of a team (templates are checked by the caller).
pub fn validate_shape(
    name: &str,
    description: &str,
    members: &[TeamMember],
    policies: &TeamPolicies,
) -> OrchResult<()> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > MAX_TEAM_NAME_LEN {
        return Err(OrchestratorError::Invalid(format!("the team name must be 1..={MAX_TEAM_NAME_LEN} characters")));
    }
    if description.chars().count() > MAX_TEAM_DESCRIPTION_LEN {
        return Err(OrchestratorError::Invalid(format!(
            "the description must be at most {MAX_TEAM_DESCRIPTION_LEN} characters"
        )));
    }
    if members.is_empty() || members.len() > MAX_TEAM_MEMBERS {
        return Err(OrchestratorError::Invalid(format!("a team needs 1..={MAX_TEAM_MEMBERS} members")));
    }
    if members.iter().any(|m| m.count == 0 || m.count > MAX_MEMBER_COUNT) {
        return Err(OrchestratorError::Invalid(format!("member count must be 1..={MAX_MEMBER_COUNT}")));
    }
    let leads = members.iter().filter(|m| m.is_lead).count();
    if leads > 1 {
        return Err(OrchestratorError::Invalid("a team has at most one lead".into()));
    }
    if members.iter().any(|m| m.is_lead && m.count != 1) {
        return Err(OrchestratorError::Invalid("the lead slot has exactly one instance".into()));
    }
    validate_policies(policies)?;
    if policies.hierarchy && (leads == 0 || members.iter().all(|m| m.is_lead)) {
        return Err(OrchestratorError::Invalid(
            "the hierarchy policy needs a lead and at least one executor member".into(),
        ));
    }
    Ok(())
}

/// Agent-scope rules for a team agent in one project: `Deny` for every
/// denied capability, and `Deny Secrets` unless the assignment allows them.
#[must_use]
pub fn scope_rules(perms: &TeamProjectPermissions) -> Vec<PermissionRule> {
    let mut caps: Vec<Capability> = perms.denied.clone();
    if !perms.secrets {
        caps.push(Capability::Secrets);
    }
    caps.sort_unstable();
    caps.dedup();
    caps.into_iter()
        .map(|capability| PermissionRule {
            capability,
            effect: PermissionEffect::Deny,
            resource: None,
            // Replaced by the agent's own scope at spawn.
            scope: PermissionScope::Global,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// routing (Manager run loop)
// ---------------------------------------------------------------------------

/// The team lead a task is delegated to.
#[derive(Debug, Clone)]
pub struct LeadRequest {
    /// Team.
    pub team: AgentTeam,
    /// Assignment in the task's project (permission scope).
    pub assignment: TeamAssignment,
    /// Depth of the lead below the Manager (1).
    pub depth: u32,
}

/// The team member that executes a task.
#[derive(Debug, Clone)]
pub struct MemberRoute {
    /// Template to spawn.
    pub template_id: AgentTemplateId,
    /// Model override of the member.
    pub model: Option<ModelRef>,
    /// Team scope (team id + narrowing rules).
    pub extras: SpawnExtras,
    /// Approval-mode cap of the team.
    pub approval_cap: Option<ApprovalMode>,
}

/// How a plan task is executed when its plan is delegated to a team.
#[derive(Debug, Clone)]
pub enum TeamRoute {
    /// Hierarchy: the team lead decomposes it.
    Lead(LeadRequest),
    /// One team member does it.
    Member(MemberRoute),
}

/// The best member slot for `role`: same role, else the first executor
/// (non-lead, non-reviewer), else any non-lead member.
#[must_use]
pub fn pick_member<'a>(
    members: &'a [(TeamMember, AgentRole)],
    role: &AgentRole,
) -> Option<&'a (TeamMember, AgentRole)> {
    members
        .iter()
        .find(|(m, r)| !m.is_lead && r == role)
        .or_else(|| members.iter().find(|(m, r)| !m.is_lead && !matches!(r, AgentRole::Reviewer)))
        .or_else(|| members.iter().find(|(m, _)| !m.is_lead))
}

impl Inner {
    pub(crate) fn team_store(&self) -> TeamStore {
        TeamStore::new(self.deps.db.clone())
    }

    /// Members with the role of their template (unknown templates skipped).
    pub(crate) async fn member_roles(&self, team: &AgentTeam) -> Vec<(TeamMember, AgentRole)> {
        let mut out = Vec::new();
        for m in &team.members {
            match self.deps.agents.templates().get(m.template_id).await {
                Ok(t) => out.push((m.clone(), t.role)),
                Err(e) => {
                    tracing::warn!(team = %team.id, template = %m.template_id, error = %e, "team member template missing")
                }
            }
        }
        out
    }

    /// The assignment of `team` in `project`, created with the default
    /// (restrictive) permissions when the Manager delegates to a team that
    /// was not assigned yet.
    pub(crate) async fn ensure_assignment(&self, team: &AgentTeam, project: ProjectId) -> OrchResult<TeamAssignment> {
        let store = self.team_store();
        if let Some(a) = store.assignment(team.id, project).await? {
            return Ok(a);
        }
        if team.project_id.is_some_and(|p| p != project) {
            return Err(OrchestratorError::Forbidden(format!("team \"{}\" belongs to another project", team.name)));
        }
        let a = TeamAssignment {
            team_id: team.id,
            project_id: project,
            plan_id: None,
            permissions: TeamProjectPermissions::default(),
            assigned_at: now(),
        };
        store.save_assignment(&a).await?;
        Ok(a)
    }

    /// The team a plan is delegated to: the request option, else the team
    /// assigned to the plan.
    pub(crate) async fn plan_team(&self, plan: &ManagerPlan) -> OrchResult<Option<AgentTeam>> {
        let store = self.team_store();
        let id = match plan.options.team_id {
            Some(id) => Some(id),
            None => store.plan_assignment(plan.id).await?.map(|a| a.team_id),
        };
        match id {
            Some(id) => match store.get(id).await {
                Ok(t) => Ok(Some(t)),
                Err(OrchestratorError::NotFound(_)) => {
                    tracing::warn!(team = %id, plan = %plan.id, "the plan's team was deleted; running without it");
                    Ok(None)
                }
                Err(e) => Err(e),
            },
            None => Ok(None),
        }
    }

    /// Routing of one plan task (`None` = the plan has no team).
    pub(crate) async fn team_route(
        &self,
        plan: &ManagerPlan,
        project: ProjectId,
        role: &AgentRole,
    ) -> OrchResult<Option<TeamRoute>> {
        let Some(team) = self.plan_team(plan).await? else { return Ok(None) };
        let assignment = self.ensure_assignment(&team, project).await?;
        let members = self.member_roles(&team).await;
        let has_lead = members.iter().any(|(m, _)| m.is_lead);
        let has_exec = members.iter().any(|(m, r)| !m.is_lead && !matches!(r, AgentRole::Reviewer));
        if team.policies.hierarchy && has_lead && has_exec {
            return Ok(Some(TeamRoute::Lead(LeadRequest { team, assignment, depth: 1 })));
        }
        let Some((member, _)) = pick_member(&members, role) else {
            return Err(OrchestratorError::Invalid(format!("team \"{}\" has no executor member", team.name)));
        };
        Ok(Some(TeamRoute::Member(MemberRoute {
            template_id: member.template_id,
            model: member.model.clone(),
            extras: SpawnExtras {
                team_id: Some(team.id),
                parent_agent_id: None,
                rules: scope_rules(&assignment.permissions),
                rule_note: Some(format!("team scope: {}", team.name)),
            },
            approval_cap: team.policies.approval_mode,
        })))
    }

    pub(crate) async fn publish_team(&self, view: &TeamView) {
        self.deps.publish(NewEvent::new(EventKind::TeamUpdated(TeamPayload { team: view.clone() }))).await;
    }

    pub(crate) async fn save_lead(&self, l: &TeamLeadRun) {
        if let Err(e) = self.team_store().save_lead(l).await {
            tracing::error!(lead = %l.id, error = %e, "failed to persist a team lead run");
            return;
        }
        self.deps
            .publish(
                NewEvent::new(EventKind::TeamLeadRunUpdated(TeamLeadRunPayload { lead: l.clone() }))
                    .project(l.project_id)
                    .task(l.parent_task_id),
            )
            .await;
    }

    async fn team_view(&self, team: AgentTeam) -> OrchResult<TeamView> {
        let store = self.team_store();
        let assignments = store.assignments(team.id).await?;
        let lead_runs = store.leads(team.id, 10).await?;
        Ok(TeamView { team, assignments, lead_runs })
    }

    /// Seeds the built-in teams once (idempotent).
    pub(crate) async fn ensure_teams_seeded(&self) -> OrchResult<()> {
        let store = self.team_store();
        if store.seeded().await? {
            return Ok(());
        }
        let builtins = self.deps.agents.templates().list(None, Some(TemplateSource::Builtin)).await?;
        let by_role = |role: AgentRole| builtins.iter().find(|t| t.role == role).map(|t| t.id);
        let slot = |role: AgentRole, count: u32, lead: bool| {
            by_role(role).map(|template_id| TeamMember { template_id, is_lead: lead, count, model: None })
        };
        let team = |name: &str, kind: TeamKind, description: &str, slots: Vec<Option<TeamMember>>, policies| {
            let at = now();
            AgentTeam {
                id: AgentTeamId::new(),
                name: name.to_owned(),
                kind,
                description: description.to_owned(),
                members: slots.into_iter().flatten().collect(),
                project_id: None,
                created_at: at,
                policies,
                builtin: true,
                updated_at: Some(at),
            }
        };
        let hier = TeamPolicies { hierarchy: true, ..TeamPolicies::default() };
        let teams = vec![
            team(
                "ERP Team",
                TeamKind::Erp,
                "Business modules (accounting, payroll, inventory): an architect lead splits each feature for backend and database engineers; QA tests, a reviewer checks.",
                vec![
                    slot(AgentRole::Architect, 1, true),
                    slot(AgentRole::Backend, 2, false),
                    slot(AgentRole::Database, 1, false),
                    slot(AgentRole::Qa, 1, false),
                    slot(AgentRole::Reviewer, 1, false),
                ],
                hier.clone(),
            ),
            team(
                "Web Team",
                TeamKind::Web,
                "Web applications: frontend and backend engineers with a UI/UX designer under an architect lead; QA and a reviewer close each feature.",
                vec![
                    slot(AgentRole::Architect, 1, true),
                    slot(AgentRole::Frontend, 2, false),
                    slot(AgentRole::Backend, 1, false),
                    slot(AgentRole::UiUx, 1, false),
                    slot(AgentRole::Qa, 1, false),
                    slot(AgentRole::Reviewer, 1, false),
                ],
                hier.clone(),
            ),
            team(
                "Mobile Team",
                TeamKind::Mobile,
                "Mobile apps: client and API engineers under an architect lead, with QA and review.",
                vec![
                    slot(AgentRole::Architect, 1, true),
                    slot(AgentRole::Frontend, 2, false),
                    slot(AgentRole::Backend, 1, false),
                    slot(AgentRole::Qa, 1, false),
                    slot(AgentRole::Reviewer, 1, false),
                ],
                hier,
            ),
            team(
                "Security Team",
                TeamKind::Security,
                "Security audits and fixes: security reviewers and a researcher; every result is reviewed.",
                vec![
                    slot(AgentRole::Security, 2, false),
                    slot(AgentRole::Researcher, 1, false),
                    slot(AgentRole::Reviewer, 1, false),
                ],
                TeamPolicies { approval_mode: Some(ApprovalMode::Ask), ..TeamPolicies::default() },
            ),
            team(
                "Rapid Prototype Team",
                TeamKind::RapidPrototype,
                "Fast prototypes: a backend and a frontend engineer, no review step.",
                vec![slot(AgentRole::Backend, 1, false), slot(AgentRole::Frontend, 1, false)],
                TeamPolicies { require_review: false, max_parallel: 2, ..TeamPolicies::default() },
            ),
        ];
        let teams: Vec<AgentTeam> = teams.into_iter().filter(|t| !t.members.is_empty()).collect();
        if store.seed(teams).await? {
            tracing::info!("built-in agent teams seeded");
        }
        Ok(())
    }

    async fn check_templates(&self, members: &[TeamMember]) -> OrchResult<()> {
        for m in members {
            if let Err(e) = self.deps.agents.templates().get(m.template_id).await {
                return Err(OrchestratorError::Invalid(format!("member template {}: {e}", m.template_id)));
            }
        }
        Ok(())
    }
}

/// Final lead statuses never change again; a lead that was active when the
/// core stopped is marked failed (its parent task is interrupted by the task
/// engine's crash recovery and retried when the run resumes).
pub(crate) async fn recover_leads(inner: &Inner) {
    let store = inner.team_store();
    let Ok(active) = store.active_leads().await else { return };
    for mut l in active {
        l.status = TeamLeadStatus::Failed;
        l.status_reason = Some("Orkestr stopped while the team lead was working — resume the run to retry".into());
        l.updated_at = now();
        l.finished_at = Some(now());
        inner.save_lead(&l).await;
    }
}

impl Manager {
    /// `teams.list`.
    pub async fn teams_list(&self, p: ListTeamsParams) -> OrchResult<ListTeamsResult> {
        self.inner.ensure_teams_seeded().await?;
        let store = self.inner.team_store();
        let mut out = Vec::new();
        let assigned: Vec<AgentTeamId> = match p.project_id {
            Some(pid) => store.project_assignments(pid).await?.into_iter().map(|a| a.team_id).collect(),
            None => Vec::new(),
        };
        for team in store.list().await? {
            if let Some(pid) = p.project_id {
                let visible = team.project_id.is_none() || team.project_id == Some(pid) || assigned.contains(&team.id);
                if !visible {
                    continue;
                }
            }
            out.push(self.inner.team_view(team).await?);
        }
        Ok(ListTeamsResult { teams: out })
    }

    /// `teams.get`.
    pub async fn teams_get(&self, id: AgentTeamId) -> OrchResult<TeamView> {
        self.inner.ensure_teams_seeded().await?;
        let team = self.inner.team_store().get(id).await?;
        self.inner.team_view(team).await
    }

    /// `teams.create`.
    pub async fn teams_create(&self, p: CreateTeamParams) -> OrchResult<TeamView> {
        self.inner.ensure_teams_seeded().await?;
        let description = p.description.unwrap_or_default().trim().to_owned();
        let policies = p.policies.unwrap_or_default();
        validate_shape(&p.name, &description, &p.members, &policies)?;
        self.inner.check_templates(&p.members).await?;
        if let Some(pid) = p.project_id {
            self.inner.project(pid).await?;
        }
        let at = now();
        let name = p.name.trim().to_owned();
        let team = AgentTeam {
            id: AgentTeamId::new(),
            kind: p.kind.unwrap_or_else(|| TeamKind::Custom(name.clone())),
            name,
            description,
            members: p.members,
            project_id: p.project_id,
            created_at: at,
            policies,
            builtin: false,
            updated_at: Some(at),
        };
        self.inner.team_store().save(&team).await?;
        let view = self.inner.team_view(team).await?;
        self.inner.publish_team(&view).await;
        tracing::info!(team = %view.team.id, "agent team created");
        Ok(view)
    }

    /// `teams.update`.
    pub async fn teams_update(&self, p: UpdateTeamParams) -> OrchResult<TeamView> {
        let store = self.inner.team_store();
        let mut team = store.get(p.team_id).await?;
        if let Some(n) = p.name {
            team.name = n.trim().to_owned();
        }
        if let Some(k) = p.kind {
            team.kind = k;
        }
        if let Some(d) = p.description {
            team.description = d.trim().to_owned();
        }
        if let Some(m) = p.members {
            self.inner.check_templates(&m).await?;
            team.members = m;
        }
        if let Some(pol) = p.policies {
            team.policies = pol;
        }
        validate_shape(&team.name, &team.description, &team.members, &team.policies)?;
        team.updated_at = Some(now());
        store.save(&team).await?;
        let view = self.inner.team_view(team).await?;
        self.inner.publish_team(&view).await;
        Ok(view)
    }

    /// `teams.delete` — refused while one of the team's leads works.
    pub async fn teams_delete(&self, id: AgentTeamId) -> OrchResult<DeleteTeamResult> {
        let store = self.inner.team_store();
        store.get(id).await?;
        if store.active_leads().await?.iter().any(|l| l.team_id == id) {
            return Err(OrchestratorError::Conflict(
                "a lead of this team is working — cancel its Manager run first".into(),
            ));
        }
        store.delete(id).await?;
        self.inner.deps.publish(NewEvent::new(EventKind::TeamDeleted(TeamDeletedPayload { team_id: id }))).await;
        Ok(DeleteTeamResult { team_id: id })
    }

    /// `teams.assign` — assign (or unassign) a team to a project and plan.
    pub async fn teams_assign(&self, p: AssignTeamParams) -> OrchResult<TeamView> {
        let store = self.inner.team_store();
        let team = store.get(p.team_id).await?;
        self.inner.project(p.project_id).await?;
        if p.unassign == Some(true) {
            if store.active_leads().await?.iter().any(|l| l.team_id == team.id && l.project_id == p.project_id) {
                return Err(OrchestratorError::Conflict("a lead of this team works in the project".into()));
            }
            store.delete_assignment(team.id, p.project_id).await?;
        } else {
            if team.project_id.is_some_and(|tp| tp != p.project_id) {
                return Err(OrchestratorError::Forbidden(format!("team \"{}\" belongs to another project", team.name)));
            }
            if let Some(plan_id) = p.plan_id {
                let plan = self.inner.store.plan(plan_id).await?;
                if plan.project_id != p.project_id {
                    return Err(OrchestratorError::Invalid("the plan belongs to another project".into()));
                }
            }
            let existing = store.assignment(team.id, p.project_id).await?;
            let perms =
                p.permissions.unwrap_or_else(|| existing.as_ref().map(|a| a.permissions.clone()).unwrap_or_default());
            if perms.denied.len() > orkestr_domain::Capability::ALL.len() {
                return Err(OrchestratorError::Invalid("too many denied capabilities".into()));
            }
            let a = TeamAssignment {
                team_id: team.id,
                project_id: p.project_id,
                plan_id: p.plan_id.or_else(|| existing.as_ref().and_then(|a| a.plan_id)),
                permissions: perms,
                assigned_at: existing.as_ref().map_or_else(now, |a| a.assigned_at),
            };
            store.save_assignment(&a).await?;
        }
        let view = self.inner.team_view(team).await?;
        self.inner.publish_team(&view).await;
        Ok(view)
    }

    /// Resolves a secret for an agent of `team` working in `project` and
    /// hands the value to `use_value` (it never leaves this call). Allowed
    /// only for a project-scoped secret **of that project** whose assignment
    /// grants `secrets`; global secrets (provider keys) and other projects'
    /// secrets are always refused (§67, §68.27). Audited by the vault.
    pub async fn with_team_secret<R>(
        &self,
        team: AgentTeamId,
        project: ProjectId,
        agent: Option<orkestr_domain::AgentId>,
        secret: SecretRefId,
        use_value: impl FnOnce(&str) -> R,
    ) -> OrchResult<R> {
        let store = self.inner.team_store();
        let assignment = store
            .assignment(team, project)
            .await?
            .ok_or_else(|| OrchestratorError::Forbidden("the team is not assigned to this project".into()))?;
        let secrets = &self.inner.deps.security.secrets;
        let r = secrets
            .get_ref(secret)
            .await
            .map_err(|e| OrchestratorError::Internal(e.to_string()))?
            .ok_or_else(|| OrchestratorError::NotFound(format!("secret {secret}")))?;
        match r.scope {
            SecretScope::Project { project_id } | SecretScope::Environment { project_id, .. }
                if project_id == project => {}
            SecretScope::Global => {
                return Err(OrchestratorError::Forbidden("team agents never read global secrets".into()));
            }
            _ => {
                return Err(OrchestratorError::Forbidden(
                    "the secret belongs to another project — team agents never see other projects' secrets".into(),
                ));
            }
        }
        if !assignment.permissions.secrets {
            return Err(OrchestratorError::Forbidden(
                "the team's assignment to this project does not allow secrets".into(),
            ));
        }
        let actor = agent.map_or(orkestr_domain::Actor::Manager, orkestr_domain::Actor::Agent);
        let value = secrets
            .resolve(secret, actor, orkestr_security::AccessContext { project_id: Some(project) })
            .await
            .map_err(|e| OrchestratorError::Forbidden(e.to_string()))?;
        Ok(use_value(&value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(lead: bool, count: u32) -> TeamMember {
        TeamMember { template_id: AgentTemplateId::new(), is_lead: lead, count, model: None }
    }

    #[test]
    fn shape_validation() {
        let p = TeamPolicies::default();
        validate_shape("Web", "", &[member(false, 2)], &p).unwrap();
        assert!(validate_shape(" ", "", &[member(false, 1)], &p).is_err());
        assert!(validate_shape("x", "", &[], &p).is_err());
        assert!(validate_shape("x", "", &[member(true, 1), member(true, 1)], &p).is_err(), "two leads");
        assert!(validate_shape("x", "", &[member(true, 2)], &p).is_err(), "lead count");
        assert!(validate_shape("x", "", &[member(false, 9)], &p).is_err(), "count bound");
        let hier = TeamPolicies { hierarchy: true, ..TeamPolicies::default() };
        assert!(validate_shape("x", "", &[member(false, 1)], &hier).is_err(), "hierarchy needs a lead");
        assert!(validate_shape("x", "", &[member(true, 1)], &hier).is_err(), "hierarchy needs executors");
        validate_shape("x", "", &[member(true, 1), member(false, 1)], &hier).unwrap();
        let wide = TeamPolicies { max_fan_out: 50, ..TeamPolicies::default() };
        assert!(validate_shape("x", "", &[member(false, 1)], &wide).is_err(), "fan-out bound");
        let budget = TeamPolicies { budget_usd: Some(f64::NAN), ..TeamPolicies::default() };
        assert!(validate_policies(&budget).is_err());
    }

    #[test]
    fn scope_rules_deny_secrets_by_default_and_never_allow() {
        let r = scope_rules(&TeamProjectPermissions::default());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].capability, Capability::Secrets);
        assert_eq!(r[0].effect, PermissionEffect::Deny);
        let r = scope_rules(&TeamProjectPermissions {
            secrets: true,
            denied: vec![Capability::Network, Capability::Network],
        });
        assert_eq!(r.iter().map(|r| r.capability).collect::<Vec<_>>(), vec![Capability::Network]);
        assert!(r.iter().all(|r| r.effect == PermissionEffect::Deny));
    }

    #[test]
    fn member_picking() {
        let lead = (member(true, 1), AgentRole::Architect);
        let be = (member(false, 1), AgentRole::Backend);
        let qa = (member(false, 1), AgentRole::Qa);
        let rv = (member(false, 1), AgentRole::Reviewer);
        let ms = vec![lead, rv.clone(), be.clone(), qa.clone()];
        assert_eq!(pick_member(&ms, &AgentRole::Qa).unwrap().1, AgentRole::Qa);
        assert_eq!(pick_member(&ms, &AgentRole::Frontend).unwrap().1, AgentRole::Backend, "first executor");
        assert_eq!(pick_member(&ms, &AgentRole::Architect).unwrap().1, AgentRole::Backend, "never the lead");
        assert_eq!(pick_member(&[rv], &AgentRole::Backend).unwrap().1, AgentRole::Reviewer, "last resort");
    }
}
