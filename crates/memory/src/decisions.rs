//! Decision log (§21): ADR-like entries with per-project numbers.

use orkestr_domain::{now, Actor, Decision, DecisionAlternative, DecisionId, DecisionStatus, ProjectId};
use orkestr_protocol::event::{DecisionRecordedPayload, EventKind};
use orkestr_protocol::methods::memory::{CreateDecisionParams, DecisionPatch};
use orkestr_protocol::NewEvent;
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::StorageError;

use crate::util::{clamp_limit, enum_name, ts_key};
use crate::validate::{self, MAX_ALTERNATIVES, MAX_LINKS};
use crate::{MemoryEngine, MemoryError, MemoryResult};

fn alternatives(values: Vec<DecisionAlternative>) -> MemoryResult<Vec<DecisionAlternative>> {
    if values.len() > MAX_ALTERNATIVES {
        return Err(MemoryError::Invalid(format!("at most {MAX_ALTERNATIVES} alternatives")));
    }
    values
        .into_iter()
        .map(|a| {
            Ok(DecisionAlternative {
                title: validate::title(&a.title)?,
                summary: validate::text("alternative summary", &a.summary)?,
                proposed_by: validate::source(a.proposed_by.as_deref())?,
                rejected_reason: match a.rejected_reason {
                    Some(r) if !r.trim().is_empty() => Some(validate::text("rejected reason", &r)?),
                    _ => None,
                },
            })
        })
        .collect()
}

fn non_empty(field: &str, value: &str) -> MemoryResult<String> {
    let v = validate::text(field, value)?;
    if v.is_empty() {
        return Err(MemoryError::Invalid(format!("{field} must not be empty")));
    }
    Ok(v)
}

