//! Emergency controls (§52, permission model §9, ADR-0022).
//!
//! Handled entirely locally — no provider, network or control-plane call is
//! on this path. Engines that run AI work (agent runtime, terminals of
//! agents, providers' streaming calls) obtain a [`CancellationToken`] from
//! [`EmergencyController::register_agent`] / [`register_project`] /
//! [`register_global`] and must stop promptly when it is cancelled.
//!
//! Token tree: `root ⊃ project ⊃ agent`. `stopAll` cancels the root and sets
//! the persisted `allStopped` flag (new registrations are refused until
//! [`EmergencyController::reset`]); `stopProject` / `stopAgent` cancel a
//! subtree. Flags (`fullAutoDisabled`, `networkToolsDisabled`,
//! `vaultLocked`, `allStopped`) are cached in memory, persisted in
//! `emergency_flags` and consulted at step 0 of every permission check.
//!
//! [`register_project`]: EmergencyController::register_project
//! [`register_global`]: EmergencyController::register_global

use std::collections::HashMap;
use std::sync::Arc;

use orkestr_domain::{Actor, AgentId, ProjectId};
use orkestr_events::EventBus;
use orkestr_protocol::event::{EmergencyFlagsChangedPayload, EmergencyScope, EmergencyStopPayload, EventKind};
use orkestr_protocol::methods::emergency::EmergencyFlagsView;
use orkestr_protocol::NewEvent;
use orkestr_storage::rusqlite::params;
use orkestr_storage::{Db, Migration};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::{SecurityError, SecurityResult};

/// Migration creating the flags table.
pub const MIGRATION: Migration = Migration {
    id: "security_0003_emergency",
    owner: "security",
    up: "CREATE TABLE emergency_flags (name TEXT PRIMARY KEY NOT NULL, value INTEGER NOT NULL, updated_at TEXT NOT NULL);",
    down: "DROP TABLE IF EXISTS emergency_flags;",
};

/// Emergency flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmergencyFlags {
    /// Stop All AI is active: agent/manager actions and new AI work are denied.
    pub all_stopped: bool,
    /// Effective approval mode is capped at SemiAuto.
    pub full_auto_disabled: bool,
    /// `Network` / `Browser` capabilities are denied to agents.
    pub network_tools_disabled: bool,
    /// The secret vault is locked: `Secrets` is denied.
    pub vault_locked: bool,
}

/// Shared, cheaply readable flags cell (the permission engine reads it on
/// every check).
#[derive(Debug, Default)]
pub struct SharedFlags(RwLock<EmergencyFlags>);

impl SharedFlags {
    /// Snapshot.
    #[must_use]
    pub fn get(&self) -> EmergencyFlags {
        *self.0.read()
    }

    pub(crate) fn update(&self, f: impl FnOnce(&mut EmergencyFlags)) -> EmergencyFlags {
        let mut g = self.0.write();
        f(&mut g);
        *g
    }
}

/// Result of a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopReport {
    /// Agent tokens cancelled.
    pub agents_stopped: u32,
    /// Other registrations (project/global work) cancelled.
    pub other_stopped: u32,
}

struct Registry {
    root: CancellationToken,
    globals: Vec<CancellationToken>,
    projects: HashMap<ProjectId, CancellationToken>,
    agents: HashMap<AgentId, (Option<ProjectId>, CancellationToken)>,
}

impl Registry {
    fn new() -> Self {
        Self { root: CancellationToken::new(), globals: Vec::new(), projects: HashMap::new(), agents: HashMap::new() }
    }
}

/// The emergency controller.
pub struct EmergencyController {
    db: Db,
    events: EventBus,
    flags: Arc<SharedFlags>,
    reg: RwLock<Registry>,
}

impl std::fmt::Debug for EmergencyController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmergencyController").field("flags", &self.flags.get()).finish_non_exhaustive()
    }
}

const FLAG_NAMES: [&str; 4] = ["all_stopped", "full_auto_disabled", "network_tools_disabled", "vault_locked"];

impl EmergencyController {
    /// Loads persisted flags.
    pub async fn open(db: Db, events: EventBus) -> SecurityResult<Self> {
        let rows: Vec<(String, i64)> = db
            .call(|c| {
                let mut stmt = c.prepare("SELECT name, value FROM emergency_flags")?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            })
            .await?;
        let mut flags = EmergencyFlags::default();
        for (name, value) in rows {
            let v = value != 0;
            match name.as_str() {
                "all_stopped" => flags.all_stopped = v,
                "full_auto_disabled" => flags.full_auto_disabled = v,
                "network_tools_disabled" => flags.network_tools_disabled = v,
                "vault_locked" => flags.vault_locked = v,
                _ => {}
            }
        }
        if flags != EmergencyFlags::default() {
            tracing::warn!(?flags, "emergency flags restored from the previous session");
        }
        Ok(Self { db, events, flags: Arc::new(SharedFlags(RwLock::new(flags))), reg: RwLock::new(Registry::new()) })
    }

