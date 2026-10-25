//! Regression guard: every child process the app spawns must hide its
//! console window on Windows. The release build runs without a console
//! (`windows_subsystem = "windows"`), so a console child spawned without
//! `CREATE_NO_WINDOW` (`git`, `whoami`, a `claude.cmd` shim run through
//! `cmd.exe`, …) pops up a visible console window.
//!
//! Scans `crates/*/src` and `apps/desktop/src-tauri/src` for `Command::new(` /
//! `Command::from(` outside test code and requires the statement or the lines
//! that follow it (up to the end of the function) to apply
//! `orkestr_platform::process::HideConsole::hide_console`,
//! `isolate_command`, `stdio_command`, `background_command` or
//! `creation_flags(CREATE_NO_WINDOW)`. A deliberate visible console is marked
//! with `// console: visible-by-design` on the line or the line above.

use std::path::{Path, PathBuf};

const HIDERS: &[&str] =
    &["hide_console(", "isolate_command(", "background_command(", "stdio_command(", "creation_flags(CREATE_NO_WINDOW"];
const ALLOW: &str = "console: visible-by-design";
/// How far after the `Command::new(` line the hiding call may appear.
const WINDOW: usize = 30;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if name != "tests" && name != "target" && name != "node_modules" {
                rust_files(&p, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
            out.push(p);
        }
    }
}

/// Source lines with `#[cfg(test)] mod … { … }` blocks blanked out.
fn non_test_lines(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim() == "#[cfg(test)]" {
            if let Some(next) = lines.get(i + 1) {
                let t = next.trim_start();
                let is_mod_block = (t.starts_with("mod ") || t.contains(" mod ")) && t.trim_end().ends_with('{');
                if is_mod_block {
                    let indent = &next[..next.len() - t.len()];
                    let close = format!("{indent}}}");
                    let mut j = i + 1;
                    while j < lines.len() && lines[j].trim_end() != close {
                        j += 1;
                    }
                    for _ in i..=j.min(lines.len() - 1) {
                        out.push(String::new());
                    }
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(line.to_owned());
        i += 1;
    }
    out
}

fn offenders_in(path: &Path, text: &str) -> Vec<String> {
    let lines = non_test_lines(text);
    let mut bad = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        if !(code.contains("Command::new(") || code.contains("Command::from(")) {
            continue;
        }
        if code.contains("CommandBuilder::new(") && !code.contains("process::Command") {
            continue; // portable-pty: ConPTY owns the console.
        }
        let allowed = line.contains(ALLOW) || n.checked_sub(1).is_some_and(|p| lines[p].contains(ALLOW));
        let mut hidden = HIDERS.iter().any(|h| code.contains(h));
        for follow in lines.iter().skip(n + 1).take(WINDOW) {
            let t = follow.trim_start();
            if t.starts_with("fn ") || t.starts_with("pub fn ") || t.starts_with("pub(crate) fn ") {
                break;
            }
            if HIDERS.iter().any(|h| follow.contains(h)) {
                hidden = true;
                break;
            }
        }
        if !(allowed || hidden) {
            bad.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
        }
    }
    bad
}

#[test]
fn every_spawn_hides_its_console() {
    let root = workspace_root();
    let mut files = Vec::new();
    if let Ok(crates) = std::fs::read_dir(root.join("crates")) {
        for c in crates.flatten() {
            rust_files(&c.path().join("src"), &mut files);
        }
    }
    rust_files(&root.join("apps").join("desktop").join("src-tauri").join("src"), &mut files);
    assert!(files.len() > 50, "scanned too few files ({}) under {}", files.len(), root.display());
    let mut bad = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap_or_default();
        bad.extend(offenders_in(f, &text));
    }
    assert!(
        bad.is_empty(),
        "child processes spawned without hiding the console window (call \
         orkestr_platform::process::HideConsole::hide_console or use background_command / \
         isolate_command, or mark `// {ALLOW}`):\n{}",
        bad.join("\n")
    );
}

#[test]
fn guard_detects_an_unhidden_spawn() {
    let src = "fn a() {\n    let out = std::process::Command::new(\"git\").output();\n}\n\
               fn b() {\n    let mut c = tokio::process::Command::new(\"x\");\n    c.hide_console();\n}\n\
               fn c() {\n    // console: visible-by-design\n    std::process::Command::new(\"wt\").spawn();\n}\n\
               #[cfg(test)]\nmod tests {\n    fn t() { std::process::Command::new(\"git\"); }\n}\n";
    let bad = offenders_in(Path::new("x.rs"), src);
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0].starts_with("x.rs:2:"), "{bad:?}");
}
