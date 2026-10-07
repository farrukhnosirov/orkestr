//! Per-program classification rules (permission model §6.2–6.3).

use std::cell::RefCell;
use std::sync::OnceLock;

use base64::Engine as _;
use orkestr_domain::DangerCategory as C;
use regex::Regex;

use super::facets::{self, check_protected_ref, CommandFacets};
use super::lexer::{self, Family, SimpleCommand, Word};
use super::sql::classify_sql;
use super::{DangerClass, DangerousActionGuard, MAX_COMMAND_LEN, MAX_DEPTH};

/// Classification context.
pub(crate) struct Ctx<'a> {
    pub guard: &'a DangerousActionGuard,
    /// Lower-cased, `/`-separated confinement root (if known).
    pub root: Option<String>,
    /// The confinement root as given (original casing), used to resolve
    /// 8.3 short-name aliases (`GIT~1`, `ENV~1`) against the filesystem.
    pub root_path: Option<std::path::PathBuf>,
    /// Whether to check shell path words for 8.3 short-name aliases. Off for
    /// structured actions: the permission engine classifies their canonical
    /// (alias-resolved) key separately.
    pub alias_check: bool,
    /// Facets collected while classifying (network use, secret reads, …).
    pub facets: RefCell<CommandFacets>,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(guard: &'a DangerousActionGuard, root: Option<String>) -> Self {
        Self { guard, root, root_path: None, alias_check: false, facets: RefCell::new(CommandFacets::default()) }
    }

    /// Context for shell-command analysis: aliases are resolved against `root`.
    pub(crate) fn for_shell(guard: &'a DangerousActionGuard, root: Option<&std::path::Path>) -> Self {
        Self {
            guard,
            root: root.map(|r| r.to_string_lossy().replace('\\', "/").to_lowercase()),
            root_path: root.map(std::path::Path::to_path_buf),
            alias_check: true,
            facets: RefCell::new(CommandFacets::default()),
        }
    }
}

/// Whether one path component looks like an NTFS 8.3 short-name alias
/// (`git~1`, `env~1`, `progra~1`, `env~1.loc`). Windows resolves these to the
/// long name, so `GIT~1` may be `.git` and `ENV~1` may be `.env`.
fn is_short_alias_component(c: &str) -> bool {
    let (stem, ext) = match c.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, e),
        _ => (c, ""),
    };
    if ext.len() > 3 {
        return false;
    }
    let Some((base, num)) = stem.rsplit_once('~') else { return false };
    !base.is_empty()
        && base.len() <= 6
        && !num.is_empty()
        && num.bytes().all(|b| b.is_ascii_digit())
        && base.len() + 1 + num.len() <= 8
}

/// Whether any component of a normalized path is an 8.3 short-name alias.
pub(crate) fn has_short_name_alias(n: &str) -> bool {
    n.split('/').any(is_short_alias_component)
}

/// Result of resolving a path word that may contain 8.3 aliases.
pub(crate) enum AliasResolution {
    /// No alias component: the text is what the shell will touch.
    NoAlias,
    /// Aliases resolved against the root: the lower-cased, `/`-separated
    /// canonical path relative to the root.
    Resolved(String),
    /// Contains an alias that cannot be resolved inside the root (no root,
    /// outside the root, variables…): treat it as possibly anything.
    Unresolved,
}

/// Resolves 8.3 short-name aliases in a shell path word against the root
/// (canonicalizing the longest existing ancestor, like the fs tools do).
pub(crate) fn resolve_short_alias(ctx: &Ctx<'_>, raw: &str) -> AliasResolution {
    if !ctx.alias_check || !has_short_name_alias(&norm_path(raw)) {
        return AliasResolution::NoAlias;
    }
    let Some(root) = &ctx.root_path else { return AliasResolution::Unresolved };
    let cleaned = raw.trim().trim_matches(['"', '\'']);
    if cleaned.starts_with(['$', '%', '~']) {
        return AliasResolution::Unresolved;
    }
    match crate::paths::confine_relative(root, cleaned) {
        Ok(rel) => AliasResolution::Resolved(rel.to_lowercase()),
        Err(_) => AliasResolution::Unresolved,
    }
}

fn d(category: C, reason: impl Into<String>) -> DangerClass {
    DangerClass::dangerous(category, reason)
}

fn f(category: C, reason: impl Into<String>) -> DangerClass {
    DangerClass::forbidden(category, reason)
}

// ---------------------------------------------------------------------------
// Regexes
// ---------------------------------------------------------------------------

struct Raw {
    fork_fn: Regex,
    cmd_fork: Regex,
    ps_fork: Regex,
    perl_fork: Regex,
    credentials: Regex,
    unsafe_agent: Regex,
    downloader: Regex,
    env_set: Regex,
    dir_delete: Regex,
    scriptblock_create: Regex,
    inline_net: Regex,
    inline_exec: Regex,
    inline_delete: Regex,
    inline_shell: Regex,
    awk_exec: Regex,
    secret_name: Regex,
}

fn raw() -> &'static Raw {
    static R: OnceLock<Raw> = OnceLock::new();
    R.get_or_init(|| {
        let re = |s: &str| Regex::new(s).expect("valid guard regex");
        Raw {
            fork_fn: re(r"([A-Za-z_:.][\w:.]*)\s*\(\s*\)\s*\{([^}]*)\}"),
            cmd_fork: re(r#"(?i)%0\s*\|\s*%0|start\s+(""\s+)?%0|\bstart\s+cmd\b.*\bgoto\b"#),
            ps_fork: re(r"(?is)while\s*\(\s*\$true\s*\)\s*\{[^}]*start-process\s+(powershell|pwsh)"),
            perl_fork: re(r"(?i)fork\s+while\s+fork|while\s*\(\s*1\s*\)\s*\{\s*fork"),
            credentials: re(
                r"(?ix)
                \.claude[\\/]+\.credentials\.json
              | \.codex[\\/]+auth\.json
              | \.config[\\/]+gh[\\/]+hosts\.yml
              | (google[\\/]+chrome|microsoft[\\/]+edge|bravesoftware|chromium)[\\/].*(cookies|login\ data)
              | mozilla[\\/]+firefox[\\/].*(cookies\.sqlite|logins\.json|key4\.db)
              | library[\\/]+keychains
              | \bsecurity\s+dump-keychain
              | \bsecurity\s+find-(generic|internet)-password\b.*\s-[wg]\b
              | \bsecret-tool\s+lookup
              | \bvaultcmd\b.*/listcreds
              | \bget-storedcredential\b
              | \bcredenumerate\b
              | \bcmdkey\s+/list
              | \bmimikatz\b
              | \bsekurlsa::
              | \bprocdump\b.*\blsass
              | comsvcs(\.dll)?.*minidump
              | \breg\s+save\s+hklm\\(sam|security|system)\b
              | \bcore\.token\b",
            ),
            unsafe_agent: re(
                r"(?i)--dangerously-skip-permissions|--dangerously-bypass-approvals-and-sandbox|(--sandbox|\s-s)[\s=]+danger-full-access|--yolo\b",
            ),
            downloader: re(
                r"(?i)\b(curl|wget|iwr|irm|invoke-webrequest|invoke-restmethod|downloadstring|downloadfile|downloaddata|net\.webclient|start-bitstransfer|bitsadmin|certutil)\b|https?://",
            ),
            env_set: re(r"(?i)\[(system\.)?environment\]::setenvironmentvariable\s*\((.*)\)"),
            dir_delete: re(r"(?i)\[(system\.)?io\.directory\]::delete\s*\([^)]*\$true"),
            scriptblock_create: re(r"(?i)\[(system\.management\.automation\.)?scriptblock\]::create\s*\("),
            inline_net: re(r"(?i)(urllib|requests\.|http\.client|urlopen|fetch\s*\(|https?://|net\.webclient|socket\.|lwp::|open-uri|net::http)"),
            inline_exec: re(r"(?i)(\bexec\s*\(|\beval\s*\(|os\.system|subprocess|child_process|\bfunction\s*\(|compile\s*\(|`|system\s*\()"),
            inline_delete: re(
                r"(?i)(shutil\.rmtree|rmsync|rmdirsync|rimraf|fs\.rm\s*\(|fs\.promises\.rm|os\.removedirs|fileutils\.rm_r|rm_rf|remove_tree|rmtree)",
            ),
            inline_shell: re(
                r"(?i)os\.system|os\.popen|os\.exec|os\.spawn|os\.execute|subprocess|pty\.spawn|commands\.getoutput|child_process|\bexecsync\b|\bspawnsync\b|\bexecfilesync\b|\bsystem\s*\(|\bexec\s*\(|\beval\s*\(|\bpassthru\s*\(|\bshell_exec\s*\(|\bproc_open\s*\(|\bpopen\s*\(|io\.popen|\bopen3\b|%x[\{\(\[]|\bqx\s*[\(\{/\[]|deno\.run|deno\.command|bun\.spawn|\bspawn\s*\(|\bexecfile\s*\(|kernel\.(system|exec|spawn)|__import__|importlib|\bexecv|runtime\.getruntime|processbuilder",
            ),
            awk_exec: re(r#"(?i)system\s*\(|\|\s*getline|\|&|print[^;}]*\|\s*""#),
            secret_name: re(r"(?i)(key|token|secret|passw|credential|apikey)"),
        }
    })
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Classifies a full command line.
pub(crate) fn classify_text(ctx: &Ctx<'_>, text: &str, family: Family, depth: usize) -> DangerClass {
    if depth > MAX_DEPTH {
        return d(C::Obfuscation, "command nested more than 4 levels deep");
    }
    if text.len() > MAX_COMMAND_LEN {
        return d(C::Unparseable, "command line too long to analyse");
    }
    let mut acc = raw_checks(ctx, text, family);
    if acc.is_forbidden() {
        return acc;
    }
    let parsed = match lexer::parse(text, family) {
        Ok(p) => p,
        Err(e) => return acc.max(d(C::Unparseable, format!("could not parse command ({e})"))),
    };
    for nested in &parsed.nested {
        acc = acc.max(classify_text(ctx, nested, family, depth + 1));
    }
    let mut stage_start = 0;
    for (i, cmd) in parsed.commands.iter().enumerate() {
        if !cmd.piped {
            stage_start = i;
        }
        let upstream: Vec<&SimpleCommand> = parsed.commands[stage_start..i].iter().collect();
        acc = acc.max(classify_simple(ctx, cmd, &upstream, family, depth));
    }
    acc
}

fn raw_checks(ctx: &Ctx<'_>, text: &str, family: Family) -> DangerClass {
    let r = raw();
    for cap in r.fork_fn.captures_iter(text) {
        let name = cap.get(1).map_or("", |m| m.as_str());
        let body: String = cap.get(2).map_or("", |m| m.as_str()).chars().filter(|c| !c.is_whitespace()).collect();
        if !name.is_empty() && body.contains(&format!("{name}|{name}")) {
            return f(C::ResourceExhaustion, "fork bomb");
        }
    }
    if r.cmd_fork.is_match(text) || r.ps_fork.is_match(text) || r.perl_fork.is_match(text) {
        return f(C::ResourceExhaustion, "self-replicating process loop (fork bomb)");
    }
    if r.credentials.is_match(text) {
        if text.to_lowercase().contains("core.token") {
            return f(C::SelfTamper, "accesses the Orkestr core token");
        }
        return f(C::CredentialAccess, "accesses another application's credentials or the OS credential store");
    }
    if r.unsafe_agent.is_match(text) {
        return f(C::UnsafeAgentMode, "launches an agent CLI with its safety sandbox disabled");
    }
    let mut acc = DangerClass::Safe;
    if family == Family::PowerShell {
        if let Some(cap) = r.env_set.captures(text) {
            let inner = cap.get(2).map_or("", |m| m.as_str()).to_lowercase();
            if inner.contains("user") || inner.contains("machine") {
                acc = acc.max(if r.secret_name.is_match(&inner) {
                    d(C::SecretChange, "persists a secret in a user/machine environment variable")
                } else {
                    d(C::SystemConfig, "persists a user/machine environment variable")
                });
            }
        }
        if r.dir_delete.is_match(text) {
            acc = acc.max(d(C::RecursiveDelete, "[IO.Directory]::Delete(…, $true) recursive delete"));
        }
        if r.scriptblock_create.is_match(text) {
            acc = acc.max(if r.downloader.is_match(text) {
                d(C::RemoteCodeExecution, "creates a script block from downloaded content")
            } else {
                d(C::Obfuscation, "creates a script block from a dynamic string")
            });
        }
    }
    if facets::raw_network(text) {
        facets::mark_network(ctx);
    }
    acc
}

fn is_assignment(w: &Word) -> bool {
    let Some((name, _)) = w.text.split_once('=') else { return false };
    !name.is_empty()
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Normalises a program word: strips directories, lower-cases, removes
/// `.exe/.com/.cmd/.bat`. Returns `(name, had_extension)`.
fn normalize_program(text: &str) -> (String, bool) {
    let base = text.rsplit(['/', '\\']).next().unwrap_or(text);
    let lower = base.to_lowercase();
    for ext in [".exe", ".com", ".cmd", ".bat"] {
        if let Some(s) = lower.strip_suffix(ext) {
            return (s.to_owned(), true);
        }
    }
    (lower, false)
}

fn ps_alias(name: &str) -> &str {
    match name {
        "rm" | "ri" | "del" | "erase" | "rd" | "rmdir" => "remove-item",
        "iex" => "invoke-expression",
        "iwr" | "curl" | "wget" => "invoke-webrequest",
        "irm" => "invoke-restmethod",
        "sp" => "set-itemproperty",
        "ni" => "new-item",
        "saps" | "start" => "start-process",
        "spps" | "kill" => "stop-process",
        "cpi" | "copy" | "cp" => "copy-item",
        "mi" | "move" | "mv" => "move-item",
        "sc" => "set-content",
        "ac" => "add-content",
        "rni" | "ren" => "rename-item",
        "si" => "set-item",
        "gc" | "cat" | "type" => "get-content",
        "gci" | "ls" | "dir" => "get-childitem",
        "icm" => "invoke-command",
        "rp" => "remove-itemproperty",
        "clc" => "clear-content",
        "tee" => "tee-object",
        "spsv" => "stop-service",
        "sasv" => "start-service",
        other => other,
    }
}

fn texts(args: &[Word]) -> Vec<String> {
    args.iter().map(|w| w.text.clone()).collect()
}

fn lower_texts(args: &[Word]) -> Vec<String> {
    args.iter().map(|w| w.text.to_lowercase()).collect()
}

fn join(args: &[Word]) -> String {
    texts(args).join(" ")
}

/// POSIX flag check: `-r`, `-rf`, `--recursive` (stops at `--`).
fn posix_flag(args: &[Word], shorts: &[char], longs: &[&str]) -> bool {
    for a in args {
        let t = a.text.as_str();
        if t == "--" {
            break;
        }
        if let Some(long) = t.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or(long).to_lowercase();
            if longs.contains(&name.as_str()) {
                return true;
            }
        } else if let Some(cluster) = t.strip_prefix('-') {
            if !cluster.is_empty()
                && !cluster.chars().all(|c| c.is_ascii_digit())
                && cluster.chars().any(|c| shorts.contains(&c))
            {
                return true;
            }
        }
    }
    false
}

/// POSIX operands (non-flag words; everything after `--`).
fn posix_operands(args: &[Word]) -> Vec<&Word> {
    let mut out = Vec::new();
    let mut opts_done = false;
    for a in args {
        if !opts_done && a.text == "--" {
            opts_done = true;
            continue;
        }
        if !opts_done && a.text.starts_with('-') && a.text.len() > 1 && !a.quoted {
            continue;
        }
        out.push(a);
    }
    out
}

fn is_ps_param(t: &str) -> bool {
    (t.starts_with('-') || t.starts_with('\u{2013}') || t.starts_with('\u{2014}'))
        && t.chars().nth(1).is_some_and(|c| c.is_alphabetic())
}

/// `(name_lower, inline_value)` of a PowerShell parameter word.
fn ps_param_name(t: &str) -> Option<(String, Option<String>)> {
    if !is_ps_param(t) {
        return None;
    }
    let body = t.trim_start_matches(['-', '\u{2013}', '\u{2014}']);
    match body.split_once(':') {
        Some((n, v)) => Some((n.to_lowercase(), Some(v.to_owned()))),
        None => Some((body.to_lowercase(), None)),
    }
}

fn ps_matches(given: &str, full: &str, min: usize) -> bool {
    given.len() >= min && full.starts_with(given)
}

/// Whether a PowerShell parameter (prefix-matched) is present.
fn ps_has(args: &[Word], full: &str, min: usize) -> bool {
    args.iter().any(|a| ps_param_name(&a.text).is_some_and(|(n, _)| ps_matches(&n, full, min)))
}

/// Value of a PowerShell parameter (`-Name value` or `-Name:value`).
fn ps_value(args: &[Word], full: &str, min: usize) -> Option<Word> {
    for (i, a) in args.iter().enumerate() {
        if let Some((n, inline)) = ps_param_name(&a.text) {
            if ps_matches(&n, full, min) {
                if let Some(v) = inline {
                    return Some(Word { text: v, ..a.clone() });
                }
                return args.get(i + 1).cloned();
            }
        }
    }
    None
}

/// Positional (unnamed) PowerShell arguments, skipping values of `value_params`.
fn ps_positionals<'a>(args: &'a [Word], value_params: &[&str]) -> Vec<&'a Word> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if let Some((n, inline)) = ps_param_name(&a.text).filter(|_| !a.quoted) {
            if inline.is_none() && value_params.iter().any(|p| ps_matches(&n, p, 2) || n == *p) {
                skip = true;
            }
            continue;
        }
        out.push(a);
    }
    out
}

