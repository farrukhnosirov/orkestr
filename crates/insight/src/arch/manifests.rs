//! Package manifest parsing: Cargo.toml, package.json, pyproject.toml,
//! setup.py, requirements.txt, go.mod, *.csproj / *.fsproj.

use once_cell::sync::Lazy;
use regex::Regex;

/// Ecosystem ids.
pub mod eco {
    /// Rust / Cargo.
    pub const CARGO: &str = "cargo";
    /// JavaScript / TypeScript (npm, pnpm, yarn).
    pub const NPM: &str = "npm";
    /// Python.
    pub const PYTHON: &str = "python";
    /// Go modules.
    pub const GO: &str = "go";
    /// .NET.
    pub const DOTNET: &str = "dotnet";
    /// Plain directory (no manifest).
    pub const DIR: &str = "dir";
}

/// A declared dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    /// Package name.
    pub name: String,
    /// Path dependency (relative to the manifest's directory, `/` separators).
    pub path: Option<String>,
    /// Development-only dependency.
    pub dev: bool,
}

/// A parsed manifest.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Manifest {
    /// Ecosystem (see [`eco`]).
    pub ecosystem: &'static str,
    /// Package name (empty for virtual workspace roots).
    pub name: String,
    /// Version.
    pub version: Option<String>,
    /// Name used by imports (`foo_bar` for crate `foo-bar`, Go module path,
    /// Python import name).
    pub import_name: Option<String>,
    /// Dependencies.
    pub deps: Vec<Dep>,
    /// Declared executables (Cargo `[[bin]]`, npm `bin`, Python scripts).
    pub bins: Vec<String>,
    /// Defines a package (false for virtual workspace roots).
    pub has_package: bool,
    /// Declares a workspace (Cargo `[workspace]`, npm `workspaces`, go.work).
    pub workspace_root: bool,
    /// Extra tags (`web`, `app`, …).
    pub tags: Vec<String>,
}

fn name_of_pep508(spec: &str) -> Option<String> {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*([A-Za-z0-9][A-Za-z0-9._-]*)").expect("regex"));
    RE.captures(spec).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned())
}

/// Python import name of a distribution name.
#[must_use]
pub fn python_import_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace(['-', '.'], "_")
}

fn toml_deps(table: Option<&toml::Value>, dev: bool, out: &mut Vec<Dep>) {
    let Some(toml::Value::Table(t)) = table else { return };
    for (key, spec) in t {
        let (name, path) = match spec {
            toml::Value::Table(st) => {
                let name = st.get("package").and_then(|v| v.as_str()).unwrap_or(key).to_owned();
                let path = st.get("path").and_then(|v| v.as_str()).map(|p| p.replace('\\', "/"));
                (name, path)
            }
            _ => (key.clone(), None),
        };
        out.push(Dep { name, path, dev });
    }
}

/// `Cargo.toml`.
pub fn parse_cargo(src: &str) -> Result<Manifest, String> {
    let v: toml::Value = toml::from_str(src).map_err(|e| format!("Cargo.toml: {}", first_line(&e.to_string())))?;
    let mut m = Manifest { ecosystem: eco::CARGO, ..Default::default() };
    if let Some(pkg) = v.get("package") {
        m.has_package = true;
        m.name = pkg.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_owned();
        m.version = pkg.get("version").and_then(|n| n.as_str()).map(str::to_owned);
        m.import_name = Some(m.name.replace('-', "_"));
    }
    m.workspace_root = v.get("workspace").is_some();
    toml_deps(v.get("dependencies"), false, &mut m.deps);
    toml_deps(v.get("dev-dependencies"), true, &mut m.deps);
    toml_deps(v.get("build-dependencies"), true, &mut m.deps);
    if let Some(toml::Value::Table(targets)) = v.get("target") {
        for cfg in targets.values() {
            toml_deps(cfg.get("dependencies"), false, &mut m.deps);
        }
    }
    if let Some(toml::Value::Array(bins)) = v.get("bin") {
        for b in bins {
            if let Some(n) = b.get("name").and_then(|n| n.as_str()) {
                m.bins.push(n.to_owned());
            }
        }
    }
    if let Some(lib) = v.get("lib") {
        if let Some(n) = lib.get("name").and_then(|n| n.as_str()) {
            m.import_name = Some(n.to_owned());
        }
    }
    if !m.has_package && !m.workspace_root {
        return Err("Cargo.toml: neither [package] nor [workspace]".into());
    }
    Ok(m)
}

