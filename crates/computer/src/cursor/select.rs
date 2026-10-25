//! Backend selection and failover for cursor mode (ADR-0050).
//!
//! The user picks a source (subscriptions ↔ API keys) and orders the
//! backends of each source. A session gets a [`FailoverModel`] over the
//! available, consented backends of the selected source in that order; an
//! auth / usage-limit / rate-limit / network / unavailable failure moves the
//! call to the next backend at once (sticky for the session) and reports
//! "switched to …".
//!
//! Everything about choosing a model lives in this module so it can later be
//! replaced by the central provider chain (wave 9 package D).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use orkestr_protocol::methods::cursor::{CursorBackendInfo, CursorSettings, CursorSource};

use super::model::{ComputerUseModel, DecideRequest, Decision, ModelError, ModelErrorKind};

/// Lists and builds model backends (core: Claude Code CLI, Codex CLI,
/// Anthropic / OpenAI over the provider stack; tests: scripted).
#[async_trait]
pub trait CursorModelFactory: Send + Sync {
    /// Every known backend with availability (any order; `consented` is filled by the caller).
    async fn backends(&self, settings: &CursorSettings) -> Vec<CursorBackendInfo>;
    /// Builds a backend for one session.
    fn build(&self, id: &str, settings: &CursorSettings) -> Option<Arc<dyn ComputerUseModel>>;
}

/// Backends that need no privacy consent (nothing leaves the machine).
#[must_use]
pub fn is_local(id: &str) -> bool {
    id == "mock"
}

fn rank(order: &[String], id: &str) -> usize {
    order.iter().position(|o| o == id).unwrap_or(order.len())
}

/// Sorts backends: the selected source first (in the user's order), then the
/// other source, then the rest (mock). Fills `consented`.
#[must_use]
pub fn ordered(mut all: Vec<CursorBackendInfo>, settings: &CursorSettings) -> Vec<CursorBackendInfo> {
    for b in &mut all {
        b.consented = is_local(&b.id) || settings.consents.contains_key(&b.id);
    }
    let key = |b: &CursorBackendInfo| {
        let group = if b.id == "mock" {
            2
        } else if b.source == settings.source {
            0
        } else {
            1
        };
        let order = match b.source {
            CursorSource::Subscriptions => &settings.subscription_order,
            CursorSource::Api => &settings.api_order,
        };
        (group, rank(order, &b.id))
    };
    all.sort_by_key(key);
    all
}

/// The candidates of a session: available backends of the selected source
/// (the mock only when it is the only thing offered, i.e. tests).
#[must_use]
pub fn candidates(ordered: &[CursorBackendInfo], settings: &CursorSettings) -> Vec<CursorBackendInfo> {
    let real: Vec<CursorBackendInfo> =
        ordered.iter().filter(|b| b.available && b.id != "mock" && b.source == settings.source).cloned().collect();
    if real.is_empty() {
        ordered.iter().filter(|b| b.available && b.id == "mock").cloned().collect()
    } else {
        real
    }
}

/// Called when the session moves to another backend: `(from, to, reason)`.
pub type SwitchFn = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;

/// Tries the chain in order; a failover-kind error moves to the next one.
pub struct FailoverModel {
    chain: Vec<Arc<dyn ComputerUseModel>>,
    current: AtomicUsize,
    on_switch: SwitchFn,
}

impl FailoverModel {
    /// New chain (must not be empty).
    #[must_use]
    pub fn new(chain: Vec<Arc<dyn ComputerUseModel>>, on_switch: SwitchFn) -> Self {
        Self { chain, current: AtomicUsize::new(0), on_switch }
    }

    /// Current backend id.
    #[must_use]
    pub fn current_id(&self) -> String {
        self.chain.get(self.current.load(Ordering::SeqCst)).map(|m| m.id().to_owned()).unwrap_or_default()
    }

    /// Current model label.
    #[must_use]
    pub fn current_label(&self) -> String {
        self.chain.get(self.current.load(Ordering::SeqCst)).map(|m| m.label()).unwrap_or_default()
    }
}

