//! `files.*` — workspace files for the Files and Code panels (§14, §15), Phase 1.
//!
//! Every path on the wire is **relative to the workspace root** (the project
//! root folder, or an agent worktree under `<data_dir>/worktrees`), uses `/`
//! separators, and `""` denotes the root itself. The core rejects traversal,
//! absolute paths, UNC/device paths, alternate data streams and symlink
//! escapes (SR-21) before touching the disk.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AgentId, ProjectId, Timestamp};

use crate::event::FileChangeActor;

use super::method;

/// Kind of a directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FileKind {
    /// Regular file.
    File,
    /// Directory.
    Dir,
}

/// One file or directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileEntry {
    /// File name (last path component; root: the root folder name).
    pub name: String,
    /// Workspace-relative path (`/`-separated, `""` = root).
    pub path: String,
    /// File or directory (symlinks report their target's kind).
    pub kind: FileKind,
    /// Whether the entry is a symbolic link / junction.
    pub symlink: bool,
    /// Size in bytes (0 for directories).
    pub size: u64,
    /// Last modification time.
    pub modified: Option<Timestamp>,
    /// Matched by `.gitignore` / `.ignore` rules (only reported when ignored entries are shown).
    pub ignored: bool,
    /// Dot-file / dot-folder.
    pub hidden: bool,
    /// Read-only on disk.
    pub readonly: bool,
}

/// Text encoding of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TextEncoding {
    /// UTF-8 without BOM.
    #[default]
    Utf8,
    /// UTF-8 with BOM.
    Utf8Bom,
    /// UTF-16 little endian with BOM.
    Utf16Le,
    /// UTF-16 big endian with BOM.
    Utf16Be,
    /// Fallback for non-UTF-8 single-byte text (ISO-8859-1, lossless byte mapping).
    Latin1,
}

/// Dominant line ending of a text file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LineEnding {
    /// `\n`.
    #[default]
    Lf,
    /// `\r\n`.
    Crlf,
}

/// Params of `files.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListFilesParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent worktree to use instead of the project root (absolute, under `<data_dir>/worktrees`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// Directory to list (`""` = root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    /// Include git-ignored entries (marked `ignored`) and `.git`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub show_ignored: Option<bool>,
}

/// Result of `files.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListFilesResult {
    /// Listed directory (normalized).
    pub path: String,
    /// Children: directories first, then files, case-insensitive by name.
    pub entries: Vec<FileEntry>,
    /// More entries exist than were returned.
    pub truncated: bool,
}

/// Params of `files.stat`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FilePathParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent worktree (see [`ListFilesParams::worktree_path`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// Target path.
    pub path: String,
}

/// Result carrying one entry (`files.stat`, `files.create`, `files.mkdir`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileEntryResult {
    /// The entry.
    pub entry: FileEntry,
}

/// Params of `files.read`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReadFileParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// File path.
    pub path: String,
    /// Size limit in bytes (default 5 MiB, max 50 MiB). Larger files fail with `bad_request` + `data.size`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_bytes: Option<u64>,
}

/// Result of `files.read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReadFileResult {
    /// Normalized path.
    pub path: String,
    /// Decoded text; `null` for binary files.
    pub content: Option<String>,
    /// Binary file (content not returned).
    pub binary: bool,
    /// Detected encoding (meaningless for binary files).
    pub encoding: TextEncoding,
    /// Dominant line ending.
    pub line_ending: LineEnding,
    /// blake3 (hex) of the raw bytes on disk — pass as `expectedHash` when writing.
    pub hash: String,
    /// Size in bytes.
    pub size: u64,
    /// Monaco language id guessed from the file name.
    pub language: String,
    /// Last modification time.
    pub modified: Option<Timestamp>,
    /// Read-only on disk.
    pub readonly: bool,
}

/// Params of `files.write`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WriteFileParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// File path.
    pub path: String,
    /// New text content.
    pub content: String,
    /// Optimistic concurrency: hash from `files.read`. Mismatch → `conflict` with `data.actualHash`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub expected_hash: Option<String>,
    /// Encoding to write (default: UTF-8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub encoding: Option<TextEncoding>,
    /// Create the file (and missing parent folders) when it does not exist. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub create: Option<bool>,
    /// Attribute the change to an agent (agent tool bridges; the UI omits it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
}