/// cmd-style switch present (`/s`, `/S`, clustered `/s/q`, `/s:x`).
fn has_switch(args: &[Word], sw: &str) -> bool {
    args.iter().any(|a| {
        a.text.starts_with('/')
            && a.text.split('/').filter(|s| !s.is_empty()).any(|s| {
                let name = s.split(':').next().unwrap_or(s);
                name.eq_ignore_ascii_case(sw)
            })
    })
}

/// A cmd-style switch word (`/s`, `/Q`, `/s/q`, `/grant:r`) — not a path.
fn is_switch_word(a: &Word) -> bool {
    let b = a.text.as_bytes();
    !a.quoted
        && b.len() >= 2
        && b[0] == b'/'
        && b[1].is_ascii_alphabetic()
        && !a.text[1..].contains(['\\', '.'])
        && (a.text.len() <= 12 || a.text.contains(':'))
        && a.text[1..].split('/').all(|seg| seg.len() <= 10)
}

fn non_switch(args: &[Word]) -> Vec<&Word> {
    args.iter().filter(|a| !is_switch_word(a)).collect()
}

fn cmd_delete(ctx: &Ctx<'_>, args: &[Word], is_rd: bool) -> DangerClass {
    let recursive = has_switch(args, "s");
    let targets = non_switch(args);
    let mut acc = DangerClass::Safe;
    for t in &targets {
        acc = acc.max(check_delete_target(ctx, t, recursive, Family::Cmd)).max(check_write_path(ctx, &t.text, true));
    }
    if recursive {
        acc.max(d(C::RecursiveDelete, if is_rd { "recursive delete (rd /s)" } else { "recursive delete (del /s)" }))
    } else if targets.iter().any(|t| t.glob || t.dynamic) {
        acc.max(d(C::RecursiveDelete, "delete with a wildcard or variable target"))
    } else {
        acc
    }
}

fn upstream_has_downloader(upstream: &[&SimpleCommand]) -> bool {
    upstream.iter().any(|c| {
        c.words.iter().any(|w| raw().downloader.is_match(&w.text))
            || c.words.first().is_some_and(|w| {
                let (p, _) = normalize_program(&w.text);
                matches!(
                    p.as_str(),
                    "curl"
                        | "wget"
                        | "iwr"
                        | "irm"
                        | "invoke-webrequest"
                        | "invoke-restmethod"
                        | "fetch"
                        | "aria2c"
                        | "http"
                        | "https"
                )
            })
    })
}

fn rce(reason: &str) -> DangerClass {
    d(C::RemoteCodeExecution, reason.to_owned())
}

// ---------------------------------------------------------------------------
// Path checks
// ---------------------------------------------------------------------------

pub(crate) fn norm_path(p: &str) -> String {
    p.trim().trim_matches(['"', '\'']).replace('\\', "/").to_lowercase()
}

/// Whether a file name is secret-bearing.
pub(crate) fn is_secret_file(path: &str) -> bool {
    let n = norm_path(path);
    let name = n.rsplit('/').next().unwrap_or(&n);
    if name.is_empty() {
        return false;
    }
    let example = name.ends_with(".example") || name.ends_with(".sample") || name.ends_with(".template");
    (name == ".env" || (name.starts_with(".env.") && !example) || name.ends_with(".env"))
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.ends_with(".pfx")
        || name.ends_with(".p12")
        || name.starts_with("id_rsa")
        || name.starts_with("id_ed25519")
        || name.starts_with("id_ecdsa")
        || name.contains("credentials")
        || name.starts_with("secrets.")
        || name == ".netrc"
        || name == ".pypirc"
        || name == ".npmrc"
        || name == ".git-credentials"
}

fn is_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

fn is_protected_root(base: &str) -> bool {
    const NAMES: &[&str] = &[
        "",
        "~",
        "$home",
        "${home}",
        "%userprofile%",
        "$env:userprofile",
        "${env:userprofile}",
        "$env:home",
        "%homepath%",
        "%homedrive%%homepath%",
        "$env:homepath",
        "%systemroot%",
        "$env:systemroot",
        "%windir%",
        "$env:windir",
        "%systemdrive%",
        "$env:systemdrive",
        "%programfiles%",
        "$env:programfiles",
        "%programfiles(x86)%",
        "${env:programfiles(x86)}",
        "%programdata%",
        "$env:programdata",
        "%appdata%",
        "$env:appdata",
        "%localappdata%",
        "$env:localappdata",
        "/etc",
        "/usr",
        "/bin",
        "/sbin",
        "/boot",
        "/lib",
        "/lib32",
        "/lib64",
        "/var",
        "/opt",
        "/system",
        "/users",
        "/home",
        "/root",
        "/library",
        "/applications",
        "/dev",
        "/proc",
        "/sys",
        "/srv",
        "/mnt",
        "/private",
        "/snap",
        "/usr/bin",
        "/usr/lib",
        "/usr/local",
        "/var/lib",
    ];
    if NAMES.contains(&base) || is_drive(base) {
        return true;
    }
    if base.starts_with('~') && !base.contains('/') {
        return true; // ~user
    }
    let parts: Vec<&str> = base.split('/').collect();
    // C:/Windows, C:/Users, C:/Users/<name>, C:/Program Files…
    if parts.len() >= 2 && is_drive(parts[0]) {
        let rest = parts[1..].join("/");
        if matches!(
            rest.as_str(),
            "windows"
                | "windows/system32"
                | "windows/syswow64"
                | "program files"
                | "program files (x86)"
                | "programdata"
                | "users"
        ) {
            return true;
        }
        if parts.len() == 3 && parts[1] == "users" {
            return true;
        }
    }
    // /home/<user>, /Users/<user>
    if parts.len() == 3 && parts[0].is_empty() && (parts[1] == "home" || parts[1] == "users") {
        return true;
    }
    false
}

/// Checks one delete target.
pub(crate) fn check_delete_target(_ctx: &Ctx<'_>, w: &Word, recursive: bool, _family: Family) -> DangerClass {
    let p = norm_path(&w.text);
    let mut base = p.as_str();
    loop {
        let before = base.len();
        base = base.trim_end_matches('*');
        base = base.trim_end_matches('/');
        base = base.strip_suffix("/.").unwrap_or(base);
        if base.len() == before {
            break;
        }
    }
    let wild = w.glob || p.ends_with('*');
    if is_protected_root(base) {
        return if recursive || wild {
            f(C::ProtectedPathDelete, format!("deletes protected location `{}`", w.text))
        } else {
            d(C::ProtectedPathDelete, format!("deletes protected location `{}`", w.text))
        };
    }
    if base.starts_with("/var/log") && (recursive || wild) {
        return f(C::LogWipe, "wipes system logs");
    }
    if (base == ".." || base.split('/').all(|s| s == "..")) && !base.is_empty() && (recursive || wild) {
        return f(C::ProtectedPathDelete, "deletes the parent of the working directory");
    }
    DangerClass::Safe
}

/// Checks a read target (structured `ReadFile` actions).
pub(crate) fn check_read_path(ctx: &Ctx<'_>, path: &str) -> DangerClass {
    if raw().credentials.is_match(path) {
        return f(C::CredentialAccess, "reads another application's credentials");
    }
    let n = norm_path(path);
    if n.contains("core.token") {
        return f(C::SelfTamper, "reads the Orkestr core token");
    }
    if ctx.guard.protected_lower.iter().any(|p| n.starts_with(p.as_str())) {
        return f(C::SelfTamper, "reads Orkestr's private data directory");
    }
    DangerClass::Safe
}

/// Whether a normalized path starts with a variable reference (`$x`, `${x}`,
/// `$env:x`, `$(…)`, `%x%`) other than the working-directory variables. Such a
/// target can expand to any location, so it is treated as absolute.
pub(crate) fn is_variable_rooted(n: &str) -> bool {
    let first = n.split('/').next().unwrap_or(n);
    if matches!(first, "$pwd" | "${pwd}" | "$(pwd)" | "$pwd.path" | "$env:pwd" | "${env:pwd}" | "%cd%" | ".") {
        return false;
    }
    n.starts_with('$') || (n.starts_with('%') && n[1..].contains('%'))
}

/// Checks a write (or delete) target path.
pub(crate) fn check_write_path(ctx: &Ctx<'_>, path: &str, is_delete: bool) -> DangerClass {
    let n = norm_path(path);
    if n.is_empty() || matches!(n.as_str(), "/dev/null" | "nul" | "$null" | "/dev/stdout" | "/dev/stderr" | "con" | "-")
    {
        return DangerClass::Safe;
    }
    let verb = if is_delete { "deletes" } else { "writes" };
    if n.contains("core.token")
        || n.ends_with("orkestr.db")
        || n.contains("orkestr.db-")
        || ctx.guard.protected_lower.iter().any(|p| n.starts_with(p.as_str()) || n.contains(p.as_str()))
    {
        return f(C::SelfTamper, format!("{verb} Orkestr's own data (core token / database / data dir)"));
    }
    for dev in
        ["/dev/sd", "/dev/nvme", "/dev/hd", "/dev/vd", "/dev/xvd", "/dev/disk", "/dev/mmcblk", "/dev/mapper", "//./"]
    {
        if n.starts_with(dev) {
            return f(C::DiskDestruction, format!("{verb} a raw disk device"));
        }
    }
    let mut acc = DangerClass::Safe;
    // 8.3 short names (`GIT~1`, `ENV~1`) reach `.git` / `.env` on Windows:
    // check the alias-resolved path as well as the text.
    let mut names = vec![n.clone()];
    match resolve_short_alias(ctx, path) {
        AliasResolution::NoAlias => {}
        AliasResolution::Resolved(rel) => names.push(rel),
        AliasResolution::Unresolved => {
            acc = acc.max(d(
                C::Obfuscation,
                format!("{verb} through an 8.3 short-name alias that cannot be resolved (`{}`)", path.trim()),
            ));
        }
    }
    if names.iter().any(|p| p.split('/').any(|c| c == ".git")) {
        acc = acc.max(d(C::GitInternals, format!("{verb} git internals (.git) directly")));
    }
    if names.iter().any(|p| is_secret_file(p)) {
        acc = acc.max(d(C::SecretChange, format!("{verb} a secrets file (`{}`)", path.trim())));
    }
    if n.ends_with("/etc/hosts") || n.contains("drivers/etc/hosts") {
        acc = acc.max(d(C::NetworkConfig, format!("{verb} the hosts file")));
    } else if ["/etc/", "/usr/", "/bin/", "/sbin/", "/boot/", "/lib/", "/system/", "/library/"]
        .iter()
        .any(|p| n.starts_with(p))
        || n.contains(":/windows/")
        || n.contains(":/program files")
        || n.starts_with("%systemroot%")
        || n.starts_with("$env:systemroot")
        || n.starts_with("%windir%")
        || n.starts_with("$env:windir")
    {
        acc = acc.max(d(C::SystemConfig, format!("{verb} system files")));
    }
    let name = n.rsplit('/').next().unwrap_or(&n);
    if matches!(
        name,
        ".bashrc"
            | ".bash_profile"
            | ".zshrc"
            | ".zprofile"
            | ".zshenv"
            | ".profile"
            | ".bash_login"
            | "profile.ps1"
            | "microsoft.powershell_profile.ps1"
            | "$profile"
            | "config.fish"
    ) {
        acc = acc.max(d(C::SystemConfig, format!("{verb} shell startup files (persistence)")));
    }
    if let Some(root) = &ctx.root {
        // Variable-rooted targets (`$env:APPDATA\…`, `${HOME}/…`, `%TEMP%\…`,
        // `$(…)/…`) may expand to anywhere: treat them as absolute, except
        // the working-directory variables (the cwd is confined to the root).
        let absolute = n.starts_with('/')
            || n.starts_with('~')
            || is_variable_rooted(&n)
            || {
                let b = n.as_bytes();
                b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
            };
        let root = root.trim_end_matches('/');
        if absolute && !(n == root || n.starts_with(&format!("{root}/"))) {
            acc = acc.max(d(C::OutsideWorktree, format!("{verb} outside the worktree (`{}`)", path.trim())));
        } else if n.split('/').any(|c| c == "..") {
            acc = acc.max(d(C::OutsideWorktree, format!("{verb} a path with `..` (may leave the worktree)")));
        }
    }
    acc
}

// ---------------------------------------------------------------------------
// Simple command classification
// ---------------------------------------------------------------------------