/// `package.json`.
pub fn parse_package_json(src: &str, dir_name: &str) -> Result<Manifest, String> {
    let v: serde_json::Value =
        serde_json::from_str(src).map_err(|e| format!("package.json: {}", first_line(&e.to_string())))?;
    let mut m = Manifest { ecosystem: eco::NPM, ..Default::default() };
    m.workspace_root = match v.get("workspaces") {
        Some(serde_json::Value::Array(a)) => !a.is_empty(),
        Some(serde_json::Value::Object(o)) => o.get("packages").is_some(),
        _ => false,
    };
    let name = v.get("name").and_then(|n| n.as_str()).map(str::to_owned);
    m.has_package = name.is_some() || !m.workspace_root;
    m.name = name.unwrap_or_else(|| dir_name.to_owned());
    m.import_name = Some(m.name.clone());
    m.version = v.get("version").and_then(|n| n.as_str()).map(str::to_owned);
    for (field, dev) in [
        ("dependencies", false),
        ("devDependencies", true),
        ("peerDependencies", false),
        ("optionalDependencies", false),
    ] {
        if let Some(serde_json::Value::Object(deps)) = v.get(field) {
            for (name, spec) in deps {
                let path = spec
                    .as_str()
                    .and_then(|s| s.strip_prefix("file:").or_else(|| s.strip_prefix("link:")))
                    .map(|p| p.replace('\\', "/"));
                m.deps.push(Dep { name: name.clone(), path, dev });
            }
        }
    }
    match v.get("bin") {
        Some(serde_json::Value::String(_)) => m.bins.push(m.name.rsplit('/').next().unwrap_or(&m.name).to_owned()),
        Some(serde_json::Value::Object(o)) => m.bins.extend(o.keys().cloned()),
        _ => {}
    }
    if let Some(serde_json::Value::Object(scripts)) = v.get("scripts") {
        if scripts.contains_key("start") {
            m.tags.push("app".into());
        }
    }
    Ok(m)
}

/// `pyproject.toml` (PEP 621 `[project]` or Poetry).
pub fn parse_pyproject(src: &str) -> Result<Manifest, String> {
    let v: toml::Value = toml::from_str(src).map_err(|e| format!("pyproject.toml: {}", first_line(&e.to_string())))?;
    let mut m = Manifest { ecosystem: eco::PYTHON, ..Default::default() };
    if let Some(p) = v.get("project") {
        m.name = p.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_owned();
        m.version = p.get("version").and_then(|n| n.as_str()).map(str::to_owned);
        if let Some(toml::Value::Array(deps)) = p.get("dependencies") {
            for d in deps.iter().filter_map(|d| d.as_str()) {
                if let Some(n) = name_of_pep508(d) {
                    m.deps.push(Dep { name: n, path: None, dev: false });
                }
            }
        }
        if let Some(toml::Value::Table(opt)) = p.get("optional-dependencies") {
            for group in opt.values() {
                for d in group.as_array().into_iter().flatten().filter_map(|d| d.as_str()) {
                    if let Some(n) = name_of_pep508(d) {
                        m.deps.push(Dep { name: n, path: None, dev: true });
                    }
                }
            }
        }
        if let Some(toml::Value::Table(scripts)) = p.get("scripts") {
            m.bins.extend(scripts.keys().cloned());
        }
    }
    if let Some(poetry) = v.get("tool").and_then(|t| t.get("poetry")) {
        if m.name.is_empty() {
            m.name = poetry.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_owned();
        }
        if m.version.is_none() {
            m.version = poetry.get("version").and_then(|n| n.as_str()).map(str::to_owned);
        }
        for (field, dev) in [("dependencies", false), ("dev-dependencies", true)] {
            if let Some(toml::Value::Table(deps)) = poetry.get(field) {
                for (name, spec) in deps {
                    if name == "python" {
                        continue;
                    }
                    let path = spec.get("path").and_then(|p| p.as_str()).map(|p| p.replace('\\', "/"));
                    m.deps.push(Dep { name: name.clone(), path, dev });
                }
            }
        }
        if let Some(toml::Value::Table(scripts)) = poetry.get("scripts") {
            m.bins.extend(scripts.keys().cloned());
        }
    }
    if m.name.is_empty() {
        return Err("pyproject.toml: no project name".into());
    }
    m.has_package = true;
    m.import_name = Some(python_import_name(&m.name));
    Ok(m)
}

