//! Log redaction (contract §3 "secrets redacted", §6 "never in logs/events").
//!
//! The redaction logic (registered secret values — the RPC token and every
//! value stored in / resolved from the secret vault — sensitive field names
//! and well-known key patterns) lives in [`orkestr_security::redact`]; this
//! module re-exports it and adds the `tracing` writer adapters the core uses
//! for stdout/JSON logs.

pub use orkestr_security::redact::{
    clear_secrets, is_sensitive_field, redact, redact_json, register_secret, unregister_secret, REDACTED,
};

/// `io::Write` adapter that redacts each formatted record before writing it.
///
/// `tracing-subscriber`'s fmt layer formats a whole record into a buffer and
/// writes it with a single `write_all`, so per-call redaction sees complete
/// lines.
#[derive(Debug)]
pub struct RedactingWriter<W: std::io::Write> {
    inner: W,
}

impl<W: std::io::Write> std::io::Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match std::str::from_utf8(buf) {
            Ok(text) => self.inner.write_all(redact(text).as_bytes())?,
            Err(_) => self.inner.write_all(buf)?,
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// `MakeWriter` producing redacting stdout writers.
#[derive(Debug, Clone, Copy, Default)]
pub struct RedactingStdout;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RedactingStdout {
    type Writer = RedactingWriter<std::io::Stdout>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter { inner: std::io::stdout() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn writer_redacts_registered_and_pattern_secrets() {
        let secret = "core-redact-writer-secret-0123456789";
        register_secret(secret);
        let mut w = RedactingWriter { inner: Vec::new() };
        w.write_all(format!("token {secret} and Bearer abc.def").as_bytes()).unwrap();
        let out = String::from_utf8(w.inner).unwrap();
        assert!(!out.contains(secret));
        assert!(out.contains("Bearer [REDACTED]"));
        unregister_secret(secret);
    }
}
