//! Memory items: CRUD, FTS5 search, effective rules.

use std::collections::BTreeMap;

use orkestr_domain::{now, MemoryItem, MemoryItemId, MemoryKind, MemoryScope, ProjectId};
use orkestr_protocol::event::{EventKind, MemoryUpdatedPayload};
use orkestr_protocol::methods::memory::{
    CreateMemoryParams, EffectiveRule, ListMemoryParams, MemoryPatch, MemorySearchHit, RuleOrigin, SearchMemoryParams,
};
use orkestr_protocol::NewEvent;
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::StorageError;

use crate::fts::{self, Combine};
use crate::util::{clamp_limit, enum_name, sql_in_list, ts_key};
use crate::{validate, MemoryEngine, MemoryError, MemoryResult};

/// Kinds treated as rules by default in [`MemoryEngine::effective_rules`].
pub const DEFAULT_RULE_KINDS: &[MemoryKind] = &[
    MemoryKind::Convention,
    MemoryKind::Preference,
    MemoryKind::QualityStandard,
    MemoryKind::ProviderPreference,
    MemoryKind::Infrastructure,
    MemoryKind::Architecture,
    MemoryKind::Requirement,
    MemoryKind::Mistake,
];

/// Relevance multiplier of project items over global items in search.
const PROJECT_BOOST: f64 = 1.25;

struct ScopeCols {
    scope: &'static str,
    project_id: Option<String>,
    agent_id: Option<String>,
}

fn scope_cols(scope: &MemoryScope) -> ScopeCols {
    match scope {
        MemoryScope::Global => ScopeCols { scope: "Global", project_id: None, agent_id: None },
        MemoryScope::Project { project_id } => {
            ScopeCols { scope: "Project", project_id: Some(project_id.to_string()), agent_id: None }
        }
        MemoryScope::Agent { agent_id } => {
            ScopeCols { scope: "Agent", project_id: None, agent_id: Some(agent_id.to_string()) }
        }
    }
}

fn row_doc(doc: &str) -> Result<MemoryItem, StorageError> {
    Ok(serde_json::from_str(doc)?)
}

fn event_for(item: &MemoryItem, deleted: bool) -> NewEvent {
    let ev = NewEvent::new(EventKind::MemoryUpdated(MemoryUpdatedPayload { item: item.clone(), deleted }));
    match item.scope {
        MemoryScope::Project { project_id } => ev.project(project_id),
        MemoryScope::Agent { agent_id } => ev.agent(agent_id),
        MemoryScope::Global => ev,
    }
}

impl MemoryEngine {
    async fn check_scope(&self, scope: &MemoryScope) -> MemoryResult<()> {
        if let MemoryScope::Project { project_id } = scope {
            self.ensure_project(*project_id).await?;
        }
        Ok(())
    }