/// Classifies one simple command (with its pipeline upstream).
pub(crate) fn classify_simple(
    ctx: &Ctx<'_>,
    cmd: &SimpleCommand,
    upstream: &[&SimpleCommand],
    family: Family,
    depth: usize,
) -> DangerClass {
    if depth > MAX_DEPTH {
        return d(C::Obfuscation, "command wrapped more than 4 levels deep");
    }
    let mut acc = DangerClass::Safe;
    for r in &cmd.redirects {
        acc = acc.max(check_write_path(ctx, &r.text, false)).max(check_protected_ref(ctx, &r.text));
    }
    let mut words: &[Word] = &cmd.words;
    if family == Family::Posix {
        while words.first().is_some_and(is_assignment) {
            if let Some((name, value)) = words[0].text.split_once('=') {
                acc = acc.max(env_hook(ctx, name, value, depth));
            }
            words = &words[1..];
        }
    }
    // Any word naming Orkestr's data dir / core token (globs, variables, `..`).
    for w in words {
        acc = acc.max(check_protected_ref(ctx, &w.text));
    }
    if acc.is_forbidden() {
        return acc;
    }
    let Some(first) = words.first() else { return acc };

    if family == Family::Posix && !first.quoted {
        // Function definitions: `f() { body; }`, `f(){ body; }`, `function f { body; }`.
        if first.text.ends_with("()") || first.text.ends_with("(){") {
            let rest: Vec<Word> = words[1..].iter().filter(|w| w.text != "{" && w.text != "}").cloned().collect();
            return acc.max(classify_simple(ctx, &inner_cmd(cmd, &rest), upstream, family, depth + 1));
        }
        if first.text == "function" {
            let rest: Vec<Word> = words
                .iter()
                .skip(2)
                .filter(|w| !matches!(w.text.as_str(), "{" | "}" | "()" | "(){"))
                .cloned()
                .collect();
            return acc.max(classify_simple(ctx, &inner_cmd(cmd, &rest), upstream, family, depth + 1));
        }
    }
    // cmd `for /f … in ('command')` sets: the quoted text is executed.
    if family == Family::Cmd && !first.quoted && (first.text.starts_with('\'') || first.text.starts_with('`')) {
        let mut ws = words.to_vec();
        ws[0].text = ws[0].text[1..].to_owned();
        if let Some(last) = ws.last_mut() {
            last.text = last.text.trim_end_matches(['\'', '`']).to_owned();
        }
        if ws[0].text.is_empty() {
            ws.remove(0);
        }
        return acc.max(classify_simple(ctx, &inner_cmd(cmd, &ws), upstream, family, depth + 1));
    }

    // Program word that is itself a command substitution: `$(curl …)`.
    if first.dynamic && (first.text.contains("$(") || first.text.contains('`')) && !cmd.call_op {
        return acc.max(if raw().downloader.is_match(&first.text) {
            rce("executes the output of a download")
        } else {
            d(C::Obfuscation, "program name comes from a command substitution")
        });
    }
    if family == Family::PowerShell && cmd.call_op && first.text.starts_with('{') && words.len() == 1 {
        // `& { … }`: the script block was classified as nested text.
        return acc;
    }
    if family == Family::PowerShell
        && !cmd.call_op
        && (first.text.starts_with('$') || first.text.starts_with('(') || first.text.starts_with('{'))
    {
        // An expression statement (`$x = …`, `$x`, `(Get-Item x).Length`), not a
        // command invocation; nested parts were classified already.
        let lower_all = words.iter().map(|w| w.text.to_lowercase()).collect::<Vec<_>>().join(" ");
        if lower_all.contains(".invoke") || lower_all.contains("invokescript") || lower_all.contains("invokecommand") {
            return acc.max(d(C::Obfuscation, "invokes a script block or command held in a variable"));
        }
        if let Some(name) = first.text.to_lowercase().strip_prefix("$env:") {
            let (name, inline) = match name.split_once('=') {
                Some((n, v)) => (n.to_owned(), Some(v.to_owned())),
                None => (name.to_owned(), None),
            };
            let value = inline.or_else(|| {
                if words.get(1).is_some_and(|w| w.text == "=") {
                    words.get(2).map(|w| w.text.clone())
                } else {
                    None
                }
            });
            if let Some(v) = value {
                acc = acc.max(env_hook(ctx, &name, &v, depth));
            }
        }
        return acc;
    }
    // A program name only known at runtime (`$X -rf /`, `& $c`, `%X% /s /q`,
    // `& ('Remove'+'-Item')`): fail closed. A variable directory with a
    // literal program name (`"$CARGO_HOME/bin/cargo"`) is still classified by name.
    if first.dynamic {
        let base = first.text.rsplit(['/', '\\']).next().unwrap_or(&first.text);
        if base.is_empty() || base.contains(['$', '%', '`', '(', ')', '{', '}', '!', '@']) {
            return acc.max(if raw().downloader.is_match(&first.text) {
                rce("executes a program named by downloaded content")
            } else {
                d(C::Obfuscation, format!("program name is only known at runtime (`{}`)", first.text))
            });
        }
    }

    let mut args: Vec<Word> = words[1..].to_vec();
    let mut prog_text = first.text.clone();
    if family == Family::Cmd && !first.quoted {
        // `rd/s/q x` → `rd` `/s/q` `x`
        if let Some(pos) = prog_text.find('/') {
            if !prog_text[..pos].contains(['\\', ':']) && pos > 0 {
                let rest = prog_text[pos..].to_owned();
                prog_text.truncate(pos);
                args.insert(0, Word::literal(&rest));
            }
        }
    }
    let (mut prog, had_ext) = normalize_program(&prog_text);
    if family == Family::PowerShell && !had_ext {
        prog = ps_alias(&prog).to_owned();
    }
    facets::record(ctx, &prog, &args, family);
    acc.max(program_rules(ctx, &prog, had_ext, &args, cmd, upstream, family, depth))
}

/// Environment variables that make a later program execute their value.
fn env_hook(ctx: &Ctx<'_>, name: &str, value: &str, depth: usize) -> DangerClass {
    let upper = name.trim().to_ascii_uppercase();
    let lower_v = value.to_lowercase();
    let hook = matches!(
        upper.as_str(),
        "GIT_SSH_COMMAND"
            | "GIT_SSH"
            | "GIT_PROXY_COMMAND"
            | "GIT_EXTERNAL_DIFF"
            | "GIT_PAGER"
            | "PAGER"
            | "GIT_EDITOR"
            | "EDITOR"
            | "VISUAL"
            | "GIT_SEQUENCE_EDITOR"
            | "GIT_ASKPASS"
            | "SSH_ASKPASS"
            | "GIT_CONFIG_PARAMETERS"
            | "LD_PRELOAD"
            | "DYLD_INSERT_LIBRARIES"
            | "BASH_ENV"
            | "ENV"
            | "PROMPT_COMMAND"
            | "PERL5OPT"
            | "RUBYOPT"
            | "PYTHONSTARTUP"
            | "COMSPEC"
    ) || (upper == "NODE_OPTIONS"
        && (lower_v.contains("--require")
            || lower_v.contains("-r ")
            || lower_v.contains("--import")
            || lower_v.contains("--loader")));
    if !hook {
        return DangerClass::Safe;
    }
    let inner = classify_text(ctx, value, Family::Posix, depth + 1);
    inner.max(d(C::Obfuscation, format!("environment variable `{upper}` makes a later program run a command")))
}

fn inner_cmd(cmd: &SimpleCommand, words: &[Word]) -> SimpleCommand {
    SimpleCommand {
        words: words.to_vec(),
        redirects: Vec::new(),
        heredocs: cmd.heredocs.clone(),
        piped: cmd.piped,
        call_op: false,
    }
}