impl MemoryEngine {
    /// Records a decision (next number in the project) and publishes `DecisionRecorded`.
    pub async fn create_decision(&self, p: CreateDecisionParams) -> MemoryResult<Decision> {
        self.ensure_project(p.project_id).await?;
        if p.task_ids.as_ref().is_some_and(|t| t.len() > MAX_LINKS) {
            return Err(MemoryError::Invalid(format!("at most {MAX_LINKS} related tasks")));
        }
        let t = now();
        let mut decision = Decision {
            id: DecisionId::new(),
            project_id: p.project_id,
            number: 0,
            title: validate::title(&p.title)?,
            context: validate::text("context", p.context.as_deref().unwrap_or_default())?,
            decision: non_empty("decision", &p.decision)?,
            alternatives: alternatives(p.alternatives.unwrap_or_default())?,
            reasoning_summary: validate::text("reasoning summary", p.reasoning_summary.as_deref().unwrap_or_default())?,
            consequences: validate::text("consequences", p.consequences.as_deref().unwrap_or_default())?,
            status: p.status.unwrap_or(DecisionStatus::Accepted),
            task_ids: dedup(p.task_ids.unwrap_or_default()),
            files: validate::files(&p.files.unwrap_or_default())?,
            decided_by: p.decided_by.unwrap_or(Actor::System),
            superseded_by: None,
            created_at: t,
            updated_at: Some(t),
        };
        let mut doc_template = decision.clone();
        let project = decision.project_id.to_string();
        let status = enum_name(&decision.status)?;
        let id = decision.id.to_string();
        let created = ts_key(&t);
        let number = self
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                let next: u32 = tx.query_row(
                    "SELECT COALESCE(MAX(number), 0) + 1 FROM decisions WHERE project_id = ?1",
                    [&project],
                    |r| r.get(0),
                )?;
                doc_template.number = next;
                let doc = serde_json::to_string(&doc_template)?;
                tx.execute(
                    "INSERT INTO decisions (id, project_id, number, status, created_at, updated_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                    params![id, project, next, status, created, doc],
                )?;
                tx.commit()?;
                Ok(next)
            })
            .await?;
        decision.number = number;
        tracing::info!(decision_id = %decision.id, number, "decision recorded");
        self.publish_decision(&decision).await?;
        Ok(decision)
    }

    async fn publish_decision(&self, d: &Decision) -> MemoryResult<()> {
        self.publish(
            NewEvent::new(EventKind::DecisionRecorded(DecisionRecordedPayload { decision: d.clone() }))
                .project(d.project_id),
        )
        .await
    }

    /// Fetches one decision.
    pub async fn get_decision(&self, id: DecisionId) -> MemoryResult<Decision> {
        let key = id.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM decisions WHERE id = ?1", [key], |r| r.get(0)).optional()?)
            })
            .await?;
        match doc {
            Some(d) => Ok(serde_json::from_str(&d).map_err(StorageError::from)?),
            None => Err(MemoryError::NotFound(format!("decision {id}"))),
        }
    }

    /// Lists decisions of a project, newest first.
    pub async fn list_decisions(
        &self,
        project: ProjectId,
        status: Option<DecisionStatus>,
        limit: Option<u32>,
    ) -> MemoryResult<Vec<Decision>> {
        let limit = clamp_limit(limit, 200, 1000);
        let status = status.map(|s| enum_name(&s)).transpose()?;
        let project = project.to_string();
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM decisions WHERE project_id = ?1 AND (?2 IS NULL OR status = ?2)
                     ORDER BY number DESC LIMIT ?3",
                )?;
                let rows = stmt.query_map(params![project, status, limit], |r| r.get(0))?.collect::<Result<_, _>>()?;
                Ok(rows)
            })
            .await?;
        docs.iter().map(|d| Ok(serde_json::from_str(d).map_err(StorageError::from)?)).collect()
    }

    /// Updates a decision and publishes `DecisionRecorded`.
    pub async fn update_decision(&self, id: DecisionId, patch: DecisionPatch) -> MemoryResult<Decision> {
        let mut d = self.get_decision(id).await?;
        if let Some(v) = patch.title {
            d.title = validate::title(&v)?;
        }
        if let Some(v) = patch.decision {
            d.decision = non_empty("decision", &v)?;
        }
        if let Some(v) = patch.context {
            d.context = validate::text("context", &v)?;
        }
        if let Some(v) = patch.alternatives {
            d.alternatives = alternatives(v)?;
        }
        if let Some(v) = patch.reasoning_summary {
            d.reasoning_summary = validate::text("reasoning summary", &v)?;
        }
        if let Some(v) = patch.consequences {
            d.consequences = validate::text("consequences", &v)?;
        }
        if let Some(v) = patch.task_ids {
            if v.len() > MAX_LINKS {
                return Err(MemoryError::Invalid(format!("at most {MAX_LINKS} related tasks")));
            }
            d.task_ids = dedup(v);
        }
        if let Some(v) = patch.files {
            d.files = validate::files(&v)?;
        }
        if let Some(s) = patch.status {
            d.status = s;
        }
        if let Some(by) = patch.superseded_by {
            if by == d.id {
                return Err(MemoryError::Invalid("a decision cannot supersede itself".into()));
            }
            let other = self.get_decision(by).await?;
            if other.project_id != d.project_id {
                return Err(MemoryError::Invalid("superseding decision belongs to another project".into()));
            }
            d.superseded_by = Some(by);
            if patch.status.is_none() {
                d.status = DecisionStatus::Superseded;
            }
        }
        let t = now();
        d.updated_at = Some(t);
        let doc = serde_json::to_string(&d).map_err(StorageError::from)?;
        let status = enum_name(&d.status)?;
        let key = d.id.to_string();
        let updated = ts_key(&t);
        let n = self
            .db
            .call(move |c| {
                Ok(c.execute(
                    "UPDATE decisions SET status = ?2, updated_at = ?3, doc = ?4 WHERE id = ?1",
                    params![key, status, updated, doc],
                )?)
            })
            .await?;
        if n == 0 {
            return Err(MemoryError::NotFound(format!("decision {id}")));
        }
        self.publish_decision(&d).await?;
        Ok(d)
    }
}