/// Result of `files.write`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WriteFileResult {
    /// Normalized path.
    pub path: String,
    /// New blake3 hash.
    pub hash: String,
    /// Bytes written.
    pub size: u64,
    /// The file did not exist before.
    pub created: bool,
}

/// Params of `files.create`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateFileParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// New file path (fails with `conflict` when it exists). Missing parents are created.
    pub path: String,
    /// Initial UTF-8 content (default empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub content: Option<String>,
    /// Attribute to an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
}

/// Params of `files.mkdir`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MkdirParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// New directory (parents created; fails with `conflict` when it exists).
    pub path: String,
    /// Attribute to an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
}

/// Params of `files.rename` (rename or move).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RenameFileParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// Source path.
    pub from: String,
    /// Destination path (parents created).
    pub to: String,
    /// Replace an existing destination **file** (never a directory). Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub overwrite: Option<bool>,
    /// Attribute to an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
}

/// Result of `files.rename`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RenameFileResult {
    /// Normalized source.
    pub from: String,
    /// The entry at its new location.
    pub entry: FileEntry,
}

/// Params of `files.delete`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteFileParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// Path to delete (never the root).
    pub path: String,
    /// Required to delete a non-empty directory. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub recursive: Option<bool>,
    /// Attribute to an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
}

/// A file copy kept in the trash (object store) for recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TrashItem {
    /// Trash entry id (pass to `files.restore`).
    pub id: String,
    /// Original workspace-relative path.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Deletion time.
    pub deleted_at: Timestamp,
    /// Who deleted it.
    pub actor: FileChangeActor,
    /// Already restored once.
    pub restored: bool,
}

/// Result of `files.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteFileResult {
    /// Normalized deleted path.
    pub path: String,
    /// Copies of every deleted file, recoverable with `files.restore`.
    pub trashed: Vec<TrashItem>,
}

/// Params of `files.trash`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListTrashParams {
    /// Project.
    pub project_id: ProjectId,
    /// Max entries (default 200, newest first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `files.trash`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListTrashResult {
    /// Entries, newest first.
    pub items: Vec<TrashItem>,
}

/// Params of `files.restore`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RestoreFileParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent worktree the entry was deleted from (if any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// Trash entry id.
    pub trash_id: String,
    /// Replace an existing file at the original path. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub overwrite: Option<bool>,
}

/// Result of `files.restore`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RestoreFileResult {
    /// Restored path.
    pub path: String,
    /// Hash of the restored content.
    pub hash: String,
}

/// Params of `files.search` (content search).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchFilesParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// Text or regular expression (Rust `regex` syntax).
    pub query: String,
    /// Treat `query` as a regular expression. Default false (literal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub is_regex: Option<bool>,
    /// Case sensitive. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub case_sensitive: Option<bool>,
    /// Whole words only. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub whole_word: Option<bool>,
    /// Only files matching any of these globs (e.g. `src/**`, `*.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub include: Option<Vec<String>>,
    /// Skip files matching any of these globs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub exclude: Option<Vec<String>>,
    /// Max matches (default 500, max 5000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_results: Option<u32>,
    /// Also search git-ignored files. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub show_ignored: Option<bool>,
}

/// One content match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchMatch {
    /// File path.
    pub path: String,
    /// 1-based line.
    pub line: u32,
    /// 1-based column in UTF-16 code units (Monaco columns).
    pub column: u32,
    /// Match length in UTF-16 code units.
    pub length: u32,
    /// The line (possibly shortened around the match).
    pub preview: String,
    /// Match start inside `preview` (UTF-16 code units).
    pub preview_start: u32,
    /// Match end inside `preview` (UTF-16 code units, exclusive).
    pub preview_end: u32,
}

/// Result of `files.search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchFilesResult {
    /// Matches sorted by path, line, column.
    pub matches: Vec<SearchMatch>,
    /// Text files searched.
    pub files_searched: u64,
    /// Stopped at `maxResults`.
    pub truncated: bool,
    /// Duration.
    pub elapsed_ms: u64,
}

/// Params of `files.find` (fuzzy file name search, quick open).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FindFilesParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// Fuzzy query (subsequence of the path; spaces ignored). Empty → first files by path.
    pub query: String,
    /// Max results (default 50, max 500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_results: Option<u32>,
    /// Include git-ignored files. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub show_ignored: Option<bool>,
}

/// One fuzzy match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FindMatch {
    /// File path.
    pub path: String,
    /// Score (higher is better).
    pub score: i64,
    /// Matched character positions in `path` (UTF-16 indices) for highlighting.
    pub positions: Vec<u32>,
}

