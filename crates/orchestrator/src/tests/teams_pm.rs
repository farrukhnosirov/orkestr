//! Agent Teams (§6), the Manager → Team Lead → Executors → Reviewer
//! hierarchy, cross-project scoping (§68.27) and the AI Product Manager
//! (§9, §68.47) over the shared harness with the offline Mock provider.

use orkestr_domain::{
    AgentTemplateId, AgentTeamId, SecretScope, TaskId, TeamMember, TeamPolicies, TemplateSource,
};
use orkestr_protocol::methods::pm::{
    PmApproveParams, PmDraftParams, PmDraftStatus, PmListParams, PmUpdateParams,
};
use orkestr_protocol::methods::teams::{
    AssignTeamParams, CreateTeamParams, ListTeamsParams, TeamLeadStatus, TeamProjectPermissions, TeamView,
    UpdateTeamParams,
};

use super::*;
use crate::OrchestratorError;

async fn template(h: &H, role: AgentRole) -> AgentTemplateId {
    h.rt.templates().list(None, Some(TemplateSource::Builtin)).await.unwrap().into_iter().find(|t| t.role == role).unwrap().id
}

fn member(template_id: AgentTemplateId, lead: bool, count: u32) -> TeamMember {
    TeamMember { template_id, is_lead: lead, count, model: None }
}

fn create(name: &str, members: Vec<TeamMember>, policies: Option<TeamPolicies>) -> CreateTeamParams {
    CreateTeamParams { name: name.into(), kind: None, description: None, members, policies, project_id: None }
}

fn assign(team: AgentTeamId, project: ProjectId, perms: Option<TeamProjectPermissions>) -> AssignTeamParams {
    AssignTeamParams { team_id: team, project_id: project, plan_id: None, permissions: perms, unassign: None }
}

async fn other_project(h: &H) -> ProjectId {
    let p = Project::new("Other", now());
    h.meta.insert(&p).await.unwrap();
    p.id
}

async fn teams(h: &H) -> Vec<TeamView> {
    h.mgr.teams_list(ListTeamsParams { project_id: Some(h.project) }).await.unwrap().teams
}

