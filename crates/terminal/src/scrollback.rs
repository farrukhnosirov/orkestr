//! Per-session scrollback ring buffer.
//!
//! Stores the most recent output (UTF-8, ANSI sequences included) up to a byte
//! capacity so late subscribers and reconnecting clients can replay it via
//! `terminal.history`. Offsets are absolute byte positions in the session's
//! output stream: `TerminalOutput{offset}` of an event equals the
//! `end_offset` of the history at the moment before it was appended, which
//! lets clients de-duplicate history and live events.

use crate::utf8::{ceil_char_boundary, floor_char_boundary};

/// Default scrollback capacity per session (2 MiB, ADR-0014).
pub const DEFAULT_SCROLLBACK_BYTES: usize = 2 * 1024 * 1024;

/// A bounded text buffer that drops the oldest output first.
#[derive(Debug, Clone)]
pub struct Scrollback {
    buf: String,
    capacity: usize,
    /// Absolute offset of `buf[0]` in the output stream.
    start_offset: u64,
}

/// A slice of the scrollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySlice {
    /// Text (valid UTF-8, starts on a char boundary).
    pub data: String,
    /// Absolute offset of the first byte of `data`.
    pub start_offset: u64,
    /// Absolute offset just after the last byte (= total bytes ever written).
    pub end_offset: u64,
    /// Whether older output exists that is not part of `data`.
    pub truncated: bool,
}

impl Scrollback {
    /// New buffer with the given capacity in bytes (minimum 1 KiB).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self { buf: String::new(), capacity: capacity.max(1024), start_offset: 0 }
    }

    /// Capacity in bytes.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether nothing is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Total bytes ever appended.
    #[must_use]
    pub fn end_offset(&self) -> u64 {
        self.start_offset + self.buf.len() as u64
    }

    /// Appends text and returns the absolute offset at which it starts.
    pub fn push(&mut self, text: &str) -> u64 {
        let offset = self.end_offset();
        if text.len() >= self.capacity {
            // Only the tail of a huge chunk fits.
            let cut = ceil_char_boundary(text, text.len() - self.capacity);
            self.start_offset = offset + cut as u64;
            self.buf.clear();
            self.buf.push_str(&text[cut..]);
            return offset;
        }
        self.buf.push_str(text);
        if self.buf.len() > self.capacity {
            // Trim to ~3/4 so trimming is amortised over many pushes.
            let target_drop = self.buf.len() - (self.capacity / 4) * 3;
            let cut = ceil_char_boundary(&self.buf, target_drop);
            self.buf.drain(..cut);
            self.start_offset += cut as u64;
        }
        offset
    }

    /// The last `max_bytes` (or everything when `None`), starting on a char boundary.
    #[must_use]
    pub fn tail(&self, max_bytes: Option<usize>) -> HistorySlice {
        let len = self.buf.len();
        let from = match max_bytes {
            Some(m) if m < len => ceil_char_boundary(&self.buf, len - m),
            _ => 0,
        };
        HistorySlice {
            data: self.buf[from..].to_owned(),
            start_offset: self.start_offset + from as u64,
            end_offset: self.end_offset(),
            truncated: self.start_offset > 0 || from > 0,
        }
    }

    /// The last `max_chars`-ish bytes as a plain preview (used for list views).
    #[must_use]
    pub fn preview(&self, max_bytes: usize) -> &str {
        let len = self.buf.len();
        if len <= max_bytes {
            return &self.buf;
        }
        let from = floor_char_boundary(&self.buf, len - max_bytes);
        &self.buf[from..]
    }
}

impl Default for Scrollback {
    fn default() -> Self {
        Self::new(DEFAULT_SCROLLBACK_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_track_total_output() {
        let mut s = Scrollback::new(1024);
        assert_eq!(s.push("abc"), 0);
        assert_eq!(s.push("de"), 3);
        let h = s.tail(None);
        assert_eq!(h.data, "abcde");
        assert_eq!((h.start_offset, h.end_offset, h.truncated), (0, 5, false));
    }

    #[test]
    fn evicts_oldest_and_keeps_offsets_consistent() {
        let mut s = Scrollback::new(1024);
        let chunk = "x".repeat(300);
        for _ in 0..10 {
            s.push(&chunk);
        }
        assert!(s.len() <= 1024);
        let h = s.tail(None);
        assert_eq!(h.end_offset, 3000);
        assert_eq!(h.start_offset + h.data.len() as u64, h.end_offset);
        assert!(h.truncated);
    }

    #[test]
    fn eviction_never_splits_code_points() {
        let mut s = Scrollback::new(1024);
        for _ in 0..2000 {
            s.push("€😀");
        }
        let h = s.tail(None);
        assert!(h.data.starts_with('€') || h.data.starts_with('😀'));
        assert_eq!(h.start_offset + h.data.len() as u64, h.end_offset);
        let t = s.tail(Some(5));
        assert!(t.data.len() <= 5);
        assert_eq!(t.start_offset + t.data.len() as u64, t.end_offset);
    }

    #[test]
    fn huge_chunk_keeps_tail() {
        let mut s = Scrollback::new(1024);
        let big = "é".repeat(2000); // 4000 bytes
        assert_eq!(s.push(&big), 0);
        assert!(s.len() <= 1024);
        let h = s.tail(None);
        assert_eq!(h.end_offset, 4000);
        assert!(h.data.chars().all(|c| c == 'é'));
        assert_eq!(h.start_offset + h.data.len() as u64, 4000);
    }

    #[test]
    fn preview_is_a_suffix() {
        let mut s = Scrollback::new(1024);
        s.push("hello world");
        assert_eq!(s.preview(5), "world");
        assert_eq!(s.preview(100), "hello world");
    }
}