/// `setup.py` (regex over `name=` / `install_requires`).
pub fn parse_setup_py(src: &str) -> Result<Manifest, String> {
    static NAME: Lazy<Regex> = Lazy::new(|| Regex::new(r#"\bname\s*=\s*['"]([^'"]+)['"]"#).expect("regex"));
    static REQ: Lazy<Regex> = Lazy::new(|| Regex::new(r"install_requires\s*=\s*\[([^\]]*)\]").expect("regex"));
    static ITEM: Lazy<Regex> = Lazy::new(|| Regex::new(r#"['"]([^'"]+)['"]"#).expect("regex"));
    let name = NAME.captures(src).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned()).ok_or("setup.py: no name")?;
    let mut m = Manifest {
        ecosystem: eco::PYTHON,
        import_name: Some(python_import_name(&name)),
        name,
        has_package: true,
        ..Default::default()
    };
    if let Some(list) = REQ.captures(src).and_then(|c| c.get(1)) {
        for item in ITEM.captures_iter(list.as_str()) {
            if let Some(n) = item.get(1).and_then(|s| name_of_pep508(s.as_str())) {
                m.deps.push(Dep { name: n, path: None, dev: false });
            }
        }
    }
    Ok(m)
}

/// `requirements.txt` (dependencies only; the caller names the module).
#[must_use]
pub fn parse_requirements(src: &str) -> Vec<Dep> {
    src.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('-'))
        .filter_map(name_of_pep508)
        .map(|name| Dep { name, path: None, dev: false })
        .collect()
}

/// `go.mod`.
pub fn parse_go_mod(src: &str) -> Result<Manifest, String> {
    static MODULE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\s*module\s+(\S+)").expect("regex"));
    static REPLACE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)^\s*(?:replace\s+)?(\S+)(?:\s+\S+)?\s*=>\s*(\.{1,2}/\S*)").expect("regex"));
    let path = MODULE.captures(src).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned()).ok_or("go.mod: no module")?;
    let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
    let mut m = Manifest { ecosystem: eco::GO, name, import_name: Some(path), has_package: true, ..Default::default() };
    let mut in_block = false;
    for line in src.lines() {
        let l = line.split("//").next().unwrap_or("").trim();
        if l.starts_with("require (") {
            in_block = true;
            continue;
        }
        if in_block {
            if l == ")" {
                in_block = false;
                continue;
            }
            if let Some(dep) = l.split_whitespace().next() {
                m.deps.push(Dep { name: dep.to_owned(), path: None, dev: line.contains("// indirect") });
            }
        } else if let Some(rest) = l.strip_prefix("require ") {
            if let Some(dep) = rest.split_whitespace().next() {
                m.deps.push(Dep { name: dep.to_owned(), path: None, dev: false });
            }
        }
    }
    for c in REPLACE.captures_iter(src) {
        let (Some(name), Some(path)) = (c.get(1), c.get(2)) else { continue };
        if let Some(d) = m.deps.iter_mut().find(|d| d.name == name.as_str()) {
            d.path = Some(path.as_str().to_owned());
        }
    }
    // Indirect requirements are noise in an architecture view.
    m.deps.retain(|d| !d.dev);
    Ok(m)
}