#[tokio::test]
async fn builtin_teams_are_seeded_once_and_crud_is_validated() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    let list = teams(&h).await;
    let names: Vec<&str> = list.iter().map(|v| v.team.name.as_str()).collect();
    assert_eq!(names, ["ERP Team", "Web Team", "Mobile Team", "Security Team", "Rapid Prototype Team"]);
    assert_eq!(teams(&h).await.len(), 5, "seeding is idempotent");
    let erp = &list[0].team;
    assert!(erp.builtin && erp.policies.hierarchy);
    assert_eq!(erp.members.iter().filter(|m| m.is_lead).count(), 1);
    assert!(!list[4].team.policies.require_review, "rapid prototype skips review");

    let backend = template(&h, AgentRole::Backend).await;
    let reviewer = template(&h, AgentRole::Reviewer).await;
    let model = ModelRef { provider_id: ProviderId::new(), model: "m".into() };
    let mut m = member(backend, false, 2);
    m.model = Some(model.clone());
    let created = h.mgr.teams_create(create("Payments", vec![m, member(reviewer, false, 1)], None)).await.unwrap();
    assert!(!created.team.builtin);
    assert_eq!(created.team.members[0].model, Some(model));
    assert_eq!(teams(&h).await.len(), 6);

    // Validation: two leads, lead count, unknown template, bad bounds, hierarchy without a lead.
    let bad = |r: Result<TeamView, OrchestratorError>| assert!(matches!(r, Err(OrchestratorError::Invalid(_))), "{r:?}");
    bad(h.mgr.teams_create(create("X", vec![member(backend, true, 1), member(reviewer, true, 1)], None)).await);
    bad(h.mgr.teams_create(create("X", vec![member(backend, true, 2)], None)).await);
    bad(h.mgr.teams_create(create("X", vec![member(AgentTemplateId::new(), false, 1)], None)).await);
    bad(h.mgr.teams_create(create(" ", vec![member(backend, false, 1)], None)).await);
    let hier = TeamPolicies { hierarchy: true, ..TeamPolicies::default() };
    bad(h.mgr.teams_create(create("X", vec![member(backend, false, 1)], Some(hier.clone()))).await);
    let wide = TeamPolicies { max_fan_out: 99, ..TeamPolicies::default() };
    bad(h.mgr.teams_create(create("X", vec![member(backend, false, 1)], Some(wide))).await);

    // Update.
    let upd = UpdateTeamParams {
        team_id: created.team.id,
        name: Some("Payments Team".into()),
        kind: None,
        description: Some("Billing".into()),
        members: Some(vec![member(backend, true, 1), member(backend, false, 3), member(reviewer, false, 1)]),
        policies: Some(hier),
    };
    let updated = h.mgr.teams_update(upd).await.unwrap();
    assert_eq!(updated.team.name, "Payments Team");
    assert!(updated.team.policies.hierarchy);
    assert!(updated.team.updated_at.unwrap() >= created.team.created_at);

    // Assign / unassign; a project-owned team cannot serve another project.
    let a = h.mgr.teams_assign(assign(created.team.id, h.project, None)).await.unwrap();
    assert_eq!(a.assignments.len(), 1);
    assert!(!a.assignments[0].permissions.secrets, "secrets are denied by default");
    let mut un = assign(created.team.id, h.project, None);
    un.unassign = Some(true);
    assert!(h.mgr.teams_assign(un).await.unwrap().assignments.is_empty());
    let other = other_project(&h).await;
    let mut owned = create("Owned", vec![member(backend, false, 1)], None);
    owned.project_id = Some(other);
    let owned = h.mgr.teams_create(owned).await.unwrap();
    assert!(!teams(&h).await.iter().any(|v| v.team.id == owned.team.id), "other project's team is hidden");
    assert!(matches!(
        h.mgr.teams_assign(assign(owned.team.id, h.project, None)).await,
        Err(OrchestratorError::Forbidden(_))
    ));

    // Delete.
    h.mgr.teams_delete(created.team.id).await.unwrap();
    assert!(matches!(h.mgr.teams_get(created.team.id).await, Err(OrchestratorError::NotFound(_))));
    assert_eq!(teams(&h).await.len(), 5);
}

