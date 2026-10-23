//! Persistence of user command sets (`<data>/sets/<id>.json`, one file per
//! set so they are easy to back up and share) and of the built-in sets'
//! enabled flags (`<data>/state.json`). Import / export format:
//! `{"format":"orkestr-command-set","version":1,"set":{…}}`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use orkestr_protocol::methods::computer::CommandSet;

use crate::catalogue::validate_set;
use crate::error::{ComputerError, ComputerResult};

/// Export format id.
pub const SET_FORMAT: &str = "orkestr-command-set";

/// Persisted flags.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// Disabled built-in sets.
    #[serde(default)]
    pub disabled_builtin: Vec<String>,
}

/// The store.
#[derive(Debug, Clone)]
pub struct SetStore {
    dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    format: String,
    version: u32,
    set: CommandSet,
}

fn write_atomic(path: &Path, bytes: &[u8]) -> ComputerResult<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl SetStore {
    /// Store rooted at `dir`.
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn sets_dir(&self) -> PathBuf {
        self.dir.join("sets")
    }

    /// Loads every valid user set (invalid files are skipped with a warning).
    #[must_use]
    pub fn load_user_sets(&self) -> Vec<CommandSet> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.sets_dir()) else { return out };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read_to_string(&p).map_err(ComputerError::from).and_then(|t| parse_set_document(&t)) {
                Ok(mut s) => {
                    s.builtin = false;
                    out.push(s);
                }
                Err(err) => tracing::warn!(path = %p.display(), error = %err, "skipping invalid command set"),
            }
        }
        out.sort_by_key(|s| s.name.to_lowercase());
        out
    }

    /// Saves a user set.
    ///
    /// # Errors
    /// `Invalid` / IO.
    pub fn save(&self, set: &CommandSet) -> ComputerResult<()> {
        validate_set(set)?;
        write_atomic(&self.sets_dir().join(format!("{}.json", set.id)), export_document(set).as_bytes())
    }

    /// Deletes a user set file.
    ///
    /// # Errors
    /// IO.
    pub fn delete(&self, id: &str) -> ComputerResult<()> {
        let p = self.sets_dir().join(format!("{id}.json"));
        if p.exists() {
            std::fs::remove_file(p)?;
        }
        Ok(())
    }

    /// Flags.
    #[must_use]
    pub fn state(&self) -> State {
        std::fs::read_to_string(self.dir.join("state.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Saves flags.
    ///
    /// # Errors
    /// IO.
    pub fn save_state(&self, s: &State) -> ComputerResult<()> {
        let json = serde_json::to_vec_pretty(s).map_err(|e| ComputerError::Invalid(e.to_string()))?;
        write_atomic(&self.dir.join("state.json"), &json)
    }
}

/// Export document of a set.
#[must_use]
pub fn export_document(set: &CommandSet) -> String {
    let mut s = set.clone();
    s.builtin = false;
    serde_json::to_string_pretty(&Envelope { format: SET_FORMAT.into(), version: 1, set: s }).unwrap_or_default()
}

/// Parses an export document or a bare set; validates it.
///
/// # Errors
/// `Invalid`.
pub fn parse_set_document(text: &str) -> ComputerResult<CommandSet> {
    if text.len() > 8 * 1024 * 1024 {
        return Err(ComputerError::Invalid("command set file is too large (max 8 MB)".into()));
    }
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ComputerError::Invalid(format!("not JSON: {e}")))?;
    let set: CommandSet = if v.get("format").is_some() {
        let env: Envelope =
            serde_json::from_value(v).map_err(|e| ComputerError::Invalid(format!("bad command set: {e}")))?;
        if env.format != SET_FORMAT || env.version != 1 {
            return Err(ComputerError::Invalid(format!("unsupported format {} v{}", env.format, env.version)));
        }
        env.set
    } else {
        serde_json::from_value(v).map_err(|e| ComputerError::Invalid(format!("bad command set: {e}")))?
    };
    validate_set(&set)?;
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set() -> CommandSet {
        serde_json::from_value(serde_json::json!({
            "id": "work", "name": "Work", "commands": [
                {"id": "standup", "phrases": ["планёрка", "standup"], "steps": [{"type": "OpenUrl", "url": "meet.example.com"}]}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn save_load_export_import_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let st = SetStore::new(d.path().to_path_buf());
        st.save(&set()).unwrap();
        let loaded = st.load_user_sets();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].enabled, "enabled by default");
        let doc = export_document(&loaded[0]);
        assert!(doc.contains(SET_FORMAT));
        assert_eq!(parse_set_document(&doc).unwrap().commands, set().commands);
        // bare sets import too
        let bare = serde_json::to_string(&set()).unwrap();
        assert_eq!(parse_set_document(&bare).unwrap().id, "work");
        st.delete("work").unwrap();
        assert!(st.load_user_sets().is_empty());
    }

    #[test]
    fn invalid_files_are_rejected_or_skipped() {
        assert!(parse_set_document("{").is_err());
        assert!(parse_set_document(r#"{"format":"other","version":1,"set":{}}"#).is_err());
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("sets")).unwrap();
        std::fs::write(d.path().join("sets/bad.json"), "{\"id\":\"BAD ID\"}").unwrap();
        assert!(SetStore::new(d.path().to_path_buf()).load_user_sets().is_empty());
    }

    #[test]
    fn state_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let st = SetStore::new(d.path().to_path_buf());
        assert_eq!(st.state(), State::default());
        st.save_state(&State { disabled_builtin: vec!["media".into()] }).unwrap();
        assert_eq!(st.state().disabled_builtin, ["media"]);
    }
}