    /// The shared flags cell (for the permission engine).
    #[must_use]
    pub fn shared_flags(&self) -> Arc<SharedFlags> {
        Arc::clone(&self.flags)
    }

    /// Current flags.
    #[must_use]
    pub fn flags(&self) -> EmergencyFlags {
        self.flags.get()
    }

    async fn persist(&self, flags: EmergencyFlags) -> SecurityResult<()> {
        let values = [flags.all_stopped, flags.full_auto_disabled, flags.network_tools_disabled, flags.vault_locked];
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                let ts = chrono::Utc::now().to_rfc3339();
                for (name, value) in FLAG_NAMES.iter().zip(values) {
                    tx.execute(
                        "INSERT INTO emergency_flags (name, value, updated_at) VALUES (?1, ?2, ?3)
                         ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                        params![name, i64::from(value), ts],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    fn ensure_running(&self) -> SecurityResult<()> {
        if self.flags.get().all_stopped {
            return Err(SecurityError::EmergencyActive("Stop All AI is active; reset emergency controls first".into()));
        }
        Ok(())
    }

    /// Token for global AI work (e.g. a Manager run not bound to a project).
    pub fn register_global(&self) -> SecurityResult<CancellationToken> {
        self.ensure_running()?;
        let mut reg = self.reg.write();
        let t = reg.root.child_token();
        reg.globals.retain(|t| !t.is_cancelled());
        reg.globals.push(t.clone());
        Ok(t)
    }

    /// Token for project-level work (child of root).
    pub fn register_project(&self, project: ProjectId) -> SecurityResult<CancellationToken> {
        self.ensure_running()?;
        let mut reg = self.reg.write();
        let root = reg.root.clone();
        let t = reg.projects.entry(project).or_insert_with(|| root.child_token());
        if t.is_cancelled() {
            *t = root.child_token();
        }
        Ok(t.child_token())
    }

    /// Token for one agent (child of its project's token).
    pub fn register_agent(&self, agent: AgentId, project: Option<ProjectId>) -> SecurityResult<CancellationToken> {
        self.ensure_running()?;
        let mut reg = self.reg.write();
        let parent = match project {
            Some(p) => {
                let root = reg.root.clone();
                let t = reg.projects.entry(p).or_insert_with(|| root.child_token());
                if t.is_cancelled() {
                    *t = root.child_token();
                }
                t.clone()
            }
            None => reg.root.clone(),
        };
        let token = parent.child_token();
        reg.agents.insert(agent, (project, token.clone()));
        Ok(token)
    }

    /// Forgets an agent that finished normally.
    pub fn unregister_agent(&self, agent: AgentId) {
        self.reg.write().agents.remove(&agent);
    }

    /// Number of live agent registrations.
    #[must_use]
    pub fn active_agents(&self) -> usize {
        self.reg.read().agents.values().filter(|(_, t)| !t.is_cancelled()).count()
    }

    async fn publish(&self, scope: EmergencyScope, agents_stopped: u32, by: Actor) {
        let ev =
            NewEvent::new(EventKind::EmergencyStop(EmergencyStopPayload { scope, agents_stopped, triggered_by: by }));
        if let Err(e) = self.events.publish(ev).await {
            // The stop itself already happened; never fail it because of the event log.
            tracing::error!(error = %e, "failed to publish EmergencyStop");
        }
    }

    /// Tells every window / client the flags now in effect (`EmergencyFlagsChanged`).
    async fn publish_flags(&self, flags: EmergencyFlags) {
        let ev = NewEvent::new(EventKind::EmergencyFlagsChanged(EmergencyFlagsChangedPayload {
            flags: EmergencyFlagsView {
                all_stopped: flags.all_stopped,
                full_auto_disabled: flags.full_auto_disabled,
                network_tools_disabled: flags.network_tools_disabled,
                vault_locked: flags.vault_locked,
            },
        }));
        if let Err(e) = self.events.publish(ev).await {
            // The change already happened; never fail it because of the event log.
            tracing::error!(error = %e, "failed to publish EmergencyFlagsChanged");
        }
    }

    /// Updates, persists and (when something changed) announces the flags.
    async fn change_flags(&self, f: impl FnOnce(&mut EmergencyFlags)) -> SecurityResult<EmergencyFlags> {
        let before = self.flags.get();
        let flags = self.flags.update(f);
        self.persist(flags).await?;
        if flags != before {
            self.publish_flags(flags).await;
        }
        Ok(flags)
    }

    /// Stop All AI: cancels every token, sets `allStopped`, emits `EmergencyStop{All}`.
    pub async fn stop_all(&self, by: Actor) -> SecurityResult<StopReport> {
        let report = {
            let mut reg = self.reg.write();
            let agents =
                u32::try_from(reg.agents.values().filter(|(_, t)| !t.is_cancelled()).count()).unwrap_or(u32::MAX);
            let other = u32::try_from(reg.globals.iter().filter(|t| !t.is_cancelled()).count() + reg.projects.len())
                .unwrap_or(u32::MAX);
            reg.root.cancel();
            *reg = Registry::new();
            StopReport { agents_stopped: agents, other_stopped: other }
        };
        let before = self.flags.get();
        let flags = self.flags.update(|f| f.all_stopped = true);
        tracing::warn!(agents = report.agents_stopped, "EMERGENCY STOP ALL");
        // Persist after the in-memory stop so a DB problem cannot delay it.
        let persisted = self.persist(flags).await;
        self.publish(EmergencyScope::All, report.agents_stopped, by).await;
        if flags != before {
            self.publish_flags(flags).await;
        }
        persisted?;
        Ok(report)
    }

    /// Stops one project's AI work.
    pub async fn stop_project(&self, project: ProjectId, by: Actor) -> SecurityResult<StopReport> {
        let report = {
            let mut reg = self.reg.write();
            let mut agents = 0u32;
            reg.agents.retain(|_, (p, t)| {
                if *p == Some(project) {
                    if !t.is_cancelled() {
                        agents += 1;
                    }
                    t.cancel();
                    false
                } else {
                    true
                }
            });
            let other = match reg.projects.remove(&project) {
                Some(t) => {
                    t.cancel();
                    1
                }
                None => 0,
            };
            StopReport { agents_stopped: agents, other_stopped: other }
        };
        tracing::warn!(%project, agents = report.agents_stopped, "emergency stop project");
        self.publish(EmergencyScope::Project { project_id: project }, report.agents_stopped, by).await;
        Ok(report)
    }

    /// Stops one agent.
    pub async fn stop_agent(&self, agent: AgentId, by: Actor) -> SecurityResult<StopReport> {
        let stopped = {
            let mut reg = self.reg.write();
            match reg.agents.remove(&agent) {
                Some((_, t)) => {
                    let live = !t.is_cancelled();
                    t.cancel();
                    u32::from(live)
                }
                None => 0,
            }
        };
        tracing::warn!(%agent, "emergency stop agent");
        self.publish(EmergencyScope::Agent { agent_id: agent }, stopped, by).await;
        Ok(StopReport { agents_stopped: stopped, other_stopped: 0 })
    }

    /// Sets/clears `fullAutoDisabled`.
    pub async fn set_full_auto_disabled(&self, disabled: bool) -> SecurityResult<EmergencyFlags> {
        self.change_flags(|f| f.full_auto_disabled = disabled).await
    }

    /// Sets/clears `networkToolsDisabled`.
    pub async fn set_network_tools_disabled(&self, disabled: bool) -> SecurityResult<EmergencyFlags> {
        self.change_flags(|f| f.network_tools_disabled = disabled).await
    }

    /// Sets/clears `vaultLocked` (driven by the secret service).
    pub async fn set_vault_locked(&self, locked: bool) -> SecurityResult<EmergencyFlags> {
        self.change_flags(|f| f.vault_locked = locked).await
    }

    /// Re-arms after an emergency: clears `allStopped`, `fullAutoDisabled`
    /// and `networkToolsDisabled`. The vault stays locked until explicitly
    /// unlocked. Emits `EmergencyFlagsChanged` so every window leaves the
    /// "stopped" state.
    pub async fn reset(&self) -> SecurityResult<EmergencyFlags> {
        let flags = self
            .change_flags(|f| {
                f.all_stopped = false;
                f.full_auto_disabled = false;
                f.network_tools_disabled = false;
            })
            .await?;
        tracing::info!("emergency controls reset");
        Ok(flags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_events::Received;

    async fn controller() -> (Db, EventBus, EmergencyController) {
        let db = Db::open_in_memory().unwrap();
        db.migrate(crate::migrations()).await.unwrap();
        let bus = EventBus::in_memory(64);
        let c = EmergencyController::open(db.clone(), bus.clone()).await.unwrap();
        (db, bus, c)
    }

    #[tokio::test]
    async fn stop_all_cancels_everything_and_blocks_new_work() {
        let (db, bus, c) = controller().await;
        let mut sub = bus.subscribe(["global"]).unwrap();
        let p = ProjectId::new();
        let a1 = c.register_agent(AgentId::new(), Some(p)).unwrap();
        let a2 = c.register_agent(AgentId::new(), None).unwrap();
        let g = c.register_global().unwrap();
        let pt = c.register_project(p).unwrap();
        let report = c.stop_all(Actor::System).await.unwrap();
        assert_eq!(report.agents_stopped, 2);
        for t in [&a1, &a2, &g, &pt] {
            assert!(t.is_cancelled());
        }
        assert!(c.flags().all_stopped);
        assert!(matches!(c.register_agent(AgentId::new(), None), Err(SecurityError::EmergencyActive(_))));
        match tokio::time::timeout(std::time::Duration::from_secs(2), sub.recv()).await.unwrap() {
            Some(Received::Event(ev)) => assert_eq!(ev.kind_name(), "EmergencyStop"),
            other => panic!("{other:?}"),
        }
        // Persisted across restart.
        let c2 = EmergencyController::open(db, bus).await.unwrap();
        assert!(c2.flags().all_stopped);
        c2.reset().await.unwrap();
        assert!(!c2.flags().all_stopped);
        let t = c2.register_agent(AgentId::new(), None).unwrap();
        assert!(!t.is_cancelled());
    }

    #[tokio::test]
    async fn stop_project_and_agent_are_scoped() {
        let (_db, _bus, c) = controller().await;
        let p1 = ProjectId::new();
        let p2 = ProjectId::new();
        let a = AgentId::new();
        let t1 = c.register_agent(a, Some(p1)).unwrap();
        let t1b = c.register_agent(AgentId::new(), Some(p1)).unwrap();
        let t2 = c.register_agent(AgentId::new(), Some(p2)).unwrap();
        let r = c.stop_project(p1, Actor::System).await.unwrap();
        assert_eq!(r.agents_stopped, 2);
        assert!(t1.is_cancelled() && t1b.is_cancelled());
        assert!(!t2.is_cancelled());
        // Project can be used again after a project stop.
        let t3 = c.register_agent(AgentId::new(), Some(p1)).unwrap();
        assert!(!t3.is_cancelled());
        let b = AgentId::new();
        let tb = c.register_agent(b, Some(p2)).unwrap();
        let r = c.stop_agent(b, Actor::System).await.unwrap();
        assert_eq!(r.agents_stopped, 1);
        assert!(tb.is_cancelled());
        assert!(!t2.is_cancelled());
        assert!(!c.flags().all_stopped);
    }

    #[tokio::test]
    async fn flags_toggle_and_persist() {
        let (db, bus, c) = controller().await;
        c.set_full_auto_disabled(true).await.unwrap();
        c.set_network_tools_disabled(true).await.unwrap();
        c.set_vault_locked(true).await.unwrap();
        let c2 = EmergencyController::open(db, bus).await.unwrap();
        let f = c2.flags();
        assert!(f.full_auto_disabled && f.network_tools_disabled && f.vault_locked && !f.all_stopped);
        let f = c2.reset().await.unwrap();
        assert!(!f.full_auto_disabled && !f.network_tools_disabled && f.vault_locked);
    }

    /// Next `EmergencyFlagsChanged` payload on `sub` (skipping other kinds).
    async fn next_flags(sub: &mut orkestr_events::Subscription) -> EmergencyFlagsView {
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(2), sub.recv()).await.unwrap() {
                Some(Received::Event(ev)) => {
                    if let EventKind::EmergencyFlagsChanged(p) = ev.kind {
                        return p.flags;
                    }
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn every_flag_change_is_announced_to_other_windows() {
        let (_db, bus, c) = controller().await;
        let mut sub = bus.subscribe(["global"]).unwrap();
        c.stop_all(Actor::System).await.unwrap();
        assert!(next_flags(&mut sub).await.all_stopped);
        // Re-arm must reach every window, or they keep showing "AI stopped".
        c.reset().await.unwrap();
        let f = next_flags(&mut sub).await;
        assert!(!f.all_stopped && !f.full_auto_disabled && !f.network_tools_disabled);
        c.set_full_auto_disabled(true).await.unwrap();
        assert!(next_flags(&mut sub).await.full_auto_disabled);
        c.set_network_tools_disabled(true).await.unwrap();
        assert!(next_flags(&mut sub).await.network_tools_disabled);
        c.set_vault_locked(true).await.unwrap();
        assert!(next_flags(&mut sub).await.vault_locked);
        // No change, no event: the next one seen is the real change below.
        c.set_vault_locked(true).await.unwrap();
        c.set_vault_locked(false).await.unwrap();
        assert!(!next_flags(&mut sub).await.vault_locked);
    }
}
