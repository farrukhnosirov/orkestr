//! Incremental UTF-8 decoding.
//!
//! PTY reads return arbitrary byte slices, so a multi-byte code point can be
//! split across two reads. [`Utf8Decoder`] carries an incomplete trailing
//! sequence over to the next call, so every emitted `String` is valid UTF-8
//! and no code point is ever split between two `TerminalOutput` events.
//! Truly invalid bytes are replaced with U+FFFD (lossy), never dropped silently.

/// Streaming UTF-8 decoder.
#[derive(Debug, Default, Clone)]
pub struct Utf8Decoder {
    /// Incomplete trailing sequence from the previous chunk (at most 3 bytes).
    pending: Vec<u8>,
}

impl Utf8Decoder {
    /// New decoder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of bytes held back waiting for the rest of a code point.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Decodes `bytes`, returning the longest valid prefix (with invalid
    /// sequences replaced) and keeping an incomplete suffix for later.
    pub fn decode(&mut self, bytes: &[u8]) -> String {
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(bytes);
        let mut out = String::with_capacity(buf.len());
        let mut rest: &[u8] = &buf;
        loop {
            match std::str::from_utf8(rest) {
                Ok(s) => {
                    out.push_str(s);
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    // `valid` bytes are valid UTF-8 by contract of `Utf8Error`.
                    out.push_str(&String::from_utf8_lossy(&rest[..valid]));
                    match e.error_len() {
                        Some(bad) => {
                            out.push(char::REPLACEMENT_CHARACTER);
                            rest = &rest[valid + bad..];
                        }
                        None => {
                            // Incomplete sequence at the end: keep it for the next call.
                            self.pending = rest[valid..].to_vec();
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    /// Flushes any held-back bytes (end of stream) as replacement characters.
    pub fn finish(&mut self) -> String {
        if self.pending.is_empty() {
            return String::new();
        }
        let pending = std::mem::take(&mut self.pending);
        String::from_utf8_lossy(&pending).into_owned()
    }
}

/// Largest index `<= max` that lies on a char boundary of `s`.
#[must_use]
pub fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Smallest index `>= min` that lies on a char boundary of `s`.
#[must_use]
pub fn ceil_char_boundary(s: &str, min: usize) -> usize {
    if min >= s.len() {
        return s.len();
    }
    let mut i = min;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Splits `s` into pieces of at most `max_bytes` bytes without splitting a
/// code point (a piece may be shorter; `max_bytes < 4` still makes progress).
#[must_use]
pub fn split_utf8(s: &str, max_bytes: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    let max = max_bytes.max(1);
    while !rest.is_empty() {
        let mut cut = floor_char_boundary(rest, max);
        if cut == 0 {
            cut = ceil_char_boundary(rest, 1);
        }
        let (head, tail) = rest.split_at(cut);
        out.push(head);
        rest = tail;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_passes_through() {
        let mut d = Utf8Decoder::new();
        assert_eq!(d.decode(b"hello\r\n"), "hello\r\n");
        assert_eq!(d.pending_len(), 0);
    }

    #[test]
    fn split_code_points_are_reassembled_at_every_offset() {
        let text = "aé€😀Ж z";
        let bytes = text.as_bytes();
        for split in 0..=bytes.len() {
            let mut d = Utf8Decoder::new();
            let a = d.decode(&bytes[..split]);
            let b = d.decode(&bytes[split..]);
            assert_eq!(format!("{a}{b}"), text, "split at {split}");
            assert_eq!(d.finish(), "");
        }
    }

    #[test]
    fn byte_by_byte_never_emits_partial_chars() {
        let text = "Привет, 世界 🌍!";
        let mut d = Utf8Decoder::new();
        let mut out = String::new();
        for b in text.as_bytes() {
            let piece = d.decode(std::slice::from_ref(b));
            assert!(!piece.contains(char::REPLACEMENT_CHARACTER));
            out.push_str(&piece);
        }
        assert_eq!(out, text);
    }

    #[test]
    fn invalid_bytes_become_replacement_chars() {
        let mut d = Utf8Decoder::new();
        assert_eq!(d.decode(b"a\xffb"), "a\u{FFFD}b");
        // A lone continuation byte is invalid, not pending.
        assert_eq!(d.decode(b"\x80c"), "\u{FFFD}c");
    }

    #[test]
    fn finish_flushes_truncated_sequence() {
        let mut d = Utf8Decoder::new();
        assert_eq!(d.decode(&[0xE2, 0x82]), "");
        assert_eq!(d.pending_len(), 2);
        assert_eq!(d.finish(), "\u{FFFD}");
        assert_eq!(d.pending_len(), 0);
    }

    #[test]
    fn split_utf8_respects_boundaries() {
        let s = "ab€€😀";
        for max in 1..=8 {
            let parts = split_utf8(s, max);
            assert_eq!(parts.concat(), s);
            for p in &parts {
                assert!(!p.is_empty());
                assert!(p.len() <= max.max(4), "{p:?} > {max}");
            }
        }
    }

    #[test]
    fn boundaries() {
        let s = "a€b";
        assert_eq!(floor_char_boundary(s, 2), 1);
        assert_eq!(ceil_char_boundary(s, 2), 4);
        assert_eq!(floor_char_boundary(s, 100), s.len());
    }
}
