//! Cross-cutting command facets and fail-closed reference checks.
//!
//! Besides the danger class, the guard records *what kind of resources* a
//! command touches so the permission engine can apply the matching
//! capability on top of `Terminal` (audit r1):
//!
//! * **network** — the command talks to the network (curl, wget, ssh, scp,
//!   `git push`, `npm publish`, URLs, …) ⇒ `Capability::Network` (and the
//!   *Disable Network Tools* emergency control);
//! * **secret_read** — a reader (`cat`, `Get-Content`, `cp`, `tar`, curl
//!   `-d @file`, …) touches a secret-bearing file (`.env*`, keys, `~/.ssh`,
//!   `~/.aws`, …), also through globs ⇒ `Capability::Secrets`;
//! * **outside_read** — a reader touches an absolute / home / variable /
//!   `..` path outside the worktree ⇒ `Capability::Read` outside the root.
//!
//! It also hosts the checks that protect Orkestr's own data directory
//! (`core.token`, the database, the vault) against references through
//! globs, environment variables (`$APPDATA`, `%APPDATA%`, `$env:APPDATA`,
//! `~`) and `..` — any such reference is `Forbidden(SelfTamper)`.

use std::sync::OnceLock;

use globset::GlobBuilder;
use orkestr_domain::DangerCategory as C;
use regex::Regex;

use super::lexer::{Family, Word};
use super::rules::{is_secret_file, norm_path, Ctx};
use super::DangerClass;

/// What a command touches beyond running a process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommandFacets {
    /// Uses the network (network clients, remote git operations, URLs, publish commands).
    pub network: bool,
    /// Reads a secret-bearing file (`.env*`, private keys, cloud credentials, …).
    pub secret_read: bool,
    /// Reads a path outside the worktree (absolute, home, variable or `..` paths).
    pub outside_read: bool,
}

impl CommandFacets {
    /// Union of two facet sets.
    #[must_use]
    pub fn union(self, other: CommandFacets) -> CommandFacets {
        CommandFacets {
            network: self.network || other.network,
            secret_read: self.secret_read || other.secret_read,
            outside_read: self.outside_read || other.outside_read,
        }
    }
}

struct Res {
    var: Regex,
    url: Regex,
    dotnet_net: Regex,
    string_lit: Regex,
}

fn res() -> &'static Res {
    static R: OnceLock<Res> = OnceLock::new();
    R.get_or_init(|| {
        let re = |s: &str| Regex::new(s).expect("valid facet regex");
        Res {
            // $VAR, ${…}, $env:VAR, ${env:VAR}, %VAR%, $(…) (already lower-cased, `/`-separated)
            var: re(r"\$\{[^}]*\}|\$\([^)]*\)|\$env:[a-z_][a-z0-9_()]*|\$[a-z_][a-z0-9_]*|%[^%/ ]+%"),
            url: re(r"(?i)\b(https?|ftps?|wss?|sftp|scp|ssh|telnet|smb|ldaps?|gopher|dict|tftp|rsync|git)://"),
            dotnet_net: re(
                r"(?i)net\.webclient|system\.net\.(http|sockets|webclient|webrequest|mail)|\[net\.|downloadstring|downloadfile|downloaddata|uploadstring|uploadfile|uploaddata|tcpclient|udpclient|httpclient|invoke-webrequest|invoke-restmethod|start-bitstransfer|send-mailmessage",
            ),
            string_lit: re(r#"'([^'\\]*(?:\\.[^'\\]*)*)'|"([^"\\]*(?:\\.[^"\\]*)*)""#),
        }
    })
}

/// Raw-text network markers (PowerShell/.NET APIs used inside expressions).
pub(crate) fn raw_network(text: &str) -> bool {
    res().dotnet_net.is_match(text)
}

/// String literals in a code snippet (`'…'` / `"…"`).
pub(crate) fn string_literals(code: &str) -> Vec<String> {
    res()
        .string_lit
        .captures_iter(code)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)).map(|m| m.as_str().to_owned()))
        .collect()
}

