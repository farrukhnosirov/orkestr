//! Chunking of knowledge documents.
//!
//! * Markdown is split by ATX headings (`#`…`######`, ignoring `#` inside
//!   fenced code blocks); every chunk carries its heading path
//!   (`Install › Windows`). Sections larger than the size limit are split on
//!   paragraph boundaries, then line boundaries, then hard character limits.
//! * Plain text and code are split by paragraphs/lines up to the size limit.

/// Default max chunk size in characters.
pub const DEFAULT_MAX_CHARS: usize = 1600;

/// Separator used in heading paths.
pub const HEADING_SEP: &str = " › ";

/// One chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Heading path (empty for text before the first heading / plain text).
    pub heading_path: String,
    /// Chunk text.
    pub text: String,
}

/// Rough token estimate (≈ 4 characters per token).
#[must_use]
pub fn estimate_tokens(text: &str) -> u32 {
    u32::try_from(text.chars().count().div_ceil(4)).unwrap_or(u32::MAX)
}

fn heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let title = rest.trim().trim_end_matches('#').trim().to_owned();
    Some((level, title))
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// Splits markdown into heading-scoped chunks.
#[must_use]
pub fn chunk_markdown(text: &str, max_chars: usize) -> Vec<Chunk> {
    let max_chars = max_chars.max(64);
    let mut out = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut current = String::new();
    let mut in_fence = false;
    let path = |stack: &[(usize, String)]| stack.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(HEADING_SEP);
    let mut current_path = String::new();
    for line in text.lines() {
        if is_fence(line) {
            in_fence = !in_fence;
        }
        if !in_fence {
            if let Some((level, title)) = heading(line) {
                flush(&mut out, &current_path, &current, max_chars);
                current.clear();
                while stack.last().is_some_and(|(l, _)| *l >= level) {
                    stack.pop();
                }
                stack.push((level, title));
                current_path = path(&stack);
            }
        }
        current.push_str(line);
        current.push('\n');
    }
    flush(&mut out, &current_path, &current, max_chars);
    out
}

/// Splits plain text / code into size-bounded chunks.
#[must_use]
pub fn chunk_plain(text: &str, max_chars: usize) -> Vec<Chunk> {
    let mut out = Vec::new();
    flush(&mut out, "", text, max_chars.max(64));
    out
}

fn flush(out: &mut Vec<Chunk>, heading_path: &str, text: &str, max_chars: usize) {
    if text.trim().is_empty() {
        return;
    }
    for piece in split_bounded(text, max_chars) {
        let piece = piece.trim_end();
        if !piece.trim().is_empty() {
            out.push(Chunk { heading_path: heading_path.to_owned(), text: piece.to_owned() });
        }
    }
}

/// Packs paragraphs (then lines, then characters) into pieces ≤ `max_chars`.
fn split_bounded(text: &str, max_chars: usize) -> Vec<String> {
    if text.chars().count() <= max_chars {
        return vec![text.to_owned()];
    }
    let mut pieces: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut buf_len = 0usize;
    let push_unit = |unit: &str, pieces: &mut Vec<String>, buf: &mut String, buf_len: &mut usize| {
        let len = unit.chars().count();
        if *buf_len + len > max_chars && !buf.is_empty() {
            pieces.push(std::mem::take(buf));
            *buf_len = 0;
        }
        if len > max_chars {
            // Hard split on character boundaries.
            let chars: Vec<char> = unit.chars().collect();
            for part in chars.chunks(max_chars) {
                pieces.push(part.iter().collect());
            }
        } else {
            buf.push_str(unit);
            *buf_len += len;
        }
    };
    for para in split_keep(text, "\n\n") {
        if para.chars().count() > max_chars {
            for line in split_keep(para, "\n") {
                push_unit(line, &mut pieces, &mut buf, &mut buf_len);
            }
        } else {
            push_unit(para, &mut pieces, &mut buf, &mut buf_len);
        }
    }
    if !buf.is_empty() {
        pieces.push(buf);
    }
    pieces
}

/// Splits after each separator, keeping it attached to the preceding part.
fn split_keep<'a>(text: &'a str, sep: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut start = 0;
    while let Some(pos) = text[start..].find(sep) {
        let end = start + pos + sep.len();
        parts.push(&text[start..end]);
        start = end;
    }
    if start < text.len() {
        parts.push(&text[start..]);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_heading_paths() {
        let md = "Intro text\n\n# Install\nRun it.\n## Windows\nUse PowerShell.\n```sh\n# not a heading\n```\n## Linux ##\nbash\n# Usage\nDo things.\n";
        let chunks = chunk_markdown(md, 1000);
        let paths: Vec<_> = chunks.iter().map(|c| c.heading_path.as_str()).collect();
        assert_eq!(paths, vec!["", "Install", "Install › Windows", "Install › Linux", "Usage"]);
        assert!(chunks[2].text.contains("# not a heading"));
        assert!(chunks[0].text.starts_with("Intro"));
        assert!(chunks[1].text.starts_with("# Install"));
    }

    #[test]
    fn not_headings() {
        assert_eq!(heading("#hashtag"), None);
        assert_eq!(heading("    # code"), None);
        assert_eq!(heading("####### seven"), None);
        assert_eq!(heading("### Three ###"), Some((3, "Three".into())));
        assert_eq!(heading("#"), Some((1, String::new())));
    }

    #[test]
    fn large_sections_are_split_within_bounds() {
        let para = "word ".repeat(60); // 300 chars
        let body = (0..10).map(|_| para.clone()).collect::<Vec<_>>().join("\n\n");
        let md = format!("# Big\n{body}");
        let chunks = chunk_markdown(&md, 700);
        assert!(chunks.len() >= 4, "{}", chunks.len());
        assert!(chunks.iter().all(|c| c.text.chars().count() <= 700));
        assert!(chunks.iter().all(|c| c.heading_path == "Big"));
        // Nothing lost (modulo trailing whitespace).
        let total: usize = chunks.iter().map(|c| c.text.split_whitespace().count()).sum();
        assert_eq!(total, 1 + 1 + 600);
    }

    #[test]
    fn huge_line_is_hard_split() {
        let line = "x".repeat(1000);
        let chunks = chunk_plain(&line, 300);
        assert_eq!(chunks.len(), 4);
        assert!(chunks.iter().all(|c| c.text.chars().count() <= 300));
    }

    #[test]
    fn plain_and_empty() {
        assert!(chunk_plain("   \n\n ", 100).is_empty());
        assert_eq!(chunk_plain("fn main() {}\n", 100).len(), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }
}