#[tokio::test]
async fn team_agents_never_see_other_projects_secrets() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    let other = other_project(&h).await;
    let team = teams(&h).await.into_iter().find(|v| v.team.name == "Security Team").unwrap().team.id;
    let user = h.sec.user();
    let secrets = &h.sec.secrets;
    let mine = secrets
        .set("DB_PASSWORD", SecretScope::Project { project_id: h.project }, zeroize::Zeroizing::new("a-secret".into()), user.clone())
        .await
        .unwrap()
        .id;
    let theirs = secrets
        .set("DB_PASSWORD", SecretScope::Project { project_id: other }, zeroize::Zeroizing::new("b-secret".into()), user.clone())
        .await
        .unwrap()
        .id;
    let global = secrets.set("PROVIDER_KEY", SecretScope::Global, zeroize::Zeroizing::new("g".into()), user).await.unwrap().id;
    let read = |project: ProjectId, secret| {
        let mgr = h.mgr.clone();
        async move { mgr.with_team_secret(team, project, None, secret, str::to_owned).await }
    };
    let forbidden = |r: Result<String, OrchestratorError>| assert!(matches!(r, Err(OrchestratorError::Forbidden(_))), "{r:?}");

    // Not assigned at all.
    forbidden(read(h.project, mine).await);
    // Assigned with the default scope: no secrets.
    h.mgr.teams_assign(assign(team, h.project, None)).await.unwrap();
    forbidden(read(h.project, mine).await);
    // Project A allows secrets, project B does not.
    let allow = TeamProjectPermissions { secrets: true, denied: vec![] };
    h.mgr.teams_assign(assign(team, h.project, Some(allow))).await.unwrap();
    h.mgr.teams_assign(assign(team, other, None)).await.unwrap();
    assert_eq!(read(h.project, mine).await.unwrap(), "a-secret");
    // Working in A, the team never reads B's secret (and vice versa), nor global keys.
    forbidden(read(h.project, theirs).await);
    forbidden(read(other, theirs).await);
    forbidden(read(other, mine).await);
    forbidden(read(h.project, global).await);
    // The agent-scope rules installed on team agents deny Secrets unless allowed.
    let rules = crate::teams::scope_rules(&TeamProjectPermissions::default());
    assert!(rules.iter().any(|r| r.capability == Capability::Secrets && r.effect == PermissionEffect::Deny));
    let narrowed = crate::teams::scope_rules(&TeamProjectPermissions { secrets: true, denied: vec![Capability::Network] });
    assert!(!narrowed.iter().any(|r| r.capability == Capability::Secrets));

    // A live team agent carries the scope: Secrets denied even in Full Auto,
    // and an `Allow` extra can never widen a grant. The agent holds its
    // first model call (gated) so its rules are observable while it runs.
    let title = "Inspect the scoped team agent";
    let model = h.mock_gated("mock-smart", &[title], 2);
    let mut extra = rules.clone();
    extra.push(PermissionRule {
        capability: Capability::Deploy,
        effect: PermissionEffect::Allow,
        resource: None,
        scope: PermissionScope::Global,
    });
    let params = orkestr_protocol::methods::agents::SpawnAgentParams {
        project_id: h.project,
        template_id: template(&h, AgentRole::Backend).await,
        task_id: None,
        new_task: Some(orkestr_protocol::methods::agents::NewTaskSpec {
            title: title.into(),
            description: None,
            acceptance_criteria: None,
            priority: None,
        }),
        model: Some(model),
        worktree_policy: Some(orkestr_domain::WorktreePolicy::SharedWorktree),
        name: None,
        max_turns: None,
        base_branch: None,
        approval_mode: Some(ApprovalMode::FullAuto),
        budget: None,
    };
    let extras = orkestr_agent_runtime::SpawnExtras {
        team_id: Some(team),
        parent_agent_id: None,
        rules: extra,
        rule_note: Some("team scope: Security Team".into()),
    };
    let (agent, task) = h.rt.spawn_with(params, extras).await.unwrap();
    assert_eq!(agent.team_id, Some(team));
    let ctx = CheckContext {
        project_id: Some(h.project),
        agent_id: Some(agent.id),
        task_id: Some(task.id),
        approval_mode: ApprovalMode::FullAuto,
        worktree_root: None,
        protected_environment: false,
    };
    let actor = orkestr_domain::Actor::Agent(agent.id);
    let d = h.sec.engine.check(&actor, Capability::Secrets, &Resource::None, &ctx);
    assert!(matches!(d, Decision::Deny(_)), "team agents cannot read secrets: {d:?}");
    let deploy = h.sec.engine.check(&actor, Capability::Deploy, &Resource::None, &ctx);
    assert!(!matches!(deploy, Decision::Allow), "extras only narrow: {deploy:?}");
    h.rt.stop(agent.id, orkestr_protocol::methods::agents::AgentStopMode::Cancel).await.unwrap();
}

fn subtask_ids(view: &TeamView) -> Vec<TaskId> {
    view.lead_runs[0].subtasks.iter().map(|s| s.task_id).collect()
}

