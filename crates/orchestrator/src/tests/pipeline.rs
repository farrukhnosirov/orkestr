//! The quality pipeline and Self-Healing (§17, §68.15–16) end to end over the
//! shared harness: real task engine, agent runtime, git worktrees, reviews,
//! Merge Manager and the offline Mock (its `[mock:heal-once]` /
//! `[mock:heal-never]` scenarios). The quality runner is replaced by a
//! deterministic check over the worktree (the Mock's scripted bug fails the
//! "test"), so no toolchain is needed.

use std::path::Path;

use orkestr_domain::{AgentId, Task};
use orkestr_protocol::methods::pipeline::{
    HealingLimits, HealingOutcome, PipelineStageKind, PipelineStatus, SetPipelineConfigParams, StageStatus,
    TaskPipeline,
};
use orkestr_protocol::methods::quality::{CheckKind, GateReport, QualityRun, TestFailure};
use orkestr_providers::adapters::mock::HEAL_BUG_COMMENT;

use super::*;
use crate::PipelineChecks;

/// "Tests" fail while a `src/*.mjs` of the workspace carries the Mock's
/// scripted bug; no build check is configured.
struct WorktreeChecks {
    runs: Mutex<u32>,
}

#[async_trait]
impl PipelineChecks for WorktreeChecks {
    async fn run_checks(
        &self,
        project: &Project,
        task: &Task,
        _agent: Option<AgentId>,
        workspace: &Path,
        kinds: &[CheckKind],
        _cancel: &CancellationToken,
    ) -> Result<Option<QualityRun>, String> {
        if !kinds.contains(&CheckKind::Test) {
            return Ok(None);
        }
        *self.runs.lock() += 1;
        let mut failures = Vec::new();
        if let Ok(dir) = std::fs::read_dir(workspace.join("src")) {
            for e in dir.flatten() {
                let text = std::fs::read_to_string(e.path()).unwrap_or_default();
                if let Some(n) = text.lines().position(|l| l.contains("return `Hello, ${name}`")) {
                    assert!(text.contains(HEAL_BUG_COMMENT));
                    failures.push(TestFailure {
                        name: "greeter greets by name".into(),
                        message: "Expected values to be strictly equal: 'Hello, Orkestr' !== 'Hello, Orkestr!'".into(),
                        file: Some(format!("src/{}", e.file_name().to_string_lossy())),
                        line: Some(u32::try_from(n + 1).unwrap()),
                    });
                }
            }
        }
        let failed = !failures.is_empty();
        let run: QualityRun = serde_json::from_value(serde_json::json!({
            "id": format!("q{}", *self.runs.lock()),
            "projectId": project.id,
            "taskId": task.id,
            "target": { "type": "project" },
            "workspace": workspace.to_string_lossy(),
            "kinds": ["Test"],
            "status": if failed { "Failed" } else { "Passed" },
            "results": [{
                "checkId": "test", "kind": "Test", "toolchain": "Cargo", "label": "tests", "command": "npm test",
                "cwd": ".", "status": if failed { "Failed" } else { "Passed" }, "durationMs": 5,
                "passed": if failed { 0 } else { 1 }, "failed": u32::from(failed), "skipped": 0,
                "failures": failures, "diagnostics": [], "rawTail": "", "summary": if failed { "1 failed" } else { "1 passed" },
            }],
            "startedAt": now(),
            "durationMs": 5,
        }))
        .map_err(|e| e.to_string())?;
        Ok(Some(run))
    }

    async fn gates(&self, _project: &Project, _task: &Task) -> Option<GateReport> {
        None
    }
}

fn wire(h: &H) -> Arc<WorktreeChecks> {
    let checks = Arc::new(WorktreeChecks { runs: Mutex::new(0) });
    h.mgr.set_pipeline_checks(Arc::clone(&checks) as Arc<dyn PipelineChecks>);
    checks
}

