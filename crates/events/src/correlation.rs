//! Ambient correlation ids (§50).
//!
//! Wrap the handling of one user request in [`with_correlation`]; every event
//! published inside that future (and not carrying an explicit id) gets the
//! same `correlationId`, which ties Manager plans, agent work and tool calls
//! together for Replay (§21) and log search.

use std::future::Future;

use orkestr_domain::CorrelationId;

tokio::task_local! {
    static CORRELATION: CorrelationId;
}

/// Creates a fresh correlation id.
#[must_use]
pub fn new_correlation() -> CorrelationId {
    CorrelationId::new()
}

/// Runs `fut` with `id` as the ambient correlation id.
pub async fn with_correlation<F: Future>(id: CorrelationId, fut: F) -> F::Output {
    CORRELATION.scope(id, fut).await
}

/// The ambient correlation id, if inside [`with_correlation`].
#[must_use]
pub fn current_correlation() -> Option<CorrelationId> {
    CORRELATION.try_with(|c| *c).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scoped_correlation() {
        assert!(current_correlation().is_none());
        let id = new_correlation();
        let seen = with_correlation(id, async { current_correlation() }).await;
        assert_eq!(seen, Some(id));
        assert!(current_correlation().is_none());
    }
}