#[tokio::test]
async fn a_team_lead_decomposes_spawns_executors_then_a_reviewer() {
    let h = harness(true, ApprovalMode::SemiAuto).await;
    h.mock("mock-smart");
    let lead = template(&h, AgentRole::Architect).await;
    let backend = template(&h, AgentRole::Backend).await;
    let qa = template(&h, AgentRole::Qa).await;
    let reviewer = template(&h, AgentRole::Reviewer).await;
    let policies = TeamPolicies { hierarchy: true, max_fan_out: 3, max_parallel: 2, ..TeamPolicies::default() };
    let team = h
        .mgr
        .teams_create(create(
            "Core Team",
            vec![member(lead, true, 1), member(backend, false, 2), member(qa, false, 1), member(reviewer, false, 1)],
            Some(policies),
        ))
        .await
        .unwrap()
        .team;
    let mut plan = h.custom_plan(vec![ptask("a", "Build the greeter feature", AgentRole::Backend, &[])]).await;
    plan.options.team_id = Some(team.id);
    h.mgr.store().save_plan(&plan).await.unwrap();
    let r = h.mgr.approve_plan(ApprovePlanParams { plan_id: plan.id, edits: None, start_run: Some(true) }).await.unwrap();
    let run = r.run.unwrap();
    let done = h
        .mgr
        .wait_run(run.id, Duration::from_secs(120), |r| final_status(r.status) || r.tasks.iter().any(|t| t.awaiting_review))
        .await
        .unwrap();
    let view = h.mgr.teams_get(team.id).await.unwrap();
    assert!(!view.lead_runs.is_empty(), "{done:#?}");
    // The lead's executors are spawned with the lead as parent, team id and scope.
    let lr = &view.lead_runs[0];
    let lead_agent = lr.lead_agent_id.unwrap();
    assert!(lr.subtasks.len() >= 2, "{lr:#?}");
    assert!(lr.subtasks.iter().all(|s| s.attempts <= 2), "executor attempts are bounded");
    for s in lr.subtasks.iter().filter(|s| s.agent_id.is_some()) {
        let a = h.rt.get(s.agent_id.unwrap()).await.unwrap();
        assert_eq!(a.team_id, Some(team.id));
        assert_eq!(a.parent_agent_id, Some(lead_agent), "level 2 below the Manager");
        assert_eq!(h.tasks.get(s.task_id).await.unwrap().parent_id, Some(done.tasks[0].task_id));
    }
    if !h.git || !node_available() {
        // Without git QA cannot see the implementation; without node its test
        // run fails: the lead then fails the delegated task within its bounds.
        eprintln!("git={} node={} - lead ended {:?}: {:?}", h.git, node_available(), lr.status, lr.status_reason);
        assert!(view.lead_runs.len() <= 2, "the run retries a failed delegation within its attempts");
        return;
    }
    assert_eq!(view.lead_runs.len(), 1, "{done:#?}");
    assert_eq!(lr.status, TeamLeadStatus::Completed, "{:?} {:#?}", lr.status_reason, lr.subtasks);
    assert_eq!(done.status, ManagerRunStatus::Completed, "{:?}", done.status_reason);
    assert_eq!(lr.depth, 1);
    // Two executor sub-tasks (implement → verify) and the reviewer.
    assert_eq!(lr.subtasks.len(), 3);
    assert!(lr.subtasks[2].is_review && !lr.subtasks[0].is_review);
    let parent = done.tasks[0].task_id;
    assert_eq!(lr.parent_task_id, parent);
    let ids = subtask_ids(&view);
    for id in &ids {
        let t = h.tasks.get(*id).await.unwrap();
        assert_eq!(t.parent_id, Some(parent), "sub-tasks are children of the delegated task");
        assert_eq!(t.status, TaskStatus::Completed);
    }
    assert_eq!(h.tasks.get(ids[1]).await.unwrap().dependencies, vec![ids[0]], "verify after implement");
    let review_deps = h.tasks.get(ids[2]).await.unwrap().dependencies;
    assert!(review_deps.contains(&ids[0]) && review_deps.contains(&ids[1]), "review after all executors");
    // Executors and the reviewer are team agents under the lead, scoped to the project.
    let lead_rec = h.rt.get(lead_agent).await.unwrap();
    assert_eq!(lead_rec.team_id, Some(team.id));
    assert_eq!(lead_rec.state, orkestr_domain::AgentState::Completed);
    for s in &lr.subtasks {
        let a = h.rt.get(s.agent_id.unwrap()).await.unwrap();
        assert_eq!(a.team_id, Some(team.id));
        assert_eq!(a.parent_agent_id, Some(lead_agent), "level 2 below the Manager");
    }
    // The delegated task is the run's one task: completed with the lead's summary.
    let parent_task = h.tasks.get(parent).await.unwrap();
    assert_eq!(parent_task.status, TaskStatus::Completed);
    assert!(parent_task.result.unwrap().success);
    // The automatic assignment created for the delegation keeps the restrictive scope.
    assert!(!view.assignments.iter().find(|a| a.project_id == h.project).unwrap().permissions.secrets);
}

