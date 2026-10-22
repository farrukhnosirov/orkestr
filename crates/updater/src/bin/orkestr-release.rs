//! `orkestr-release` — release tooling used by `scripts/release/build-installer.ps1`.
//!
//! ```text
//! orkestr-release sign     --key <file> --password-file <file> --version <v> <installer>
//! orkestr-release verify   --pubkey <base64|file> --version <v> <installer>
//! orkestr-release manifest --version <v> [--channel stable|beta] [--notes-file <md>]
//!                          [--notes-ru-file <md>] [--base-url <url>] --out <latest.json>
//!                          --platform <key>=<installer> [--platform …]
//! ```
//! Keys are in the `tauri signer generate` format; the private key never
//! lives in the repository (default: `%USERPROFILE%\.orkestr-tools\keys\updater.key`).
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow_lite::{bail, Context, Result};
use orkestr_updater::manifest::{parse_version, Manifest, PlatformEntry};
use orkestr_updater::signature;

/// Tiny local error helper (keeps `anyhow` out of the library's dependencies).
mod anyhow_lite {
    pub type Result<T> = std::result::Result<T, String>;
    pub trait Context<T> {
        fn context(self, what: impl std::fmt::Display) -> Result<T>;
    }
    impl<T, E: std::fmt::Display> Context<T> for std::result::Result<T, E> {
        fn context(self, what: impl std::fmt::Display) -> Result<T> {
            self.map_err(|e| format!("{what}: {e}"))
        }
    }
    macro_rules! bail {
        ($($t:tt)*) => { return Err(format!($($t)*)) };
    }
    pub(crate) use bail;
}

struct Args {
    flags: BTreeMap<String, Vec<String>>,
    positional: Vec<String>,
}

impl Args {
    fn parse(raw: impl Iterator<Item = String>) -> Result<Self> {
        let mut flags: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut positional = Vec::new();
        let mut it = raw.peekable();
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                let Some(v) = it.next() else { bail!("--{name} needs a value") };
                flags.entry(name.to_owned()).or_default().push(v);
            } else {
                positional.push(a);
            }
        }
        Ok(Self { flags, positional })
    }
    fn one(&self, name: &str) -> Result<&str> {
        self.opt(name).ok_or_else(|| format!("missing --{name}"))
    }
    fn opt(&self, name: &str) -> Option<&str> {
        self.flags.get(name).and_then(|v| v.last()).map(String::as_str)
    }
    fn all(&self, name: &str) -> &[String] {
        self.flags.get(name).map_or(&[], Vec::as_slice)
    }
}

fn read_trimmed(path: &str) -> Result<String> {
    std::fs::read_to_string(path).map(|s| s.trim().to_owned()).context(format!("reading {path}"))
}

fn sign(a: &Args) -> Result<()> {
    let key = read_trimmed(a.one("key")?)?;
    let password = match a.opt("password-file") {
        Some(f) => read_trimmed(f)?,
        None => std::env::var("TAURI_SIGNING_PRIVATE_KEY_PASSWORD").unwrap_or_default(),
    };
    let version = parse_version(a.one("version")?).context("--version")?;
    let [file] = a.positional.as_slice() else { bail!("sign takes exactly one file") };
    let data = std::fs::read(file).context(format!("reading {file}"))?;
    let name = Path::new(file).file_name().and_then(|n| n.to_str()).unwrap_or("installer");
    let sig = signature::sign(&data, &key, &password, name, &version).map_err(|e| e.to_string())?;
    let sig_path = format!("{file}.sig");
    std::fs::write(&sig_path, &sig).context(format!("writing {sig_path}"))?;
    println!("{sig_path}");
    Ok(())
}

fn verify(a: &Args) -> Result<()> {
    let pk_arg = a.one("pubkey")?;
    let pubkey = if Path::new(pk_arg).is_file() { read_trimmed(pk_arg)? } else { pk_arg.to_owned() };
    let version = parse_version(a.one("version")?).context("--version")?;
    let [file] = a.positional.as_slice() else { bail!("verify takes exactly one file") };
    let data = std::fs::read(file).context(format!("reading {file}"))?;
    let sig = read_trimmed(&format!("{file}.sig"))?;
    let comment = signature::verify(&data, &sig, &pubkey, &version).map_err(|e| e.to_string())?;
    println!("OK: {comment}");
    Ok(())
}

fn manifest(a: &Args) -> Result<()> {
    let version = parse_version(a.one("version")?).context("--version")?;
    let notes = a.opt("notes-file").map(read_trimmed).transpose()?;
    let mut notes_i18n = BTreeMap::new();
    if let Some(ru) = a.opt("notes-ru-file") {
        notes_i18n.insert("ru".to_owned(), read_trimmed(ru)?);
    }
    let base = a.opt("base-url").unwrap_or_default().trim_end_matches('/').to_owned();
    let mut platforms = BTreeMap::new();
    for spec in a.all("platform") {
        let Some((key, file)) = spec.split_once('=') else { bail!("--platform expects <key>=<installer>, got {spec}") };
        let signature = read_trimmed(&format!("{file}.sig"))?;
        let name = PathBuf::from(file).file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        let url = if base.is_empty() { name } else { format!("{base}/{name}") };
        platforms.insert(key.to_owned(), PlatformEntry { url, signature });
    }
    if platforms.is_empty() {
        bail!("at least one --platform is required");
    }
    let m = Manifest {
        version: version.to_string(),
        notes,
        notes_i18n,
        pub_date: Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        channel: Some(a.opt("channel").unwrap_or("stable").to_owned()),
        platforms,
    };
    let out = a.one("out")?;
    let json = serde_json::to_string_pretty(&m).context("encoding")?;
    std::fs::write(out, json).context(format!("writing {out}"))?;
    println!("{out}");
    Ok(())
}

fn main() {
    let mut raw = std::env::args().skip(1);
    let cmd = raw.next().unwrap_or_default();
    let result = Args::parse(raw).and_then(|a| match cmd.as_str() {
        "sign" => sign(&a),
        "verify" => verify(&a),
        "manifest" => manifest(&a),
        _ => Err("usage: orkestr-release <sign|verify|manifest> … (see the source header)".to_owned()),
    });
    if let Err(e) = result {
        eprintln!("orkestr-release: {e}");
        std::process::exit(1);
    }
}
