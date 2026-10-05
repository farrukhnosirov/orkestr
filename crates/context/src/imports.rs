//! Import / dependency graph (§20 "include neighbours of relevant files").
//!
//! [`extract_imports`] pulls raw import specifiers out of a file with
//! line-oriented regexes (Rust `use` / `mod`, TS/JS `import` / `export … from`
//! / `require()` / dynamic `import()`, Python `import` / `from … import`, Go
//! `import`, C# `using`). [`ImportGraph::build`] resolves them against the
//! [`FileIndex`] into project-internal edges; external packages are dropped.
//! Resolution is heuristic but deterministic (sorted sets everywhere).

use std::collections::{BTreeMap, BTreeSet};

use once_cell::sync::Lazy;
use regex::Regex;

use crate::index::{FileIndex, IndexedFile};
use crate::symbols::Language;

/// Max raw imports kept per file.
pub const MAX_IMPORTS: usize = 200;

#[allow(clippy::expect_used)]
fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid import regex")
}

static RS_USE: Lazy<Regex> = Lazy::new(|| re(r"^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+([A-Za-z_][A-Za-z0-9_:]*)"));
static RS_MOD: Lazy<Regex> = Lazy::new(|| re(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;"));
static JS_FROM: Lazy<Regex> = Lazy::new(|| re(r#"(?:^|\s)(?:import|export)\b[^'"]*?\bfrom\s*['"]([^'"]+)['"]"#));
static JS_BARE: Lazy<Regex> = Lazy::new(|| re(r#"^\s*import\s*['"]([^'"]+)['"]"#));
static JS_CALL: Lazy<Regex> = Lazy::new(|| re(r#"\b(?:require|import)\s*\(\s*['"]([^'"]+)['"]\s*\)"#));
static PY_FROM: Lazy<Regex> = Lazy::new(|| re(r"^\s*from\s+(\.*[A-Za-z0-9_.]*)\s+import\s+([A-Za-z0-9_., *()]+)"));
static PY_IMPORT: Lazy<Regex> = Lazy::new(|| re(r"^\s*import\s+([A-Za-z0-9_., ]+)"));
static GO_SINGLE: Lazy<Regex> = Lazy::new(|| re(r#"^\s*import\s+(?:[A-Za-z_.]+\s+)?"([^"]+)""#));
static GO_LINE: Lazy<Regex> = Lazy::new(|| re(r#"^\s*(?:[A-Za-z_.]+\s+)?"([^"]+)"\s*$"#));
static CS_USING: Lazy<Regex> =
    Lazy::new(|| re(r"^\s*(?:global\s+)?using\s+(?:static\s+)?(?:[A-Za-z_][A-Za-z0-9_]*\s*=\s*)?([A-Za-z_][A-Za-z0-9_.]*)\s*;"));
static CS_NAMESPACE: Lazy<Regex> = Lazy::new(|| re(r"^\s*namespace\s+([A-Za-z_][A-Za-z0-9_.]*)"));

/// Raw import specifiers of a file, in source order, deduplicated.
///
/// Python `from x import a, b` yields `x` plus `x.a` / `x.b` (sub-modules);
/// Rust `mod foo;` yields `mod:foo`.
#[must_use]
pub fn extract_imports(language: Language, text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |s: String, out: &mut Vec<String>| {
        if !s.is_empty() && out.len() < MAX_IMPORTS && !out.contains(&s) {
            out.push(s);
        }
    };
    let mut in_go_block = false;
    for line in text.lines() {
        match language {
            Language::Rust => {
                if let Some(c) = RS_USE.captures(line) {
                    push(c[1].trim_end_matches(':').to_owned(), &mut out);
                } else if let Some(c) = RS_MOD.captures(line) {
                    push(format!("mod:{}", &c[1]), &mut out);
                }
            }
            Language::TypeScript | Language::JavaScript => {
                for r in [&*JS_FROM, &*JS_BARE, &*JS_CALL] {
                    for c in r.captures_iter(line) {
                        push(c[1].to_owned(), &mut out);
                    }
                }
            }
            Language::Python => {
                if let Some(c) = PY_FROM.captures(line) {
                    let base = c[1].to_owned();
                    push(base.clone(), &mut out);
                    for name in c[2].split(',') {
                        let name = name.trim_matches(|ch: char| ch == '(' || ch == ')' || ch.is_whitespace());
                        let name = name.split_whitespace().next().unwrap_or_default();
                        if !name.is_empty() && name != "*" {
                            let sep = if base.ends_with('.') { "" } else { "." };
                            push(format!("{base}{sep}{name}"), &mut out);
                        }
                    }
                } else if let Some(c) = PY_IMPORT.captures(line) {
                    for m in c[1].split(',') {
                        if let Some(name) = m.split_whitespace().next() {
                            push(name.to_owned(), &mut out);
                        }
                    }
                }
            }
            Language::Go => {
                let t = line.trim();
                if in_go_block {
                    if t.starts_with(')') {
                        in_go_block = false;
                    } else if let Some(c) = GO_LINE.captures(line) {
                        push(c[1].to_owned(), &mut out);
                    }
                } else if t.starts_with("import (") || t == "import(" {
                    in_go_block = true;
                } else if let Some(c) = GO_SINGLE.captures(line) {
                    push(c[1].to_owned(), &mut out);
                }
            }
            Language::CSharp => {
                if let Some(c) = CS_USING.captures(line) {
                    push(c[1].to_owned(), &mut out);
                }
            }
            _ => {}
        }
    }
    out
}

/// Namespaces a C# file declares (used to resolve `using`).
#[must_use]
pub fn csharp_namespaces(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(c) = CS_NAMESPACE.captures(line) {
            let ns = c[1].to_owned();
            if !out.contains(&ns) {
                out.push(ns);
            }
        }
    }
    out
}

/// Resolved project-internal dependency graph.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportGraph {
    /// `file → files it imports`.
    pub imports: BTreeMap<String, BTreeSet<String>>,
    /// `file → files importing it`.
    pub imported_by: BTreeMap<String, BTreeSet<String>>,
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn join(dir: &str, rel: &str) -> String {
    if dir.is_empty() {
        rel.to_owned()
    } else {
        format!("{dir}/{rel}")
    }
}

/// Normalizes `a/./b/../c` → `a/c`; `None` when it escapes the root.
fn normalize_rel(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

struct Resolver<'a> {
    paths: BTreeSet<&'a str>,
    /// dir → files in it.
    by_dir: BTreeMap<&'a str, Vec<&'a str>>,
    /// C# namespace → declaring files.
    namespaces: BTreeMap<String, Vec<&'a str>>,
    /// Rust crate dir name (`context`, `orkestr-context`) → `…/src`.
    rust_crates: BTreeMap<String, String>,
}

const JS_EXTS: &[&str] = &["ts", "tsx", "mts", "cts", "d.ts", "js", "jsx", "mjs", "cjs"];

impl<'a> Resolver<'a> {
    fn new(index: &'a FileIndex) -> Self {
        let mut by_dir: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut namespaces: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        let mut rust_crates: BTreeMap<String, String> = BTreeMap::new();
        for f in &index.files {
            if let Some(lib) = f.path.strip_suffix("/src/lib.rs") {
                let name = lib.rsplit('/').next().unwrap_or(lib);
                rust_crates.entry(name.to_owned()).or_insert_with(|| format!("{lib}/src"));
            }
            by_dir.entry(parent_dir(&f.path)).or_default().push(&f.path);
            if f.language == Language::CSharp {
                for ns in csharp_namespaces(&f.sample) {
                    namespaces.entry(ns).or_default().push(&f.path);
                }
            }
        }
        Self { paths: index.files.iter().map(|f| f.path.as_str()).collect(), by_dir, namespaces, rust_crates }
    }

    fn has(&self, p: &str) -> Option<String> {
        self.paths.contains(p).then(|| p.to_owned())
    }

    fn resolve(&self, file: &IndexedFile, spec: &str) -> Vec<String> {
        match file.language {
            Language::Rust => self.rust(&file.path, spec),
            Language::TypeScript | Language::JavaScript => self.js(&file.path, spec).into_iter().collect(),
            Language::Python => self.python(&file.path, spec).into_iter().collect(),
            Language::Go => self.go(spec),
            Language::CSharp => self.csharp(spec),
            _ => Vec::new(),
        }
    }

    /// Module directory of a Rust file: `src/lib.rs` → `src`, `src/a.rs` →
    /// `src/a`, `src/a/mod.rs` → `src/a`.
    fn rust_module_dir(path: &str) -> String {
        let dir = parent_dir(path);
        let file = path.rsplit('/').next().unwrap_or(path);
        match file {
            "lib.rs" | "main.rs" | "mod.rs" => dir.to_owned(),
            f => join(dir, f.trim_end_matches(".rs")),
        }
    }

    /// Crate source root: the nearest ancestor named `src` (or the file dir).
    fn rust_crate_root(path: &str) -> String {
        let segs: Vec<&str> = path.split('/').collect();
        match segs.iter().rposition(|s| *s == "src") {
            Some(i) => segs[..=i].join("/"),
            None => parent_dir(path).to_owned(),
        }
    }

    fn rust_module_file(&self, dir: &str, segs: &[&str]) -> Option<String> {
        // Longest prefix that names a module file wins (`a::b::Item` → a/b.rs or a.rs).
        for n in (1..=segs.len()).rev() {
            let base = join(dir, &segs[..n].join("/"));
            if let Some(p) = self.has(&format!("{base}.rs")).or_else(|| self.has(&format!("{base}/mod.rs"))) {
                return Some(p);
            }
        }
        None
    }

    fn rust(&self, path: &str, spec: &str) -> Vec<String> {
        if let Some(m) = spec.strip_prefix("mod:") {
            let dir = Self::rust_module_dir(path);
            return self.rust_module_file(&dir, &[m]).into_iter().collect();
        }
        let segs: Vec<&str> = spec.split("::").filter(|s| !s.is_empty()).collect();
        let Some((&first, rest)) = segs.split_first() else { return Vec::new() };
        let (dir, rest) = match first {
            "crate" => (Self::rust_crate_root(path), rest),
            "self" => (Self::rust_module_dir(path), rest),
            "super" => {
                let mut dir = Self::rust_module_dir(path);
                let mut rest = rest;
                dir = parent_dir(&dir).to_owned();
                while rest.first() == Some(&"super") {
                    dir = parent_dir(&dir).to_owned();
                    rest = &rest[1..];
                }
                (dir, rest)
            }
            // Workspace crate (`orkestr_context::x` → crates/context/src/...): match a
            // crate dir whose name equals the crate name with `_`/`-` folded.
            other => {
                let wanted = other.replace('_', "-");
                let short = wanted.rsplit('-').next().unwrap_or(&wanted).to_owned();
                let root = [other, wanted.as_str(), short.as_str()]
                    .iter()
                    .find_map(|n| self.rust_crates.get(*n))
                    .cloned();
                match root {
                    Some(r) => {
                        if rest.is_empty() {
                            return self.has(&format!("{r}/lib.rs")).into_iter().collect();
                        }
                        (r, rest)
                    }
                    None => return Vec::new(),
                }
            }
        };
        if rest.is_empty() {
            return Vec::new();
        }
        self.rust_module_file(&dir, rest).into_iter().collect()
    }

    fn js(&self, path: &str, spec: &str) -> Option<String> {
        if !(spec.starts_with("./") || spec.starts_with("../")) {
            return None;
        }
        let target = normalize_rel(&join(parent_dir(path), spec))?;
        if let Some(p) = self.has(&target) {
            return Some(p);
        }
        // `./x.js` written for a TS source.
        let stem = target
            .strip_suffix(".js")
            .or_else(|| target.strip_suffix(".jsx"))
            .or_else(|| target.strip_suffix(".mjs"))
            .unwrap_or(&target);
        for ext in JS_EXTS {
            if let Some(p) = self.has(&format!("{stem}.{ext}")) {
                return Some(p);
            }
        }
        for ext in JS_EXTS {
            if let Some(p) = self.has(&format!("{target}/index.{ext}")) {
                return Some(p);
            }
        }
        None
    }

    fn python(&self, path: &str, spec: &str) -> Option<String> {
        let dots = spec.chars().take_while(|c| *c == '.').count();
        let module = &spec[dots..];
        let rel = module.replace('.', "/");
        let candidates_for = |base: &str| -> Option<String> {
            let target = if rel.is_empty() { base.to_owned() } else { join(base, &rel) };
            self.has(&format!("{target}.py"))
                .or_else(|| self.has(&format!("{target}.pyi")))
                .or_else(|| self.has(&format!("{target}/__init__.py")))
        };
        if dots > 0 {
            let mut base = parent_dir(path).to_owned();
            for _ in 1..dots {
                base = parent_dir(&base).to_owned();
            }
            return candidates_for(&base);
        }
        if rel.is_empty() {
            return None;
        }
        // Absolute import: try the file's ancestors (package roots), nearest first,
        // then common source roots.
        let mut base = parent_dir(path).to_owned();
        loop {
            if let Some(p) = candidates_for(&base) {
                return Some(p);
            }
            if base.is_empty() {
                break;
            }
            base = parent_dir(&base).to_owned();
        }
        candidates_for("src")
    }

    fn go(&self, spec: &str) -> Vec<String> {
        // Longest directory suffix of the import path that exists in the project.
        let segs: Vec<&str> = spec.split('/').collect();
        for start in 0..segs.len() {
            let suffix = segs[start..].join("/");
            let tail = format!("/{suffix}");
            let go: Vec<String> = self
                .by_dir
                .iter()
                .filter(|(d, _)| **d == suffix || d.ends_with(&tail))
                .flat_map(|(_, fs)| fs.iter().filter(|f| f.ends_with(".go") && !f.ends_with("_test.go")))
                .map(|f| (*f).to_owned())
                .collect();
            if !go.is_empty() {
                return go;
            }
        }
        Vec::new()
    }

    fn csharp(&self, spec: &str) -> Vec<String> {
        if let Some(files) = self.namespaces.get(spec) {
            return files.iter().map(|f| (*f).to_owned()).collect();
        }
        // Folder convention: `Company.App.Services` → …/App/Services/*.cs
        let segs: Vec<&str> = spec.split('.').collect();
        for start in 0..segs.len() {
            let suffix = segs[start..].join("/");
            let hit: Vec<String> = self
                .by_dir
                .iter()
                .filter(|(d, _)| **d == suffix || d.ends_with(&format!("/{suffix}")))
                .flat_map(|(_, fs)| fs.iter().filter(|f| f.ends_with(".cs")).map(|f| (*f).to_owned()))
                .collect();
            if !hit.is_empty() {
                return hit;
            }
        }
        Vec::new()
    }
}

impl ImportGraph {
    /// Resolves every file's imports against the index.
    #[must_use]
    pub fn build(index: &FileIndex) -> Self {
        let resolver = Resolver::new(index);
        let mut g = ImportGraph::default();
        for f in &index.files {
            for spec in &f.imports {
                for target in resolver.resolve(f, spec) {
                    if target != f.path {
                        g.imports.entry(f.path.clone()).or_default().insert(target.clone());
                        g.imported_by.entry(target).or_default().insert(f.path.clone());
                    }
                }
            }
        }
        g
    }

    /// Files `path` imports (sorted).
    #[must_use]
    pub fn imports_of(&self, path: &str) -> Vec<&str> {
        self.imports.get(path).map(|s| s.iter().map(String::as_str).collect()).unwrap_or_default()
    }

    /// Files importing `path` (sorted).
    #[must_use]
    pub fn importers_of(&self, path: &str) -> Vec<&str> {
        self.imported_by.get(path).map(|s| s.iter().map(String::as_str).collect()).unwrap_or_default()
    }

    /// Number of edges.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.imports.values().map(BTreeSet::len).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_per_language() {
        let rs = "use crate::payroll::calc::Overtime;\npub use super::tax;\nmod util;\npub(crate) mod db;\nuse std::fmt;";
        assert_eq!(
            extract_imports(Language::Rust, rs),
            vec!["crate::payroll::calc::Overtime", "super::tax", "mod:util", "mod:db", "std::fmt"]
        );
        let ts = "import { a } from './a';\nimport type { B } from \"../b/index\";\nexport * from './c.js';\nimport './side.css';\nconst x = require('./x');\nconst y = await import('./y');\nimport React from 'react';";
        assert_eq!(
            extract_imports(Language::TypeScript, ts),
            vec!["./a", "../b/index", "./c.js", "./side.css", "./x", "./y", "react"]
        );
        let py = "import os, payroll.calc as c\nfrom .tax import rate, bands\nfrom .. import shared\nfrom app.models import *";
        assert_eq!(
            extract_imports(Language::Python, py),
            vec!["os", "payroll.calc", ".tax", ".tax.rate", ".tax.bands", "..", "..shared", "app.models"]
        );
        let go = "package main\nimport \"fmt\"\nimport (\n  \"example.com/app/payroll\"\n  db \"example.com/app/store\"\n)\n";
        assert_eq!(extract_imports(Language::Go, go), vec!["fmt", "example.com/app/payroll", "example.com/app/store"]);
        let cs = "using System;\nusing static App.Util.Math;\nusing Alias = App.Payroll;\nglobal using App.Core;\nnamespace App.Web;";
        assert_eq!(extract_imports(Language::CSharp, cs), vec!["System", "App.Util.Math", "App.Payroll", "App.Core"]);
        assert_eq!(csharp_namespaces(cs), vec!["App.Web"]);
        assert!(extract_imports(Language::Markdown, "import x from './y'").is_empty());
    }

    #[test]
    fn resolves_graph() {
        let idx = FileIndex::from_sources([
            ("crates/pay/src/lib.rs", "pub mod calc;\nmod tax;\n"),
            ("crates/pay/src/calc.rs", "use crate::tax::rate;\nuse super::missing;\n"),
            ("crates/pay/src/tax/mod.rs", "use self::bands::Band;\nmod bands;"),
            ("crates/pay/src/tax/bands.rs", "pub struct Band;"),
            ("crates/app/src/main.rs", "use orkestr_pay::calc::compute;\nuse pay::calc;"),
            ("web/src/App.tsx", "import { api } from './api';\nimport Btn from './ui';\nimport x from 'react';"),
            ("web/src/api.ts", "export const api = 1;"),
            ("web/src/ui/index.tsx", "export default function Btn() {}"),
            ("py/app/main.py", "from .models import user\nimport app.util"),
            ("py/app/models/__init__.py", ""),
            ("py/app/models/user.py", ""),
            ("py/app/util.py", ""),
            ("go/cmd/main.go", "import (\n \"example.com/x/payroll\"\n)"),
            ("go/payroll/pay.go", "package payroll"),
            ("go/payroll/pay_test.go", "package payroll"),
            ("cs/Web/Program.cs", "using App.Services;\nusing App.Models;"),
            ("cs/Services/PayService.cs", "namespace App.Services;\nclass PayService {}"),
            ("cs/App/Models/User.cs", "class User {}"),
        ]);
        let g = ImportGraph::build(&idx);
        assert_eq!(g.imports_of("crates/pay/src/lib.rs"), vec!["crates/pay/src/calc.rs", "crates/pay/src/tax/mod.rs"]);
        assert_eq!(g.imports_of("crates/pay/src/calc.rs"), vec!["crates/pay/src/tax/mod.rs"]);
        assert_eq!(g.imports_of("crates/pay/src/tax/mod.rs"), vec!["crates/pay/src/tax/bands.rs"]);
        assert_eq!(g.imports_of("crates/app/src/main.rs"), vec!["crates/pay/src/calc.rs"]);
        assert_eq!(g.imports_of("web/src/App.tsx"), vec!["web/src/api.ts", "web/src/ui/index.tsx"]);
        assert_eq!(g.imports_of("py/app/main.py"), vec!["py/app/models/__init__.py", "py/app/models/user.py", "py/app/util.py"]);
        assert_eq!(g.imports_of("go/cmd/main.go"), vec!["go/payroll/pay.go"]);
        assert_eq!(g.imports_of("cs/Web/Program.cs"), vec!["cs/App/Models/User.cs", "cs/Services/PayService.cs"]);
        assert_eq!(
            g.importers_of("crates/pay/src/calc.rs"),
            vec!["crates/app/src/main.rs", "crates/pay/src/lib.rs"]
        );
        assert!(g.edge_count() >= 10);
        assert_eq!(ImportGraph::build(&idx), g, "deterministic");
    }

    #[test]
    fn normalize_paths() {
        assert_eq!(normalize_rel("a/./b/../c").as_deref(), Some("a/c"));
        assert_eq!(normalize_rel("../x"), None);
    }
}
