//! Knowledge base (§20): file/folder sources chunked into an FTS5 index.
//!
//! Supported now: Markdown, plain text and source-code documentation files
//! ([`SUPPORTED_EXTENSIONS`]). PDF and URL sources are *recorded* with status
//! `Failed` and an explicit "not supported yet" message instead of pretending
//! to index them.

use std::path::{Path, PathBuf};

use orkestr_domain::{now, IndexStatus, KnowledgeKind, KnowledgeSource, KnowledgeSourceId, ProjectId};
use orkestr_protocol::methods::memory::{AddKnowledgeParams, KnowledgeHit, SearchKnowledgeParams};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::StorageError;

use crate::chunk::{self, Chunk};
use crate::fts::{self, Combine};
use crate::util::{clamp_limit, ts_key};
use crate::{MemoryEngine, MemoryError, MemoryResult};

/// File extensions that are ingested (lowercase).
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    // documents
    "md", "markdown", "mdx", "txt", "text", "rst", "adoc", "asciidoc", "org", // code docs / schemas
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "cs", "java", "kt", "rb", "php", "c", "h", "cpp", "hpp",
    "swift", "sql", "graphql", "proto", "toml", "yaml", "yml", "json", "ini", "cfg", "sh", "ps1",
];

const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdx"];

/// Message stored for PDF sources.
pub const PDF_UNSUPPORTED: &str =
    "PDF ingestion is not supported yet. Convert the PDF to Markdown or text and add that file instead.";
/// Message stored for URL sources.
pub const URL_UNSUPPORTED: &str =
    "URL sources are not supported yet. Save the page as Markdown or text and add the file instead.";

/// Max location length.
const MAX_LOCATION_LEN: usize = 4096;

/// Ingestion limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestLimits {
    /// Max files per source.
    pub max_files: usize,
    /// Files larger than this are skipped.
    pub max_file_bytes: u64,
    /// Max chunks per source.
    pub max_chunks: usize,
    /// Max folder depth.
    pub max_depth: usize,
    /// Max characters per chunk.
    pub chunk_chars: usize,
}

impl Default for IngestLimits {
    fn default() -> Self {
        Self { max_files: 2000, max_file_bytes: 1024 * 1024, max_chunks: 20_000, max_depth: 16, chunk_chars: 1600 }
    }
}

#[derive(Debug, Default)]
struct Ingested {
    chunks: Vec<(String, u32, Chunk)>,
    files: u32,
    notes: Vec<String>,
}

fn extension(path: &Path) -> Option<String> {
    path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase)
}

