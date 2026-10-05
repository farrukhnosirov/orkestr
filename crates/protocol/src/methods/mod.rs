//! RPC methods: `<area>.<verb>` names with typed params/results (contract §4).
//!
//! Each area has its own module. A method is a zero-sized marker type
//! implementing [`Method`]; the core router registers handlers by marker type
//! and the TS `Methods` map is generated from [`registry()`].
//!
//! Implemented areas: [`system`], [`projects`], [`events`], [`settings`]
//! (`ui.*` still reserved), [`nodes`].
//! Agent runtime: [`agents`], [`templates`]. AI Manager: [`manager`].
//! Placeholder areas (names reserved, types arrive with their phase):
//! [`tasks`], [`providers`], [`terminal`], [`approvals`],
//! [`emergency`], [`memory`], [`vcs`], [`files`], [`snapshots`], [`cost`].

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub mod agents;
pub mod approvals;
pub mod audit;
pub mod cost;
pub mod emergency;
pub mod events;
pub mod files;
pub mod manager;
pub mod memory;
pub mod nodes;
pub mod permissions;
pub mod projects;
pub mod providers;
pub mod quality;
pub mod secrets;
pub mod settings;
pub mod snapshots;
pub mod system;
pub mod tasks;
pub mod templates;
pub mod terminal;
pub mod vcs;

/// A typed RPC method.
pub trait Method: 'static {
    /// Wire name, e.g. `projects.list`.
    const NAME: &'static str;
    /// Params type.
    type Params: Serialize + DeserializeOwned + TS + Send + 'static;
    /// Result type.
    type Result: Serialize + DeserializeOwned + TS + Send + 'static;
}

/// Params/result type for methods without data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Empty {}