#[async_trait]
impl ComputerUseModel for FailoverModel {
    fn id(&self) -> &str {
        self.chain.get(self.current.load(Ordering::SeqCst)).map_or("none", |m| m.id())
    }
    fn label(&self) -> String {
        self.current_label()
    }
    async fn decide(&self, req: &DecideRequest, cancel: &CancellationToken) -> Result<Decision, ModelError> {
        let mut last = ModelError::new(ModelErrorKind::Unavailable, "no model backend");
        let mut i = self.current.load(Ordering::SeqCst);
        while let Some(m) = self.chain.get(i) {
            if cancel.is_cancelled() {
                return Err(ModelError::cancelled());
            }
            match m.decide(req, cancel).await {
                Ok(d) => return Ok(d),
                Err(e) if e.kind.fails_over() && !cancel.is_cancelled() => {
                    if let Some(next) = self.chain.get(i + 1) {
                        tracing::warn!(from = m.id(), to = next.id(), error = %e.message, "cursor model failover");
                        (self.on_switch)(m.id(), next.id(), &e.message);
                        self.current.store(i + 1, Ordering::SeqCst);
                    }
                    last = e;
                    i += 1;
                }
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use orkestr_protocol::methods::cursor::CursorAction;

    use super::*;
    use crate::cursor::model::ScriptedModel;

    fn info(id: &str, source: CursorSource, available: bool) -> CursorBackendInfo {
        CursorBackendInfo {
            id: id.into(),
            label: id.into(),
            source,
            available,
            detail: String::new(),
            model: String::new(),
            consented: false,
        }
    }

    #[test]
    fn order_follows_source_and_priority() {
        let all = vec![
            info("openai", CursorSource::Api, true),
            info("mock", CursorSource::Subscriptions, true),
            info("codex", CursorSource::Subscriptions, true),
            info("anthropic", CursorSource::Api, false),
            info("claudeCode", CursorSource::Subscriptions, true),
        ];
        let mut s = CursorSettings::default();
        s.consents.insert("codex".into(), chrono::Utc::now());
        let o = ordered(all.clone(), &s);
        let ids: Vec<&str> = o.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["claudeCode", "codex", "anthropic", "openai", "mock"]);
        assert!(o[1].consented && !o[0].consented && o[4].consented);
        let c: Vec<String> = candidates(&o, &s).into_iter().map(|b| b.id).collect();
        assert_eq!(c, ["claudeCode", "codex"]);

        s.source = CursorSource::Api;
        s.api_order = vec!["openai".into(), "anthropic".into()];
        let o = ordered(all, &s);
        assert_eq!(o[0].id, "openai");
        let c: Vec<String> = candidates(&o, &s).into_iter().map(|b| b.id).collect();
        assert_eq!(c, ["openai"], "unavailable anthropic is skipped");
    }

    #[test]
    fn mock_only_when_nothing_real() {
        let s = CursorSettings::default();
        let o = ordered(
            vec![
                info("mock", CursorSource::Subscriptions, true),
                info("claudeCode", CursorSource::Subscriptions, false),
            ],
            &s,
        );
        assert_eq!(candidates(&o, &s)[0].id, "mock");
    }

    #[tokio::test]
    async fn fails_over_on_limits_and_stays_there() {
        let a = Arc::new(ScriptedModel::with_results(
            "claudeCode",
            vec![Err(ModelError::classified("Claude AI usage limit reached"))],
        ));
        let b = Arc::new(ScriptedModel::actions(
            "codex",
            vec![CursorAction::Wait { ms: 100 }, CursorAction::Wait { ms: 200 }],
        ));
        let switched = Arc::new(Mutex::new(Vec::new()));
        let sw = Arc::clone(&switched);
        let f = FailoverModel::new(
            vec![a.clone(), b.clone()],
            Arc::new(move |from, to, _| sw.lock().push(format!("{from}->{to}"))),
        );
        let cancel = CancellationToken::new();
        let d = f.decide(&DecideRequest::default(), &cancel).await.unwrap();
        assert_eq!(d.action, CursorAction::Wait { ms: 100 });
        assert_eq!(f.current_id(), "codex");
        let d = f.decide(&DecideRequest::default(), &cancel).await.unwrap();
        assert_eq!(d.action, CursorAction::Wait { ms: 200 });
        assert_eq!(a.seen().len(), 1, "the failed backend is not retried");
        assert_eq!(*switched.lock(), ["claudeCode->codex"]);
    }

    #[tokio::test]
    async fn other_errors_do_not_fail_over() {
        let a = Arc::new(ScriptedModel::with_results(
            "claudeCode",
            vec![Err(ModelError::new(ModelErrorKind::BadOutput, "junk"))],
        ));
        let b = Arc::new(ScriptedModel::actions("codex", vec![]));
        let f = FailoverModel::new(vec![a, b.clone()], Arc::new(|_, _, _| {}));
        let e = f.decide(&DecideRequest::default(), &CancellationToken::new()).await.unwrap_err();
        assert_eq!(e.kind, ModelErrorKind::BadOutput);
        assert!(b.seen().is_empty());
    }

    #[tokio::test]
    async fn all_failing_returns_the_last_error() {
        let a = Arc::new(ScriptedModel::with_results("a", vec![Err(ModelError::classified("429"))]));
        let b = Arc::new(ScriptedModel::with_results("b", vec![Err(ModelError::classified("401 unauthorized"))]));
        let f = FailoverModel::new(vec![a, b], Arc::new(|_, _, _| {}));
        let e = f.decide(&DecideRequest::default(), &CancellationToken::new()).await.unwrap_err();
        assert_eq!(e.kind, ModelErrorKind::Auth);
    }
}
