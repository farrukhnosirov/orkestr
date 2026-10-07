//! LSP base protocol framing: `Content-Length: <n>\r\n\r\n<n bytes of JSON>`.

use crate::error::{ExtError, ExtResult};

/// Max size of one message (64 MiB).
pub const MAX_MESSAGE: usize = 64 << 20;
/// Max size of a header block.
const MAX_HEADER: usize = 8 * 1024;

/// Frames one JSON message.
#[must_use]
pub fn encode(json: &str) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", json.len()).into_bytes();
    out.extend_from_slice(json.as_bytes());
    out
}

/// Incremental decoder of a server's stdout.
#[derive(Debug, Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

impl Decoder {
    /// New decoder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends bytes read from the stream.
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Next complete message, `Ok(None)` when more bytes are needed. A
    /// malformed header is an error (the stream cannot be resynchronized).
    pub fn next_message(&mut self) -> ExtResult<Option<String>> {
        let Some(end) = find(&self.buf, b"\r\n\r\n") else {
            if self.buf.len() > MAX_HEADER {
                return Err(ExtError::Invalid("language server sent an oversized header".into()));
            }
            return Ok(None);
        };
        if end > MAX_HEADER {
            return Err(ExtError::Invalid("language server sent an oversized header".into()));
        }
        let header = std::str::from_utf8(&self.buf[..end])
            .map_err(|_| ExtError::Invalid("language server sent a non-UTF-8 header".into()))?;
        let mut length: Option<usize> = None;
        for line in header.split("\r\n") {
            let Some((k, v)) = line.split_once(':') else {
                return Err(ExtError::Invalid(format!("malformed LSP header line `{line}`")));
            };
            if k.trim().eq_ignore_ascii_case("content-length") {
                length = Some(
                    v.trim().parse().map_err(|_| ExtError::Invalid(format!("bad Content-Length `{}`", v.trim())))?,
                );
            }
        }
        let len = length.ok_or_else(|| ExtError::Invalid("LSP message without Content-Length".into()))?;
        if len > MAX_MESSAGE {
            return Err(ExtError::TooLarge(format!("LSP message of {len} bytes")));
        }
        let start = end + 4;
        if self.buf.len() < start + len {
            return Ok(None);
        }
        let body = self.buf[start..start + len].to_vec();
        self.buf.drain(..start + len);
        String::from_utf8(body).map(Some).map_err(|_| ExtError::Invalid("LSP message is not UTF-8".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_partial_reads() {
        let a = r#"{"jsonrpc":"2.0","id":1,"result":"привет"}"#;
        let b = r#"{"jsonrpc":"2.0","method":"x"}"#;
        let mut stream = encode(a);
        stream.extend(encode(b));
        let mut d = Decoder::new();
        // Feed byte by byte.
        let mut got = Vec::new();
        for byte in &stream {
            d.push(std::slice::from_ref(byte));
            while let Some(m) = d.next_message().unwrap() {
                got.push(m);
            }
        }
        assert_eq!(got, vec![a.to_string(), b.to_string()]);
    }

    #[test]
    fn content_length_counts_bytes_not_chars() {
        let msg = r#"{"a":"ё"}"#;
        let framed = encode(msg);
        assert!(String::from_utf8_lossy(&framed).starts_with(&format!("Content-Length: {}\r\n", msg.len())));
        assert_ne!(msg.len(), msg.chars().count());
    }

    #[test]
    fn extra_headers_and_case() {
        let mut d = Decoder::new();
        d.push(b"content-length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}");
        assert_eq!(d.next_message().unwrap().as_deref(), Some("{}"));
        assert_eq!(d.next_message().unwrap(), None);
    }

    #[test]
    fn malformed_headers_fail() {
        let mut d = Decoder::new();
        d.push(b"Content-Type: x\r\n\r\n{}");
        assert!(d.next_message().is_err());
        let mut d = Decoder::new();
        d.push(b"Content-Length: abc\r\n\r\n");
        assert!(d.next_message().is_err());
        let mut d = Decoder::new();
        d.push(format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE + 1).as_bytes());
        assert!(d.next_message().is_err());
        let mut d = Decoder::new();
        d.push(&vec![b'x'; MAX_HEADER + 10]);
        assert!(d.next_message().is_err());
    }
}