fn is_url(location: &str) -> bool {
    let l = location.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

fn infer_kind(path: &Path) -> KnowledgeKind {
    if path.is_dir() {
        return KnowledgeKind::Docs;
    }
    match extension(path).as_deref() {
        Some("pdf") => KnowledgeKind::Pdf,
        Some(e) if MARKDOWN_EXTENSIONS.contains(&e) => KnowledgeKind::Markdown,
        Some("sql" | "graphql" | "proto" | "json") => KnowledgeKind::Schema,
        _ => KnowledgeKind::Other,
    }
}

fn read_text(path: &Path, limits: &IngestLimits) -> Result<Option<String>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if meta.len() > limits.max_file_bytes {
        return Ok(None);
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

fn chunk_file(rel: &str, path: &Path, text: &str, limits: &IngestLimits, out: &mut Ingested) {
    let is_md = extension(path).is_some_and(|e| MARKDOWN_EXTENSIONS.contains(&e.as_str()));
    let chunks = if is_md {
        chunk::chunk_markdown(text, limits.chunk_chars)
    } else {
        chunk::chunk_plain(text, limits.chunk_chars)
    };
    for (i, c) in chunks.into_iter().enumerate() {
        if out.chunks.len() >= limits.max_chunks {
            return;
        }
        out.chunks.push((rel.to_owned(), u32::try_from(i).unwrap_or(u32::MAX), c));
    }
}

/// Reads and chunks a file or folder (blocking).
fn ingest(location: &Path, limits: &IngestLimits) -> Result<Ingested, String> {
    let meta = std::fs::metadata(location).map_err(|_| format!("location no longer exists: {}", location.display()))?;
    let mut out = Ingested::default();
    if meta.is_file() {
        let ext = extension(location);
        if ext.as_deref() == Some("pdf") {
            return Err(PDF_UNSUPPORTED.to_owned());
        }
        if !ext.as_deref().is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e)) {
            return Err(format!(
                "unsupported file type `{}`; supported: Markdown, text and source/doc files",
                ext.unwrap_or_default()
            ));
        }
        let name = location.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match read_text(location, limits)? {
            Some(text) => {
                chunk_file(&name, location, &text, limits, &mut out);
                out.files = 1;
            }
            None => return Err("file is binary or larger than the size limit".to_owned()),
        }
        return Ok(out);
    }

    let mut files: Vec<PathBuf> = Vec::new();
    let (mut pdfs, mut skipped_errors) = (0usize, 0usize);
    let walker = ignore::WalkBuilder::new(location)
        .hidden(true)
        .git_ignore(true)
        .follow_links(false)
        .max_depth(Some(limits.max_depth))
        .build();
    for entry in walker {
        let Ok(entry) = entry else {
            skipped_errors += 1;
            continue;
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.into_path();
        match extension(&path).as_deref() {
            Some("pdf") => pdfs += 1,
            Some(e) if SUPPORTED_EXTENSIONS.contains(&e) => files.push(path),
            _ => {}
        }
    }
    files.sort();
    let total = files.len();
    if total > limits.max_files {
        out.notes.push(format!("indexed the first {} of {total} files (file limit)", limits.max_files));
        files.truncate(limits.max_files);
    }
    let mut skipped = 0usize;
    for path in &files {
        let rel = path.strip_prefix(location).unwrap_or(path).to_string_lossy().replace('\\', "/");
        match read_text(path, limits) {
            Ok(Some(text)) => {
                chunk_file(&rel, path, &text, limits, &mut out);
                out.files += 1;
            }
            Ok(None) | Err(_) => skipped += 1,
        }
        if out.chunks.len() >= limits.max_chunks {
            out.notes.push(format!("stopped at {} chunks (chunk limit)", limits.max_chunks));
            break;
        }
    }
    if pdfs > 0 {
        out.notes.push(format!("skipped {pdfs} PDF file(s): PDF ingestion is not supported yet"));
    }
    if skipped > 0 {
        out.notes.push(format!("skipped {skipped} binary, unreadable or oversized file(s)"));
    }
    if skipped_errors > 0 {
        out.notes.push(format!("{skipped_errors} folder entr(y/ies) could not be read"));
    }
    Ok(out)
}

impl MemoryEngine {
    /// Registers a file/folder (or records an unsupported PDF/URL) and ingests it.
    pub async fn add_knowledge(&self, p: AddKnowledgeParams) -> MemoryResult<KnowledgeSource> {
        if let Some(pid) = p.project_id {
            self.ensure_project(pid).await?;
        }
        let location = p.location.trim().to_owned();
        if location.is_empty() || location.len() > MAX_LOCATION_LEN {
            return Err(MemoryError::Invalid("location must be a non-empty path or URL".into()));
        }
        let (kind, default_name) = if is_url(&location) {
            (KnowledgeKind::Url, location.clone())
        } else {
            let path = Path::new(&location);
            if !path.is_absolute() {
                return Err(MemoryError::Invalid("location must be an absolute path".into()));
            }
            if !path.exists() {
                return Err(MemoryError::Invalid(format!("location does not exist: {location}")));
            }
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| location.clone());
            (infer_kind(path), name)
        };
        let name = p.name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()).unwrap_or(default_name);
        if name.chars().count() > 200 {
            return Err(MemoryError::Invalid("name must be at most 200 characters".into()));
        }
        let source = KnowledgeSource {
            id: KnowledgeSourceId::new(),
            project_id: p.project_id,
            name,
            kind: p.kind.unwrap_or(kind),
            location,
            object_id: None,
            status: IndexStatus::Pending,
            indexed_at: None,
            created_at: now(),
            error: None,
            chunk_count: 0,
            file_count: 0,
        };
        let doc = serde_json::to_string(&source).map_err(StorageError::from)?;
        let (id, project, name, created, loc) = (
            source.id.to_string(),
            source.project_id.map(|p| p.to_string()),
            source.name.clone(),
            ts_key(&source.created_at),
            source.location.clone(),
        );
        let inserted = self
            .db
            .call(move |c| {
                let dup: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM knowledge_sources
                     WHERE project_id IS ?1 AND json_extract(doc, '$.location') = ?2)",
                    params![project, loc],
                    |r| r.get(0),
                )?;
                if dup {
                    return Ok(false);
                }
                c.execute(
                    "INSERT INTO knowledge_sources (id, project_id, name, created_at, doc) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![id, project, name, created, doc],
                )?;
                Ok(true)
            })
            .await?;
        if !inserted {
            return Err(MemoryError::Conflict(
                "this location is already a knowledge source; reindex it instead".into(),
            ));
        }
        tracing::info!(source_id = %source.id, kind = ?source.kind, "knowledge source added");
        self.index_source(source).await
    }

    async fn save_source(&self, source: &KnowledgeSource) -> MemoryResult<()> {
        let doc = serde_json::to_string(source).map_err(StorageError::from)?;
        let (id, name) = (source.id.to_string(), source.name.clone());
        let n = self
            .db
            .call(move |c| {
                Ok(c.execute("UPDATE knowledge_sources SET name = ?2, doc = ?3 WHERE id = ?1", params![id, name, doc])?)
            })
            .await?;
        if n == 0 {
            return Err(MemoryError::NotFound(format!("knowledge source {}", source.id)));
        }
        Ok(())
    }

    async fn replace_chunks(&self, source: KnowledgeSourceId, chunks: Vec<(String, u32, Chunk)>) -> MemoryResult<()> {
        let id = source.to_string();
        self.db
            .call(move |c| {
                let tx = c.transaction()?;
                tx.execute("DELETE FROM knowledge_chunks WHERE source_id = ?1", [&id])?;
                {
                    let mut stmt = tx.prepare(
                        "INSERT INTO knowledge_chunks (source_id, path, ordinal, heading_path, text, token_count)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    )?;
                    for (path, ordinal, chunk) in &chunks {
                        stmt.execute(params![
                            id,
                            path,
                            ordinal,
                            chunk.heading_path,
                            chunk.text,
                            chunk::estimate_tokens(&chunk.text)
                        ])?;
                    }
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    async fn index_source(&self, mut source: KnowledgeSource) -> MemoryResult<KnowledgeSource> {
        let unsupported = match source.kind {
            KnowledgeKind::Url => Some(URL_UNSUPPORTED.to_owned()),
            KnowledgeKind::Pdf => Some(PDF_UNSUPPORTED.to_owned()),
            _ if is_url(&source.location) => Some(URL_UNSUPPORTED.to_owned()),
            _ => None,
        };
        let result = match unsupported {
            Some(msg) => Err(msg),
            None => {
                source.status = IndexStatus::Indexing;
                self.save_source(&source).await?;
                let location = PathBuf::from(&source.location);
                let limits = self.limits;
                tokio::task::spawn_blocking(move || ingest(&location, &limits))
                    .await
                    .map_err(|e| MemoryError::Storage(StorageError::Join(e.to_string())))?
            }
        };
        match result {
            Ok(ingested) => {
                source.chunk_count = u32::try_from(ingested.chunks.len()).unwrap_or(u32::MAX);
                source.file_count = ingested.files;
                source.error = (!ingested.notes.is_empty()).then(|| ingested.notes.join("; "));
                self.replace_chunks(source.id, ingested.chunks).await?;
                source.status = IndexStatus::Ready;
                source.indexed_at = Some(now());
                tracing::info!(source_id = %source.id, chunks = source.chunk_count, files = source.file_count, "knowledge source indexed");
            }
            Err(msg) => {
                self.replace_chunks(source.id, Vec::new()).await?;
                source.chunk_count = 0;
                source.file_count = 0;
                source.status = IndexStatus::Failed;
                source.error = Some(msg);
                tracing::warn!(source_id = %source.id, error = ?source.error, "knowledge source not indexed");
            }
        }
        self.save_source(&source).await?;
        Ok(source)
    }

    /// Fetches one source.
    pub async fn get_knowledge(&self, id: KnowledgeSourceId) -> MemoryResult<KnowledgeSource> {
        let key = id.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM knowledge_sources WHERE id = ?1", [key], |r| r.get(0)).optional()?)
            })
            .await?;
        match doc {
            Some(d) => Ok(serde_json::from_str(&d).map_err(StorageError::from)?),
            None => Err(MemoryError::NotFound(format!("knowledge source {id}"))),
        }
    }

    /// Lists sources (newest first).
    pub async fn list_knowledge(
        &self,
        project: Option<ProjectId>,
        include_global: bool,
    ) -> MemoryResult<Vec<KnowledgeSource>> {
        let project = project.map(|p| p.to_string());
        let include_global = include_global || project.is_none();
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM knowledge_sources
                     WHERE project_id = ?1 OR (?2 AND project_id IS NULL)
                     ORDER BY created_at DESC, id DESC",
                )?;
                let rows = stmt.query_map(params![project, include_global], |r| r.get(0))?.collect::<Result<_, _>>()?;
                Ok(rows)
            })
            .await?;
        docs.iter().map(|d| Ok(serde_json::from_str(d).map_err(StorageError::from)?)).collect()
    }

    /// Removes a source and its chunks (files on disk are never touched).
    pub async fn remove_knowledge(&self, id: KnowledgeSourceId) -> MemoryResult<()> {
        let key = id.to_string();
        let n = self.db.call(move |c| Ok(c.execute("DELETE FROM knowledge_sources WHERE id = ?1", [key])?)).await?;
        if n == 0 {
            return Err(MemoryError::NotFound(format!("knowledge source {id}")));
        }
        tracing::info!(source_id = %id, "knowledge source removed");
        Ok(())
    }

    /// Re-reads the location and rebuilds the chunks.
    pub async fn reindex_knowledge(&self, id: KnowledgeSourceId) -> MemoryResult<KnowledgeSource> {
        let source = self.get_knowledge(id).await?;
        self.index_source(source).await
    }

    /// FTS5 search over chunks (bm25: text ×1, heading ×2, path ×1.5).
    pub async fn search_knowledge(&self, p: SearchKnowledgeParams) -> MemoryResult<Vec<KnowledgeHit>> {
        let limit = clamp_limit(p.limit, 30, 200);
        let include_global = p.project_id.is_none() || p.include_global.unwrap_or(true);
        let project = p.project_id.map(|id| id.to_string());
        for combine in [Combine::All, Combine::Any] {
            let Some(expr) = fts::match_expr(&p.query, combine) else {
                return Ok(Vec::new());
            };
            let project = project.clone();
            let rows: Vec<(String, String, String, String, u32, String, f64)> = self
                .db
                .call(move |c| {
                    let mut stmt = c.prepare(
                        "SELECT s.id, s.name, k.path, k.heading_path, k.ordinal,
                                snippet(knowledge_fts, 0, char(2), char(3), '…', 32),
                                bm25(knowledge_fts, 1.0, 2.0, 1.5) AS rank
                         FROM knowledge_fts
                         JOIN knowledge_chunks k ON k.id = knowledge_fts.rowid
                         JOIN knowledge_sources s ON s.id = k.source_id
                         WHERE knowledge_fts MATCH ?1 AND (s.project_id = ?2 OR (?3 AND s.project_id IS NULL))
                         ORDER BY rank LIMIT ?4",
                    )?;
                    let rows = stmt
                        .query_map(params![expr, project, include_global, limit], |r| {
                            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
                        })?
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(rows)
                })
                .await?;
            if !rows.is_empty() || fts::terms(&p.query).len() < 2 {
                return rows
                    .into_iter()
                    .map(|(sid, name, path, heading_path, ordinal, snippet, rank)| {
                        Ok(KnowledgeHit {
                            source_id: KnowledgeSourceId::parse(&sid)
                                .map_err(|e| StorageError::Corrupt(e.to_string()))?,
                            source_name: name,
                            path,
                            heading_path,
                            ordinal,
                            snippet,
                            score: fts::score_from_rank(rank),
                        })
                    })
                    .collect();
            }
        }
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{engine, project};

    fn add(location: &Path, project_id: Option<ProjectId>) -> AddKnowledgeParams {
        AddKnowledgeParams { project_id, location: location.to_string_lossy().into_owned(), name: None, kind: None }
    }

    fn search(q: &str, project_id: Option<ProjectId>) -> SearchKnowledgeParams {
        SearchKnowledgeParams { query: q.into(), project_id, include_global: None, limit: None }
    }

    #[tokio::test]
    async fn folder_ingest_search_reindex_remove() {
        let (eng, _bus) = engine().await;
        let pid = project(&eng, "Docs").await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("guide")).unwrap();
        std::fs::write(
            dir.path().join("guide/install.md"),
            "# Install\nGeneral.\n## Windows\nRun the MSI installer with PowerShell.\n## Linux\nUse apt.\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("api.ts"), "export function computeNetSalary() {}\n").unwrap();
        std::fs::write(dir.path().join("manual.pdf"), b"%PDF-1.4 binary").unwrap();
        std::fs::write(dir.path().join("image.png"), [0u8, 1, 2]).unwrap();

        let src = eng.add_knowledge(add(dir.path(), Some(pid))).await.unwrap();
        assert_eq!(src.status, IndexStatus::Ready);
        assert_eq!(src.kind, KnowledgeKind::Docs);
        assert_eq!(src.file_count, 2);
        assert_eq!(src.chunk_count, 4);
        assert!(src.error.as_deref().unwrap().contains("PDF"), "{:?}", src.error);
        assert!(src.indexed_at.is_some());

        let hits = eng.search_knowledge(search("powershell installer", Some(pid))).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "guide/install.md");
        assert_eq!(hits[0].heading_path, "Install › Windows");
        assert!(hits[0].snippet.contains('\u{2}'));
        // Heading text is searchable; global-only search does not see project sources.
        assert_eq!(eng.search_knowledge(search("linux", Some(pid))).await.unwrap().len(), 1);
        assert!(eng.search_knowledge(search("linux", None)).await.unwrap().is_empty());

        // Duplicate registration conflicts.
        assert!(matches!(eng.add_knowledge(add(dir.path(), Some(pid))).await, Err(MemoryError::Conflict(_))));

        // Reindex picks up changes.
        std::fs::write(dir.path().join("guide/faq.md"), "# FAQ\nQuokka questions.\n").unwrap();
        let re = eng.reindex_knowledge(src.id).await.unwrap();
        assert_eq!(re.file_count, 3);
        assert_eq!(eng.search_knowledge(search("quokka", Some(pid))).await.unwrap().len(), 1);
        assert_eq!(eng.list_knowledge(Some(pid), false).await.unwrap().len(), 1);

        eng.remove_knowledge(src.id).await.unwrap();
        assert!(eng.search_knowledge(search("quokka", Some(pid))).await.unwrap().is_empty());
        assert!(matches!(eng.remove_knowledge(src.id).await, Err(MemoryError::NotFound(_))));
        let left: i64 = eng
            .db()
            .call(|c| Ok(c.query_row("SELECT COUNT(*) FROM knowledge_chunks", [], |r| r.get(0))?))
            .await
            .unwrap();
        assert_eq!(left, 0, "chunks cascade with the source");
        assert!(dir.path().join("guide/install.md").exists(), "files on disk are never touched");
    }

    #[tokio::test]
    async fn single_files_pdf_url_and_errors() {
        let (eng, _bus) = engine().await;
        let dir = tempfile::tempdir().unwrap();
        let md = dir.path().join("rules.md");
        std::fs::write(&md, "# Rules\nAlways write tests.\n").unwrap();
        let src = eng.add_knowledge(add(&md, None)).await.unwrap();
        assert_eq!(src.kind, KnowledgeKind::Markdown);
        assert_eq!(src.status, IndexStatus::Ready);
        let hits = eng.search_knowledge(search("tests", None)).await.unwrap();
        assert_eq!(hits[0].path, "rules.md");

        let pdf = dir.path().join("spec.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        let p = eng.add_knowledge(add(&pdf, None)).await.unwrap();
        assert_eq!(p.status, IndexStatus::Failed);
        assert_eq!(p.kind, KnowledgeKind::Pdf);
        assert_eq!(p.error.as_deref(), Some(PDF_UNSUPPORTED));

        let url = eng
            .add_knowledge(AddKnowledgeParams {
                project_id: None,
                location: "https://example.com/docs".into(),
                name: Some("Example".into()),
                kind: None,
            })
            .await
            .unwrap();
        assert_eq!(url.status, IndexStatus::Failed);
        assert_eq!(url.name, "Example");

        let docx = dir.path().join("a.docx");
        std::fs::write(&docx, b"PK").unwrap();
        let d = eng.add_knowledge(add(&docx, None)).await.unwrap();
        assert_eq!(d.status, IndexStatus::Failed);
        assert!(d.error.unwrap().contains("unsupported file type"));

        assert!(matches!(
            eng.add_knowledge(AddKnowledgeParams {
                project_id: None,
                location: "relative/path".into(),
                name: None,
                kind: None
            })
            .await,
            Err(MemoryError::Invalid(_))
        ));
        assert!(matches!(
            eng.add_knowledge(add(&dir.path().join("missing"), None)).await,
            Err(MemoryError::Invalid(_))
        ));

        // A deleted location fails on reindex with a clear message and drops chunks.
        std::fs::remove_file(&md).unwrap();
        let re = eng.reindex_knowledge(src.id).await.unwrap();
        assert_eq!(re.status, IndexStatus::Failed);
        assert!(re.error.unwrap().contains("no longer exists"));
        assert!(eng.search_knowledge(search("tests", None)).await.unwrap().is_empty());
        assert_eq!(eng.list_knowledge(None, false).await.unwrap().len(), 4);
    }

    #[tokio::test]
    async fn limits_are_enforced() {
        let (eng, _bus) = engine().await;
        let eng = eng.with_limits(IngestLimits { max_files: 2, ..IngestLimits::default() });
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), format!("file number {i}")).unwrap();
        }
        let src = eng.add_knowledge(add(dir.path(), None)).await.unwrap();
        assert_eq!(src.file_count, 2);
        assert!(src.error.unwrap().contains("first 2 of 5"));
    }
}