/// Result of `files.find`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FindFilesResult {
    /// Best matches first.
    pub matches: Vec<FindMatch>,
    /// Files considered.
    pub files_scanned: u64,
    /// The scan stopped at the file limit.
    pub truncated: bool,
}

/// Params of `files.watch` / `files.unwatch`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WatchFilesParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
}

/// Result of `files.watch` / `files.unwatch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WatchFilesResult {
    /// Whether a watcher is active for the workspace after the call.
    pub watching: bool,
    /// Number of sessions watching the workspace.
    pub subscribers: u32,
}

/// How `files.openExternal` opens a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum OpenTarget {
    /// The OS default application (folders open in the file manager).
    #[default]
    Default,
    /// Show the item selected in the OS file manager.
    Reveal,
    /// An external IDE (`ide`, or the first one found on PATH: VS Code, Cursor, Windsurf, Zed, …).
    Ide,
}

/// Params of `files.openExternal`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OpenExternalParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// Path (default: the root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    /// Target (default: OS default app).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<OpenTarget>,
    /// IDE command for `target: "ide"` (allow-listed names only, e.g. `code`, `cursor`, `zed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ide: Option<String>,
}

/// Result of `files.openExternal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OpenExternalResult {
    /// Program that was launched.
    pub program: String,
}

method!(
    /// `files.list` — lazy, one directory level.
    List, "files.list", ListFilesParams, ListFilesResult
);
method!(
    /// `files.stat`.
    Stat, "files.stat", FilePathParams, FileEntryResult
);
method!(
    /// `files.read`.
    Read, "files.read", ReadFileParams, ReadFileResult
);
method!(
    /// `files.write` — atomic, optimistic concurrency; emits `FileChanged`.
    Write, "files.write", WriteFileParams, WriteFileResult
);
method!(
    /// `files.create` — emits `FileChanged`.
    Create, "files.create", CreateFileParams, FileEntryResult
);
method!(
    /// `files.mkdir` — emits `FileChanged`.
    Mkdir, "files.mkdir", MkdirParams, FileEntryResult
);
method!(
    /// `files.rename` — rename/move; emits `FileChanged` (Renamed).
    Rename, "files.rename", RenameFileParams, RenameFileResult
);
method!(
    /// `files.delete` — copies to the trash first; emits `FileChanged` (Deleted).
    Delete, "files.delete", DeleteFileParams, DeleteFileResult
);
method!(
    /// `files.trash` — recoverable deleted files.
    Trash, "files.trash", ListTrashParams, ListTrashResult
);
method!(
    /// `files.restore` — restores a trashed file; emits `FileChanged`.
    Restore, "files.restore", RestoreFileParams, RestoreFileResult
);
method!(
    /// `files.search` — parallel content search.
    Search, "files.search", SearchFilesParams, SearchFilesResult
);
method!(
    /// `files.find` — fuzzy file name search.
    Find, "files.find", FindFilesParams, FindFilesResult
);
method!(
    /// `files.watch` — start (ref-counted per session) emitting `FileChanged` for external changes.
    Watch, "files.watch", WatchFilesParams, WatchFilesResult
);
method!(
    /// `files.unwatch`.
    Unwatch, "files.unwatch", WatchFilesParams, WatchFilesResult
);
method!(
    /// `files.openExternal` — OS default app, reveal in file manager, or external IDE.
    OpenExternal, "files.openExternal", OpenExternalParams, OpenExternalResult
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_use_camel_case_and_defaults() {
        let p: ListFilesParams =
            serde_json::from_value(serde_json::json!({"projectId": ProjectId::new().to_string()})).unwrap();
        assert!(p.path.is_none());
        assert!(p.show_ignored.is_none());
        let w: WriteFileParams = serde_json::from_value(serde_json::json!({
            "projectId": ProjectId::new().to_string(), "path": "a.txt", "content": "x", "expectedHash": "ab",
            "encoding": "utf8Bom"
        }))
        .unwrap();
        assert_eq!(w.expected_hash.as_deref(), Some("ab"));
        assert_eq!(w.encoding, Some(TextEncoding::Utf8Bom));
        assert_eq!(serde_json::to_value(OpenTarget::Reveal).unwrap(), "reveal");
        assert_eq!(serde_json::to_value(FileKind::Dir).unwrap(), "dir");
    }
}