#[tokio::test]
async fn a_lead_below_a_lead_is_refused() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    h.mock("mock-smart");
    let erp = teams(&h).await.into_iter().find(|v| v.team.name == "ERP Team").unwrap().team;
    let assignment = h.mgr.inner.ensure_assignment(&erp, h.project).await.unwrap();
    let task = h
        .tasks
        .create(
            orkestr_protocol::methods::tasks::CreateTaskParams {
                project_id: h.project,
                title: "Too deep".into(),
                ..Default::default()
            },
            orkestr_domain::TaskActor::User,
        )
        .await
        .unwrap();
    let d = crate::team_lead::Delegation {
        parent_task: task.id,
        run_id: None,
        budget_usd: None,
        approval_mode: ApprovalMode::SemiAuto,
        model: None,
        control: None,
        cancel: CancellationToken::new(),
    };
    let req = crate::teams::LeadRequest { team: erp, assignment, depth: 2 };
    let r = crate::team_lead::delegate(Arc::clone(&h.mgr.inner), req, d).await;
    assert!(matches!(r, Err(OrchestratorError::Invalid(_))), "a lead below a lead is refused: {r:?}");
}

#[tokio::test]
async fn product_manager_drafts_edits_and_approves_into_a_task_hierarchy() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    h.mock("mock-smart");
    let d = h
        .mgr
        .pm_draft(PmDraftParams { project_id: h.project, idea: "a payroll module for small shops".into(), options: None })
        .await
        .unwrap();
    assert_eq!(d.status, PmDraftStatus::Draft);
    assert_eq!(d.epic.features.len(), 2);
    let stories: Vec<_> = d.epic.features.iter().flat_map(|f| &f.stories).collect();
    assert_eq!(stories.len(), 3);
    assert!(stories.iter().all(|s| !s.acceptance_criteria.is_empty()));
    assert_eq!(stories.iter().map(|s| s.tasks.len()).sum::<usize>(), 4);
    assert!(h.tasks.list(Default::default()).await.unwrap().is_empty(), "a draft creates no tasks");

    // Edits are validated: a cycle is refused, a valid edit is stored.
    let mut cyc = d.epic.clone();
    cyc.features[0].stories[0].tasks[0].depends_on = vec!["t4".into()];
    assert!(matches!(
        h.mgr.pm_update(PmUpdateParams { draft_id: d.id.clone(), epic: cyc }).await,
        Err(OrchestratorError::Invalid(_))
    ));
    let mut epic = d.epic.clone();
    epic.features[0].stories[0].tasks[1].title = "Implement the payroll service API".into();
    epic.features[0].stories[0].acceptance_criteria.push("Totals are rounded to cents".into());
    let d = h.mgr.pm_update(PmUpdateParams { draft_id: d.id.clone(), epic }).await.unwrap();
    assert_eq!(h.mgr.pm_list(PmListParams { project_id: h.project, include_discarded: None }).await.unwrap().drafts.len(), 1);

    // Approve (no run): epic → features → stories → tasks with dependencies.
    let team = teams(&h).await.into_iter().find(|v| v.team.name == "Web Team").unwrap().team.id;
    let a = h
        .mgr
        .pm_approve(PmApproveParams { draft_id: d.id.clone(), start_run: Some(false), team_id: Some(team) })
        .await
        .unwrap();
    assert!(a.run.is_none());
    assert_eq!(a.draft.status, PmDraftStatus::Approved);
    assert_eq!(a.plan.tasks.len(), 4);
    assert_eq!(a.plan.options.team_id, Some(team), "the run is delegated to the team");
    assert_eq!(h.mgr.teams_get(team).await.unwrap().assignments[0].plan_id, Some(a.plan.id));
    let e = &a.draft.epic;
    let epic_task = h.tasks.get(e.task_id.unwrap()).await.unwrap();
    assert_eq!(epic_task.parent_id, None);
    for f in &e.features {
        let ft = h.tasks.get(f.task_id.unwrap()).await.unwrap();
        assert_eq!(ft.parent_id, e.task_id);
        for s in &f.stories {
            let st = h.tasks.get(s.task_id.unwrap()).await.unwrap();
            assert_eq!(st.parent_id, f.task_id);
            assert_eq!(st.acceptance_criteria.len(), s.acceptance_criteria.len());
            for t in &s.tasks {
                let tt = h.tasks.get(t.task_id.unwrap()).await.unwrap();
                assert_eq!(tt.parent_id, s.task_id);
            }
        }
    }
    let key_id = |k: &str| {
        e.features.iter().flat_map(|f| &f.stories).flat_map(|s| &s.tasks).find(|t| t.key == k).unwrap().task_id.unwrap()
    };
    assert_eq!(h.tasks.get(key_id("t4")).await.unwrap().dependencies, vec![key_id("t2")], "cross-feature dependency");
    assert_eq!(h.tasks.list(Default::default()).await.unwrap().len(), 1 + 2 + 3 + 4);
    let got = h.mgr.pm_get(&d.id).await.unwrap();
    assert_eq!(got.progress.unwrap().total, 4);
    // Decided drafts are frozen.
    assert!(matches!(
        h.mgr.pm_approve(PmApproveParams { draft_id: d.id.clone(), start_run: None, team_id: None }).await,
        Err(OrchestratorError::Conflict(_))
    ));
    assert!(matches!(h.mgr.pm_discard(&d.id).await, Err(OrchestratorError::Conflict(_))));

    // Discard another draft; it disappears from the default list.
    let d2 = h.mgr.pm_draft(PmDraftParams { project_id: h.project, idea: "a todo app".into(), options: None }).await.unwrap();
    assert_eq!(h.mgr.pm_discard(&d2.id).await.unwrap().status, PmDraftStatus::Discarded);
    assert_eq!(h.mgr.pm_list(PmListParams { project_id: h.project, include_discarded: None }).await.unwrap().drafts.len(), 1);
    let all = h.mgr.pm_list(PmListParams { project_id: h.project, include_discarded: Some(true) }).await.unwrap();
    assert_eq!(all.drafts.len(), 2);
    // Invalid ideas.
    assert!(h.mgr.pm_draft(PmDraftParams { project_id: h.project, idea: "  ".into(), options: None }).await.is_err());
}