/// Skips leading flags (and values of `with_value` flags), then optionally
/// one positional operand; returns the index of the wrapped command.
fn skip_opts(args: &[Word], with_value: &[&str], skip_positional: usize) -> usize {
    let mut i = 0;
    while i < args.len() {
        let t = args[i].text.as_str();
        if t == "--" {
            i += 1;
            break;
        }
        if t.starts_with('-') && t.len() > 1 {
            let name = t.split('=').next().unwrap_or(t);
            i += if with_value.contains(&name) && !t.contains('=') { 2 } else { 1 };
            continue;
        }
        break;
    }
    let mut remaining = skip_positional;
    while remaining > 0 && i < args.len() {
        i += 1;
        remaining -= 1;
    }
    i.min(args.len())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn program_rules(
    ctx: &Ctx<'_>,
    prog: &str,
    had_ext: bool,
    args: &[Word],
    cmd: &SimpleCommand,
    upstream: &[&SimpleCommand],
    family: Family,
    depth: usize,
) -> DangerClass {
    let lower = lower_texts(args);
    let has = |w: &str| lower.iter().any(|a| a == w);
    let sub = lower.iter().find(|a| !a.starts_with('-')).cloned().unwrap_or_default();
    let unwrap = |start: usize, fam: Family| -> DangerClass {
        if start >= args.len() {
            return DangerClass::Safe;
        }
        classify_simple(ctx, &inner_cmd(cmd, &args[start..]), upstream, fam, depth + 1)
    };
    let text = |t: &str, fam: Family| classify_text(ctx, t, fam, depth + 1);
    let piped_rce = !upstream.is_empty() && upstream_has_downloader(upstream);

    match prog {
        // ---------------- wrappers ----------------
        "sudo" | "doas" | "pkexec" | "gsudo" | "sudo-rs" => {
            let priv_ = d(C::PrivilegeEscalation, format!("runs with elevated privileges ({prog})"));
            let i = skip_opts(
                args,
                &["-u", "-g", "-h", "-p", "-U", "-C", "-D", "-r", "-t", "--user", "--group", "--host"],
                0,
            );
            unwrap(i, family).max(priv_)
        }
        "su" => {
            let priv_ = d(C::PrivilegeEscalation, "switches user (su)");
            match ps_value(args, "c", 1).or_else(|| ps_value(args, "command", 3)) {
                Some(c) => text(&c.text, Family::Posix).max(priv_),
                None => priv_,
            }
        }
        "runas" => {
            let priv_ = d(C::PrivilegeEscalation, "runs as another user (runas)");
            match non_switch(args).last() {
                Some(w) => text(&w.text, Family::Cmd).max(priv_),
                None => priv_,
            }
        }
        "psexec" | "psexec64" | "paexec" => d(C::PrivilegeEscalation, "remote/elevated execution (psexec)"),
        "env" => {
            let mut i = 0;
            while i < args.len() {
                let t = args[i].text.as_str();
                if t == "-u" || t == "-C" || t == "--unset" || t == "--chdir" {
                    i += 2;
                } else if t.starts_with('-') || is_assignment(&args[i]) {
                    i += 1;
                } else {
                    break;
                }
            }
            unwrap(i, family)
        }
        "nohup" | "time" | "command" | "exec" | "builtin" | "setsid" | "chronic" | "unbuffer" | "caffeinate"
        | "stdbuf" | "strace" | "ltrace" | "flock" | "call" => {
            let i = skip_opts(args, &["-o", "-n", "-e", "-c", "-w"], usize::from(prog == "flock"));
            unwrap(i, family)
        }
        "nice" | "ionice" => unwrap(skip_opts(args, &["-n", "-c", "-p", "--adjustment"], 0), family),
        "timeout" => unwrap(skip_opts(args, &["-s", "-k", "--signal", "--kill-after"], 1), family),
        "watch" => {
            let i = skip_opts(args, &["-n", "-d", "--interval"], 0);
            text(&join(&args[i.min(args.len())..]), family)
        }
        "xargs" => {
            let i = skip_opts(args, &["-I", "-n", "-P", "-d", "-L", "-s", "-E", "-a"], 0);
            let inner = unwrap(i, family);
            let target = args.get(i).map(|w| normalize_program(&w.text).0).unwrap_or_default();
            if matches!(target.as_str(), "rm" | "del" | "unlink" | "shred" | "remove-item" | "rmdir") {
                inner.max(d(C::RecursiveDelete, "mass delete via xargs"))
            } else {
                inner
            }
        }
        "find" => {
            let mut acc = DangerClass::Safe;
            if has("-delete") {
                acc = acc.max(d(C::RecursiveDelete, "find … -delete mass delete"));
            }
            let mut i = 0;
            while i < args.len() {
                let t = lower[i].as_str();
                if matches!(t, "-exec" | "-execdir" | "-ok" | "-okdir") {
                    let start = i + 1;
                    let mut end = start;
                    while end < args.len() && args[end].text != ";" && args[end].text != "+" && args[end].text != "\\;"
                    {
                        end += 1;
                    }
                    if start < end {
                        let inner = inner_cmd(cmd, &args[start..end]);
                        acc = acc.max(classify_simple(ctx, &inner, &[], family, depth + 1));
                        let p = normalize_program(&args[start].text).0;
                        if matches!(p.as_str(), "rm" | "del" | "unlink" | "shred" | "rmdir") {
                            acc = acc.max(d(C::RecursiveDelete, "find … -exec rm mass delete"));
                        }
                    }
                    i = end;
                }
                i += 1;
            }
            acc
        }
        "bash" | "sh" | "zsh" | "dash" | "ksh" | "fish" | "ash" | "mksh" | "busybox" => {
            let mut acc = DangerClass::Safe;
            let mut idx = 0;
            if prog == "busybox" {
                // `busybox rm -rf x`
                if args.first().is_some_and(|w| !w.text.starts_with('-')) {
                    return unwrap(0, Family::Posix);
                }
            }
            let mut script_given = false;
            while idx < args.len() {
                let t = args[idx].text.as_str();
                if t.starts_with('-') && !t.starts_with("--") && t.len() > 1 && t[1..].contains('c') {
                    if let Some(c) = args.get(idx + 1) {
                        return acc.max(text(&c.text, Family::Posix)).max(
                            if raw().downloader.is_match(&c.text) && c.text.contains("$(") {
                                rce("shell executes downloaded content")
                            } else {
                                DangerClass::Safe
                            },
                        );
                    }
                    break;
                }
                if !t.starts_with('-') {
                    script_given = true;
                    if args[idx].dynamic && raw().downloader.is_match(t) {
                        acc = acc.max(rce("shell executes a downloaded script (process substitution)"));
                    }
                    break;
                }
                idx += 1;
            }
            for h in &cmd.heredocs {
                acc = acc.max(text(h, Family::Posix));
            }
            if !script_given {
                acc = acc.max(piped_script(ctx, upstream, Stdin::Shell(Family::Posix), depth));
            }
            acc
        }
        "cmd" => {
            if let Some(pos) = args.iter().position(|a| {
                let l = a.text.to_lowercase();
                l == "/c" || l == "/k" || l == "/r"
            }) {
                let rest: Vec<String> = texts(&args[pos + 1..]);
                return text(&rest.join(" "), Family::Cmd);
            }
            piped_script(ctx, upstream, Stdin::Shell(Family::Cmd), depth)
        }
        "powershell" | "pwsh" | "powershell_ise" => {
            let mut i = 0;
            while i < args.len() {
                let t = args[i].text.as_str();
                let Some(name) = t.strip_prefix(['-', '/']).map(str::to_lowercase) else { break };
                if name.is_empty() {
                    // `powershell -` reads commands from stdin
                    return piped_script(ctx, upstream, Stdin::Shell(Family::PowerShell), depth);
                }
                if name == "ec" || "encodedcommand".starts_with(name.as_str()) {
                    let Some(val) = args.get(i + 1) else {
                        return d(C::Obfuscation, "encoded PowerShell command");
                    };
                    let base = d(C::Obfuscation, "encoded PowerShell command (-EncodedCommand)");
                    return match decode_ps_base64(&val.text) {
                        Some(decoded) => base.max(text(&decoded, Family::PowerShell)),
                        None => base,
                    };
                }
                if "command".starts_with(name.as_str()) {
                    let rest = texts(&args[i + 1..]).join(" ");
                    if rest.trim() == "-" {
                        return piped_script(ctx, upstream, Stdin::Shell(Family::PowerShell), depth);
                    }
                    return text(&rest, Family::PowerShell);
                }
                if "file".starts_with(name.as_str()) {
                    return DangerClass::Safe;
                }
                let with_value = [
                    "executionpolicy",
                    "ep",
                    "ex",
                    "exec",
                    "windowstyle",
                    "w",
                    "version",
                    "v",
                    "configurationname",
                    "outputformat",
                    "of",
                    "inputformat",
                    "if",
                    "workingdirectory",
                    "wd",
                    "settingsfile",
                    "custompipename",
                    "psconsolefile",
                ];
                i += if with_value.iter().any(|v| (v.starts_with(name.as_str()) && name.len() >= 2) || *v == name) {
                    2
                } else {
                    1
                };
            }
            if i < args.len() {
                let rest = texts(&args[i..]).join(" ");
                if args.len() - i == 1 && rest.to_lowercase().ends_with(".ps1") {
                    return DangerClass::Safe;
                }
                return text(&rest, Family::PowerShell);
            }
            piped_script(ctx, upstream, Stdin::Shell(Family::PowerShell), depth)
        }
        "wsl" => {
            if has("--unregister") {
                return d(C::ContainerDestruction, "unregisters (deletes) a WSL distribution");
            }
            let mut i = 0;
            while i < args.len() {
                let t = lower[i].as_str();
                match t {
                    "-d" | "--distribution" | "-u" | "--user" | "--cd" => i += 2,
                    "-e" | "--exec" => return unwrap(i + 1, Family::Posix),
                    "--" => return text(&texts(&args[i + 1..]).join(" "), Family::Posix),
                    _ if t.starts_with('-') => i += 1,
                    _ => return text(&texts(&args[i..]).join(" "), Family::Posix),
                }
            }
            DangerClass::Safe
        }
        "invoke-expression" => {
            let arg = ps_value(args, "command", 1).or_else(|| ps_positionals(args, &[]).first().map(|w| (*w).clone()));
            match arg {
                None => {
                    if piped_rce {
                        rce("pipes downloaded content into Invoke-Expression")
                    } else {
                        d(C::Obfuscation, "Invoke-Expression on pipeline input")
                    }
                }
                Some(w) if w.dynamic => {
                    if raw().downloader.is_match(&w.text) {
                        rce("Invoke-Expression on downloaded content")
                    } else {
                        d(C::Obfuscation, "Invoke-Expression with dynamic input")
                    }
                }
                Some(w) => text(&w.text, Family::PowerShell),
            }
        }
        "start-process" => {
            let mut acc = DangerClass::Safe;
            if ps_value(args, "verb", 2).is_some_and(|v| v.text.eq_ignore_ascii_case("runas")) {
                acc = d(C::PrivilegeEscalation, "Start-Process -Verb RunAs (elevation)");
            }
            let pos = ps_positionals(
                args,
                &[
                    "verb",
                    "workingdirectory",
                    "windowstyle",
                    "credential",
                    "redirectstandardoutput",
                    "redirectstandarderror",
                    "redirectstandardinput",
                ],
            );
            let named_file = ps_value(args, "filepath", 2);
            let file = named_file.clone().or_else(|| pos.first().map(|w| (*w).clone()));
            // `-ArgumentList '-c','Remove-Item …'` (an array literal, possibly
            // split over several words) or positional arguments after the file.
            let arglist = ps_array_value(args, &["argumentlist", "args"]).or_else(|| {
                let skip = usize::from(named_file.is_none());
                let rest: Vec<String> = pos.iter().skip(skip).map(|w| w.text.clone()).collect();
                (!rest.is_empty()).then(|| rest.join(" "))
            });
            if let Some(file) = file {
                if file.dynamic && file.text.contains(['$', '(']) {
                    acc = acc.max(d(C::Obfuscation, "Start-Process with a program only known at runtime"));
                }
                let mut t = file.text.clone();
                if let Some(a) = arglist {
                    t.push(' ');
                    t.push_str(&a.replace(',', " "));
                }
                acc = acc.max(text(&t, Family::PowerShell));
            }
            acc
        }
        "start" if family == Family::Cmd => {
            let mut i = 0;
            if args.first().is_some_and(|w| w.quoted) {
                i = 1; // window title
            }
            while i < args.len() && args[i].text.starts_with('/') {
                i += if args[i].text.eq_ignore_ascii_case("/d") { 2 } else { 1 };
            }
            unwrap(i, Family::Cmd)
        }
        "npx" | "bunx" | "pnpx" => unwrap(skip_opts(args, &["-p", "--package", "-c", "--call"], 0), family),
        "pnpm" | "yarn" | "bun" | "npm" if matches!(sub.as_str(), "dlx" | "exec" | "x") => {
            let i = args.iter().position(|a| a.text.eq_ignore_ascii_case(&sub)).map_or(args.len(), |p| p + 1);
            let i = i + skip_opts(&args[i.min(args.len())..], &["-p", "--package", "-c"], 0);
            unwrap(i, family)
        }
        "uv" | "poetry" | "pipenv" | "pdm" | "hatch" | "rye" if sub == "run" => {
            let i = args.iter().position(|a| a.text == "run").map_or(args.len(), |p| p + 1);
            unwrap(i, family)
        }
        "bundle" if sub == "exec" => unwrap(1, family),
        "eval" => {
            let joined = join(args);
            if raw().downloader.is_match(&joined) {
                rce("eval of downloaded content")
            } else if args.iter().any(|a| a.dynamic) {
                d(C::Obfuscation, "eval of dynamic input")
            } else {
                text(&joined, family)
            }
        }
        "." if family == Family::PowerShell && args.first().is_some_and(|a| a.dynamic) => {
            d(C::Obfuscation, "dot-sources a command or script held in a variable")
        }
        "source" | "." => {
            if args.iter().any(|a| a.dynamic && raw().downloader.is_match(&a.text)) || piped_rce {
                rce("sources downloaded content")
            } else {
                DangerClass::Safe
            }
        }
        "python" | "python3" | "python2" | "py" | "node" | "nodejs" | "deno" | "perl" | "ruby" | "php" | "lua" => {
            inline_code_rules(ctx, prog, args, cmd, upstream, depth)
        }

        "awk" | "gawk" | "mawk" | "nawk" => {
            let joined = join(args);
            if raw().awk_exec.is_match(&joined) {
                let mut acc = d(C::Obfuscation, "awk program runs shell commands");
                for lit in facets::string_literals(&joined) {
                    acc = acc.max(text(&lit, Family::Posix));
                }
                acc
            } else {
                DangerClass::Safe
            }
        }
        "vim" | "vi" | "nvim" | "gvim" | "ex" | "view" | "vimdiff" => {
            let mut acc = DangerClass::Safe;
            let mut i = 0;
            while i < args.len() {
                let t = args[i].text.as_str();
                let cmd_text = if t == "-c" || t == "--cmd" {
                    i += 1;
                    args.get(i).map(|w| w.text.as_str())
                } else {
                    t.strip_prefix('+')
                };
                if let Some(c) = cmd_text {
                    let lc = c.to_lowercase();
                    if let Some(pos) = c.find('!') {
                        acc = acc.max(text(&c[pos + 1..], Family::Posix));
                        acc = acc.max(d(C::Obfuscation, "editor command runs a shell command"));
                    } else if lc.contains("system(") || lc.contains("terminal") || lc.contains("source") {
                        acc = acc.max(d(C::Obfuscation, "editor command runs a shell command"));
                    }
                }
                i += 1;
            }
            acc
        }
        "if" if family == Family::Cmd => {
            // if [/i] [not] (exist X | defined X | errorlevel N | cmdextversion N | a==b | a op b) command
            let mut i = 0;
            while i < args.len() && matches!(lower[i].as_str(), "/i" | "not") {
                i += 1;
            }
            match lower.get(i).map(String::as_str) {
                Some("exist" | "defined" | "errorlevel" | "cmdextversion") => i += 2,
                Some(c) if c.contains("==") => i += 1,
                Some(_)
                    if lower
                        .get(i + 1)
                        .is_some_and(|op| matches!(op.as_str(), "equ" | "neq" | "lss" | "leq" | "gtr" | "geq")) =>
                {
                    i += 3;
                }
                _ => i += 1,
            }
            unwrap(i, family)
        }
        "do" | "else" if family == Family::Cmd => unwrap(0, family),
        "forfiles" => {
            let mut acc = DangerClass::Safe;
            for (i, a) in lower.iter().enumerate() {
                if a == "/c" || a == "-c" {
                    if let Some(c) = args.get(i + 1) {
                        acc = acc.max(text(&c.text.replace("0x22", "\""), Family::Cmd));
                        let l = c.text.to_lowercase();
                        if [" del ", " erase ", " rd ", " rmdir ", "remove-item", " rm "]
                            .iter()
                            .any(|k| format!(" {l} ").contains(k))
                        {
                            acc = acc.max(d(C::RecursiveDelete, "forfiles mass delete"));
                        }
                    }
                }
            }
            acc
        }
        "wmic" if has("process") && has("create") => {
            let inner = args.last().map(|w| text(&w.text, Family::Cmd)).unwrap_or(DangerClass::Safe);
            inner.max(d(C::Obfuscation, "wmic process call create starts an arbitrary process"))
        }
        "rimraf" | "rmrf" | "del-cli" | "trash" | "trash-put" | "rm-cli" | "premove" => {
            let mut acc = d(C::RecursiveDelete, format!("recursive delete ({prog})"));
            for t in args.iter().filter(|a| !a.text.starts_with('-')) {
                acc = acc.max(check_delete_target(ctx, t, true, family)).max(check_write_path(ctx, &t.text, true));
            }
            acc
        }

        // ---------------- deletion ----------------
        "rm" => {
            let recursive = posix_flag(args, &['r', 'R'], &["recursive"]);
            let mut acc = DangerClass::Safe;
            let ops = posix_operands(args);
            for t in &ops {
                acc = acc.max(check_delete_target(ctx, t, recursive, family)).max(check_write_path(ctx, &t.text, true));
            }
            if recursive && has("--no-preserve-root") {
                acc = acc.max(f(C::ProtectedPathDelete, "rm --no-preserve-root"));
            }
            if recursive {
                acc = acc.max(d(C::RecursiveDelete, "recursive delete (rm -r)"));
            } else if ops.iter().any(|t| t.glob || t.dynamic) {
                acc = acc.max(d(C::RecursiveDelete, "delete with a wildcard or variable target"));
            }
            acc
        }
        "rmdir" | "rd" if family == Family::Cmd || (had_ext && prog == "rd") => cmd_delete(ctx, args, true),
        "rmdir" | "unlink" => {
            let mut acc = DangerClass::Safe;
            for t in posix_operands(args) {
                acc = acc.max(check_delete_target(ctx, t, false, family)).max(check_write_path(ctx, &t.text, true));
            }
            acc
        }
        "del" | "erase" => cmd_delete(ctx, args, false),
        "shred" | "srm" | "wipe" => {
            let mut acc = d(C::DataDestruction, "securely destroys file contents");
            for t in posix_operands(args) {
                acc = acc.max(check_write_path(ctx, &t.text, true)).max(check_delete_target(ctx, t, true, family));
            }
            acc
        }
        "remove-item" => {
            let recursive = ps_has(args, "recurse", 1) || has_switch(args, "s");
            let mut acc = DangerClass::Safe;
            let mut targets: Vec<Word> =
                ps_positionals(args, &["filter", "include", "exclude", "credential", "stream"])
                    .into_iter()
                    .filter(|w| !is_switch_word(w))
                    .cloned()
                    .collect();
            for p in ["path", "literalpath", "lp", "pspath"] {
                if let Some(v) = ps_value(args, p, 2) {
                    targets.push(v);
                }
            }
            if !upstream.is_empty() {
                acc = acc.max(d(C::RecursiveDelete, "deletes every item coming from a pipeline"));
            }
            for t in &targets {
                if is_registry_path(&t.text) {
                    acc = acc.max(d(C::SystemConfig, "deletes registry keys"));
                    continue;
                }
                acc = acc.max(check_delete_target(ctx, t, recursive, family)).max(check_write_path(ctx, &t.text, true));
            }
            if recursive {
                acc = acc.max(d(C::RecursiveDelete, "recursive delete (Remove-Item -Recurse)"));
            } else if targets.iter().any(|t| t.glob || t.dynamic) {
                acc = acc.max(d(C::RecursiveDelete, "delete with a wildcard or variable target"));
            }
            acc
        }
        "robocopy" => {
            let mut acc = DangerClass::Safe;
            if has_switch(args, "mir") || has_switch(args, "purge") || has_switch(args, "move") {
                acc = d(C::RecursiveDelete, "robocopy /MIR or /PURGE deletes destination files");
            }
            if let Some(dst) = non_switch(args).get(1) {
                acc = acc.max(check_write_path(ctx, &dst.text, false));
            }
            acc
        }
        "rsync" => {
            let mut acc = DangerClass::Safe;
            if lower.iter().any(|a| a.starts_with("--delete") || a == "--remove-source-files") {
                acc = d(C::RecursiveDelete, "rsync --delete removes destination files");
            }
            if let Some(dst) = posix_operands(args).last() {
                acc = acc.max(check_write_path(ctx, &dst.text, false));
            }
            if let Some(v) = ssh_local_exec(&lower) {
                acc =
                    acc.max(text(&v, Family::Posix)).max(d(C::Obfuscation, "rsync remote shell runs a local command"));
            }
            acc
        }

        // ---------------- disks / boot / backups ----------------
        "format" => f(C::DiskDestruction, "formats a drive"),
        "format-volume" | "clear-disk" | "initialize-disk" | "remove-partition" | "new-partition" | "diskpart"
        | "wipefs" | "newfs" | "mke2fs" | "mkswap" => {
            f(C::DiskDestruction, format!("disk/volume destruction ({prog})"))
        }
        p if p.starts_with("mkfs") => f(C::DiskDestruction, "creates a filesystem (mkfs)"),
        "dd" => {
            let mut acc = DangerClass::Safe;
            for a in args {
                if let Some(of) = a.text.strip_prefix("of=") {
                    acc = acc.max(check_write_path(ctx, of, false));
                }
            }
            acc
        }
        "fdisk" | "sfdisk" | "parted" | "sgdisk" | "gdisk" | "cfdisk" => {
            if has("-l") || has("--list") || has("print") || has("-p") {
                DangerClass::Safe
            } else {
                f(C::DiskDestruction, format!("partition table edit ({prog})"))
            }
        }
        "diskutil" => {
            if matches!(
                sub.as_str(),
                "erasedisk" | "erasevolume" | "partitiondisk" | "zerodisk" | "randomdisk" | "secureerase" | "reformat"
            ) {
                f(C::DiskDestruction, "diskutil erase/partition")
            } else {
                DangerClass::Safe
            }
        }
        "fsutil" => {
            if has("setzerodata") {
                f(C::DiskDestruction, "fsutil setzerodata")
            } else if lower.iter().any(|a| a.starts_with("set") || a.starts_with("delete") || a.starts_with("create")) {
                d(C::SystemConfig, "fsutil write operation")
            } else {
                DangerClass::Safe
            }
        }
        "cipher" if has_switch(args, "w") => f(C::BackupDestruction, "cipher /w wipes free space"),
        "vssadmin" if has("delete") || has("resize") => f(C::BackupDestruction, "deletes volume shadow copies"),
        "wmic" if lower.iter().any(|a| a.contains("shadowcopy")) && has("delete") => {
            f(C::BackupDestruction, "deletes volume shadow copies")
        }
        "wbadmin" if has("delete") => f(C::BackupDestruction, "deletes Windows backups"),
        "bcdedit" | "bcdboot" | "grub-install" => {
            f(C::BackupDestruction, format!("modifies boot configuration ({prog})"))
        }
        "efibootmgr" if args.iter().any(|a| a.text == "-B" || a.text == "--delete-bootnum") => {
            f(C::BackupDestruction, "deletes an EFI boot entry")
        }
        "tmutil" if matches!(sub.as_str(), "delete" | "deletelocalsnapshots" | "disable") => {
            f(C::BackupDestruction, "deletes Time Machine backups")
        }
        "disable-computerrestore" => f(C::BackupDestruction, "disables System Restore"),
        "remove-wmiobject" | "remove-ciminstance" => {
            let ctx_text: String = upstream
                .iter()
                .flat_map(|c| c.words.iter().map(|w| w.text.to_lowercase()))
                .collect::<Vec<_>>()
                .join(" ");
            if ctx_text.contains("shadowcopy") || lower.iter().any(|a| a.contains("shadowcopy")) {
                f(C::BackupDestruction, "deletes volume shadow copies")
            } else {
                d(C::SystemConfig, "removes WMI/CIM objects")
            }
        }

        // ---------------- security tooling / logs ----------------
        "setenforce" if has("0") || has("permissive") => f(C::SecurityToolingDisable, "disables SELinux enforcement"),
        "ufw" => match sub.as_str() {
            "disable" => f(C::SecurityToolingDisable, "disables the firewall (ufw)"),
            "status" | "show" | "app" | "version" | "" => DangerClass::Safe,
            _ => d(C::NetworkConfig, "changes firewall rules (ufw)"),
        },
        "systemctl" | "service" => systemctl_rules(prog, &lower),
        "spctl" if lower.iter().any(|a| a.contains("disable")) => f(C::SecurityToolingDisable, "disables Gatekeeper"),
        "csrutil" if has("disable") => f(C::SecurityToolingDisable, "disables System Integrity Protection"),
        "set-mppreference" => {
            if args.iter().any(|a| ps_param_name(&a.text).is_some_and(|(n, _)| n.starts_with("disable"))) {
                f(C::SecurityToolingDisable, "disables Microsoft Defender protection")
            } else {
                d(C::SystemConfig, "changes Microsoft Defender settings")
            }
        }
        "add-mppreference"
            if args.iter().any(|a| ps_param_name(&a.text).is_some_and(|(n, _)| n.starts_with("exclusion"))) =>
        {
            f(C::SecurityToolingDisable, "adds a Microsoft Defender exclusion")
        }
        "set-netfirewallprofile" => {
            let off = ps_value(args, "enabled", 2).is_some_and(|v| {
                let v = v.text.to_lowercase();
                v.contains("false") || v == "0"
            });
            if off {
                f(C::SecurityToolingDisable, "disables the Windows firewall")
            } else {
                d(C::NetworkConfig, "changes Windows firewall profiles")
            }
        }
        "netsh" => {
            let joined = lower.join(" ");
            if (joined.contains("firewall"))
                && (joined.contains("state off")
                    || joined.contains("opmode disable")
                    || joined.contains("mode=disable"))
            {
                return f(C::SecurityToolingDisable, "disables the Windows firewall");
            }
            if joined.contains("portproxy") {
                if let Some(port) = ctx.guard.config.core_port {
                    if joined.contains(&format!("connectport={port}")) {
                        return f(C::PortExposure, "exposes the Orkestr core port via portproxy");
                    }
                }
            }
            if lower.iter().any(|a| {
                matches!(a.as_str(), "set" | "add" | "delete" | "del" | "reset" | "install" | "uninstall" | "import")
            }) {
                d(C::NetworkConfig, "changes network/firewall configuration (netsh)")
            } else {
                DangerClass::Safe
            }
        }
        "sc" if family == Family::Cmd || had_ext => {
            let verb = lower.first().cloned().unwrap_or_default();
            let svc = lower.get(1).cloned().unwrap_or_default();
            if matches!(verb.as_str(), "stop" | "config" | "delete" | "pause" | "failure" | "sdset")
                && is_security_service(&svc)
            {
                f(C::SecurityToolingDisable, format!("stops/reconfigures security service `{svc}`"))
            } else if matches!(
                verb.as_str(),
                "create" | "config" | "delete" | "stop" | "start" | "pause" | "failure" | "sdset"
            ) {
                d(C::SystemConfig, format!("changes Windows services (sc {verb})"))
            } else {
                DangerClass::Safe
            }
        }
        "stop-service" | "set-service" | "new-service" | "remove-service" | "restart-service" | "suspend-service" => {
            let name = ps_value(args, "name", 1)
                .map(|w| w.text.to_lowercase())
                .or_else(|| ps_positionals(args, &["startuptype", "status"]).first().map(|w| w.text.to_lowercase()))
                .unwrap_or_default();
            if is_security_service(&name) && prog != "new-service" {
                f(C::SecurityToolingDisable, format!("stops/reconfigures security service `{name}`"))
            } else {
                d(C::SystemConfig, format!("changes Windows services ({prog})"))
            }
        }
        "clear-eventlog" | "remove-eventlog" | "limit-eventlog" => f(C::LogWipe, "clears Windows event logs"),
        "wevtutil" => match sub.as_str() {
            "cl" | "clear-log" => f(C::LogWipe, "clears Windows event logs"),
            "um" | "uninstall-manifest" | "sl" | "set-log" => d(C::SystemConfig, "changes event log configuration"),
            _ => DangerClass::Safe,
        },
        "journalctl" if lower.iter().any(|a| a.starts_with("--vacuum")) => f(C::LogWipe, "vacuums the systemd journal"),
        "auditctl" if has("-d") || has("-e") && has("0") => f(C::LogWipe, "disables/clears audit rules"),
        "history" if has("-c") => d(C::LogWipe, "clears shell history"),

        // ---------------- credentials ----------------
        "security" => match sub.as_str() {
            "dump-keychain" => f(C::CredentialAccess, "dumps the macOS keychain"),
            "find-generic-password" | "find-internet-password" if has("-w") || has("-g") => {
                f(C::CredentialAccess, "reads passwords from the macOS keychain")
            }
            "add-generic-password"
            | "add-internet-password"
            | "delete-generic-password"
            | "delete-internet-password" => d(C::SecretChange, "changes macOS keychain secrets"),
            _ => DangerClass::Safe,
        },
        "cmdkey" => {
            if has_switch(args, "list") {
                f(C::CredentialAccess, "enumerates stored Windows credentials")
            } else if has_switch(args, "add") || has_switch(args, "delete") || has_switch(args, "generic") {
                d(C::SecretChange, "changes stored Windows credentials")
            } else {
                DangerClass::Safe
            }
        }

        // ---------------- tunnels / port exposure ----------------
        "ngrok" | "cloudflared" | "lt" | "localtunnel" | "bore" | "frpc" | "tailscale"
            if prog != "tailscale" || has("funnel") =>
        {
            port_exposure(ctx, &lower, "opens a public tunnel to a local port")
        }
        "ssh" | "autossh" | "scp" | "sftp" if ssh_local_exec(&lower).is_some() => {
            let value = ssh_local_exec(&lower).unwrap_or_default();
            text(&value, Family::Posix)
                .max(d(C::Obfuscation, "ssh option runs a local command (ProxyCommand/LocalCommand)"))
        }
        "ssh" | "autossh" if ssh_remote_command(args).is_some() => {
            let remote = ssh_remote_command(args).unwrap_or_default();
            let tunnel = args.iter().any(|a| {
                let t = a.text.as_str();
                t.starts_with("-R") || t.starts_with("-L") || t.starts_with("-D")
            });
            let inner = text(&remote, Family::Posix);
            if tunnel {
                inner.max(port_exposure(ctx, &lower, "creates an SSH tunnel / port forward"))
            } else {
                inner
            }
        }
        "ssh" | "autossh" => {
            if args.iter().any(|a| {
                let t = a.text.as_str();
                t.starts_with("-R")
                    || t.starts_with("-L")
                    || t.starts_with("-D")
                    || (t.starts_with('-')
                        && !t.starts_with("--")
                        && t.len() > 2
                        && t[1..].chars().all(|c| c.is_ascii_alphabetic())
                        && t[1..].contains(['R', 'L', 'D']))
            }) {
                port_exposure(ctx, &lower, "creates an SSH tunnel / port forward")
            } else {
                DangerClass::Safe
            }
        }
        "socat" if lower.iter().any(|a| a.contains("tcp-listen") || a.contains("tcp4-listen")) => {
            port_exposure(ctx, &lower, "listens on a TCP port and forwards traffic")
        }

        // ---------------- git ----------------
        "git" => git_exec_hooks(ctx, args, depth).max(git_rules(&texts(args))),

        // ---------------- SQL / data ----------------
        "psql" | "mysql" | "mariadb" | "sqlite3" | "sqlcmd" | "osql" | "isql" | "invoke-sqlcmd" | "mongosh"
        | "mongo" | "redis-cli" | "clickhouse-client" | "cockroach" | "duckdb" | "sqlplus" => {
            sql_client_rules(prog, args, cmd, upstream)
        }
        "dropdb" | "dropuser" | "mysqladmin" if prog != "mysqladmin" || has("drop") => {
            d(C::DataDestruction, format!("drops a database ({prog})"))
        }
        "prisma" if (has("migrate") && has("reset")) || has("--force-reset") || (has("db") && has("drop")) => {
            d(C::DataDestruction, "prisma reset drops all data")
        }
        "rails" | "rake"
            if lower.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "db:drop" | "db:reset" | "db:purge" | "db:schema:load" | "db:truncate_all" | "db:migrate:reset"
                )
            }) =>
        {
            d(C::DataDestruction, "rails database reset/drop")
        }
        "sequelize" | "sequelize-cli" if lower.iter().any(|a| a == "db:drop" || a == "db:migrate:undo:all") => {
            d(C::DataDestruction, "sequelize db:drop")
        }
        "typeorm" if has("schema:drop") => d(C::DataDestruction, "typeorm schema:drop"),
        "drizzle-kit" if has("drop") => d(C::DataDestruction, "drizzle-kit drop"),
        "flyway" if has("clean") => d(C::DataDestruction, "flyway clean drops all objects"),
        "diesel" if has("database") && has("reset") => d(C::DataDestruction, "diesel database reset"),
        "dotnet" => {
            if has("ef") && has("database") && has("drop") {
                d(C::DataDestruction, "dotnet ef database drop")
            } else if has("nuget") && (has("push") || has("delete")) {
                d(C::ProductionChange, "publishes/deletes a NuGet package")
            } else {
                DangerClass::Safe
            }
        }

        // ---------------- deploy / publish ----------------
        "kubectl" | "oc" | "k" | "kubecolor" => kubectl_rules(&lower),
        "helm" if matches!(sub.as_str(), "install" | "upgrade" | "uninstall" | "delete" | "del" | "rollback") => {
            d(C::ProductionChange, format!("helm {sub} changes a cluster"))
        }
        "terraform" | "tofu" | "terragrunt" => {
            let destructive =
                matches!(sub.as_str(), "apply" | "destroy" | "import" | "taint" | "untaint" | "force-unlock")
                    || (sub == "state"
                        && lower.iter().any(|a| matches!(a.as_str(), "rm" | "mv" | "push" | "replace-provider")))
                    || (sub == "workspace" && has("delete"))
                    || (sub == "run-all" && (has("apply") || has("destroy")));
            if destructive {
                d(C::ProductionChange, format!("terraform {sub} changes real infrastructure"))
            } else {
                DangerClass::Safe
            }
        }
        "pulumi"
            if matches!(sub.as_str(), "up" | "update" | "destroy" | "cancel" | "import")
                || (sub == "stack" && has("rm")) =>
        {
            d(C::ProductionChange, format!("pulumi {sub} changes real infrastructure"))
        }
        "vercel" | "vc" => {
            if has("env") && (has("add") || has("rm") || has("remove")) {
                d(C::SecretChange, "changes Vercel environment secrets")
            } else if has("--prod")
                || has("--production")
                || matches!(sub.as_str(), "rm" | "remove" | "promote" | "alias" | "rollback")
            {
                d(C::ProductionChange, "Vercel production deploy/change")
            } else {
                DangerClass::Safe
            }
        }
        "netlify" => {
            if lower.iter().any(|a| a.starts_with("env:set") || a.starts_with("env:unset")) {
                d(C::SecretChange, "changes Netlify environment secrets")
            } else if (sub == "deploy" && (has("--prod") || has("-p"))) || sub.starts_with("sites:delete") {
                d(C::ProductionChange, "Netlify production deploy/delete")
            } else {
                DangerClass::Safe
            }
        }
        "wrangler" => {
            if sub == "secret" {
                d(C::SecretChange, "changes Cloudflare Worker secrets")
            } else if matches!(sub.as_str(), "deploy" | "publish" | "delete" | "rollback") {
                d(C::ProductionChange, format!("wrangler {sub}"))
            } else {
                DangerClass::Safe
            }
        }
        "fly" | "flyctl" => {
            if sub == "secrets" && (has("set") || has("unset") || has("import")) {
                d(C::SecretChange, "changes Fly.io secrets")
            } else if matches!(sub.as_str(), "deploy" | "destroy" | "scale")
                || (sub == "apps" && (has("destroy") || has("delete")))
            {
                d(C::ProductionChange, format!("fly {sub}"))
            } else {
                DangerClass::Safe
            }
        }
        "firebase" if sub == "deploy" || lower.iter().any(|a| a.ends_with(":delete")) => {
            d(C::ProductionChange, "firebase deploy/delete")
        }
        "serverless" | "sls" if matches!(sub.as_str(), "deploy" | "remove") => {
            d(C::ProductionChange, format!("serverless {sub}"))
        }
        "eb" if matches!(sub.as_str(), "deploy" | "terminate" | "swap") => d(C::ProductionChange, format!("eb {sub}")),
        "heroku" => {
            if lower.iter().any(|a| a.starts_with("config:set") || a.starts_with("config:unset")) {
                d(C::SecretChange, "changes Heroku config vars")
            } else if lower.iter().any(|a| a.starts_with("pg:reset")) {
                d(C::DataDestruction, "heroku pg:reset")
            } else if lower
                .iter()
                .any(|a| a.contains("destroy") || a.starts_with("releases:rollback") || a.starts_with("ps:scale"))
            {
                d(C::ProductionChange, "Heroku production change")
            } else {
                DangerClass::Safe
            }
        }
        "aws" => aws_rules(&lower),
        "az" => {
            if lower.iter().any(|a| a == "secret") && (has("set") || has("delete")) {
                d(C::SecretChange, "changes Azure Key Vault secrets")
            } else if lower.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "delete"
                        | "purge"
                        | "create"
                        | "update"
                        | "set"
                        | "deploy"
                        | "restart"
                        | "stop"
                        | "remove"
                        | "swap"
                )
            }) {
                d(C::ProductionChange, "changes Azure resources")
            } else {
                DangerClass::Safe
            }
        }
        "gcloud" | "gsutil" => {
            if has("secrets") && (has("create") || has("add") || has("delete") || has("update")) {
                d(C::SecretChange, "changes GCP secrets")
            } else if lower.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "delete"
                        | "deploy"
                        | "create"
                        | "update"
                        | "rm"
                        | "set-iam-policy"
                        | "add-iam-policy-binding"
                        | "remove-iam-policy-binding"
                )
            }) {
                d(C::ProductionChange, "changes Google Cloud resources")
            } else {
                DangerClass::Safe
            }
        }
        "gh" => {
            let second = lower.iter().filter(|a| !a.starts_with('-')).nth(1).cloned().unwrap_or_default();
            match (sub.as_str(), second.as_str()) {
                ("secret", "set" | "delete" | "remove") | ("variable", "set" | "delete") => {
                    d(C::SecretChange, "changes GitHub secrets")
                }
                ("release", "create" | "delete" | "upload" | "edit") => {
                    d(C::ProductionChange, format!("gh release {second}"))
                }
                ("pr", "merge") => d(C::ProductionChange, "merges a pull request"),
                ("repo", "delete" | "archive" | "rename" | "edit" | "transfer") => {
                    d(C::ProductionChange, format!("gh repo {second}"))
                }
                ("workflow", "run") | ("run", "rerun") => d(C::ProductionChange, "triggers a CI/CD workflow"),
                _ => DangerClass::Safe,
            }
        }
        "twine" if sub == "upload" => d(C::ProductionChange, "publishes a Python package"),
        "publish-module" | "publish-script" => d(C::ProductionChange, "publishes a PowerShell module"),
        "gem" if matches!(sub.as_str(), "push" | "yank" | "owner") => d(C::ProductionChange, format!("gem {sub}")),
        "cargo" if matches!(sub.as_str(), "publish" | "yank" | "owner") => {
            d(C::ProductionChange, format!("cargo {sub}"))
        }
        "npm" | "pnpm" | "yarn" | "bun" => {
            if matches!(sub.as_str(), "publish" | "unpublish" | "deprecate" | "owner" | "dist-tag") {
                d(C::ProductionChange, format!("{prog} {sub} changes the public registry"))
            } else if (has("-g") || has("--global") || sub == "global")
                && matches!(sub.as_str(), "install" | "i" | "add" | "uninstall" | "remove" | "rm" | "global" | "update")
            {
                d(C::SystemConfig, "global package install/uninstall")
            } else {
                DangerClass::Safe
            }
        }

        // ---------------- containers ----------------
        "docker" | "podman" | "nerdctl" => docker_rules(&lower),
        "docker-compose" if has("down") && (has("-v") || has("--volumes")) => {
            d(C::ContainerDestruction, "removes containers and their volumes")
        }
        "vagrant" if sub == "destroy" => d(C::ContainerDestruction, "destroys a Vagrant VM"),
        "multipass" if matches!(sub.as_str(), "delete" | "purge") => {
            d(C::ContainerDestruction, "deletes Multipass VMs")
        }
        "minikube" | "kind" | "k3d" if sub == "delete" => d(C::ContainerDestruction, format!("{prog} delete cluster")),

        // ---------------- OS configuration ----------------
        "chmod" | "chown" | "chgrp" => {
            let recursive = posix_flag(args, &['R'], &["recursive"]);
            let ops = posix_operands(args);
            let mut acc = DangerClass::Safe;
            if recursive {
                for t in ops.iter().skip(1) {
                    if check_delete_target(ctx, t, true, family).is_forbidden() {
                        return f(C::SystemConfig, format!("recursive {prog} on a system/home root"));
                    }
                }
                acc = d(C::SystemConfig, format!("recursive permission/ownership change ({prog} -R)"));
            }
            if prog == "chmod"
                && ops
                    .first()
                    .is_some_and(|m| m.text.contains("777") || m.text.contains("a+rwx") || m.text.contains("o+w"))
            {
                acc = acc.max(d(C::SystemConfig, "makes files world-writable"));
            }
            for t in ops.iter().skip(1) {
                acc = acc.max(check_write_path(ctx, &t.text, false));
            }
            acc
        }
        "crontab" => {
            if has("-l") {
                DangerClass::Safe
            } else {
                d(C::SystemConfig, "changes scheduled jobs (crontab)")
            }
        }
        "launchctl"
            if matches!(
                sub.as_str(),
                "load" | "unload" | "bootstrap" | "bootout" | "remove" | "enable" | "disable" | "submit"
            ) =>
        {
            d(C::SystemConfig, "changes launchd services")
        }
        "defaults" if matches!(sub.as_str(), "write" | "delete") => d(C::SystemConfig, "changes macOS defaults"),
        "shutdown" | "reboot" | "poweroff" | "halt" | "restart-computer" | "stop-computer" | "logoff" => {
            d(C::SystemConfig, format!("shuts down / restarts the machine ({prog})"))
        }
        "init" | "telinit" if has("0") || has("6") => d(C::SystemConfig, "changes the runlevel (shutdown/reboot)"),
        "kill" => {
            if (lower.iter().any(|a| a == "-9" || a == "-kill" || a == "-sigkill") && has("1")) || has("-1") {
                d(C::SystemConfig, "kills init or all processes")
            } else {
                DangerClass::Safe
            }
        }
        "killall" | "pkill" | "taskkill" | "stop-process" | "tskill" | "skill" => {
            if lower.iter().any(|a| a.contains("orkestr")) {
                f(C::SelfTamper, "kills the Orkestr core")
            } else {
                d(C::SystemConfig, format!("terminates processes by name or force ({prog})"))
            }
        }
        "apt" | "apt-get" | "aptitude" | "dnf" | "yum" | "zypper" | "apk" | "emerge" | "snap" | "flatpak" | "port"
        | "pkg" => {
            if lower.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "install"
                        | "remove"
                        | "purge"
                        | "autoremove"
                        | "upgrade"
                        | "dist-upgrade"
                        | "add"
                        | "del"
                        | "erase"
                        | "reinstall"
                )
            }) {
                d(C::SystemConfig, format!("system package change ({prog})"))
            } else {
                DangerClass::Safe
            }
        }
        "pacman"
            if lower.iter().any(|a| a.starts_with("-s") || a.starts_with("-r") || a.starts_with("-u"))
                && !has("-ss")
                && !has("-si") =>
        {
            d(C::SystemConfig, "system package change (pacman)")
        }
        "brew" | "winget" | "choco" | "scoop"
            if matches!(sub.as_str(), "install" | "uninstall" | "remove" | "upgrade" | "tap" | "link" | "rm") =>
        {
            d(C::SystemConfig, format!("system package change ({prog} {sub})"))
        }
        "reg" => {
            let verb = sub.as_str();
            if matches!(verb, "add" | "delete" | "import" | "load" | "unload" | "restore" | "copy") {
                let machine = lower.iter().any(|a| a.starts_with("hklm") || a.starts_with("hkey_local_machine"));
                d(
                    C::SystemConfig,
                    format!("modifies the {} registry (reg {verb})", if machine { "machine-wide" } else { "Windows" }),
                )
            } else {
                DangerClass::Safe
            }
        }
        "regedit" | "regedt32" | "regini" if prog == "regini" || has_switch(args, "s") => {
            d(C::SystemConfig, "imports registry changes")
        }
        "set-itemproperty"
        | "new-itemproperty"
        | "remove-itemproperty"
        | "rename-itemproperty"
        | "copy-itemproperty"
        | "move-itemproperty"
        | "clear-itemproperty"
        | "new-item"
        | "set-item"
        | "clear-item" => {
            let paths: Vec<Word> =
                ps_positionals(args, &["name", "value", "propertytype", "type", "itemtype", "credential"])
                    .into_iter()
                    .cloned()
                    .chain(["path", "literalpath"].iter().filter_map(|p| ps_value(args, p, 2)))
                    .collect();
            let mut acc = DangerClass::Safe;
            for p in &paths {
                if is_registry_path(&p.text) {
                    acc = acc.max(d(C::SystemConfig, format!("modifies the registry ({prog})")));
                } else if prog.starts_with("new-item") || prog == "set-item" {
                    acc = acc.max(check_write_path(ctx, &p.text, false));
                }
            }
            if paths.is_empty() && prog.contains("itemproperty") {
                acc = acc.max(d(C::SystemConfig, format!("modifies item properties ({prog})")));
            }
            acc
        }
        "set-executionpolicy" => d(C::SystemConfig, "changes the PowerShell execution policy"),
        "register-scheduledtask" | "unregister-scheduledtask" | "set-scheduledtask" | "disable-scheduledtask" => {
            d(C::SystemConfig, "changes scheduled tasks")
        }
        "schtasks" if has_switch(args, "create") || has_switch(args, "delete") || has_switch(args, "change") => {
            d(C::SystemConfig, "changes scheduled tasks (schtasks)")
        }
        "icacls" | "cacls" | "xcacls" => {
            if lower.iter().any(|a| {
                a.starts_with("/grant")
                    || a.starts_with("/deny")
                    || a == "/reset"
                    || a == "/setowner"
                    || a.starts_with("/remove")
                    || a.starts_with("/inheritance")
                    || a == "/e"
                    || a == "/g"
            }) {
                d(C::SystemConfig, "changes file ACLs")
            } else {
                DangerClass::Safe
            }
        }
        "takeown" => d(C::SystemConfig, "takes ownership of files"),
        "attrib"
            if has_switch(args, "s")
                && lower.iter().any(|a| matches!(a.as_str(), "-s" | "-h" | "+s" | "+h" | "-r")) =>
        {
            d(C::SystemConfig, "recursively changes system/hidden attributes")
        }
        "setx" => {
            let name = lower.iter().find(|a| !a.starts_with('/')).cloned().unwrap_or_default();
            if raw().secret_name.is_match(&name) {
                d(C::SecretChange, "persists a secret in a user environment variable (setx)")
            } else {
                d(C::SystemConfig, "persists an environment variable (setx)")
            }
        }

        // ---------------- network configuration ----------------
        "route" if matches!(sub.as_str(), "add" | "delete" | "del" | "change" | "flush" | "-f") || has("-f") => {
            d(C::NetworkConfig, "changes the routing table")
        }
        "ip" => {
            let obj = lower.iter().find(|a| !a.starts_with('-')).cloned().unwrap_or_default();
            if matches!(
                obj.as_str(),
                "route"
                    | "r"
                    | "ro"
                    | "addr"
                    | "a"
                    | "address"
                    | "link"
                    | "l"
                    | "rule"
                    | "neigh"
                    | "tunnel"
                    | "netns"
                    | "xfrm"
            ) && lower.iter().any(|a| {
                matches!(a.as_str(), "add" | "del" | "delete" | "set" | "flush" | "change" | "replace" | "append")
            }) {
                d(C::NetworkConfig, "changes network configuration (ip)")
            } else {
                DangerClass::Safe
            }
        }
        "iptables" | "ip6tables" | "nft" | "firewall-cmd" | "pfctl" | "ebtables" | "arptables" | "iptables-restore" => {
            let listing = lower.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "-l" | "--list" | "--list-all" | "list" | "-nl" | "-vl" | "-nvl" | "--state" | "-sr" | "-s"
                )
            }) && !lower.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "-a" | "-d"
                        | "-i"
                        | "-f"
                        | "-x"
                        | "-p"
                        | "--add-rule"
                        | "--remove-rule"
                        | "add"
                        | "delete"
                        | "flush"
                        | "-e"
                )
            });
            if listing {
                DangerClass::Safe
            } else {
                d(C::NetworkConfig, format!("changes firewall rules ({prog})"))
            }
        }
        "ifconfig" if args.len() >= 2 => d(C::NetworkConfig, "changes network interfaces"),
        "networksetup"
            if lower.iter().any(|a| {
                a.starts_with("-set") || a.starts_with("-add") || a.starts_with("-remove") || a.starts_with("-delete")
            }) =>
        {
            d(C::NetworkConfig, "changes macOS network settings")
        }
        "new-netfirewallrule"
        | "set-netfirewallrule"
        | "remove-netfirewallrule"
        | "disable-netfirewallrule"
        | "enable-netfirewallrule"
        | "new-netipaddress"
        | "remove-netipaddress"
        | "set-netipaddress"
        | "set-dnsclientserveraddress"
        | "new-netroute"
        | "remove-netroute"
        | "set-netroute"
        | "set-netipinterface"
        | "disable-netadapter"
        | "enable-netadapter"
        | "set-netadapter"
        | "set-netconnectionprofile" => d(C::NetworkConfig, format!("changes network configuration ({prog})")),

        // ---------------- download & execute ----------------
        "certutil" if lower.iter().any(|a| a == "-urlcache" || a == "/urlcache" || a == "-verifyctl") => {
            rce("certutil -urlcache downloads files (LOLBin)")
        }
        "certutil" if lower.iter().any(|a| a == "-decode" || a == "-decodehex") => {
            d(C::Obfuscation, "certutil decodes an embedded payload")
        }
        "bitsadmin" if has_switch(args, "transfer") || has_switch(args, "addfile") => rce("bitsadmin downloads files"),
        "mshta" => rce("mshta executes HTML applications"),
        "regsvr32" if lower.iter().any(|a| a.contains("http") || a.contains("scrobj")) => {
            rce("regsvr32 remote scriptlet")
        }
        "rundll32" if lower.iter().any(|a| a.contains("http") || a.contains("javascript")) => {
            rce("rundll32 remote/script execution")
        }
        "add-type" if args.iter().any(|a| a.dynamic && raw().downloader.is_match(&a.text)) => {
            rce("Add-Type compiles downloaded code")
        }
        "curl" | "wget" | "invoke-webrequest" | "invoke-restmethod" => {
            let mut acc = DangerClass::Safe;
            for name in ["-o", "--output", "-outfile", "-O", "--output-document"] {
                if let Some(pos) = args.iter().position(|a| a.text.eq_ignore_ascii_case(name)) {
                    if let Some(t) = args.get(pos + 1) {
                        acc = acc.max(check_write_path(ctx, &t.text, false));
                    }
                }
            }
            acc
        }

        // ---------------- file writes (target checks) ----------------
        "cp" | "install" | "copy" | "xcopy" | "copy-item" | "ln" | "mklink" => {
            let mut acc = DangerClass::Safe;
            let dest = if prog == "copy-item" {
                ps_value(args, "destination", 1).or_else(|| {
                    ps_positionals(args, &["filter", "include", "exclude", "credential"]).get(1).map(|w| (*w).clone())
                })
            } else if family == Family::Cmd || prog == "xcopy" || prog == "copy" || prog == "mklink" {
                non_switch(args).get(if prog == "mklink" { 0 } else { 1 }).map(|w| (*w).clone())
            } else {
                posix_operands(args).last().map(|w| (*w).clone())
            };
            if let Some(dst) = dest {
                acc = acc.max(check_write_path(ctx, &dst.text, false));
            }
            if prog == "xcopy" && has_switch(args, "s") {
                acc = acc.max(DangerClass::Safe);
            }
            acc
        }
        "mv" | "move" | "move-item" | "rename-item" | "ren" | "rename" => {
            let mut acc = DangerClass::Safe;
            let targets: Vec<&Word> = if prog.ends_with("-item") {
                ps_positionals(args, &["filter", "include", "exclude", "credential"])
            } else if family == Family::Cmd {
                non_switch(args)
            } else {
                posix_operands(args)
            };
            for t in targets {
                acc = acc.max(check_write_path(ctx, &t.text, true));
            }
            if let Some(dst) = ps_value(args, "destination", 1).or_else(|| ps_value(args, "newname", 1)) {
                acc = acc.max(check_write_path(ctx, &dst.text, false));
            }
            acc
        }
        "tee" | "touch" | "mkdir" | "truncate" | "tee-object" => {
            let mut acc = DangerClass::Safe;
            let ops: Vec<Word> = if prog == "tee-object" {
                ps_value(args, "filepath", 1)
                    .into_iter()
                    .chain(ps_positionals(args, &["variable", "encoding"]).into_iter().cloned())
                    .collect()
            } else {
                posix_operands(args).into_iter().cloned().collect()
            };
            for t in ops {
                acc = acc.max(check_write_path(ctx, &t.text, false));
            }
            acc
        }
        "sed" if posix_flag(args, &['i'], &["in-place"]) => {
            let mut acc = DangerClass::Safe;
            if let Some(t) = posix_operands(args).last() {
                acc = acc.max(check_write_path(ctx, &t.text, false));
            }
            acc
        }
        "set-content" | "add-content" | "out-file" | "clear-content" | "export-csv" | "export-clixml" => {
            let mut acc = DangerClass::Safe;
            let mut targets: Vec<Word> = Vec::new();
            for p in ["path", "filepath", "literalpath"] {
                if let Some(v) = ps_value(args, p, 2) {
                    targets.push(v);
                }
            }
            if targets.is_empty() {
                if let Some(first) = ps_positionals(args, &["value", "encoding", "inputobject", "width"]).first() {
                    targets.push((*first).clone());
                }
            }
            for t in &targets {
                acc = acc.max(check_write_path(ctx, &t.text, false));
            }
            acc
        }
        _ => {
            let _ = had_ext;
            DangerClass::Safe
        }
    }
}