    async fn write_item(&self, item: &MemoryItem, insert: bool) -> MemoryResult<()> {
        let cols = scope_cols(&item.scope);
        let kind = enum_name(&item.kind)?;
        let doc = serde_json::to_string(item).map_err(StorageError::from)?;
        let id = item.id.to_string();
        let title = item.title.clone();
        let body = item.content.clone();
        let tags = item.tags.join(" ");
        let key = item.key.clone();
        let pinned = item.pinned;
        let created = ts_key(&item.created_at);
        let updated = ts_key(&item.updated_at);
        let changed = self
            .db
            .call(move |c| {
                let n = if insert {
                    c.execute(
                        "INSERT INTO memory_items (id, scope, project_id, agent_id, kind, title, body, tags, rule_key,
                                                   pinned, created_at, updated_at, doc)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                        params![
                            id,
                            cols.scope,
                            cols.project_id,
                            cols.agent_id,
                            kind,
                            title,
                            body,
                            tags,
                            key,
                            pinned,
                            created,
                            updated,
                            doc
                        ],
                    )?
                } else {
                    c.execute(
                        "UPDATE memory_items SET kind = ?2, title = ?3, body = ?4, tags = ?5, rule_key = ?6,
                                pinned = ?7, updated_at = ?8, doc = ?9 WHERE id = ?1",
                        params![id, kind, title, body, tags, key, pinned, updated, doc],
                    )?
                };
                Ok(n)
            })
            .await?;
        if changed == 0 {
            return Err(MemoryError::NotFound(format!("memory item {}", item.id)));
        }
        Ok(())
    }

    /// Creates a memory item and publishes `MemoryUpdated`.
    pub async fn create_item(&self, p: CreateMemoryParams) -> MemoryResult<MemoryItem> {
        self.check_scope(&p.scope).await?;
        let t = now();
        let item = MemoryItem {
            id: MemoryItemId::new(),
            scope: p.scope,
            kind: p.kind,
            title: validate::title(&p.title)?,
            content: validate::body(&p.content)?,
            tags: validate::tags(p.tags.as_deref().unwrap_or_default())?,
            source: validate::source(Some(p.source.as_deref().unwrap_or("user")))?,
            pinned: p.pinned.unwrap_or(false),
            key: validate::key(p.key.as_deref())?,
            created_at: t,
            updated_at: t,
        };
        self.write_item(&item, true).await?;
        tracing::info!(memory_id = %item.id, kind = ?item.kind, "memory item created");
        self.publish(event_for(&item, false)).await?;
        Ok(item)
    }

    /// Fetches one item.
    pub async fn get_item(&self, id: MemoryItemId) -> MemoryResult<MemoryItem> {
        let key = id.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM memory_items WHERE id = ?1", [key], |r| r.get(0)).optional()?)
            })
            .await?;
        match doc {
            Some(d) => Ok(row_doc(&d)?),
            None => Err(MemoryError::NotFound(format!("memory item {id}"))),
        }
    }

    /// Applies a patch and publishes `MemoryUpdated`.
    pub async fn update_item(&self, id: MemoryItemId, patch: MemoryPatch) -> MemoryResult<MemoryItem> {
        let mut item = self.get_item(id).await?;
        if let Some(k) = patch.kind {
            item.kind = k;
        }
        if let Some(t) = patch.title {
            item.title = validate::title(&t)?;
        }
        if let Some(c) = patch.content {
            item.content = validate::body(&c)?;
        }
        if let Some(tags) = patch.tags {
            item.tags = validate::tags(&tags)?;
        }
        if let Some(s) = patch.source {
            item.source = validate::source(Some(&s))?;
        }
        if let Some(p) = patch.pinned {
            item.pinned = p;
        }
        if let Some(k) = patch.key {
            item.key = validate::key(Some(&k))?;
        }
        item.updated_at = now();
        self.write_item(&item, false).await?;
        self.publish(event_for(&item, false)).await?;
        Ok(item)
    }

    /// Deletes an item and publishes `MemoryUpdated { deleted: true }`.
    pub async fn delete_item(&self, id: MemoryItemId) -> MemoryResult<MemoryItem> {
        let item = self.get_item(id).await?;
        let key = id.to_string();
        let n = self.db.call(move |c| Ok(c.execute("DELETE FROM memory_items WHERE id = ?1", [key])?)).await?;
        if n == 0 {
            return Err(MemoryError::NotFound(format!("memory item {id}")));
        }
        tracing::info!(memory_id = %id, "memory item deleted");
        self.publish(event_for(&item, true)).await?;
        Ok(item)
    }

    /// Lists items of one scope: pinned first, then most recently updated.
    pub async fn list_items(&self, p: ListMemoryParams) -> MemoryResult<Vec<MemoryItem>> {
        let cols = scope_cols(&p.scope);
        let limit = clamp_limit(p.limit, 500, 2000) as usize;
        let kinds = match p.kinds.as_deref() {
            Some(k) if !k.is_empty() => Some(sql_in_list(k)?),
            _ => None,
        };
        let pinned_only = p.pinned_only.unwrap_or(false);
        let tag = p.tag.map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty());
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let kind_sql = kinds.map(|k| format!(" AND kind IN {k}")).unwrap_or_default();
                let sql = format!(
                    "SELECT doc FROM memory_items
                     WHERE scope = ?1 AND project_id IS ?2 AND agent_id IS ?3 AND (?4 = 0 OR pinned = 1){kind_sql}
                     ORDER BY pinned DESC, updated_at DESC, seq DESC"
                );
                let mut stmt = c.prepare(&sql)?;
                let rows = stmt
                    .query_map(params![cols.scope, cols.project_id, cols.agent_id, pinned_only], |r| r.get(0))?
                    .collect::<Result<Vec<String>, _>>()?;
                Ok(rows)
            })
            .await?;
        let mut out = Vec::new();
        for d in docs {
            let item = row_doc(&d)?;
            if let Some(tag) = &tag {
                if !item.tags.iter().any(|t| t.to_lowercase() == *tag) {
                    continue;
                }
            }
            out.push(item);
            if out.len() == limit {
                break;
            }
        }
        Ok(out)
    }

    /// FTS5 search (bm25: title ×3, tags ×2, body ×1). All words must match;
    /// when nothing does, any word may match.
    pub async fn search(&self, p: SearchMemoryParams) -> MemoryResult<Vec<MemorySearchHit>> {
        let limit = clamp_limit(p.limit, 50, 200);
        let include_global = p.project_id.is_none() || p.include_global.unwrap_or(true);
        let kinds = match p.kinds.as_deref() {
            Some(k) if !k.is_empty() => Some(sql_in_list(k)?),
            _ => None,
        };
        for combine in [Combine::All, Combine::Any] {
            let Some(expr) = fts::match_expr(&p.query, combine) else {
                return Ok(Vec::new());
            };
            let hits = self.search_expr(expr, p.project_id, include_global, kinds.clone(), limit).await?;
            if !hits.is_empty() || fts::terms(&p.query).len() < 2 {
                return Ok(hits);
            }
        }
        Ok(Vec::new())
    }

    async fn search_expr(
        &self,
        expr: String,
        project: Option<ProjectId>,
        include_global: bool,
        kinds: Option<String>,
        limit: u32,
    ) -> MemoryResult<Vec<MemorySearchHit>> {
        let project = project.map(|p| p.to_string());
        let rows: Vec<(String, f64, String)> = self
            .db
            .call(move |c| {
                let kind_sql = kinds.map(|k| format!(" AND m.kind IN {k}")).unwrap_or_default();
                let sql = format!(
                    "SELECT m.doc, bm25(memory_fts, 3.0, 1.0, 2.0) AS rank,
                            snippet(memory_fts, 1, char(2), char(3), '…', 24)
                     FROM memory_fts JOIN memory_items m ON m.seq = memory_fts.rowid
                     WHERE memory_fts MATCH ?1
                       AND ((m.scope = 'Project' AND m.project_id = ?2) OR (?3 AND m.scope = 'Global')){kind_sql}
                     ORDER BY rank LIMIT ?4"
                );
                let mut stmt = c.prepare(&sql)?;
                let rows = stmt
                    .query_map(params![expr, project, include_global, limit], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        let mut hits = Vec::with_capacity(rows.len());
        for (doc, rank, snippet) in rows {
            let item = row_doc(&doc)?;
            let mut score = fts::score_from_rank(rank);
            if matches!(item.scope, MemoryScope::Project { .. }) {
                score *= PROJECT_BOOST;
            }
            hits.push(MemorySearchHit { item, score, snippet });
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.item.id.cmp(&b.item.id)));
        Ok(hits)
    }

    /// Resolves effective rules: project items override global items with the
    /// same resolution key ([`validate::rule_key`]).
    pub async fn effective_rules(
        &self,
        project: Option<ProjectId>,
        kinds: Option<Vec<MemoryKind>>,
    ) -> MemoryResult<Vec<EffectiveRule>> {
        let kinds = kinds.filter(|k| !k.is_empty()).unwrap_or_else(|| DEFAULT_RULE_KINDS.to_vec());
        let global = self
            .list_items(ListMemoryParams {
                scope: MemoryScope::Global,
                kinds: Some(kinds.clone()),
                tag: None,
                pinned_only: None,
                limit: Some(2000),
            })
            .await?;
        let project_items = match project {
            Some(project_id) => {
                self.list_items(ListMemoryParams {
                    scope: MemoryScope::Project { project_id },
                    kinds: Some(kinds),
                    tag: None,
                    pinned_only: None,
                    limit: Some(2000),
                })
                .await?
            }
            None => Vec::new(),
        };
        Ok(resolve_rules(global, project_items))
    }
}

/// Pure resolution of effective rules (exposed for the context optimizer).
#[must_use]
pub fn resolve_rules(global: Vec<MemoryItem>, project: Vec<MemoryItem>) -> Vec<EffectiveRule> {
    // Within one scope, the most recently updated item wins a key (lists are
    // ordered pinned-first then newest; prefer pinned, then newest).
    let mut by_key: BTreeMap<String, EffectiveRule> = BTreeMap::new();
    for item in global {
        let key = validate::rule_key(item.key.as_deref(), &item.title);
        by_key.entry(key.clone()).or_insert(EffectiveRule { key, item, origin: RuleOrigin::Global, overrides: None });
    }
    let mut project_seen = std::collections::BTreeSet::new();
    for item in project {
        let key = validate::rule_key(item.key.as_deref(), &item.title);
        if !project_seen.insert(key.clone()) {
            continue;
        }
        let overrides = by_key.get(&key).filter(|r| r.origin == RuleOrigin::Global).map(|r| r.item.id);
        by_key.insert(key.clone(), EffectiveRule { key, item, origin: RuleOrigin::Project, overrides });
    }
    let mut rules: Vec<EffectiveRule> = by_key.into_values().collect();
    rules.sort_by(|a, b| {
        b.item
            .pinned
            .cmp(&a.item.pinned)
            .then_with(|| (a.origin == RuleOrigin::Global).cmp(&(b.origin == RuleOrigin::Global)))
            .then_with(|| a.item.title.to_lowercase().cmp(&b.item.title.to_lowercase()))
            .then_with(|| a.key.cmp(&b.key))
    });
    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{engine, project};

    fn create(scope: MemoryScope, kind: MemoryKind, title: &str, body: &str) -> CreateMemoryParams {
        CreateMemoryParams {
            scope,
            kind,
            title: title.into(),
            content: body.into(),
            tags: None,
            source: None,
            pinned: None,
            key: None,
        }
    }

    fn list(scope: MemoryScope) -> ListMemoryParams {
        ListMemoryParams { scope, kinds: None, tag: None, pinned_only: None, limit: None }
    }

    fn search(q: &str, project_id: Option<ProjectId>) -> SearchMemoryParams {
        SearchMemoryParams { query: q.into(), project_id, include_global: None, kinds: None, limit: None }
    }

    #[tokio::test]
    async fn crud_roundtrip_and_events() {
        let (eng, bus) = engine().await;
        let mut sub = bus.subscribe_all();
        let pid = project(&eng, "Payroll").await;
        let scope = MemoryScope::Project { project_id: pid };
        let mut p = create(scope, MemoryKind::Convention, "Money as integer cents", "Never use floats for money.");
        p.tags = Some(vec!["money".into(), "Money".into(), "rust".into()]);
        let item = eng.create_item(p).await.unwrap();
        assert_eq!(item.tags, vec!["money", "rust"]);
        assert_eq!(item.source.as_deref(), Some("user"));

        let ev = sub.recv().await.unwrap();
        let ev = match ev {
            orkestr_events::Received::Event(e) => e,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(ev.kind_name(), "MemoryUpdated");
        assert_eq!(ev.project_id, Some(pid));

        let got = eng.get_item(item.id).await.unwrap();
        assert_eq!(got, item);

        let updated = eng
            .update_item(
                item.id,
                MemoryPatch { pinned: Some(true), title: Some("Money in cents".into()), ..MemoryPatch::default() },
            )
            .await
            .unwrap();
        assert!(updated.pinned);
        assert_eq!(updated.title, "Money in cents");

        let items = eng.list_items(list(scope)).await.unwrap();
        assert_eq!(items.len(), 1);
        assert!(eng.list_items(list(MemoryScope::Global)).await.unwrap().is_empty());

        eng.delete_item(item.id).await.unwrap();
        assert!(matches!(eng.get_item(item.id).await, Err(MemoryError::NotFound(_))));
        assert!(matches!(eng.delete_item(item.id).await, Err(MemoryError::NotFound(_))));
        // FTS index no longer returns the deleted row.
        assert!(eng.search(search("cents", Some(pid))).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn validation_and_missing_project() {
        let (eng, _bus) = engine().await;
        let missing = MemoryScope::Project { project_id: ProjectId::new() };
        assert!(matches!(
            eng.create_item(create(missing, MemoryKind::Todo, "x", "")).await,
            Err(MemoryError::NotFound(_))
        ));
        assert!(matches!(
            eng.create_item(create(MemoryScope::Global, MemoryKind::Todo, "  ", "")).await,
            Err(MemoryError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn fts_ranking_scopes_and_filters() {
        let (eng, _bus) = engine().await;
        let pid = project(&eng, "Shop").await;
        let other = project(&eng, "Other").await;
        let ps = MemoryScope::Project { project_id: pid };
        eng.create_item(create(ps, MemoryKind::Architecture, "Payment gateway", "Stripe adapter behind a trait."))
            .await
            .unwrap();
        eng.create_item(create(ps, MemoryKind::Todo, "Refactor cart", "Mentions payment once.")).await.unwrap();
        let mut tagged = create(MemoryScope::Global, MemoryKind::Convention, "Error handling", "Use thiserror.");
        tagged.tags = Some(vec!["payment".into()]);
        eng.create_item(tagged).await.unwrap();
        eng.create_item(create(
            MemoryScope::Project { project_id: other },
            MemoryKind::Todo,
            "Payment elsewhere",
            "payment payment",
        ))
        .await
        .unwrap();

        let hits = eng.search(search("payment", Some(pid))).await.unwrap();
        let titles: Vec<_> = hits.iter().map(|h| h.item.title.as_str()).collect();
        assert_eq!(titles.len(), 3, "{titles:?}");
        assert_eq!(titles[0], "Payment gateway", "title match ranks first");
        assert!(!titles.contains(&"Payment elsewhere"), "other projects are never searched");
        assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
        assert!(hits[1].snippet.contains('\u{2}') || hits[2].snippet.contains('\u{2}'));

        // Prefix match + exclude global.
        let mut p = search("paym", Some(pid));
        p.include_global = Some(false);
        assert_eq!(eng.search(p).await.unwrap().len(), 2);

        // Kind filter.
        let mut p = search("payment", Some(pid));
        p.kinds = Some(vec![MemoryKind::Todo]);
        let hits = eng.search(p).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item.title, "Refactor cart");

        // All-words first, any-word fallback.
        assert_eq!(eng.search(search("stripe trait", Some(pid))).await.unwrap().len(), 1);
        assert_eq!(eng.search(search("stripe zzzunknown", Some(pid))).await.unwrap().len(), 1);
        // Hostile syntax does not error.
        assert!(eng.search(search("\"NEAR(* -:", Some(pid))).await.unwrap().is_empty());
        // Global-only search.
        assert_eq!(eng.search(search("payment", None)).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_filters() {
        let (eng, _bus) = engine().await;
        let mut a = create(MemoryScope::Global, MemoryKind::Preference, "A", "");
        a.tags = Some(vec!["UI".into()]);
        a.pinned = Some(true);
        eng.create_item(a).await.unwrap();
        eng.create_item(create(MemoryScope::Global, MemoryKind::Convention, "B", "")).await.unwrap();
        eng.create_item(create(MemoryScope::Global, MemoryKind::Convention, "C", "")).await.unwrap();
        let all = eng.list_items(list(MemoryScope::Global)).await.unwrap();
        assert_eq!(all.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(), vec!["A", "C", "B"]);
        let mut p = list(MemoryScope::Global);
        p.kinds = Some(vec![MemoryKind::Convention]);
        assert_eq!(eng.list_items(p).await.unwrap().len(), 2);
        let mut p = list(MemoryScope::Global);
        p.tag = Some("ui".into());
        assert_eq!(eng.list_items(p).await.unwrap().len(), 1);
        let mut p = list(MemoryScope::Global);
        p.pinned_only = Some(true);
        assert_eq!(eng.list_items(p).await.unwrap().len(), 1);
        let mut p = list(MemoryScope::Global);
        p.limit = Some(2);
        assert_eq!(eng.list_items(p).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn project_rules_override_global_by_key() {
        let (eng, _bus) = engine().await;
        let pid = project(&eng, "P").await;
        let ps = MemoryScope::Project { project_id: pid };
        let mut g = create(MemoryScope::Global, MemoryKind::Convention, "Indentation", "4 spaces");
        g.key = Some("indent".into());
        let g = eng.create_item(g).await.unwrap();
        let mut pr = create(ps, MemoryKind::Convention, "Indent with tabs", "tabs here");
        pr.key = Some("INDENT".into());
        let pr = eng.create_item(pr).await.unwrap();
        // Same title, no key ⇒ title-based override.
        eng.create_item(create(MemoryScope::Global, MemoryKind::QualityStandard, "Tests required", "global"))
            .await
            .unwrap();
        let pt = eng.create_item(create(ps, MemoryKind::QualityStandard, "tests  REQUIRED", "project")).await.unwrap();
        eng.create_item(create(MemoryScope::Global, MemoryKind::Preference, "Dark theme", "")).await.unwrap();
        // Non-rule kind is ignored by default.
        eng.create_item(create(ps, MemoryKind::Todo, "Ship it", "")).await.unwrap();

        let rules = eng.effective_rules(Some(pid), None).await.unwrap();
        assert_eq!(rules.len(), 3, "{rules:?}");
        let indent = rules.iter().find(|r| r.key == "indent").unwrap();
        assert_eq!(indent.item.id, pr.id);
        assert_eq!(indent.origin, RuleOrigin::Project);
        assert_eq!(indent.overrides, Some(g.id));
        let tests = rules.iter().find(|r| r.key == "title:tests required").unwrap();
        assert_eq!(tests.item.id, pt.id);
        assert_eq!(rules.last().unwrap().origin, RuleOrigin::Global, "project rules sort before global");

        let global_only = eng.effective_rules(None, None).await.unwrap();
        assert_eq!(global_only.len(), 3);
        assert!(global_only.iter().all(|r| r.origin == RuleOrigin::Global));
    }

    #[tokio::test]
    async fn deleting_project_cascades() {
        let (eng, _bus) = engine().await;
        let pid = project(&eng, "Gone").await;
        eng.create_item(create(MemoryScope::Project { project_id: pid }, MemoryKind::Todo, "x", "y")).await.unwrap();
        let db = eng.db().clone();
        let id = pid.to_string();
        db.call(move |c| Ok(c.execute("DELETE FROM projects WHERE id = ?1", [id])?)).await.unwrap();
        assert!(eng.list_items(list(MemoryScope::Project { project_id: pid })).await.unwrap().is_empty());
        assert!(eng.search(search("x", Some(pid))).await.unwrap().is_empty());
    }
}