fn dedup<T: PartialEq>(values: Vec<T>) -> Vec<T> {
    let mut out: Vec<T> = Vec::with_capacity(values.len());
    for v in values {
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{engine, project};
    use orkestr_domain::TaskId;
    use orkestr_events::Received;

    fn params(pid: ProjectId, title: &str) -> CreateDecisionParams {
        CreateDecisionParams {
            project_id: pid,
            title: title.into(),
            decision: "Use SQLite FTS5".into(),
            context: Some("Need search".into()),
            alternatives: Some(vec![DecisionAlternative {
                title: "Tantivy".into(),
                summary: "Separate index".into(),
                proposed_by: Some("architect".into()),
                rejected_reason: Some("extra dependency".into()),
            }]),
            reasoning_summary: Some("Keep one database".into()),
            consequences: Some("Simple backups".into()),
            status: None,
            task_ids: Some(vec![]),
            files: Some(vec!["crates/memory/src/lib.rs".into()]),
            decided_by: None,
        }
    }

    #[tokio::test]
    async fn numbering_listing_and_events() {
        let (eng, bus) = engine().await;
        let mut sub = bus.subscribe_all();
        let p1 = project(&eng, "One").await;
        let p2 = project(&eng, "Two").await;
        let a = eng.create_decision(params(p1, "First")).await.unwrap();
        let b = eng.create_decision(params(p1, "Second")).await.unwrap();
        let c = eng.create_decision(params(p2, "Other")).await.unwrap();
        assert_eq!((a.number, b.number, c.number), (1, 2, 1));
        assert_eq!(a.status, DecisionStatus::Accepted);
        match sub.recv().await.unwrap() {
            Received::Event(e) => {
                assert_eq!(e.kind_name(), "DecisionRecorded");
                assert_eq!(e.project_id, Some(p1));
            }
            other => panic!("{other:?}"),
        }
        let list = eng.list_decisions(p1, None, None).await.unwrap();
        assert_eq!(list.iter().map(|d| d.number).collect::<Vec<_>>(), vec![2, 1]);
        assert_eq!(eng.get_decision(a.id).await.unwrap(), a);
        assert!(eng.list_decisions(p1, Some(DecisionStatus::Rejected), None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn supersede_and_validation() {
        let (eng, _bus) = engine().await;
        let p1 = project(&eng, "One").await;
        let p2 = project(&eng, "Two").await;
        let a = eng.create_decision(params(p1, "Old")).await.unwrap();
        let b = eng.create_decision(params(p1, "New")).await.unwrap();
        let foreign = eng.create_decision(params(p2, "Foreign")).await.unwrap();
        let tid = TaskId::new();
        let up = eng
            .update_decision(
                a.id,
                DecisionPatch { superseded_by: Some(b.id), task_ids: Some(vec![tid, tid]), ..DecisionPatch::default() },
            )
            .await
            .unwrap();
        assert_eq!(up.status, DecisionStatus::Superseded);
        assert_eq!(up.superseded_by, Some(b.id));
        assert_eq!(up.task_ids, vec![tid]);
        assert!(up.updated_at.unwrap() >= a.created_at);
        assert!(eng
            .update_decision(a.id, DecisionPatch { superseded_by: Some(a.id), ..DecisionPatch::default() })
            .await
            .is_err());
        assert!(eng
            .update_decision(a.id, DecisionPatch { superseded_by: Some(foreign.id), ..DecisionPatch::default() })
            .await
            .is_err());
        let mut bad = params(p1, "x");
        bad.decision = "  ".into();
        assert!(matches!(eng.create_decision(bad).await, Err(MemoryError::Invalid(_))));
        assert!(matches!(eng.create_decision(params(ProjectId::new(), "x")).await, Err(MemoryError::NotFound(_))));
        assert!(matches!(eng.get_decision(DecisionId::new()).await, Err(MemoryError::NotFound(_))));
    }
}