fn decode_ps_base64(s: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(s.trim().trim_matches(['"', '\''])).ok()?;
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16(&units).ok()
}

fn is_registry_path(p: &str) -> bool {
    let l = p.trim_matches(['"', '\'']).to_lowercase();
    ["hklm:", "hkcu:", "hkcr:", "hku:", "hkcc:", "registry::", "hkey_", "microsoft.powershell.core\\registry::"]
        .iter()
        .any(|x| l.starts_with(x))
}

fn is_security_service(name: &str) -> bool {
    let n = name.trim_matches(['"', '\'']).to_lowercase();
    [
        "windefend",
        "mpssvc",
        "wscsvc",
        "sense",
        "wdnissvc",
        "securityhealthservice",
        "bfe",
        "eventlog",
        "firewalld",
        "apparmor",
        "ufw",
        "auditd",
        "iptables",
        "nftables",
        "selinux",
        "clamav",
        "clamd",
        "falcon-sensor",
        "osqueryd",
        "wazuh-agent",
    ]
    .iter()
    .any(|s| n == *s || n.starts_with(&format!("{s}.")))
}

fn port_exposure(ctx: &Ctx<'_>, lower: &[String], reason: &str) -> DangerClass {
    if let Some(port) = ctx.guard.config.core_port {
        let p = port.to_string();
        if lower.iter().any(|a| {
            a == &p
                || a.ends_with(&format!(":{p}"))
                || a.contains(&format!(":{p}:"))
                || a.contains(&format!(",{p}"))
                || a.contains(&format!("tcp:127.0.0.1:{p}"))
        }) {
            return f(C::PortExposure, "exposes the Orkestr core port");
        }
    }
    d(C::NetworkConfig, reason.to_owned())
}

