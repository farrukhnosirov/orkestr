//! `projects.*` — project CRUD and opening (§13).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{ApprovalMode, DataClassification, PrivacyPolicy, Project, ProjectId};

use super::method;

/// Max length of a project name (characters).
pub const MAX_NAME_LEN: usize = 120;
/// Max length of a project description (characters).
pub const MAX_DESCRIPTION_LEN: usize = 10_000;

/// Params of `projects.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListProjectsParams {
    /// Include archived projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub include_archived: Option<bool>,
}

/// Result of `projects.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListProjectsResult {
    /// Projects ordered by last opened (most recent first), then name.
    pub projects: Vec<Project>,
}

/// Params of `projects.get`, `projects.open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectIdParams {
    /// Target project.
    pub project_id: ProjectId,
}

/// Result of methods returning one project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectResult {
    /// The project.
    pub project: Project,
}

/// Params of `projects.create`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateProjectParams {
    /// Name (1..=120 chars, trimmed).
    pub name: String,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    /// Local root folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub root_path: Option<String>,
    /// Accent color (`#RRGGBB`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub color: Option<String>,
    /// Icon name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub icon: Option<String>,
    /// Classification (default Internal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub classification: Option<DataClassification>,
    /// Approval mode (default Ask).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval_mode: Option<ApprovalMode>,
    /// Privacy policy (default cloud allowed + sync allowed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub privacy: Option<PrivacyPolicy>,
    /// Tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tags: Option<Vec<String>>,
}

/// Partial update. Absent fields are unchanged. For nullable string fields
/// (`rootPath`, `color`, `icon`) an empty string clears the value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectPatch {
    /// New name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// New description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    /// New root path (`""` clears).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub root_path: Option<String>,
    /// New color (`""` clears).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub color: Option<String>,
    /// New icon (`""` clears).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub icon: Option<String>,
    /// New classification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub classification: Option<DataClassification>,
    /// New approval mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval_mode: Option<ApprovalMode>,
    /// New privacy policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub privacy: Option<PrivacyPolicy>,
    /// New tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tags: Option<Vec<String>>,
    /// Archive / unarchive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub archived: Option<bool>,
}

/// Params of `projects.update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateProjectParams {
    /// Target project.
    pub project_id: ProjectId,
    /// Changes.
    pub patch: ProjectPatch,
}

/// Params of `projects.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteProjectParams {
    /// Target project.
    pub project_id: ProjectId,
    /// Stop the project's running work first (agents, Manager runs, merges,
    /// quality runs — an emergency stop scoped to the project, plus aborting
    /// open merges) and wait for it to end. Without it a project with running
    /// work is refused with `conflict` (`data.reason = "project_busy"`).
    #[serde(default)]
    #[ts(optional)]
    pub stop_work: Option<bool>,
}

/// Result of `projects.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteProjectResult {
    /// Deleted project id.
    pub project_id: ProjectId,
}

/// Validation error for project input.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProjectValidationError {
    /// Empty name.
    #[error("project name must not be empty")]
    EmptyName,
    /// Name too long.
    #[error("project name must be at most {MAX_NAME_LEN} characters")]
    NameTooLong,
    /// Description too long.
    #[error("project description must be at most {MAX_DESCRIPTION_LEN} characters")]
    DescriptionTooLong,
    /// Bad color.
    #[error("color must be a #RRGGBB hex value")]
    InvalidColor,
}

/// Validates and normalizes a project name.
pub fn validate_name(name: &str) -> Result<String, ProjectValidationError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ProjectValidationError::EmptyName);
    }
    if trimmed.chars().count() > MAX_NAME_LEN {
        return Err(ProjectValidationError::NameTooLong);
    }
    Ok(trimmed.to_owned())
}

/// Validates a description.
pub fn validate_description(desc: &str) -> Result<(), ProjectValidationError> {
    if desc.chars().count() > MAX_DESCRIPTION_LEN {
        return Err(ProjectValidationError::DescriptionTooLong);
    }
    Ok(())
}

/// Validates a `#RRGGBB` color.
pub fn validate_color(color: &str) -> Result<(), ProjectValidationError> {
    let ok = color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit());
    if ok {
        Ok(())
    } else {
        Err(ProjectValidationError::InvalidColor)
    }
}

method!(
    /// `projects.list`.
    List, "projects.list", ListProjectsParams, ListProjectsResult
);
method!(
    /// `projects.get`.
    Get, "projects.get", ProjectIdParams, ProjectResult
);
method!(
    /// `projects.create` — emits `ProjectCreated`.
    Create, "projects.create", CreateProjectParams, ProjectResult
);
method!(
    /// `projects.update` — emits `ProjectUpdated`.
    Update, "projects.update", UpdateProjectParams, ProjectResult
);
method!(
    /// `projects.delete` — refuses (`conflict`) while the project has running
    /// work unless `stopWork`; emits `ProjectDeleted`.
    Delete, "projects.delete", DeleteProjectParams, DeleteProjectResult
);
method!(
    /// `projects.open` — marks the project as opened (sets `lastOpenedAt`), emits `ProjectUpdated`.
    Open, "projects.open", ProjectIdParams, ProjectResult
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_rules() {
        assert_eq!(validate_name("  Demo ").unwrap(), "Demo");
        assert_eq!(validate_name("   "), Err(ProjectValidationError::EmptyName));
        assert_eq!(validate_name(&"x".repeat(121)), Err(ProjectValidationError::NameTooLong));
    }

    #[test]
    fn color_rules() {
        assert!(validate_color("#a1B2c3").is_ok());
        assert!(validate_color("a1b2c3").is_err());
        assert!(validate_color("#a1b2c").is_err());
        assert!(validate_color("#zzzzzz").is_err());
    }

    #[test]
    fn create_params_minimal_json() {
        let p: CreateProjectParams = serde_json::from_str(r#"{"name":"X"}"#).unwrap();
        assert_eq!(p.name, "X");
        assert!(p.description.is_none());
    }
}
