//! Builds and signs a static registry directory from package files.
//!
//! ```text
//! cargo run -p orkestr-marketplace --example sign_registry -- <registry-dir> <key-seed-file> [registry-name]
//! ```
//!
//! Every `*.agent.json` under `<registry-dir>/packages` becomes an agent
//! template entry (publisher `orkestr`); the index and every package are
//! signed with the ed25519 seed in `<key-seed-file>` (created when missing;
//! keep it OUT of the repository). Prints the public key to embed.

use std::path::PathBuf;

use orkestr_marketplace::index::{IndexDocument, IndexEntry, IndexKey};
use orkestr_marketplace::publish::describe;
use orkestr_marketplace::service::write_index;
use orkestr_marketplace::signing::{package_message, sha256_hex, KeyPair};
use orkestr_protocol::methods::marketplace::PackageKind;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(dir), Some(seed_file)) = (args.first(), args.get(1)) else {
        eprintln!("usage: sign_registry <registry-dir> <key-seed-file> [name]");
        std::process::exit(2);
    };
    let name = args.get(2).cloned().unwrap_or_else(|| "Orkestr official".into());
    let seed_path = PathBuf::from(seed_file);
    let key = if seed_path.exists() {
        KeyPair::from_seed(&std::fs::read_to_string(&seed_path)?)?
    } else {
        let k = KeyPair::generate();
        std::fs::write(&seed_path, k.seed_b64())?;
        k
    };
    let root = PathBuf::from(dir);
    let mut doc = IndexDocument::empty(&name);
    doc.keys.push(IndexKey { key_id: key.key_id(), publisher: "orkestr".into(), public_key: key.public_key() });
    let mut files: Vec<_> =
        std::fs::read_dir(root.join("packages"))?.filter_map(Result::ok).map(|e| e.path()).collect();
    files.sort();
    for path in files {
        let file = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        if !file.ends_with(".agent.json") {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        let meta = describe(PackageKind::AgentTemplate, &bytes, "orkestr")?;
        let sha256 = sha256_hex(&bytes);
        doc.entries.push(IndexEntry {
            id: meta.id,
            kind: meta.kind,
            name: meta.name,
            version: meta.version,
            publisher: meta.publisher,
            description: meta.description,
            tags: meta.tags,
            permissions: meta.permissions,
            key_id: key.key_id(),
            signature: key.sign(&package_message(&sha256)?),
            sha256,
            url: format!("packages/{file}"),
            revoked: false,
        });
    }
    write_index(&root, &doc, &key).await?;
    println!("{}", key.public_key());
    Ok(())
}