fn systemctl_rules(prog: &str, lower: &[String]) -> DangerClass {
    let non_flags: Vec<&String> = lower.iter().filter(|a| !a.starts_with('-')).collect();
    let (verb, units): (String, Vec<&String>) = if prog == "service" {
        (
            non_flags.get(1).map(|s| (*s).clone()).unwrap_or_default(),
            non_flags.first().map(|u| vec![*u]).unwrap_or_default(),
        )
    } else {
        (non_flags.first().map(|s| (*s).clone()).unwrap_or_default(), non_flags.iter().skip(1).copied().collect())
    };
    let stops = matches!(verb.as_str(), "stop" | "disable" | "mask" | "kill");
    if stops && units.iter().any(|u| is_security_service(u)) {
        return f(C::SecurityToolingDisable, "stops/disables a security service");
    }
    if matches!(
        verb.as_str(),
        "enable"
            | "disable"
            | "stop"
            | "mask"
            | "unmask"
            | "restart"
            | "start"
            | "kill"
            | "isolate"
            | "poweroff"
            | "reboot"
            | "halt"
            | "set-default"
            | "edit"
            | "link"
            | "preset"
            | "reload"
            | "try-restart"
            | "force-reload"
    ) {
        return d(C::SystemConfig, format!("changes system services ({prog} {verb})"));
    }
    DangerClass::Safe
}

