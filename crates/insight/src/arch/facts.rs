//! Per-file facts: classification of a file and everything the model needs
//! from it (manifest, compose services, routes, imports, …). Facts are pure
//! functions of `(path, content)`, which makes them cacheable by size+mtime.

use once_cell::sync::Lazy;
use orkestr_protocol::methods::architecture::ApiRoute;
use regex::Regex;

use super::manifests::{self, Dep, Manifest};
use super::routes;
use super::yaml::{self, Yaml};

/// Source language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    /// Rust.
    Rust,
    /// JavaScript / TypeScript.
    JsTs,
    /// Python.
    Python,
    /// Go.
    Go,
    /// C#.
    CSharp,
    /// Java / Kotlin.
    Java,
    /// Anything else (counted, not parsed).
    Other,
}

/// What a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// `Cargo.toml`.
    Cargo,
    /// `package.json`.
    PackageJson,
    /// `pyproject.toml`.
    Pyproject,
    /// `setup.py`.
    SetupPy,
    /// `requirements.txt`.
    Requirements,
    /// `go.mod`.
    GoMod,
    /// `*.csproj` / `*.fsproj`.
    DotnetProject,
    /// `Dockerfile`, `*.Dockerfile`, `Containerfile`.
    Dockerfile,
    /// docker-compose file.
    Compose,
    /// OpenAPI / Swagger document.
    OpenApi,
    /// Prisma schema.
    Prisma,
    /// File inside a migrations folder.
    Migration,
    /// Source code.
    Source(Lang),
    /// Anything else.
    Other,
}

impl Role {
    /// Whether the content must be read to extract facts.
    #[must_use]
    pub fn needs_content(self) -> bool {
        !matches!(self, Role::Migration | Role::Other | Role::Source(Lang::Other))
    }
}

/// A docker-compose service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeService {
    /// Service name.
    pub name: String,
    /// Image.
    pub image: Option<String>,
    /// Build context (relative to the compose file's directory).
    pub build: Option<String>,
    /// `depends_on` services.
    pub depends_on: Vec<String>,
    /// Published ports.
    pub ports: Vec<String>,
}

/// Facts extracted from one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFacts {
    /// Role.
    pub role: Role,
    /// Manifest (manifest roles).
    pub manifest: Option<Manifest>,
    /// Requirements-only dependencies.
    pub requirements: Vec<Dep>,
    /// Compose services.
    pub compose: Vec<ComposeService>,
    /// Final `FROM` image of a Dockerfile.
    pub docker_from: Option<String>,
    /// HTTP routes.
    pub routes: Vec<ApiRoute>,
    /// OpenAPI title.
    pub title: Option<String>,
    /// Import specifiers (language specific, deduplicated, capped).
    pub imports: Vec<String>,
    /// Go `package main`.
    pub go_main: bool,
    /// Source uses SQLite directly (`import sqlite3`, …).
    pub uses_sqlite: bool,
    /// Prisma datasource provider.
    pub prisma_provider: Option<String>,
    /// Parse problems.
    pub warnings: Vec<String>,
}

impl FileFacts {
    fn new(role: Role) -> Self {
        Self {
            role,
            manifest: None,
            requirements: Vec::new(),
            compose: Vec::new(),
            docker_from: None,
            routes: Vec::new(),
            title: None,
            imports: Vec::new(),
            go_main: false,
            uses_sqlite: false,
            prisma_provider: None,
            warnings: Vec::new(),
        }
    }
}

/// Max imports kept per file.
pub const MAX_IMPORTS_PER_FILE: usize = 300;

const MIGRATION_DIRS: &[&str] = &["migrations", "migration", "migrate", "alembic"];