fn has_glob(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

/// Number of alphanumeric characters left after removing wildcards / variables.
fn literal_chars(pattern: &str) -> usize {
    pattern.chars().filter(char::is_ascii_alphanumeric).count()
}

/// Turns a (normalised) path with variables / `~` / globs into a glob
/// pattern where every unknown part matches anything (including `/`).
fn to_pattern(n: &str) -> String {
    let mut p = res().var.replace_all(n, "*").into_owned();
    if p.starts_with('~') {
        let rest = p.find('/').map_or("", |i| &p[i..]).to_owned();
        p = format!("*{rest}");
    }
    p
}

fn glob_matches(pattern: &str, candidates: &[String]) -> bool {
    let build =
        |p: &str| GlobBuilder::new(p).case_insensitive(true).literal_separator(false).backslash_escape(false).build();
    // An unbalanced class (`a[`) is retried with `?` wildcards (still matching more, never less).
    let g = match build(pattern) {
        Ok(g) => g,
        Err(_) => match build(&pattern.replace(['[', ']', '{', '}'], "?")) {
            Ok(g) => g,
            Err(_) => return false,
        },
    };
    let m = g.compile_matcher();
    candidates.iter().any(|c| m.is_match(c))
}

/// Joins a relative path onto the root and resolves `.` / `..` textually.
fn resolve_relative(root: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = root.trim_end_matches('/').split('/').collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.len() > 1 {
                    parts.pop();
                }
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn is_absolute_like(n: &str) -> bool {
    let b = n.as_bytes();
    n.starts_with('/')
        || n.starts_with('~')
        || n.starts_with('$')
        || n.starts_with('%')
        || n.starts_with('*')
        || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// `/c/x` (Git Bash) and `/mnt/c/x` (WSL) → `c:/x`.
fn windows_form(n: &str) -> Option<String> {
    let rest = n.strip_prefix("/mnt/").or_else(|| n.strip_prefix('/'))?;
    let b = rest.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b'/' {
        Some(format!("{}:{}", &rest[..1], &rest[1..]))
    } else if b.len() == 1 && b[0].is_ascii_alphabetic() {
        Some(format!("{rest}:"))
    } else {
        None
    }
}

/// Values a word may carry a path in: the word, `--opt=value`, `@file`,
/// `name=@file`, `if=file`.
fn path_candidates(text: &str) -> Vec<String> {
    let t = text.trim().trim_matches(['"', '\'']);
    let mut out = vec![t.to_owned()];
    if let Some((_, v)) = t.split_once("=@") {
        out.push(v.to_owned());
    }
    if let Some(v) = t.strip_prefix('@') {
        out.push(v.to_owned());
    }
    if let Some((k, v)) = t.split_once('=') {
        if !k.contains(['/', '\\']) && !v.is_empty() {
            out.push(v.trim_start_matches('@').to_owned());
        }
    }
    out
}

/// Sensitive files inside a protected (data) directory.
const DATA_FILES: &[&str] = &[
    "",
    "/core.token",
    "/core.json",
    "/orkestr.db",
    "/orkestr.db-wal",
    "/orkestr.db-shm",
    "/vault",
    "/vault.bin",
    "/objects",
];

/// Whether a word references Orkestr's own data directory or core token —
/// literally, through a glob, an environment variable, `~` or `..`.
pub(crate) fn check_protected_ref(ctx: &Ctx<'_>, text: &str) -> DangerClass {
    if text.is_empty() {
        return DangerClass::Safe;
    }
    let forbidden = || {
        DangerClass::forbidden(
            C::SelfTamper,
            "references Orkestr's private data directory (core token / database / vault)",
        )
    };
    for cand in path_candidates(text) {
        let n = norm_path(&cand);
        if n.is_empty() {
            continue;
        }
        if n.contains("core.token") {
            return forbidden();
        }
        let n = windows_form(&n).unwrap_or(n);
        if ctx.guard.protected_lower.iter().any(|p| n.starts_with(p.as_str()) || n.contains(p.as_str())) {
            return forbidden();
        }
        // The file name alone globbing to the token (`core.tok*`, `*.token`).
        let name = n.rsplit('/').next().unwrap_or(&n);
        if has_glob(name) && literal_chars(name) > 0 && glob_matches(name, &["core.token".to_owned()]) {
            return forbidden();
        }
        let dynamic = has_glob(&n) || n.contains('$') || n.contains('%') || n.starts_with('~');
        let has_dotdot = n.split('/').any(|s| s == "..");
        if !dynamic && !has_dotdot {
            continue;
        }
        // Resolve `..` against the worktree when known.
        let full = if is_absolute_like(&n) {
            n.clone()
        } else if let Some(root) = &ctx.root {
            if has_dotdot {
                resolve_relative(root, &n)
            } else {
                continue; // a plain relative path stays inside the worktree
            }
        } else if has_dotdot {
            let tail = n.rsplit("../").next().unwrap_or(&n);
            format!("*/{tail}")
        } else {
            continue;
        };
        let full = windows_form(&full).unwrap_or(full);
        if !dynamic {
            if ctx.guard.protected_lower.iter().any(|p| full.starts_with(p.as_str())) {
                return forbidden();
            }
            continue;
        }
        let pattern = to_pattern(&full);
        if literal_chars(&pattern) < 3 {
            continue; // `%APPDATA%`, `$HOME/*`: too generic to name the data dir
        }
        let targets: Vec<String> =
            ctx.guard.protected_lower.iter().flat_map(|p| DATA_FILES.iter().map(move |f| format!("{p}{f}"))).collect();
        if glob_matches(&pattern, &targets) {
            return forbidden();
        }
    }
    DangerClass::Safe
}

/// Programs that read (and may exfiltrate) the files named in their operands.
const READERS: &[&str] = &[
    "cat",
    "tac",
    "nl",
    "head",
    "tail",
    "less",
    "more",
    "bat",
    "batcat",
    "od",
    "xxd",
    "hexdump",
    "base64",
    "base32",
    "basenc",
    "strings",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "ag",
    "ack",
    "findstr",
    "select-string",
    "sls",
    "sed",
    "awk",
    "gawk",
    "mawk",
    "nawk",
    "cut",
    "sort",
    "uniq",
    "wc",
    "diff",
    "cmp",
    "comm",
    "paste",
    "join",
    "rev",
    "fold",
    "fmt",
    "pr",
    "expand",
    "split",
    "csplit",
    "cp",
    "scp",
    "rsync",
    "tar",
    "bsdtar",
    "zip",
    "7z",
    "7za",
    "7zr",
    "gzip",
    "bzip2",
    "xz",
    "zstd",
    "lz4",
    "openssl",
    "gpg",
    "sqlite3",
    "duckdb",
    "jq",
    "yq",
    "curl",
    "wget",
    "get-content",
    "type",
    "copy",
    "xcopy",
    "robocopy",
    "copy-item",
    "compress-archive",
    "import-csv",
    "get-filehash",
    "format-hex",
    "select-xml",
    "certutil",
    "find",
    "dd",
    "iconv",
    "source",
    ".",
    "vim",
    "vi",
    "nvim",
    "view",
    "nano",
    "emacs",
    "ed",
    "ex",
    "install",
    "ln",
    "mklink",
    "move",
    "mv",
    "move-item",
    "invoke-webrequest",
    "invoke-restmethod",
    "nc",
    "ncat",
    "netcat",
    "socat",
    "sftp",
    "ftp",
    "md5sum",
    "sha1sum",
    "sha256sum",
    "sha512sum",
    "shasum",
    "b2sum",
    "cksum",
    "file",
    "python",
    "python3",
    "node",
    "ruby",
    "perl",
    "php",
    "import-clixml",
    "convertfrom-json",
];

/// Directories whose content is secret material.
const SECRET_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".azure", ".kube", ".docker", "gcloud", ".password-store"];

/// Representative secret file names a glob operand is tested against.
const SECRET_NAMES: &[&str] = &[
    ".env",
    ".env.local",
    ".env.production",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "credentials",
    "server.pem",
    "server.key",
    "cert.pfx",
    "cert.p12",
    ".npmrc",
    ".netrc",
    ".pypirc",
    ".git-credentials",
    "secrets.yaml",
];

fn is_secret_ref(n: &str) -> bool {
    if is_secret_file(n) {
        return true;
    }
    if n.split('/').any(|s| SECRET_DIRS.contains(&s)) {
        return true;
    }
    let name = n.rsplit('/').next().unwrap_or(n);
    has_glob(name)
        && literal_chars(name) > 0
        && glob_matches(name, &SECRET_NAMES.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
}

fn is_outside_ref(ctx: &Ctx<'_>, n: &str) -> bool {
    if matches!(n, "" | "-" | "/dev/null" | "nul" | "$null" | "/dev/stdin" | "/dev/stdout" | "/dev/stderr" | "con") {
        return false;
    }
    if n.split('/').any(|s| s == "..") {
        return true;
    }
    if !is_absolute_like(n) {
        return false;
    }
    let n = windows_form(n).unwrap_or_else(|| n.to_owned());
    match &ctx.root {
        Some(root) => {
            let root = root.trim_end_matches('/');
            !(n == root || n.starts_with(&format!("{root}/")))
        }
        None => true,
    }
}

fn is_flag_word(w: &Word, family: Family) -> bool {
    let t = w.text.as_str();
    if w.quoted {
        return false;
    }
    match family {
        Family::Cmd => {
            t.starts_with('/')
                && t.len() <= 12
                && t[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '-')
        }
        _ => t.starts_with('-') && t.len() > 1 && !t.contains("=@") && !t.contains('='),
    }
}

/// Programs (normalised) that always talk to the network.
const NETWORK_PROGRAMS: &[&str] = &[
    "curl",
    "wget",
    "wget2",
    "invoke-webrequest",
    "invoke-restmethod",
    "nc",
    "ncat",
    "netcat",
    "socat",
    "telnet",
    "ftp",
    "tftp",
    "sftp",
    "scp",
    "ssh",
    "autossh",
    "mosh",
    "aria2c",
    "axel",
    "http",
    "https",
    "xh",
    "httpie",
    "lynx",
    "w3m",
    "links",
    "elinks",
    "bitsadmin",
    "start-bitstransfer",
    "send-mailmessage",
    "sendmail",
    "mail",
    "mailx",
    "mutt",
    "nslookup",
    "dig",
    "host",
    "whois",
    "test-netconnection",
    "tnc",
    "resolve-dnsname",
    "smbclient",
    "gh",
    "glab",
    "hub",
    "aws",
    "az",
    "gcloud",
    "gsutil",
    "vercel",
    "netlify",
    "wrangler",
    "fly",
    "flyctl",
    "heroku",
    "firebase",
    "ngrok",
    "cloudflared",
    "localtunnel",
    "lt",
    "bore",
    "frpc",
];

pub(crate) fn git_is_network(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') {
        i += if matches!(args[i].as_str(), "-c" | "-C" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env") {
            2
        } else {
            1
        };
    }
    let Some(sub) = args.get(i) else { return false };
    let rest = &args[i + 1..];
    match sub.as_str() {
        "push" | "fetch" | "pull" | "clone" | "ls-remote" | "send-email" | "fetch-pack" | "send-pack" | "svn"
        | "request-pull" => true,
        "remote" => rest.iter().any(|a| matches!(a.as_str(), "update" | "show" | "prune" | "set-head")),
        "submodule" => rest.iter().any(|a| matches!(a.as_str(), "update" | "sync" | "add" | "foreach")),
        "lfs" => rest.iter().any(|a| matches!(a.as_str(), "push" | "pull" | "fetch" | "clone")),
        "archive" => rest.iter().any(|a| a.starts_with("--remote")),
        _ => false,
    }
}

/// Records the facets of one resolved simple command.
pub(crate) fn record(ctx: &Ctx<'_>, prog: &str, args: &[Word], family: Family) {
    let lower: Vec<String> = args.iter().map(|w| w.text.to_lowercase()).collect();
    let sub = lower.iter().find(|a| !a.starts_with('-')).cloned().unwrap_or_default();
    let mut network = NETWORK_PROGRAMS.contains(&prog)
        || match prog {
            "git" => git_is_network(&lower),
            "npm" | "pnpm" | "yarn" | "bun" => matches!(
                sub.as_str(),
                "publish" | "unpublish" | "login" | "adduser" | "deprecate" | "dist-tag" | "owner" | "access" | "token"
            ),
            "cargo" => matches!(sub.as_str(), "publish" | "login" | "yank" | "owner"),
            "twine" => sub == "upload",
            "gem" => matches!(sub.as_str(), "push" | "yank" | "owner"),
            "docker" | "podman" | "nerdctl" => matches!(sub.as_str(), "push" | "login" | "pull"),
            "rsync" => lower.iter().any(|a| {
                !a.starts_with('-')
                    && (a.contains("::") || a.split_once(':').is_some_and(|(h, _)| h.len() > 1 && !h.contains('/')))
            }),
            "net" => matches!(sub.as_str(), "use" | "view" | "send"),
            "certutil" => lower.iter().any(|a| a.contains("urlcache") || a.contains("verifyctl")),
            _ => false,
        };
    let url_exempt = matches!(
        prog,
        "echo"
            | "printf"
            | "write-host"
            | "write-output"
            | "git"
            | "grep"
            | "egrep"
            | "rg"
            | "findstr"
            | "select-string"
            | "sls"
    );
    if !network && !url_exempt {
        network = args.iter().any(|w| {
            res().url.is_match(&w.text) || {
                let t = w.text.trim_matches(['"', '\'']);
                (t.starts_with("\\\\") && !t.starts_with("\\\\?\\") && !t.starts_with("\\\\.\\")) || t.starts_with("//")
            }
        });
    }
    let reader = READERS.contains(&prog);
    let mut secret = false;
    let mut outside = false;
    if reader {
        for w in args {
            if is_flag_word(w, family) && !w.text.contains('=') {
                continue;
            }
            for cand in path_candidates(&w.text) {
                let n = norm_path(&cand);
                if n.is_empty() || (n.starts_with('-') && !w.quoted) {
                    continue;
                }
                if is_secret_ref(&n) {
                    secret = true;
                }
                if is_outside_ref(ctx, &n) || (w.dynamic && !is_absolute_like(&n)) {
                    outside = true;
                }
            }
        }
    }
    let mut f = ctx.facets.borrow_mut();
    f.network |= network;
    f.secret_read |= secret;
    f.outside_read |= outside;
}

/// Marks the command as using the network.
pub(crate) fn mark_network(ctx: &Ctx<'_>) {
    ctx.facets.borrow_mut().network = true;
}