/// Receives the original-case arguments (git is case-sensitive: `-D` ≠ `-d`).
fn git_rules(lower: &[String]) -> DangerClass {
    let mut i = 0;
    while i < lower.len() {
        let a = lower[i].as_str();
        if !a.starts_with('-') {
            break;
        }
        i += if matches!(
            a,
            "-c" | "-C"
                | "--git-dir"
                | "--work-tree"
                | "--namespace"
                | "--super-prefix"
                | "--config-env"
                | "--exec-path"
        ) {
            2
        } else {
            1
        };
    }
    let Some(sub) = lower.get(i) else { return DangerClass::Safe };
    let rest = &lower[i + 1..];
    let has = |w: &str| rest.iter().any(|a| a == w);
    let short =
        |c: char| rest.iter().any(|a| a.starts_with('-') && !a.starts_with("--") && a.len() > 1 && a[1..].contains(c));
    let operands: Vec<&String> = rest.iter().filter(|a| !a.starts_with('-')).collect();
    let g = |r: &str| d(C::GitDestructive, r.to_owned());
    match sub.as_str() {
        "push" => {
            let destructive = rest.iter().any(|a| {
                a == "--force"
                    || a.starts_with("--force-with-lease")
                    || a == "--force-if-includes"
                    || a == "--mirror"
                    || a == "--delete"
                    || a == "--prune"
            }) || short('f')
                || short('d')
                || operands.iter().skip(1).any(|r| r.starts_with('+') || r.starts_with(':'));
            if destructive {
                return g("force/destructive git push (rewrites or deletes remote history)");
            }
            let protected = operands.iter().skip(1).any(|r| {
                let dst = r.rsplit(':').next().unwrap_or(r).trim_start_matches("refs/heads/");
                matches!(dst, "main" | "master" | "trunk" | "production" | "prod") || dst.starts_with("release/")
            });
            if protected {
                return g("pushes directly to a protected branch");
            }
            DangerClass::Safe
        }
        "reset" if has("--hard") => g("git reset --hard discards uncommitted work"),
        "clean" if has("--force") || short('f') => g("git clean -f deletes untracked files"),
        "checkout" => {
            if has("-f") || has("--force") {
                return g("git checkout --force discards local changes");
            }
            if let Some(pos) = rest.iter().position(|a| a == "--") {
                if pos + 1 < rest.len() {
                    return g("git checkout -- <paths> discards local changes");
                }
            }
            if operands.iter().any(|o| o.as_str() == "." || o.as_str() == "*") {
                return g("git checkout . discards local changes");
            }
            DangerClass::Safe
        }
        "restore" => {
            let staged_only = (has("--staged") || has("-s") || short('S')) && !(has("--worktree") || short('W'));
            if !staged_only && !operands.is_empty() {
                g("git restore discards working-tree changes")
            } else {
                DangerClass::Safe
            }
        }
        "switch" if has("--discard-changes") || has("-f") || has("--force") => g("git switch discards local changes"),
        "branch" => {
            let force_delete = rest.iter().any(|a| {
                a.starts_with('-')
                    && !a.starts_with("--")
                    && (a.contains('D') || a.contains('M') || a.contains('C') || (a.contains('d') && a.contains('f')))
            }) || ((has("--delete") || has("-d")) && (has("--force") || has("-f")));
            if force_delete {
                g("force-deletes/overwrites a branch")
            } else {
                DangerClass::Safe
            }
        }
        "tag" if has("-d") || has("--delete") || has("-f") || has("--force") => g("deletes/moves a git tag"),
        "filter-branch" | "filter-repo" => g("rewrites git history"),
        "update-ref" if has("-d") => g("deletes a git ref"),
        "reflog" if operands.first().is_some_and(|o| o.as_str() == "expire" || o.as_str() == "delete") => {
            g("expires/deletes reflog entries")
        }
        "gc" if rest.iter().any(|a| a.starts_with("--prune=now") || a.starts_with("--prune=all")) => {
            g("git gc --prune=now drops unreachable objects")
        }
        "prune" => g("git prune drops unreachable objects"),
        "stash" if operands.first().is_some_and(|o| o.as_str() == "drop" || o.as_str() == "clear") => {
            g("drops stashed work")
        }
        "worktree" if operands.first().is_some_and(|o| o.as_str() == "remove") && (has("-f") || has("--force")) => {
            g("force-removes a worktree with changes")
        }
        "rm" if has("-r") || short('r') => d(C::RecursiveDelete, "git rm -r recursive delete"),
        "remote" if operands.first().is_some_and(|o| matches!(o.as_str(), "remove" | "rm" | "set-url")) => {
            g("changes git remotes")
        }
        "config" if has("--system") => d(C::SystemConfig, "changes system-wide git config"),
        _ => DangerClass::Safe,
    }
}

fn kubectl_rules(lower: &[String]) -> DangerClass {
    let mut i = 0;
    while i < lower.len() {
        let a = lower[i].as_str();
        if !a.starts_with('-') {
            break;
        }
        i += if matches!(
            a,
            "-n" | "--namespace" | "--context" | "--kubeconfig" | "--cluster" | "--user" | "-s" | "--server"
        ) {
            2
        } else {
            1
        };
    }
    let verb = lower.get(i).cloned().unwrap_or_default();
    if verb == "create" && lower.get(i + 1).is_some_and(|n| n == "secret") {
        return d(C::SecretChange, "creates a Kubernetes secret");
    }
    if matches!(
        verb.as_str(),
        "apply"
            | "delete"
            | "scale"
            | "rollout"
            | "patch"
            | "replace"
            | "drain"
            | "cordon"
            | "uncordon"
            | "edit"
            | "set"
            | "create"
            | "annotate"
            | "label"
            | "taint"
            | "exec"
            | "cp"
            | "run"
            | "expose"
            | "autoscale"
    ) {
        return d(C::ProductionChange, format!("kubectl {verb} changes a cluster"));
    }
    DangerClass::Safe
}

fn aws_rules(lower: &[String]) -> DangerClass {
    let non_flags: Vec<&String> = lower.iter().filter(|a| !a.starts_with('-')).collect();
    let service = non_flags.first().map(|s| s.as_str()).unwrap_or_default();
    let op = non_flags.get(1).map(|s| s.as_str()).unwrap_or_default();
    if (service == "secretsmanager"
        && (op.starts_with("put-")
            || op.starts_with("create-")
            || op.starts_with("delete-")
            || op.starts_with("update-")))
        || (service == "ssm" && (op == "put-parameter" || op == "delete-parameter"))
    {
        return d(C::SecretChange, "changes AWS secrets/parameters");
    }
    if service == "s3" {
        let dests_s3 = non_flags.last().is_some_and(|l| l.starts_with("s3://"));
        if matches!(op, "rm" | "rb" | "mv") || (op == "sync" && lower.iter().any(|a| a == "--delete")) {
            return d(C::ProductionChange, format!("aws s3 {op} deletes/moves objects"));
        }
        if matches!(op, "cp" | "sync") && dests_s3 {
            return d(C::ProductionChange, "uploads to S3");
        }
        return DangerClass::Safe;
    }
    let mutating = [
        "delete",
        "terminate",
        "put-",
        "remove",
        "create",
        "update",
        "modify",
        "stop",
        "reboot",
        "deregister",
        "detach",
        "revoke",
        "authorize",
        "attach",
        "run-instances",
        "deploy",
        "invoke",
    ];
    if mutating.iter().any(|m| op.starts_with(m) || op.contains(&format!("-{m}"))) {
        return d(C::ProductionChange, format!("aws {service} {op} changes cloud resources"));
    }
    DangerClass::Safe
}

fn docker_rules(lower: &[String]) -> DangerClass {
    let mut i = 0;
    while i < lower.len() {
        let a = lower[i].as_str();
        if !a.starts_with('-') {
            break;
        }
        i += if matches!(a, "-h" | "--host" | "-c" | "--context" | "--config" | "-l" | "--log-level") { 2 } else { 1 };
    }
    let rest = &lower[i.min(lower.len())..];
    let ops: Vec<&String> = rest.iter().filter(|a| !a.starts_with('-')).collect();
    let has = |w: &str| rest.iter().any(|a| a == w);
    let c = |r: &str| d(C::ContainerDestruction, r.to_owned());
    match (ops.first().map(|s| s.as_str()), ops.get(1).map(|s| s.as_str())) {
        (Some("system"), Some("prune")) => c("docker system prune removes containers, images and caches"),
        (Some("system"), Some("reset")) => c("podman system reset wipes all containers and images"),
        (Some("volume"), Some("rm" | "prune" | "remove")) => c("removes docker volumes (data)"),
        (Some("image"), Some("prune")) if has("-a") || has("--all") => c("removes all unused images"),
        (Some("container" | "network" | "builder"), Some("prune")) => c("prunes docker objects"),
        (Some("rm"), _) if has("-f") || has("--force") || has("-v") || has("--volumes") => {
            c("force-removes containers")
        }
        (Some("rmi"), _) if has("-f") || has("--force") => c("force-removes images"),
        (Some("compose"), Some("down")) if has("-v") || has("--volumes") || has("--rmi") => {
            c("removes compose services and volumes")
        }
        (Some("push"), _) => d(C::ProductionChange, "pushes an image to a registry"),
        (Some("run" | "create"), _)
            if has("--privileged")
                || rest
                    .iter()
                    .any(|a| a.starts_with("/:/") || a.starts_with("/var/run/docker.sock") || a == "--pid=host") =>
        {
            d(C::PrivilegeEscalation, "runs a privileged container / mounts the host root or docker socket")
        }
        _ => DangerClass::Safe,
    }
}

fn sql_client_rules(prog: &str, args: &[Word], cmd: &SimpleCommand, upstream: &[&SimpleCommand]) -> DangerClass {
    let mut acc = DangerClass::Safe;
    let mut sqls: Vec<String> = Vec::new();
    let value_flags: &[&str] = match prog {
        "psql" => &["-c", "--command"],
        "mysql" | "mariadb" => &["-e", "--execute"],
        "sqlcmd" | "osql" | "isql" => &["-q", "-Q"],
        "invoke-sqlcmd" => &["-query"],
        "mongosh" | "mongo" => &["--eval"],
        "clickhouse-client" => &["-q", "--query"],
        "cockroach" => &["-e", "--execute"],
        _ => &[],
    };
    let mut i = 0;
    while i < args.len() {
        let t = args[i].text.as_str();
        let tl = t.to_lowercase();
        if let Some(flag) = value_flags.iter().find(|fl| tl == fl.to_lowercase() || (t == **fl)) {
            let _ = flag;
            if let Some(v) = args.get(i + 1) {
                sqls.push(v.text.clone());
            }
            i += 2;
            continue;
        }
        if let Some((name, val)) = t.split_once('=') {
            if value_flags.iter().any(|fl| fl.eq_ignore_ascii_case(name)) {
                sqls.push(val.to_owned());
            }
        }
        i += 1;
    }
    if matches!(prog, "sqlite3" | "duckdb") {
        sqls.extend(posix_operands(args).into_iter().skip(1).map(|w| w.text.clone()));
    }
    if prog == "redis-cli" {
        sqls.push(join(args));
    }
    sqls.extend(cmd.heredocs.iter().cloned());
    // `echo "DROP TABLE x" | psql`
    if let Some(prev) = upstream.last() {
        if prev.words.first().is_some_and(|w| {
            matches!(normalize_program(&w.text).0.as_str(), "echo" | "printf" | "write-output" | "cat")
        }) {
            sqls.push(join(&prev.words[1..]));
            sqls.extend(prev.heredocs.iter().cloned());
        }
    }
    for s in sqls {
        acc = acc.max(classify_sql(&s));
    }
    acc
}

