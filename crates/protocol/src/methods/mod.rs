//! RPC methods: `<area>.<verb>` names with typed params/results (contract §4).
//!
//! Each area has its own module. A method is a zero-sized marker type
//! implementing [`Method`]; the core router registers handlers by marker type
//! and the TS `Methods` map is generated from [`registry()`].
//!
//! Implemented areas: [`system`], [`projects`], [`events`].
//! Placeholder areas (names reserved, types arrive with their phase):
//! [`tasks`], [`agents`], [`providers`], [`terminal`], [`approvals`],
//! [`emergency`], [`memory`], [`vcs`], [`files`], [`snapshots`], [`cost`],
//! [`nodes`], [`settings`].

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub mod agents;
pub mod approvals;
pub mod cost;
pub mod emergency;
pub mod events;
pub mod files;
pub mod memory;
pub mod nodes;
pub mod projects;
pub mod providers;
pub mod settings;
pub mod snapshots;
pub mod system;
pub mod tasks;
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