/// `*.csproj` / `*.fsproj` (`name` = file stem).
#[must_use]
pub fn parse_csproj(src: &str, name: &str) -> Manifest {
    static PROJ_REF: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"<ProjectReference\s+Include\s*=\s*"([^"]+)""#).expect("regex"));
    static PKG_REF: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"<PackageReference\s+Include\s*=\s*"([^"]+)""#).expect("regex"));
    static SDK: Lazy<Regex> = Lazy::new(|| Regex::new(r#"<Project\s+Sdk\s*=\s*"([^"]+)""#).expect("regex"));
    let mut m = Manifest {
        ecosystem: eco::DOTNET,
        name: name.to_owned(),
        import_name: Some(name.to_owned()),
        has_package: true,
        ..Default::default()
    };
    for c in PROJ_REF.captures_iter(src) {
        let path = c[1].replace('\\', "/");
        let stem = path.rsplit('/').next().unwrap_or(&path);
        let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s).to_owned();
        let dir = path.rsplit_once('/').map(|(d, _)| d.to_owned()).unwrap_or_else(|| ".".into());
        m.deps.push(Dep { name: stem, path: Some(dir), dev: false });
    }
    for c in PKG_REF.captures_iter(src) {
        m.deps.push(Dep { name: c[1].to_owned(), path: None, dev: false });
    }
    let sdk = SDK.captures(src).map(|c| c[1].to_owned()).unwrap_or_default();
    if sdk.contains(".Web") {
        m.tags.push("web".into());
        m.bins.push(name.to_owned());
    } else if src.contains("<OutputType>Exe</OutputType>") || src.contains("<OutputType>WinExe</OutputType>") {
        m.bins.push(name.to_owned());
    }
    m
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or(s).chars().take(160).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_package_and_workspace() {
        let m = parse_cargo(
            r#"[package]
name = "my-core"
version = "0.2.0"
[dependencies]
serde = "1"
my-domain = { path = "../domain" }
renamed = { package = "real-name", version = "1" }
shared.workspace = true
[dev-dependencies]
tempfile = "3"
[[bin]]
name = "serverd"
path = "src/bin/serverd.rs"
"#,
        )
        .unwrap();
        assert_eq!(m.name, "my-core");
        assert_eq!(m.import_name.as_deref(), Some("my_core"));
        assert_eq!(m.bins, vec!["serverd"]);
        let dom = m.deps.iter().find(|d| d.name == "my-domain").unwrap();
        assert_eq!(dom.path.as_deref(), Some("../domain"));
        assert!(m.deps.iter().any(|d| d.name == "real-name"));
        assert!(m.deps.iter().any(|d| d.name == "shared"));
        assert!(m.deps.iter().find(|d| d.name == "tempfile").unwrap().dev);

        let ws = parse_cargo("[workspace]\nmembers = [\"crates/*\"]\n").unwrap();
        assert!(ws.workspace_root && !ws.has_package);
        assert!(parse_cargo("not toml [").is_err());
    }

    #[test]
    fn package_json() {
        let m = parse_package_json(
            r#"{"name":"@acme/web","version":"1.0.0","bin":{"acme":"cli.js"},"scripts":{"start":"node ."},
               "dependencies":{"react":"^18","@acme/ui":"workspace:*","local":"file:../local"},"devDependencies":{"vitest":"1"}}"#,
            "web",
        )
        .unwrap();
        assert_eq!(m.name, "@acme/web");
        assert_eq!(m.bins, vec!["acme"]);
        assert!(m.tags.contains(&"app".to_owned()));
        assert_eq!(m.deps.iter().find(|d| d.name == "local").unwrap().path.as_deref(), Some("../local"));
        let root = parse_package_json(r#"{"private":true,"workspaces":["apps/*"]}"#, "repo").unwrap();
        assert!(root.workspace_root && !root.has_package);
    }

    #[test]
    fn python_go_dotnet() {
        let p = parse_pyproject(
            "[project]\nname = \"My-Service\"\ndependencies = [\"fastapi>=0.100\", \"SQLAlchemy[asyncio]\"]\n[project.scripts]\nserve = \"x:main\"\n",
        )
        .unwrap();
        assert_eq!(p.import_name.as_deref(), Some("my_service"));
        assert_eq!(p.deps.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), vec!["fastapi", "SQLAlchemy"]);
        assert_eq!(p.bins, vec!["serve"]);
        let poetry = parse_pyproject(
            "[tool.poetry]\nname = \"svc\"\n[tool.poetry.dependencies]\npython = \"^3.11\"\nflask = \"*\"\n",
        )
        .unwrap();
        assert_eq!(poetry.deps.len(), 1);
        assert_eq!(parse_requirements("# c\nrequests==2.0\n-r other.txt\npsycopg2-binary\n").len(), 2);
        let s = parse_setup_py("setup(name='legacy', install_requires=['six', 'redis>=4'])").unwrap();
        assert_eq!(s.deps.len(), 2);

        let g = parse_go_mod(
            "module github.com/acme/api\n\ngo 1.22\n\nrequire (\n\tgithub.com/gin-gonic/gin v1.9.0\n\tgolang.org/x/sys v0.1.0 // indirect\n\tgithub.com/acme/lib v0.0.0\n)\nreplace github.com/acme/lib => ../lib\n",
        )
        .unwrap();
        assert_eq!(g.name, "api");
        assert_eq!(g.deps.len(), 2);
        assert_eq!(g.deps.iter().find(|d| d.name == "github.com/acme/lib").unwrap().path.as_deref(), Some("../lib"));

        let c = parse_csproj(
            r#"<Project Sdk="Microsoft.NET.Sdk.Web"><ItemGroup><ProjectReference Include="..\Core\Core.csproj" /><PackageReference Include="Npgsql" Version="8" /></ItemGroup></Project>"#,
            "Api",
        );
        assert_eq!(c.bins, vec!["Api"]);
        assert_eq!(c.deps[0].name, "Core");
        assert_eq!(c.deps[0].path.as_deref(), Some("../Core"));
        assert_eq!(c.deps[1].name, "Npgsql");
    }
}
