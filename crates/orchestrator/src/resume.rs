//! Session resume (§57): "Last session: Backend API completed; Frontend 70%;
//! QA found 2 issues; 1 approval pending; last snapshot …" — computed from
//! the task engine, plans, runs, approvals and snapshots of a project.

use orkestr_domain::{ApprovalStatus, ProjectId, TaskStatus};
use orkestr_protocol::methods::manager::{GoalProgress, ManagerResumeSummary, PlanStatus, SnapshotBrief};
use orkestr_protocol::methods::tasks::ListTasksParams;
use orkestr_security::ApprovalFilter;

use crate::error::OrchResult;
use crate::service::Manager;
use crate::util::one_line;

fn percent(done: u32, total: u32) -> u32 {
    if total == 0 {
        0
    } else {
        ((u64::from(done) * 100) / u64::from(total)) as u32
    }
}

impl Manager {
    /// The "Last session" summary of a project.
    #[allow(clippy::too_many_lines)]
    pub async fn resume_summary(&self, project_id: ProjectId) -> OrchResult<ManagerResumeSummary> {
        let inner = &self.inner;
        let project = inner.project(project_id).await?;
        let tasks = inner
            .deps
            .tasks
            .list(ListTasksParams { project_id: Some(project.id), ..ListTasksParams::default() })
            .await?;
        let count =
            |f: &dyn Fn(TaskStatus) -> bool| u32::try_from(tasks.iter().filter(|t| f(t.status)).count()).unwrap_or(0);
        let total = u32::try_from(tasks.len()).unwrap_or(u32::MAX);
        let completed = count(&|s| s == TaskStatus::Completed);
        let in_progress = count(&|s| {
            matches!(
                s,
                TaskStatus::Running
                    | TaskStatus::Waiting
                    | TaskStatus::Review
                    | TaskStatus::Testing
                    | TaskStatus::Recovering
            )
        });
        let failed = count(&|s| matches!(s, TaskStatus::Failed | TaskStatus::Interrupted));
        let blocked = count(&|s| s == TaskStatus::Blocked);
        let cancelled = count(&|s| s == TaskStatus::Cancelled);
        let effective_total = total - cancelled;

        // Goals (plans that produced tasks).
        let plans = inner.store.plans(project.id, None, 20).await?;
        let mut goals = Vec::new();
        for p in plans.iter().filter(|p| {
            matches!(
                p.status,
                PlanStatus::Approved
                    | PlanStatus::Running
                    | PlanStatus::Completed
                    | PlanStatus::Failed
                    | PlanStatus::Cancelled
            )
        }) {
            let ids: Vec<_> = p.tasks.iter().filter_map(|t| t.task_id).collect();
            if ids.is_empty() {
                continue;
            }
            let done = u32::try_from(
                tasks.iter().filter(|t| ids.contains(&t.id) && t.status == TaskStatus::Completed).count(),
            )
            .unwrap_or(0);
            let n = u32::try_from(
                tasks.iter().filter(|t| ids.contains(&t.id) && t.status != TaskStatus::Cancelled).count(),
            )
            .unwrap_or(0);
            goals.push(GoalProgress {
                plan_id: p.id,
                goal: one_line(&p.goal, 140),
                completed: done,
                total: n,
                percent: percent(done, n),
            });
            if goals.len() >= 5 {
                break;
            }
        }

        // Issues.
        let mut issues: Vec<String> = tasks
            .iter()
            .filter(|t| matches!(t.status, TaskStatus::Failed | TaskStatus::Interrupted))
            .take(8)
            .map(|t| {
                format!(
                    "\"{}\" is {:?}{}",
                    one_line(&t.title, 100),
                    t.status,
                    t.result.as_ref().map(|r| format!(": {}", one_line(&r.summary, 140))).unwrap_or_default()
                )
            })
            .collect();
        let review = tasks.iter().filter(|t| matches!(t.status, TaskStatus::Review | TaskStatus::Testing)).count();
        if review > 0 {
            issues.push(format!("{review} task(s) wait for review"));
        }
        let active_run = inner.active_run(project.id).await?;
        if let Some(r) = active_run
            .as_ref()
            .filter(|r| r.status == orkestr_protocol::methods::manager::ManagerRunStatus::Interrupted)
        {
            issues.push(format!("the run for \"{}\" was interrupted — resume it to continue", one_line(&r.goal, 100)));
        }
        let approvals = inner
            .deps
            .security
            .approvals
            .list(&ApprovalFilter {
                status: Some(ApprovalStatus::Pending),
                project_id: Some(project.id),
                limit: Some(200),
            })
            .await
            .map(|l| u32::try_from(l.len()).unwrap_or(0))
            .unwrap_or(0);
        let last_snapshot = inner
            .deps
            .snapshots
            .list(project.id, None, None, 1)
            .await
            .ok()
            .and_then(|l| l.into_iter().next())
            .map(|s| SnapshotBrief { id: s.id, label: s.label, created_at: s.created_at });
        let last_task_at = tasks.iter().map(|t| t.updated_at).max();
        let last_msg_at = inner.store.last_message_at(project.id).await?;
        let last_activity_at = match (last_task_at, last_msg_at) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };

        let pct = percent(completed, effective_total);
        let is_empty = total == 0 && last_msg_at.is_none();
        let mut parts: Vec<String> = Vec::new();
        if effective_total > 0 {
            parts.push(format!("{completed} of {effective_total} task(s) completed ({pct}%)"));
        }
        if in_progress > 0 {
            parts.push(format!("{in_progress} in progress"));
        }
        if failed > 0 {
            parts.push(format!("{failed} need attention"));
        }
        if approvals > 0 {
            parts.push(format!("{approvals} approval(s) pending"));
        }
        let headline = if is_empty {
            "No previous session in this project yet.".to_owned()
        } else if parts.is_empty() {
            "Last session: no open work.".to_owned()
        } else {
            format!("Last session: {}.", parts.join("; "))
        };
        let mut lines: Vec<String> = goals
            .iter()
            .map(|g| {
                if g.completed == g.total && g.total > 0 {
                    format!("{} — completed", g.goal)
                } else {
                    format!("{} — {}% ({}/{})", g.goal, g.percent, g.completed, g.total)
                }
            })
            .collect();
        if let Some(r) = &active_run {
            lines.push(format!(
                "Manager run: {:?}{}",
                r.status,
                r.status_reason.as_ref().map(|s| format!(" — {s}")).unwrap_or_default()
            ));
        }
        lines.extend(issues.iter().cloned());
        if let Some(s) = &last_snapshot {
            lines.push(format!("Last snapshot: {} ({})", s.label, s.created_at.format("%Y-%m-%d %H:%M UTC")));
        }
        Ok(ManagerResumeSummary {
            project_id: project.id,
            headline,
            lines,
            tasks_total: total,
            tasks_completed: completed,
            tasks_in_progress: in_progress,
            tasks_failed: failed,
            tasks_blocked: blocked,
            tasks_cancelled: cancelled,
            percent_complete: pct,
            goals,
            issues,
            pending_approvals: approvals,
            last_snapshot,
            active_run,
            last_activity_at,
            is_empty,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn percent_rounds_down() {
        assert_eq!(super::percent(2, 3), 66);
        assert_eq!(super::percent(0, 0), 0);
        assert_eq!(super::percent(4, 4), 100);
    }
}