/// Declares a method marker type.
macro_rules! method {
    ($(#[$m:meta])* $marker:ident, $name:literal, $params:ty, $result:ty) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, Default)]
        pub struct $marker;

        impl $crate::methods::Method for $marker {
            const NAME: &'static str = $name;
            type Params = $params;
            type Result = $result;
        }
    };
}
pub(crate) use method;

/// Type-erased description of a method, used for TS generation and docs.
#[derive(Debug, Clone, Copy)]
pub struct MethodDescriptor {
    /// Wire name.
    pub name: &'static str,
    /// TS type name of the params.
    pub params_ts: fn(&ts_rs::Config) -> String,
    /// TS type name of the result.
    pub result_ts: fn(&ts_rs::Config) -> String,
    /// Output path (relative to the export dir) of the params type, if it has one.
    pub params_path: fn() -> Option<std::path::PathBuf>,
    /// Output path of the result type.
    pub result_path: fn() -> Option<std::path::PathBuf>,
    /// Exports params + result with dependencies.
    pub export: fn(&ts_rs::Config) -> Result<(), ts_rs::ExportError>,
}

impl MethodDescriptor {
    /// Builds a descriptor for `M`.
    #[must_use]
    pub fn of<M: Method>() -> Self {
        fn export<M: Method>(cfg: &ts_rs::Config) -> Result<(), ts_rs::ExportError> {
            <M::Params as TS>::export_all(cfg)?;
            <M::Result as TS>::export_all(cfg)
        }
        Self {
            name: M::NAME,
            params_ts: <M::Params as TS>::name,
            result_ts: <M::Result as TS>::name,
            params_path: <M::Params as TS>::output_path,
            result_path: <M::Result as TS>::output_path,
            export: export::<M>,
        }
    }
}

/// All implemented methods. Append new methods here when their handler lands.
#[must_use]
pub fn registry() -> Vec<MethodDescriptor> {
    vec![
        MethodDescriptor::of::<system::Health>(),
        MethodDescriptor::of::<system::Info>(),
        MethodDescriptor::of::<projects::List>(),
        MethodDescriptor::of::<projects::Get>(),
        MethodDescriptor::of::<projects::Create>(),
        MethodDescriptor::of::<projects::Update>(),
        MethodDescriptor::of::<projects::Delete>(),
        MethodDescriptor::of::<projects::Open>(),
        MethodDescriptor::of::<events::Replay>(),
        // security (Phase 1)
        MethodDescriptor::of::<permissions::List>(),
        MethodDescriptor::of::<permissions::Set>(),
        MethodDescriptor::of::<permissions::Delete>(),
        MethodDescriptor::of::<permissions::Check>(),
        MethodDescriptor::of::<permissions::GetMode>(),
        MethodDescriptor::of::<permissions::SetMode>(),
        MethodDescriptor::of::<approvals::List>(),
        MethodDescriptor::of::<approvals::Get>(),
        MethodDescriptor::of::<approvals::Resolve>(),
        MethodDescriptor::of::<secrets::List>(),
        MethodDescriptor::of::<secrets::Set>(),
        MethodDescriptor::of::<secrets::Delete>(),
        MethodDescriptor::of::<secrets::Lock>(),
        MethodDescriptor::of::<secrets::Unlock>(),
        MethodDescriptor::of::<secrets::Status>(),
        MethodDescriptor::of::<emergency::StopAll>(),
        MethodDescriptor::of::<emergency::StopProject>(),
        MethodDescriptor::of::<emergency::StopAgent>(),
        MethodDescriptor::of::<emergency::DisableFullAuto>(),
        MethodDescriptor::of::<emergency::DisableNetworkTools>(),
        MethodDescriptor::of::<emergency::LockVault>(),
        MethodDescriptor::of::<emergency::Status>(),
        MethodDescriptor::of::<emergency::Reset>(),
        MethodDescriptor::of::<audit::List>(),
        MethodDescriptor::of::<providers::List>(),
        MethodDescriptor::of::<providers::Get>(),
        MethodDescriptor::of::<providers::Create>(),
        MethodDescriptor::of::<providers::Update>(),
        MethodDescriptor::of::<providers::Delete>(),
        MethodDescriptor::of::<providers::SetCredential>(),
        MethodDescriptor::of::<providers::ClearCredential>(),
        MethodDescriptor::of::<providers::Health>(),
        MethodDescriptor::of::<providers::Models>(),
        MethodDescriptor::of::<providers::Test>(),
        MethodDescriptor::of::<providers::Kinds>(),
        MethodDescriptor::of::<providers::GetManager>(),
        MethodDescriptor::of::<providers::SetManager>(),
        MethodDescriptor::of::<providers::GetRouting>(),
        MethodDescriptor::of::<providers::SetRouting>(),
        MethodDescriptor::of::<tasks::List>(),
        MethodDescriptor::of::<tasks::Get>(),
        MethodDescriptor::of::<tasks::Create>(),
        MethodDescriptor::of::<tasks::Update>(),
        MethodDescriptor::of::<tasks::Delete>(),
        MethodDescriptor::of::<tasks::Transition>(),
        MethodDescriptor::of::<tasks::AddDependency>(),
        MethodDescriptor::of::<tasks::RemoveDependency>(),
        MethodDescriptor::of::<tasks::Graph>(),
        MethodDescriptor::of::<tasks::History>(),
        MethodDescriptor::of::<tasks::Retry>(),
        // vcs (Phase 1)
        MethodDescriptor::of::<vcs::Status>(),
        MethodDescriptor::of::<vcs::Diff>(),
        MethodDescriptor::of::<vcs::Log>(),
        MethodDescriptor::of::<vcs::Show>(),
        MethodDescriptor::of::<vcs::Branches>(),
        MethodDescriptor::of::<vcs::CreateBranch>(),
        MethodDescriptor::of::<vcs::DeleteBranch>(),
        MethodDescriptor::of::<vcs::Checkout>(),
        MethodDescriptor::of::<vcs::Stage>(),
        MethodDescriptor::of::<vcs::Unstage>(),
        MethodDescriptor::of::<vcs::Discard>(),
        MethodDescriptor::of::<vcs::Commit>(),
        MethodDescriptor::of::<vcs::Worktrees>(),
        MethodDescriptor::of::<vcs::AddWorktree>(),
        MethodDescriptor::of::<vcs::RemoveWorktree>(),
        MethodDescriptor::of::<vcs::MergePreviewMethod>(),
        MethodDescriptor::of::<vcs::Merge>(),
        MethodDescriptor::of::<vcs::AbortMerge>(),
        MethodDescriptor::of::<vcs::Init>(),
        // snapshots (Phase 1)
        MethodDescriptor::of::<snapshots::List>(),
        MethodDescriptor::of::<snapshots::Get>(),
        MethodDescriptor::of::<snapshots::Create>(),
        MethodDescriptor::of::<snapshots::Diff>(),
        MethodDescriptor::of::<snapshots::RestoreFile>(),
        MethodDescriptor::of::<snapshots::PlanRollback>(),
        MethodDescriptor::of::<snapshots::RollbackWorkspace>(),
        MethodDescriptor::of::<snapshots::RollbackTask>(),
        MethodDescriptor::of::<snapshots::Delete>(),
        // terminal (Phase 1)
        MethodDescriptor::of::<terminal::Shells>(),
        MethodDescriptor::of::<terminal::Open>(),
        MethodDescriptor::of::<terminal::Write>(),
        MethodDescriptor::of::<terminal::ExecLine>(),
        MethodDescriptor::of::<terminal::Resize>(),
        MethodDescriptor::of::<terminal::Rename>(),
        MethodDescriptor::of::<terminal::Close>(),
        MethodDescriptor::of::<terminal::List>(),
        MethodDescriptor::of::<terminal::History>(),
        MethodDescriptor::of::<terminal::Run>(),
        // files.* (Phase 1, files-editor)
        MethodDescriptor::of::<files::List>(),
        MethodDescriptor::of::<files::Stat>(),
        MethodDescriptor::of::<files::Read>(),
        MethodDescriptor::of::<files::Write>(),
        MethodDescriptor::of::<files::Create>(),
        MethodDescriptor::of::<files::Mkdir>(),
        MethodDescriptor::of::<files::Rename>(),
        MethodDescriptor::of::<files::Delete>(),
        MethodDescriptor::of::<files::Trash>(),
        MethodDescriptor::of::<files::Restore>(),
        MethodDescriptor::of::<files::Search>(),
        MethodDescriptor::of::<files::Find>(),
        MethodDescriptor::of::<files::Watch>(),
        MethodDescriptor::of::<files::Unwatch>(),
        MethodDescriptor::of::<files::OpenExternal>(),
        // memory-context-cost (Phase 1)
        MethodDescriptor::of::<memory::List>(),
        MethodDescriptor::of::<memory::Get>(),
        MethodDescriptor::of::<memory::Create>(),
        MethodDescriptor::of::<memory::Update>(),
        MethodDescriptor::of::<memory::Delete>(),
        MethodDescriptor::of::<memory::Search>(),
        MethodDescriptor::of::<memory::EffectiveRules>(),
        MethodDescriptor::of::<memory::DecisionsList>(),
        MethodDescriptor::of::<memory::DecisionsGet>(),
        MethodDescriptor::of::<memory::DecisionsCreate>(),
        MethodDescriptor::of::<memory::DecisionsUpdate>(),
        MethodDescriptor::of::<memory::KnowledgeList>(),
        MethodDescriptor::of::<memory::KnowledgeAdd>(),
        MethodDescriptor::of::<memory::KnowledgeRemove>(),
        MethodDescriptor::of::<memory::KnowledgeReindex>(),
        MethodDescriptor::of::<memory::KnowledgeSearch>(),
        MethodDescriptor::of::<cost::Summary>(),
        MethodDescriptor::of::<cost::ByDimension>(),
        MethodDescriptor::of::<cost::Timeseries>(),
        MethodDescriptor::of::<cost::Records>(),
        MethodDescriptor::of::<cost::BudgetsList>(),
        MethodDescriptor::of::<cost::BudgetsSet>(),
        MethodDescriptor::of::<cost::BudgetsDelete>(),
        MethodDescriptor::of::<cost::BudgetsCheck>(),
        // platform-shell (Phase 1)
        MethodDescriptor::of::<system::Checks>(),
        MethodDescriptor::of::<system::ResumeSummaryMethod>(),
        MethodDescriptor::of::<settings::Get>(),
        MethodDescriptor::of::<settings::Update>(),
        MethodDescriptor::of::<nodes::List>(),
        MethodDescriptor::of::<nodes::Info>(),
        MethodDescriptor::of::<nodes::Resources>(),
        // agent runtime (Phase 1)
        MethodDescriptor::of::<templates::List>(),
        MethodDescriptor::of::<templates::Get>(),
        MethodDescriptor::of::<templates::Create>(),
        MethodDescriptor::of::<templates::Update>(),
        MethodDescriptor::of::<templates::Delete>(),
        MethodDescriptor::of::<templates::Export>(),
        MethodDescriptor::of::<templates::Import>(),
        MethodDescriptor::of::<agents::List>(),
        MethodDescriptor::of::<agents::Get>(),
        MethodDescriptor::of::<agents::Spawn>(),
        MethodDescriptor::of::<agents::Stop>(),
        MethodDescriptor::of::<agents::Pause>(),
        MethodDescriptor::of::<agents::Resume>(),
        MethodDescriptor::of::<agents::Transcript>(),
        MethodDescriptor::of::<agents::HandoffList>(),
        MethodDescriptor::of::<agents::Stats>(),
        // AI Manager (Phase 1)
        MethodDescriptor::of::<manager::Chat>(),
        MethodDescriptor::of::<manager::History>(),
        MethodDescriptor::of::<manager::Clear>(),
        MethodDescriptor::of::<manager::PlanMethod>(),
        MethodDescriptor::of::<manager::ApprovePlan>(),
        MethodDescriptor::of::<manager::RejectPlan>(),
        MethodDescriptor::of::<manager::Run>(),
        MethodDescriptor::of::<manager::Pause>(),
        MethodDescriptor::of::<manager::Resume>(),
        MethodDescriptor::of::<manager::Cancel>(),
        MethodDescriptor::of::<manager::Status>(),
        MethodDescriptor::of::<manager::ResumeSummaryMethod>(),
        // quality runner + gates (Phase 2)
        MethodDescriptor::of::<quality::Detect>(),
        MethodDescriptor::of::<quality::GetProfile>(),
        MethodDescriptor::of::<quality::SetProfile>(),
        MethodDescriptor::of::<quality::Run>(),
        MethodDescriptor::of::<quality::Runs>(),
        MethodDescriptor::of::<quality::GetRun>(),
        MethodDescriptor::of::<quality::Cancel>(),
        MethodDescriptor::of::<quality::Gates>(),
        MethodDescriptor::of::<quality::SetGates>(),
        MethodDescriptor::of::<quality::Evaluate>(),
    ]
}

/// Checks a method name has the `<area>.<verb>` shape with a lowerCamel verb.
#[must_use]
pub fn is_valid_method_name(name: &str) -> bool {
    let Some((area, verb)) = name.split_once('.') else {
        return false;
    };
    let lower_ident = |s: &str| {
        let mut chars = s.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_lowercase()) && chars.all(|c| c.is_ascii_alphanumeric())
    };
    lower_ident(area) && lower_ident(verb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_names_are_valid_and_unique() {
        let reg = registry();
        let mut names: Vec<_> = reg.iter().map(|m| m.name).collect();
        for n in &names {
            assert!(is_valid_method_name(n), "bad method name {n}");
        }
        names.sort_unstable();
        let len = names.len();
        names.dedup();
        assert_eq!(len, names.len(), "duplicate method names");
    }

    #[test]
    fn name_validation() {
        assert!(is_valid_method_name("projects.list"));
        assert!(is_valid_method_name("emergency.stopAll"));
        assert!(!is_valid_method_name("Projects.list"));
        assert!(!is_valid_method_name("projects"));
        assert!(!is_valid_method_name("projects.list.x"));
    }
}