/// Classifies a path (relative, `/` separators).
#[must_use]
pub fn classify(path: &str) -> Role {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    let dirs: Vec<&str> = path.split('/').collect();
    let in_migrations = dirs.len() > 1
        && dirs[..dirs.len() - 1].iter().any(|d| MIGRATION_DIRS.contains(&d.to_ascii_lowercase().as_str()));
    match lower.as_str() {
        "cargo.toml" => return Role::Cargo,
        "package.json" => return Role::PackageJson,
        "pyproject.toml" => return Role::Pyproject,
        "setup.py" => return Role::SetupPy,
        "requirements.txt" => return Role::Requirements,
        "go.mod" => return Role::GoMod,
        "dockerfile" | "containerfile" => return Role::Dockerfile,
        "schema.prisma" => return Role::Prisma,
        _ => {}
    }
    if lower.starts_with("dockerfile.") || lower.ends_with(".dockerfile") {
        return Role::Dockerfile;
    }
    if (lower.starts_with("docker-compose") || lower.starts_with("compose."))
        && (lower.ends_with(".yml") || lower.ends_with(".yaml"))
    {
        return Role::Compose;
    }
    let (stem, ext) = lower.rsplit_once('.').unwrap_or((lower.as_str(), ""));
    if matches!(ext, "json" | "yaml" | "yml")
        && (stem == "openapi" || stem == "swagger" || stem.ends_with(".openapi") || stem.ends_with("-openapi"))
    {
        return Role::OpenApi;
    }
    if ext == "csproj" || ext == "fsproj" {
        return Role::DotnetProject;
    }
    if in_migrations {
        return Role::Migration;
    }
    let lang = match ext {
        "rs" => Lang::Rust,
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" => Lang::JsTs,
        "py" => Lang::Python,
        "go" => Lang::Go,
        "cs" => Lang::CSharp,
        "java" | "kt" => Lang::Java,
        "c" | "h" | "cpp" | "hpp" | "cc" | "swift" | "rb" | "php" | "scala" | "vue" | "svelte" | "dart" | "lua"
        | "ex" | "exs" | "fs" => Lang::Other,
        _ => return Role::Other,
    };
    if lower.ends_with(".d.ts") {
        return Role::Other;
    }
    Role::Source(lang)
}

/// Extracts facts.
#[must_use]
pub fn extract(path: &str, role: Role, text: &str) -> FileFacts {
    let mut f = FileFacts::new(role);
    let dir_name = path.rsplit('/').nth(1).unwrap_or("").to_owned();
    let file_stem = {
        let n = path.rsplit('/').next().unwrap_or(path);
        n.rsplit_once('.').map_or(n, |(s, _)| s).to_owned()
    };
    let manifest = match role {
        Role::Cargo => Some(manifests::parse_cargo(text)),
        Role::PackageJson => {
            Some(manifests::parse_package_json(text, if dir_name.is_empty() { "root" } else { &dir_name }))
        }
        Role::Pyproject => Some(manifests::parse_pyproject(text)),
        Role::SetupPy => Some(manifests::parse_setup_py(text)),
        Role::GoMod => Some(manifests::parse_go_mod(text)),
        Role::DotnetProject => Some(Ok(manifests::parse_csproj(text, &file_stem))),
        _ => None,
    };
    match manifest {
        Some(Ok(m)) => f.manifest = Some(m),
        Some(Err(e)) => f.warnings.push(format!("{path}: {e}")),
        None => {}
    }
    match role {
        Role::Requirements => f.requirements = manifests::parse_requirements(text),
        Role::Dockerfile => f.docker_from = docker_from(text),
        Role::Compose => f.compose = compose_services(text),
        Role::OpenApi => {
            let (title, routes) = routes::openapi(path, text);
            f.title = title;
            f.routes = routes;
        }
        Role::Prisma => f.prisma_provider = prisma_provider(text),
        Role::Source(lang) => {
            f.routes = routes::extract(lang, path, text);
            f.imports = imports(lang, text);
            f.go_main = lang == Lang::Go && GO_MAIN.is_match(text);
            f.uses_sqlite = lang == Lang::Python && f.imports.iter().any(|i| i == "sqlite3");
        }
        _ => {}
    }
    f
}

static GO_MAIN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\s*package\s+main\b").expect("regex"));

fn docker_from(text: &str) -> Option<String> {
    static FROM: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?im)^\s*FROM\s+(?:--\S+\s+)*(\S+)").expect("regex"));
    FROM.captures_iter(text).filter_map(|c| c.get(1)).map(|m| m.as_str().to_owned()).last()
}

