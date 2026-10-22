//! The [`EvidenceCollector`] trait (binding). **Owner: WP6.**

use std::time::Duration;

use async_trait::async_trait;
use orkestr_protocol::methods::detective::{Evidence, EvidenceKind, Incident};

use crate::DetectiveError;

/// Limits of one collection pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvidenceBudget {
    /// Max evidence items per collector.
    pub max_items: usize,
    /// Max bytes of content per collector.
    pub max_bytes: usize,
    /// Look-back window.
    pub window: Duration,
}

impl Default for EvidenceBudget {
    fn default() -> Self {
        Self { max_items: 20, max_bytes: 32 * 1024, window: Duration::from_secs(3600) }
    }
}

/// Collects one kind of evidence for an incident.
#[async_trait]
pub trait EvidenceCollector: Send + Sync {
    /// Kind produced.
    fn kind(&self) -> EvidenceKind;
    /// Collects (redacted, within `budget`).
    async fn collect(&self, inc: &Incident, budget: &EvidenceBudget) -> Result<Vec<Evidence>, DetectiveError>;
}
