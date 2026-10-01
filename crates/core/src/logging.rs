//! Logging setup (§50) and forwarding of core logs to the event bus so the
//! Logs panel shows them live.
//!
//! * [`init`] installs a global `tracing` subscriber (pretty or JSON, filtered
//!   by `RUST_LOG`, default `info`). Call once from binaries/hosts.
//! * [`EventBusLayer`] forwards `INFO`+ records of `orkestr*` targets as `Log`
//!   events. Forwarding is lossy by design (bounded channel, drops on overload)
//!   and never logs about itself, so it cannot loop.
//!
//! Secrets must never be logged. As defense in depth every record — stdout
//! output and forwarded `Log` events alike — passes through
//! [`crate::redact`], which scrubs registered secrets (the RPC token),
//! sensitive field names and well-known key formats.

use std::sync::OnceLock;

use orkestr_events::EventBus;
use orkestr_protocol::event::{EventKind, LogLevel, LogPayload};
use orkestr_protocol::NewEvent;
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::{Event as TracingEvent, Level, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::redact::{is_sensitive_field, redact, RedactingStdout, REDACTED};

const FORWARD_BUFFER: usize = 1024;

static FORWARDER: OnceLock<RwLock<Option<mpsc::Sender<LogPayload>>>> = OnceLock::new();

fn forwarder() -> &'static RwLock<Option<mpsc::Sender<LogPayload>>> {
    FORWARDER.get_or_init(|| RwLock::new(None))
}

/// Installs the global subscriber. Safe to call more than once (later calls
/// are no-ops).
pub fn init(json: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let registry = tracing_subscriber::registry().with(filter).with(EventBusLayer);
    let result = if json {
        registry.with(tracing_subscriber::fmt::layer().json().with_target(true).with_writer(RedactingStdout)).try_init()
    } else {
        registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(true)
                    .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
                    .with_writer(RedactingStdout),
            )
            .try_init()
    };
    if result.is_err() {
        tracing::debug!("tracing subscriber already installed");
    }
}

/// Starts forwarding log records to `bus` (replacing any previous bus).
pub fn attach_event_bus(bus: EventBus) {
    let (tx, mut rx) = mpsc::channel::<LogPayload>(FORWARD_BUFFER);
    *forwarder().write() = Some(tx);
    tokio::spawn(async move {
        while let Some(payload) = rx.recv().await {
            // Errors are ignored on purpose: logging them would recurse.
            let _ = bus.publish(NewEvent::new(EventKind::Log(payload))).await;
        }
    });
}

/// Stops forwarding.
pub fn detach_event_bus() {
    *forwarder().write() = None;
}

/// `tracing` layer forwarding records to the attached bus.
#[derive(Debug, Clone, Copy, Default)]
pub struct EventBusLayer;

#[derive(Default)]
struct FieldCollector {
    message: Option<String>,
    fields: serde_json::Map<String, serde_json::Value>,
}

impl FieldCollector {
    fn put(&mut self, field: &Field, value: serde_json::Value) {
        let value = if is_sensitive_field(field.name()) { REDACTED.into() } else { value };
        self.fields.insert(field.name().to_owned(), value);
    }

    fn put_text(&mut self, field: &Field, text: &str) {
        if field.name() == "message" {
            self.message = Some(redact(text));
        } else if is_sensitive_field(field.name()) {
            self.fields.insert(field.name().to_owned(), REDACTED.into());
        } else {
            self.fields.insert(field.name().to_owned(), redact(text).into());
        }
    }
}

impl Visit for FieldCollector {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.put_text(field, value);
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.put(field, value.into());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.put(field, value.into());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.put(field, value.into());
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.put(field, value.into());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.put_text(field, &format!("{value:?}"));
    }
}

fn level_of(level: &Level) -> LogLevel {
    match *level {
        Level::ERROR => LogLevel::Error,
        Level::WARN => LogLevel::Warn,
        Level::INFO => LogLevel::Info,
        Level::DEBUG => LogLevel::Debug,
        Level::TRACE => LogLevel::Trace,
    }
}

impl<S: Subscriber> Layer<S> for EventBusLayer {
    fn on_event(&self, event: &TracingEvent<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if *meta.level() > Level::INFO {
            return;
        }
        let target = meta.target();
        if !target.starts_with("orkestr") || target.starts_with("orkestr_events") {
            return;
        }
        let guard = forwarder().read();
        let Some(tx) = guard.as_ref() else { return };
        let mut collector = FieldCollector::default();
        event.record(&mut collector);
        let payload = LogPayload {
            level: level_of(meta.level()),
            target: target.to_owned(),
            message: collector.message.unwrap_or_default(),
            fields: serde_json::Value::Object(collector.fields),
        };
        let _ = tx.try_send(payload);
    }
}