#[tokio::test]
async fn an_approved_product_draft_runs_and_completes_its_containers() {
    let h = harness(false, ApprovalMode::SemiAuto).await;
    h.mock("mock-smart");
    let d = h
        .mgr
        .pm_draft(PmDraftParams { project_id: h.project, idea: "a greeter".into(), options: None })
        .await
        .unwrap();
    // Keep it small and offline-deterministic: one feature, one story, two tasks.
    let mut epic = d.epic.clone();
    epic.features.truncate(1);
    epic.features[0].stories.truncate(1);
    let d = h.mgr.pm_update(PmUpdateParams { draft_id: d.id.clone(), epic }).await.unwrap();
    let a = h.mgr.pm_approve(PmApproveParams { draft_id: d.id.clone(), start_run: Some(true), team_id: None }).await.unwrap();
    let run = a.run.unwrap();
    assert_eq!(a.draft.run_id, Some(run.id));
    let done = h.mgr.wait_run(run.id, Duration::from_secs(90), |r| final_status(r.status)).await.unwrap();
    assert_eq!(done.status, ManagerRunStatus::Completed, "{:?} {:#?}", done.status_reason, done.tasks);
    let got = h.mgr.pm_get(&d.id).await.unwrap();
    let p = got.progress.unwrap();
    assert_eq!((p.total, p.completed, p.failed), (2, 2, 0));
    for id in [got.epic.task_id, got.epic.features[0].task_id, got.epic.features[0].stories[0].task_id] {
        assert_eq!(h.tasks.get(id.unwrap()).await.unwrap().status, TaskStatus::Completed, "containers complete");
    }
}