async fn one_task_run(h: &H, title: &str) -> orkestr_protocol::methods::manager::ManagerRun {
    let plan = h.custom_plan(vec![ptask("a", title, AgentRole::Backend, &[])]).await;
    h.mgr
        .approve_plan(ApprovePlanParams { plan_id: plan.id, edits: None, start_run: Some(true) })
        .await
        .unwrap()
        .run
        .unwrap()
}

async fn pending_approval(h: &H) -> orkestr_domain::Approval {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let list = h
            .sec
            .approvals
            .list(&orkestr_security::ApprovalFilter {
                status: Some(ApprovalStatus::Pending),
                project_id: Some(h.project),
                limit: None,
            })
            .await
            .unwrap();
        if let Some(a) = list.into_iter().next() {
            return a;
        }
        assert!(tokio::time::Instant::now() < deadline, "no escalation approval appeared");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

async fn latest(h: &H, task: orkestr_domain::TaskId) -> TaskPipeline {
    h.mgr.pipeline(task).await.unwrap().pipeline.expect("a pipeline ran")
}

fn stage(p: &TaskPipeline, k: PipelineStageKind) -> &orkestr_protocol::methods::pipeline::PipelineStage {
    p.stage(k).unwrap()
}

#[tokio::test]
async fn failing_tests_are_healed_on_the_second_attempt_and_the_merge_is_proposed() {
    let h = harness(true, ApprovalMode::SemiAuto).await;
    if !h.git {
        eprintln!("git is not installed — the pipeline scenario needs worktrees");
        return;
    }
    h.mock("mock-smart");
    let checks = wire(&h);
    let merger = crate::merge::MergeManager::new(crate::merge::MergeDeps::from_manager(&h.mgr));
    merger.init().await.unwrap();
    h.mgr.set_merge_manager(merger.clone());
    let run = one_task_run(&h, "Implement the greeter module [mock:heal-once]").await;
    let done = h.mgr.wait_run(run.id, Duration::from_secs(120), |r| final_status(r.status)).await.unwrap();
    let task_id = done.tasks[0].task_id;
    let p = latest(&h, task_id).await;
    assert_eq!(done.status, ManagerRunStatus::Completed, "{:?} {p:#?}", done.status_reason);
    assert_eq!(p.status, PipelineStatus::Passed, "{p:#?}");
    // Implement → Test fails → analyse → fix (attempt 1) → Build → Test → Review → Security.
    assert_eq!(p.healing.len(), 1, "{p:#?}");
    let a = &p.healing[0];
    assert_eq!(a.outcome, HealingOutcome::Fixed);
    assert_eq!(a.analysis.stage, PipelineStageKind::Test);
    assert_eq!(a.analysis.failing_tests[0].name, "greeter greets by name");
    assert!(a.analysis.excerpts.iter().any(|e| e.file == "src/greeter.mjs" && e.text.contains("Hello, ${name}")));
    assert_eq!(*checks.runs.lock(), 2, "tests ran before and after the fix");
    assert_eq!(stage(&p, PipelineStageKind::Test).status, StageStatus::Passed);
    assert_eq!(stage(&p, PipelineStageKind::Test).runs, 2);
    assert_eq!(stage(&p, PipelineStageKind::Review).runs, 2, "the fix is reviewed again");
    assert_eq!(stage(&p, PipelineStageKind::Build).status, StageStatus::Skipped, "no build check configured");
    assert_eq!(stage(&p, PipelineStageKind::Security).status, StageStatus::Passed);
    assert_eq!(p.review_verdict, Some(orkestr_protocol::methods::reviews::ReviewVerdict::Approve));
    // The fix agent got the analysis as its briefing, on top of the author's branch.
    let fixer = a.agent_id.unwrap();
    let tr = h.rt.transcript(fixer, None).await.unwrap();
    let first = &tr.messages.iter().find(|m| m.text.contains("# Task:")).expect("task message").text;
    assert!(first.contains("# Self-healing fix (attempt 1 of 3)"), "{first}");
    assert!(first.contains("greeter greets by name"), "{first}");
    assert_ne!(Some(fixer), p.author_agent_id);
    let task = h.tasks.get(task_id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Completed);
    let fix_exec = h.rt.store().executions(fixer).await.unwrap();
    assert_eq!(fix_exec.len(), 1, "the fix attempt is recorded as an execution of its agent");
    assert_eq!(fix_exec[0].task_id, task_id);
    let wt = PathBuf::from(task.worktree.unwrap().path);
    assert!(!std::fs::read_to_string(wt.join("src/greeter.mjs")).unwrap().contains(HEAL_BUG_COMMENT));
    // Merge stage: a Merge Manager proposal for the fixed branch (Manual policy → awaiting approval).
    assert_eq!(stage(&p, PipelineStageKind::Merge).status, StageStatus::Waiting, "{p:#?}");
    let proposal = merger.get(p.merge_proposal_id.unwrap()).await.unwrap();
    assert_eq!(proposal.branches[0].branch, p.branch.clone().unwrap());
    // Run card + final report carry the pipeline.
    let rt = done.tasks[0].pipeline.as_ref().unwrap();
    assert_eq!((rt.status, rt.healing_attempts), (PipelineStatus::Passed, 1));
    let report = done.report.unwrap();
    assert_eq!(report.pipelines.len(), 1);
    assert_eq!(report.pipelines[0].healing_attempts, 1);
    assert!(report.summary.contains("### Quality pipeline"), "{}", report.summary);
    assert!(report.summary.contains("self-healing: 1 attempt(s)"), "{}", report.summary);
    assert!(report.merge_proposals[0].note.contains("proposed"), "{:?}", report.merge_proposals);
    assert_eq!(done.agents_spawned, 1, "fix agents belong to the pipeline, not the run's agent budget");
}

#[tokio::test]
async fn a_fix_that_changes_nothing_escalates_and_rejection_fails_the_task() {
    let h = harness(true, ApprovalMode::SemiAuto).await;
    if !h.git {
        eprintln!("git is not installed — the pipeline scenario needs worktrees");
        return;
    }
    h.mock("mock-smart");
    wire(&h);
    let run = one_task_run(&h, "Implement the greeter module [mock:heal-never]").await;
    let approval = pending_approval(&h).await;
    assert!(approval.action_summary.contains("Self-healing gave up"), "{}", approval.action_summary);
    let waiting = h.mgr.wait_run(run.id, Duration::from_secs(30), |r| r.tasks[0].awaiting_review).await.unwrap();
    let task_id = waiting.tasks[0].task_id;
    let p = latest(&h, task_id).await;
    assert_eq!(p.status, PipelineStatus::Escalated);
    assert!(p.status_reason.as_deref().unwrap().contains("no progress"), "{:?}", p.status_reason);
    assert_eq!(p.healing.len(), 1, "the repeated signature stops after one fix: {p:#?}");
    assert_eq!(p.healing[0].outcome, HealingOutcome::StillFailing);
    assert!(waiting.status_reason.unwrap_or_default().contains("review"));
    h.sec.approvals.resolve(approval.id, orkestr_security::ApprovalDecision::Reject, None, h.sec.user()).await.unwrap();
    let done = h.mgr.wait_run(run.id, Duration::from_secs(60), |r| final_status(r.status)).await.unwrap();
    assert_eq!(done.status, ManagerRunStatus::Failed, "{:?}", done.status_reason);
    let p = latest(&h, task_id).await;
    assert_eq!(p.status, PipelineStatus::Failed);
    assert!(p.status_reason.unwrap().contains("declined"));
    assert_eq!(h.tasks.get(task_id).await.unwrap().status, TaskStatus::Failed);
    assert_eq!(done.agents_spawned, 1, "no blind retry after the pipeline failed");
    assert_eq!(done.report.unwrap().pipelines[0].status, PipelineStatus::Failed);
}

#[tokio::test]
async fn attempt_limit_escalates_and_an_approval_grants_more_attempts() {
    let h = harness(true, ApprovalMode::SemiAuto).await;
    if !h.git {
        eprintln!("git is not installed — the pipeline scenario needs worktrees");
        return;
    }
    h.mock("mock-smart");
    wire(&h);
    let cfg = h
        .mgr
        .set_pipeline_config(SetPipelineConfigParams {
            project_id: h.project,
            enabled: None,
            stages: Some(vec![PipelineStageKind::Build, PipelineStageKind::Test]),
            overrides: None,
            healing: Some(HealingLimits { max_attempts: 1, stop_on_repeat: false, ..HealingLimits::default() }),
            merge: None,
            reset: None,
        })
        .await
        .unwrap();
    assert!(cfg.customized && !cfg.stages.contains(&PipelineStageKind::Review));
    let bad = h
        .mgr
        .set_pipeline_config(SetPipelineConfigParams {
            project_id: h.project,
            enabled: None,
            stages: None,
            overrides: None,
            healing: Some(HealingLimits { max_attempts: 99, ..HealingLimits::default() }),
            merge: None,
            reset: None,
        })
        .await;
    assert!(bad.is_err(), "limits are validated");
    let run = one_task_run(&h, "Implement the greeter module [mock:heal-never]").await;
    // Attempt 1 fails → the attempt limit (1) escalates.
    let first = pending_approval(&h).await;
    let task_id = h.mgr.get_run(run.id).await.unwrap().tasks[0].task_id;
    let p = latest(&h, task_id).await;
    assert_eq!(p.healing.len(), 1);
    assert!(p.status_reason.as_deref().unwrap().contains("1 of 1 fix attempt"), "{:?}", p.status_reason);
    assert_eq!(stage(&p, PipelineStageKind::Review).status, StageStatus::Skipped, "disabled stages are skipped");
    // The user grants more attempts: one more (escalating the model — only one is configured).
    h.sec.approvals.resolve(first.id, orkestr_security::ApprovalDecision::Approve, None, h.sec.user()).await.unwrap();
    let second = loop {
        let a = pending_approval(&h).await;
        if a.id != first.id {
            break a;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    };
    let p = latest(&h, task_id).await;
    assert_eq!(p.granted_attempts, 1);
    assert_eq!(p.healing.len(), 2, "{p:#?}");
    assert!(p.healing[1].note.as_deref().unwrap_or_default().contains("no other model"), "{:?}", p.healing[1]);
    assert!(p.status_reason.as_deref().unwrap().contains("2 of 2"), "{:?}", p.status_reason);
    h.sec.approvals.resolve(second.id, orkestr_security::ApprovalDecision::Reject, None, h.sec.user()).await.unwrap();
    let done = h.mgr.wait_run(run.id, Duration::from_secs(60), |r| final_status(r.status)).await.unwrap();
    assert_eq!(done.status, ManagerRunStatus::Failed);
    assert_eq!(latest(&h, task_id).await.status, PipelineStatus::Failed);
    // Reset restores the defaults.
    let reset = h
        .mgr
        .set_pipeline_config(SetPipelineConfigParams {
            project_id: h.project,
            enabled: None,
            stages: None,
            overrides: None,
            healing: None,
            merge: None,
            reset: Some(true),
        })
        .await
        .unwrap();
    assert!(!reset.customized && reset.healing.max_attempts == 3);
}

#[tokio::test]
async fn a_disabled_pipeline_keeps_the_phase1_acceptance() {
    let h = harness(true, ApprovalMode::SemiAuto).await;
    h.mock("mock-smart");
    wire(&h);
    h.mgr
        .set_pipeline_config(SetPipelineConfigParams {
            project_id: h.project,
            enabled: Some(false),
            stages: None,
            overrides: None,
            healing: None,
            merge: None,
            reset: None,
        })
        .await
        .unwrap();
    let run = one_task_run(&h, "Implement the greeter module [mock:heal-never]").await;
    let done = h.mgr.wait_run(run.id, Duration::from_secs(120), |r| final_status(r.status)).await.unwrap();
    assert_eq!(done.status, ManagerRunStatus::Completed, "{:?}", done.status_reason);
    assert!(h.mgr.pipeline(done.tasks[0].task_id).await.unwrap().pipeline.is_none());
}
