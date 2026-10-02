//! Token estimation without a model tokenizer.
//!
//! Prose averages ≈ 4 characters per token for BPE tokenizers; source code
//! (punctuation, short identifiers, indentation) is denser, ≈ 3.2 chars per
//! token, hence the 1.25 code factor. Estimates are deliberately a little
//! pessimistic so packed bundles stay under the real limit.

/// Characters per token for prose.
pub const CHARS_PER_TOKEN: f64 = 4.0;
/// Multiplier applied to code.
pub const CODE_FACTOR: f64 = 1.25;

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn estimate(chars: usize, factor: f64) -> u32 {
    if chars == 0 {
        return 0;
    }
    let v = (chars as f64 / CHARS_PER_TOKEN * factor).ceil();
    if v >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        v as u32
    }
}

/// Estimated tokens of prose / markdown.
#[must_use]
pub fn estimate_tokens(text: &str) -> u32 {
    estimate(text.chars().count(), 1.0)
}

/// Estimated tokens of source code.
#[must_use]
pub fn estimate_code_tokens(text: &str) -> u32 {
    estimate(text.chars().count(), CODE_FACTOR)
}

/// Truncates `text` (on a char boundary, preferring a line end) so that its
/// prose estimate fits `max_tokens`; appends `…` when cut.
#[must_use]
pub fn truncate_to_tokens(text: &str, max_tokens: u32) -> String {
    if estimate_tokens(text) <= max_tokens {
        return text.to_owned();
    }
    let max_chars = (max_tokens as usize).saturating_mul(4).saturating_sub(1);
    let cut: String = text.chars().take(max_chars).collect();
    let cut = match cut.rfind('\n') {
        Some(pos) if pos > cut.len() / 2 => cut[..pos].to_owned(),
        _ => cut,
    };
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_code_tokens(&"x".repeat(400)), 125);
        assert!(estimate_code_tokens("fn main() {}") >= estimate_tokens("fn main() {}"));
        // Unicode counts characters, not bytes.
        assert_eq!(estimate_tokens("привет"), 2);
    }

    #[test]
    fn truncation_fits() {
        let text = "line one\n".repeat(100);
        let t = truncate_to_tokens(&text, 20);
        assert!(estimate_tokens(&t) <= 20, "{}", estimate_tokens(&t));
        assert!(t.ends_with('…'));
        assert_eq!(truncate_to_tokens("short", 20), "short");
    }
}
