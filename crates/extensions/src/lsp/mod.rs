//! Language-server bridge: recipes (discovery), framing, supervision and
//! the approval fingerprint.

pub mod framing;
pub mod manager;
pub mod recipes;

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

pub use manager::{LaunchSpec, LspConfig, LspEvent, LspManager};

/// Fingerprint of an exact server command: recipe id, program path,
/// arguments and the SHA-256 of the program file. An approval covers this
/// fingerprint only — a replaced binary or a different command needs a new
/// approval.
pub fn fingerprint(server_id: &str, program: &Path, args: &[String]) -> std::io::Result<String> {
    let mut file_hash = Sha256::new();
    let mut f = std::fs::File::open(program)?;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file_hash.update(&buf[..n]);
    }
    let mut h = Sha256::new();
    h.update(server_id.as_bytes());
    h.update([0]);
    h.update(program.display().to_string().as_bytes());
    h.update([0]);
    for a in args {
        h.update(a.as_bytes());
        h.update([0]);
    }
    h.update(file_hash.finalize());
    Ok(hex::encode(h.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_changes_with_binary_and_args() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("server");
        std::fs::write(&p, b"v1").unwrap();
        let a = fingerprint("rust-analyzer", &p, &[]).unwrap();
        assert_eq!(a, fingerprint("rust-analyzer", &p, &[]).unwrap());
        assert_ne!(a, fingerprint("rust-analyzer", &p, &["--stdio".into()]).unwrap());
        assert_ne!(a, fingerprint("other", &p, &[]).unwrap());
        std::fs::write(&p, b"v2").unwrap();
        assert_ne!(a, fingerprint("rust-analyzer", &p, &[]).unwrap(), "a replaced binary needs a new approval");
        assert!(fingerprint("x", &dir.path().join("missing"), &[]).is_err());
    }
}