fn prisma_provider(text: &str) -> Option<String> {
    static DS: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"(?s)datasource\s+\w+\s*\{[^}]*?provider\s*=\s*"([^"]+)""#).expect("regex"));
    DS.captures(text).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned())
}

fn compose_services(text: &str) -> Vec<ComposeService> {
    let doc = yaml::parse(text);
    let Some(services) = doc.get("services") else { return Vec::new() };
    services
        .entries()
        .iter()
        .map(|(name, svc)| {
            let build = match svc.get("build") {
                Some(Yaml::Scalar(s)) if !s.is_empty() => Some(s.clone()),
                Some(b @ Yaml::Map(_)) => Some(b.get("context").and_then(Yaml::as_str).unwrap_or(".").to_owned()),
                _ => None,
            };
            let depends_on = match svc.get("depends_on") {
                Some(Yaml::List(l)) => l.iter().filter_map(Yaml::as_str).map(str::to_owned).collect(),
                Some(Yaml::Map(m)) => m.iter().map(|(k, _)| k.clone()).collect(),
                _ => Vec::new(),
            };
            let ports = svc
                .get("ports")
                .map(|p| p.items().iter().filter_map(Yaml::as_str).map(str::to_owned).collect())
                .unwrap_or_default();
            ComposeService {
                name: name.clone(),
                image: svc.get("image").and_then(Yaml::as_str).filter(|s| !s.is_empty()).map(str::to_owned),
                build,
                depends_on,
                ports,
            }
        })
        .collect()
}

/// Import specifiers of a source file.
#[must_use]
pub fn imports(lang: Lang, text: &str) -> Vec<String> {
    static RUST_USE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:use\s+(?:::)?|extern\s+crate\s+)([A-Za-z_][A-Za-z0-9_]*)")
            .expect("regex")
    });
    static JS: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r#"(?:\bfrom\s+|\bimport\s+|\brequire\(\s*|\bimport\(\s*)['"]([^'"\n]+)['"]"#).expect("regex")
    });
    static PY_IMPORT: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\s*import\s+([\w\., ]+)").expect("regex"));
    static PY_FROM: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\s*from\s+(\.*[\w\.]*)\s+import\b").expect("regex"));
    static GO_SINGLE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"(?m)^\s*import\s+(?:\w+\s+)?"([^"]+)""#).expect("regex"));
    static GO_BLOCK: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)import\s*\((.*?)\)").expect("regex"));
    static GO_ITEM: Lazy<Regex> = Lazy::new(|| Regex::new(r#""([^"]+)""#).expect("regex"));
    static CS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\s*using\s+(?:static\s+)?([\w\.]+)\s*;").expect("regex"));
    static JAVA: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\s*import\s+(?:static\s+)?([\w\.]+)").expect("regex"));
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        let s = s.trim();
        if !s.is_empty() && out.len() < MAX_IMPORTS_PER_FILE && !out.iter().any(|x| x == s) {
            out.push(s.to_owned());
        }
    };
    match lang {
        Lang::Rust => {
            for c in RUST_USE.captures_iter(text) {
                let ident = &c[1];
                if !matches!(ident, "crate" | "self" | "super" | "std" | "core" | "alloc") {
                    push(ident);
                }
            }
        }
        Lang::JsTs => {
            for c in JS.captures_iter(text) {
                push(&c[1]);
            }
        }
        Lang::Python => {
            for c in PY_IMPORT.captures_iter(text) {
                for part in c[1].split(',') {
                    let name = part.split_whitespace().next().unwrap_or("");
                    push(name);
                }
            }
            for c in PY_FROM.captures_iter(text) {
                push(&c[1]);
            }
        }
        Lang::Go => {
            for c in GO_SINGLE.captures_iter(text) {
                push(&c[1]);
            }
            for block in GO_BLOCK.captures_iter(text) {
                for item in GO_ITEM.captures_iter(&block[1]) {
                    push(&item[1]);
                }
            }
        }
        Lang::CSharp => {
            for c in CS.captures_iter(text) {
                push(&c[1]);
            }
        }
        Lang::Java => {
            for c in JAVA.captures_iter(text) {
                push(&c[1]);
            }
        }
        Lang::Other => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        assert_eq!(classify("Cargo.toml"), Role::Cargo);
        assert_eq!(classify("apps/web/package.json"), Role::PackageJson);
        assert_eq!(classify("deploy/api.Dockerfile"), Role::Dockerfile);
        assert_eq!(classify("Dockerfile.prod"), Role::Dockerfile);
        assert_eq!(classify("docker-compose.override.yml"), Role::Compose);
        assert_eq!(classify("compose.yaml"), Role::Compose);
        assert_eq!(classify("docs/openapi.yaml"), Role::OpenApi);
        assert_eq!(classify("api/v1.openapi.json"), Role::OpenApi);
        assert_eq!(classify("db/migrations/0001_init.sql"), Role::Migration);
        assert_eq!(classify("alembic/versions/abc.py"), Role::Migration);
        assert_eq!(classify("src/lib.rs"), Role::Source(Lang::Rust));
        assert_eq!(classify("src/types.d.ts"), Role::Other);
        assert_eq!(classify("README.md"), Role::Other);
        assert_eq!(classify("Api/Api.csproj"), Role::DotnetProject);
        assert!(!Role::Migration.needs_content());
    }

    #[test]
    fn import_extraction() {
        assert_eq!(
            imports(Lang::Rust, "use std::io;\nuse orkestr_domain::X;\npub use ::serde::Y;\npub(crate) use crate::a;\nextern crate foo;"),
            vec!["orkestr_domain", "serde", "foo"]
        );
        assert_eq!(
            imports(Lang::JsTs, "import x from './a';\nimport '@acme/ui/styles.css';\nconst y = require(\"lodash\");\nexport * from \"../b\";"),
            vec!["./a", "@acme/ui/styles.css", "lodash", "../b"]
        );
        assert_eq!(
            imports(Lang::Python, "import os, sys as s\nfrom .models import A\nfrom pkg.sub import B\n"),
            vec!["os", "sys", ".models", "pkg.sub"]
        );
        assert_eq!(
            imports(
                Lang::Go,
                "package main\nimport (\n  \"fmt\"\n  api \"github.com/acme/api/x\"\n)\nimport \"net/http\""
            ),
            vec!["net/http", "fmt", "github.com/acme/api/x"]
        );
        assert_eq!(imports(Lang::CSharp, "using System;\nusing Acme.Core.Models;"), vec!["System", "Acme.Core.Models"]);
    }

    #[test]
    fn extract_dockerfile_compose_prisma() {
        let f = extract(
            "Dockerfile",
            Role::Dockerfile,
            "FROM rust:1.80 AS build\nRUN x\nFROM --platform=linux/amd64 debian:bookworm-slim\n",
        );
        assert_eq!(f.docker_from.as_deref(), Some("debian:bookworm-slim"));
        let f = extract(
            "docker-compose.yml",
            Role::Compose,
            "services:\n  api:\n    build: .\n    ports: [\"80:80\"]\n    depends_on: [db]\n  db:\n    image: postgres:16\n",
        );
        assert_eq!(f.compose.len(), 2);
        assert_eq!(f.compose[0].build.as_deref(), Some("."));
        assert_eq!(f.compose[0].ports, vec!["80:80"]);
        assert_eq!(f.compose[1].image.as_deref(), Some("postgres:16"));
        let f = extract(
            "prisma/schema.prisma",
            Role::Prisma,
            "datasource db {\n  provider = \"postgresql\"\n  url = env(\"X\")\n}",
        );
        assert_eq!(f.prisma_provider.as_deref(), Some("postgresql"));
        let f = extract("Cargo.toml", Role::Cargo, "garbage = [");
        assert!(f.manifest.is_none() && !f.warnings.is_empty());
        let f = extract("cmd/srv/main.go", Role::Source(Lang::Go), "package main\nfunc main(){}");
        assert!(f.go_main);
    }
}
