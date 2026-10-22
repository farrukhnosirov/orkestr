//! Deployment logs: redacted lines buffered in memory while a deployment runs,
//! then stored in the object store (`Deployment.log_object_id`).

use std::collections::HashMap;
use std::sync::Arc;

use orkestr_domain::DeploymentId;
use orkestr_storage::{FsObjectStore, ObjectId, ObjectStore};
use parking_lot::Mutex;

use crate::DeployError;

/// Max lines kept per deployment (older lines past this are dropped with a marker).
pub const MAX_LINES: usize = 20_000;
/// Max characters per line.
const MAX_LINE: usize = 4_000;

#[derive(Default)]
struct Live {
    lines: Vec<String>,
    secrets: Vec<String>,
    dropped: bool,
}

/// Redacts `line` with the deployment's own secret values plus the global redactor.
#[must_use]
pub fn redact_with(line: &str, secrets: &[String]) -> String {
    let mut out = line.to_owned();
    for s in secrets {
        if s.len() >= 4 && out.contains(s.as_str()) {
            out = out.replace(s.as_str(), "[REDACTED]");
        }
    }
    let mut out = orkestr_security::redact::redact(&out);
    if out.len() > MAX_LINE {
        let mut cut = MAX_LINE;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push('…');
    }
    out
}

/// Per-deployment log buffers.
pub struct LogBook {
    objects: Arc<FsObjectStore>,
    live: Mutex<HashMap<DeploymentId, Live>>,
}

impl std::fmt::Debug for LogBook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogBook").field("live", &self.live.lock().len()).finish()
    }
}

impl LogBook {
    /// Log book storing finished logs in `objects`.
    #[must_use]
    pub fn new(objects: Arc<FsObjectStore>) -> Self {
        Self { objects, live: Mutex::new(HashMap::new()) }
    }

    /// Starts (or extends) the buffer of `id`, scrubbing `secrets`.
    pub fn open(&self, id: DeploymentId, secrets: Vec<String>) {
        let mut live = self.live.lock();
        let l = live.entry(id).or_default();
        l.secrets.extend(secrets.into_iter().filter(|s| s.len() >= 4));
        l.secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    }

    /// Appends one line; returns the redacted line.
    pub fn push(&self, id: DeploymentId, line: &str) -> String {
        let mut live = self.live.lock();
        let l = live.entry(id).or_default();
        let red = redact_with(line, &l.secrets);
        if l.lines.len() >= MAX_LINES {
            if !l.dropped {
                l.dropped = true;
                l.lines.push(format!("[orkestr] log truncated after {MAX_LINES} lines"));
            }
        } else {
            l.lines.push(red.clone());
        }
        red
    }

    /// Whether `id` is still buffering.
    #[must_use]
    pub fn is_live(&self, id: DeploymentId) -> bool {
        self.live.lock().contains_key(&id)
    }

    /// Stores the buffer in the object store and forgets it.
    pub async fn finish(&self, id: DeploymentId) -> Result<Option<String>, DeployError> {
        let Some(l) = self.live.lock().remove(&id) else { return Ok(None) };
        let mut text = l.lines.join("\n");
        text.push('\n');
        let oid = self.objects.put(text.into_bytes()).await?;
        Ok(Some(oid.to_string()))
    }

    /// Lines from `offset`: from the live buffer, else from the stored object.
    pub async fn read(
        &self,
        id: DeploymentId,
        object: Option<&str>,
        offset: u64,
    ) -> Result<(Vec<String>, u64, bool), DeployError> {
        let off = usize::try_from(offset).unwrap_or(usize::MAX);
        if let Some(l) = self.live.lock().get(&id) {
            let lines: Vec<String> = l.lines.iter().skip(off).cloned().collect();
            let next = offset + lines.len() as u64;
            return Ok((lines, next, false));
        }
        let Some(object) = object else { return Ok((Vec::new(), offset, true)) };
        let oid = ObjectId::parse(object)?;
        let bytes = self.objects.get(&oid).await?.unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        let all: Vec<&str> = text.lines().collect();
        let lines: Vec<String> = all.iter().skip(off).map(|s| (*s).to_owned()).collect();
        let next = offset + lines.len() as u64;
        Ok((lines, next, true))
    }
}