fn inline_code_rules(
    ctx: &Ctx<'_>,
    prog: &str,
    args: &[Word],
    cmd: &SimpleCommand,
    upstream: &[&SimpleCommand],
    depth: usize,
) -> DangerClass {
    let lower = lower_texts(args);
    // Django / Laravel destructive management commands.
    if lower.iter().any(|a| a.ends_with("manage.py"))
        && lower.iter().any(|a| matches!(a.as_str(), "flush" | "reset_db" | "sqlflush"))
    {
        return d(C::DataDestruction, "manage.py flush deletes all data");
    }
    if prog == "php"
        && lower.iter().any(|a| a == "artisan")
        && lower.iter().any(|a| {
            a.starts_with("migrate:fresh")
                || a.starts_with("migrate:reset")
                || a.starts_with("db:wipe")
                || a.starts_with("migrate:refresh")
        })
    {
        return d(C::DataDestruction, "artisan wipes the database");
    }
    if prog == "deno" && lower.first().is_some_and(|s| s == "run") && lower.iter().any(|a| a.starts_with("http")) {
        return rce("runs remote code (deno run https://…)");
    }
    let code_flags = ["-c", "-e", "--eval", "-r", "-p", "--print", "-E"];
    let mut code: Option<&Word> = None;
    for (i, a) in args.iter().enumerate() {
        if code_flags.contains(&a.text.as_str()) {
            code = args.get(i + 1);
            break;
        }
        if !a.text.starts_with('-') {
            break;
        }
    }
    let mut acc = DangerClass::Safe;
    for h in &cmd.heredocs {
        acc = acc.max(code_rules(ctx, prog, h, depth));
    }
    if let Some(code) = code {
        acc = acc.max(code_rules(ctx, prog, &code.text, depth));
        if code.dynamic {
            acc = acc.max(d(C::Obfuscation, "inline script built from variables"));
        }
    } else if args.iter().all(|a| a.text.starts_with('-')) && cmd.heredocs.is_empty() {
        // The interpreter reads its program from stdin.
        acc = acc.max(piped_script(ctx, upstream, Stdin::Code(prog), depth));
    }
    acc
}

/// Analyses inline interpreter code (`python -c`, `node -e`, piped or
/// here-doc programs).
fn code_rules(ctx: &Ctx<'_>, prog: &str, code: &str, depth: usize) -> DangerClass {
    let r = raw();
    let mut acc = DangerClass::Safe;
    if r.inline_net.is_match(code) {
        facets::mark_network(ctx);
        if r.inline_exec.is_match(code) {
            acc = acc.max(rce("inline script downloads and executes code"));
        }
    }
    if r.inline_delete.is_match(code) {
        acc = acc.max(d(C::RecursiveDelete, "inline script recursively deletes files"));
    }
    if r.credentials.is_match(code) {
        acc = acc.max(f(C::CredentialAccess, "inline script reads other applications' credentials"));
    }
    // Orkestr's data dir assembled inside the code (`'/orkestr/core.' + 'token'`).
    let lc = code.to_lowercase();
    for p in &ctx.guard.protected_lower {
        let name = p.rsplit('/').next().unwrap_or_default();
        if !name.is_empty()
            && lc.contains(name)
            && (lc.contains("core") || lc.contains("token") || lc.contains(".db") || lc.contains("vault"))
        {
            return f(C::SelfTamper, "inline script references Orkestr's private data directory");
        }
    }
    let lits = facets::string_literals(code);
    for l in &lits {
        acc = acc.max(check_protected_ref(ctx, l));
    }
    if acc.is_forbidden() {
        return acc;
    }
    // Paths named in the code count as reads (secrets / outside the worktree).
    let as_words: Vec<Word> = lits.iter().map(|l| Word::literal(l)).collect();
    facets::record(ctx, "cat", &as_words, Family::Posix);
    let backtick_exec = matches!(prog, "perl" | "ruby" | "php") && code.contains('`');
    if r.inline_shell.is_match(code) || backtick_exec {
        for l in &lits {
            acc = acc.max(classify_text(ctx, l, Family::Posix, depth + 1)).max(classify_text(
                ctx,
                l,
                Family::Cmd,
                depth + 1,
            ));
        }
        acc = acc.max(classify_text(ctx, &lits.join(" "), Family::Posix, depth + 1));
        acc = acc.max(d(C::Obfuscation, "inline script runs shell commands or evaluates generated code"));
    }
    acc
}

/// Where piped stdin goes.
#[derive(Clone, Copy)]
enum Stdin<'p> {
    /// A shell reading its script from stdin.
    Shell(Family),
    /// An interpreter (`python`, `node`, …) reading code from stdin.
    Code(&'p str),
}

/// Classifies script text that a pipeline feeds into a shell or interpreter
/// reading from stdin (`echo 'rm -rf ~' | sh`). Literal text is analysed in
/// the target's grammar; a literal script file (`cat x.sh | sh`) counts like
/// `sh x.sh`; anything else cannot be resolved and fails closed.
fn piped_script(ctx: &Ctx<'_>, upstream: &[&SimpleCommand], target: Stdin<'_>, depth: usize) -> DangerClass {
    let Some(prev) = upstream.last() else { return DangerClass::Safe };
    if upstream_has_downloader(upstream) {
        return rce("pipes downloaded content into a shell or interpreter");
    }
    let analyse = |t: &str| match target {
        Stdin::Shell(fam) => classify_text(ctx, t, fam, depth + 1),
        Stdin::Code(p) => code_rules(ctx, p, t, depth),
    };
    let mut acc = DangerClass::Safe;
    for h in &prev.heredocs {
        acc = acc.max(analyse(h));
    }
    let Some(pw) = prev.words.first() else { return acc };
    let (p, _) = normalize_program(&pw.text);
    let p = ps_alias(&p).to_owned();
    let args = &prev.words[1..];
    match p.as_str() {
        "echo" | "printf" | "write-output" | "write-host" | "print" | "echo." => {
            let body: Vec<&Word> = args
                .iter()
                .skip_while(|w| p == "echo" && matches!(w.text.as_str(), "-n" | "-e" | "-E" | "-ne" | "-en"))
                .collect();
            let joined = body.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
            acc = acc.max(analyse(&joined));
            if joined.contains("\\n") {
                acc = acc.max(analyse(&joined.replace("\\n", "\n")));
            }
            if pw.dynamic || body.iter().any(|w| w.dynamic) {
                acc = acc.max(d(C::Obfuscation, "pipes text built from variables into a shell or interpreter"));
            }
            acc
        }
        "get-content" | "cat" | "type" | "more" => {
            let ops: Vec<&Word> = args.iter().filter(|w| !w.text.starts_with('-') || w.quoted).collect();
            if !prev.heredocs.is_empty() || (!ops.is_empty() && ops.iter().all(|w| !w.dynamic)) {
                acc // a literal script file: like `sh file.sh`
            } else {
                acc.max(d(C::Obfuscation, "pipes unresolved input into a shell or interpreter"))
            }
        }
        _ => acc.max(d(C::Obfuscation, format!("pipes the output of `{p}` into a shell or interpreter"))),
    }
}

/// Value of a PowerShell array parameter (`-ArgumentList '-c','x'` /
/// `-ArgumentList '-c', 'x'`), with the items joined by spaces.
fn ps_array_value(args: &[Word], names: &[&str]) -> Option<String> {
    for (i, a) in args.iter().enumerate() {
        let Some((n, inline)) = ps_param_name(&a.text) else { continue };
        if !names.iter().any(|full| ps_matches(&n, full, 2)) {
            continue;
        }
        let mut items: Vec<String> = Vec::new();
        let mut j = i + 1;
        if let Some(v) = inline {
            items.push(v);
        } else if let Some(w) = args.get(j) {
            items.push(w.text.clone());
            j += 1;
        }
        // Continue over `, 'x'` continuations.
        while let Some(w) = args.get(j) {
            let continues = items.last().is_some_and(|l| l.ends_with(',')) || w.text.starts_with(',');
            if !continues {
                break;
            }
            items.push(w.text.clone());
            j += 1;
        }
        return Some(items.join(" ").replace(',', " "));
    }
    None
}

/// `ssh -o ProxyCommand=…` / `LocalCommand` / `KnownHostsCommand` /
/// `rsync -e …` / `--rsh=…` / `--rsync-path=…`: the local command text.
fn ssh_local_exec(lower: &[String]) -> Option<String> {
    for (i, a) in lower.iter().enumerate() {
        let a = a.as_str();
        let opt = if a == "-o" { lower.get(i + 1).map(String::as_str) } else { a.strip_prefix("-o") };
        if let Some(o) = opt {
            let o = o.trim_matches(['"', '\'']);
            for key in ["proxycommand", "localcommand", "knownhostscommand", "permitlocalcommand=yes"] {
                if let Some(rest) = o.strip_prefix(key) {
                    return Some(rest.trim_start_matches(['=', ' ']).to_owned());
                }
            }
        }
        if a == "-e" || a == "--rsh" {
            if let Some(v) = lower.get(i + 1) {
                let first = v.split_whitespace().next().unwrap_or_default();
                if !matches!(normalize_program(first).0.as_str(), "ssh" | "rsh" | "plink")
                    || v.contains(['|', ';', '&'])
                {
                    return Some(v.clone());
                }
            }
        }
        for prefix in ["--rsh=", "--rsync-path="] {
            if let Some(v) = a.strip_prefix(prefix) {
                let first = v.split_whitespace().next().unwrap_or_default();
                if !matches!(normalize_program(first).0.as_str(), "ssh" | "rsh" | "rsync" | "plink")
                    || v.contains(['|', ';', '&'])
                {
                    return Some(v.to_owned());
                }
            }
        }
    }
    None
}

/// The command `ssh [options] host command…` runs on the remote host.
fn ssh_remote_command(args: &[Word]) -> Option<String> {
    const VALUE: &[char] =
        &['b', 'c', 'D', 'E', 'e', 'F', 'I', 'i', 'J', 'L', 'l', 'm', 'O', 'o', 'p', 'Q', 'R', 'S', 'W', 'w', 'B'];
    let mut i = 0;
    while i < args.len() {
        let t = args[i].text.as_str();
        if t == "--" {
            i += 1;
            break;
        }
        if t.starts_with('-') && t.len() > 1 && !args[i].quoted {
            let cluster: Vec<char> = t[1..].chars().collect();
            // the value is attached (`-p22`) unless the option letter ends the word
            let last = *cluster.last().unwrap_or(&' ');
            let attached = cluster.iter().position(|c| VALUE.contains(c)).is_some_and(|p| p + 1 < cluster.len());
            i += if VALUE.contains(&last) && !attached { 2 } else { 1 };
            continue;
        }
        break;
    }
    // args[i] is the host; the rest is the remote command.
    let rest = args.get(i + 1..)?;
    if rest.is_empty() {
        return None;
    }
    Some(texts(rest).join(" "))
}

/// `git config` keys whose value is executed as a command (or that pull in
/// other config / hooks).
fn git_exec_key(key: &str, value: &str) -> bool {
    let k = key.trim().to_lowercase();
    [
        "core.sshcommand",
        "core.pager",
        "core.editor",
        "core.fsmonitor",
        "core.hookspath",
        "core.gitproxy",
        "core.askpass",
        "core.alternaterefscommand",
        "sequence.editor",
        "diff.external",
        "credential.helper",
        "gpg.",
        "pager.",
        "interactive.difffilter",
        "uploadpack.",
        "include.",
        "includeif.",
        "protocol.",
        "filter.",
    ]
    .iter()
    .any(|p| k.starts_with(p))
        || k.ends_with(".textconv")
        || k.ends_with(".command")
        || k.ends_with(".uploadpack")
        || k.ends_with(".receivepack")
        || k.ends_with(".helper")
        || (k.starts_with("merge.") && k.ends_with(".driver"))
        || (k.starts_with("alias.") && value.trim().starts_with('!'))
}

/// Git options and subcommands that execute arbitrary commands
/// (`-c core.sshCommand=…`, `git config core.hooksPath …`, `rebase --exec`,
/// `submodule foreach`, `bisect run`, `--upload-pack=…`).
fn git_exec_hooks(ctx: &Ctx<'_>, args: &[Word], depth: usize) -> DangerClass {
    let text = |t: &str| classify_text(ctx, t, Family::Posix, depth + 1);
    let hook = |what: &str| d(C::Obfuscation, format!("git {what} runs an arbitrary command"));
    let mut acc = DangerClass::Safe;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].text.as_str();
        if !a.starts_with('-') {
            break;
        }
        if a == "-c" || a == "--config-env" {
            if let Some(kv) = args.get(i + 1) {
                let (k, v) = kv.text.split_once('=').unwrap_or((kv.text.as_str(), ""));
                if git_exec_key(k, v) || (a == "--config-env" && git_exec_key(k, "!")) {
                    acc = acc.max(text(v.trim_start_matches('!'))).max(hook(&format!("-c {k}")));
                }
            }
            i += 2;
            continue;
        }
        if let Some(kv) = a.strip_prefix("--config-env=") {
            let k = kv.split_once('=').map_or(kv, |(k, _)| k);
            if git_exec_key(k, "!") {
                acc = acc.max(hook(&format!("--config-env {k}")));
            }
        }
        i += if matches!(a, "-C" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path") { 2 } else { 1 };
    }
    let Some(sub) = args.get(i).map(|w| w.text.to_lowercase()) else { return acc };
    let rest = &args[(i + 1).min(args.len())..];
    let lower: Vec<String> = rest.iter().map(|w| w.text.to_lowercase()).collect();
    for (j, a) in lower.iter().enumerate() {
        for opt in ["--upload-pack", "--receive-pack", "--exec"] {
            if a == opt || a.starts_with(&format!("{opt}=")) {
                if sub == "rebase" && opt == "--exec" {
                    continue;
                }
                let v = a.split_once('=').map(|(_, v)| v.to_owned()).or_else(|| lower.get(j + 1).cloned());
                acc = acc.max(text(&v.unwrap_or_default())).max(hook(opt));
            }
        }
    }
    match sub.as_str() {
        "config" => {
            let reading = lower.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "--get"
                        | "--get-all"
                        | "--get-regexp"
                        | "--list"
                        | "-l"
                        | "--unset"
                        | "--unset-all"
                        | "get"
                        | "list"
                        | "unset"
                )
            });
            let ops: Vec<&Word> =
                rest.iter().filter(|w| !w.text.starts_with('-') && !matches!(w.text.as_str(), "set" | "add")).collect();
            if !reading {
                if let (Some(k), Some(v)) = (ops.first(), ops.get(1)) {
                    if git_exec_key(&k.text, &v.text) {
                        acc = acc
                            .max(text(v.text.trim_start_matches('!')))
                            .max(d(C::SystemConfig, format!("configures `{}`, which makes git run a command", k.text)));
                    }
                }
            }
        }
        "rebase" => {
            for (j, a) in lower.iter().enumerate() {
                if a == "--exec" || a == "-x" {
                    if let Some(c) = rest.get(j + 1) {
                        acc = acc.max(text(&c.text));
                    }
                } else if let Some(c) = a.strip_prefix("--exec=") {
                    acc = acc.max(text(c));
                }
            }
        }
        "submodule" if lower.iter().any(|a| a == "foreach") => {
            let pos = lower.iter().position(|a| a == "foreach").unwrap_or(0);
            let cmds: Vec<&Word> = rest[pos + 1..].iter().filter(|w| !w.text.starts_with("--")).collect();
            acc = acc.max(text(&cmds.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")));
        }
        "bisect" if lower.first().is_some_and(|a| a == "run") => {
            acc = acc.max(text(&texts(&rest[1..]).join(" ")));
        }
        _ => {}
    }
    acc
}
