//! Log sources (files, deployment logs, ingest). The [`LogSource`] trait is
//! binding (WP6 collectors use it). **Owner: WP5.**

use async_trait::async_trait;
use orkestr_protocol::methods::monitor::{LogLine, LogQuery};
use tokio::sync::broadcast;

use crate::MonitorError;

/// A source of (redacted) log lines.
#[async_trait]
pub trait LogSource: Send + Sync {
    /// Source id.
    fn id(&self) -> &str;
    /// Queries stored / tailed lines.
    async fn query(&self, q: &LogQuery) -> Result<Vec<LogLine>, MonitorError>;
    /// Live tail (lagging receivers drop lines: backpressure never blocks the source).
    fn tail(&self) -> broadcast::Receiver<LogLine>;
}
