//! Bearer-token authentication (contract §3, §6).

use rand::RngCore;
use subtle::ConstantTimeEq;

/// Generates a random 256-bit token (64 hex chars).
#[must_use]
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Constant-time token comparison.
#[must_use]
pub fn token_matches(expected: &str, provided: &str) -> bool {
    // `ct_eq` on slices of different length returns false without leaking
    // where they differ; the length itself is not secret (always 64).
    bool::from(expected.as_bytes().ct_eq(provided.as_bytes()))
}

/// Extracts the token from an `Authorization: Bearer <token>` header value.
#[must_use]
pub fn bearer_token(header: &str) -> Option<&str> {
    let (scheme, token) = header.trim().split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim()).filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_and_long() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }

    #[test]
    fn comparison() {
        let t = generate_token();
        assert!(token_matches(&t, &t.clone()));
        assert!(!token_matches(&t, &t[..63]));
        assert!(!token_matches(&t, ""));
        assert!(!token_matches(&t, &generate_token()));
    }

    #[test]
    fn bearer_parsing() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer   abc "), Some("abc"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("Bearer"), None);
    }
}
