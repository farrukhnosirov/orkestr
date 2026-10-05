//! Builds the architecture model (nodes + edges) from per-file facts.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use orkestr_protocol::methods::architecture::{ApiRoute, ArchEdge, ArchEdgeKind, ArchNode, ArchNodeKind, ArchStats};

use super::facts::{FileFacts, Lang, Role};
use super::manifests::{eco, python_import_name, Dep};

/// Max key files listed per node.
pub const MAX_NODE_FILES: usize = 12;
/// Max evidence labels per edge.
pub const MAX_EDGE_LABELS: usize = 20;
/// Max routes per API node.
pub const MAX_API_ROUTES: usize = 500;

/// Model output.
#[derive(Debug, Clone, Default)]
pub struct Model {
    /// Nodes (sorted by kind, id).
    pub nodes: Vec<ArchNode>,
    /// Edges (sorted by id).
    pub edges: Vec<ArchEdge>,
    /// Problems.
    pub warnings: Vec<String>,
}

impl Model {
    /// Counts (scan fields left at zero).
    #[must_use]
    pub fn stats(&self) -> ArchStats {
        let count =
            |k: ArchNodeKind| u32::try_from(self.nodes.iter().filter(|n| n.kind == k).count()).unwrap_or(u32::MAX);
        ArchStats {
            modules: count(ArchNodeKind::Module),
            services: count(ArchNodeKind::Service),
            databases: count(ArchNodeKind::Database),
            apis: count(ArchNodeKind::Api),
            routes: u32::try_from(self.nodes.iter().map(|n| n.routes.len()).sum::<usize>()).unwrap_or(u32::MAX),
            external_dependencies: u32::try_from(
                self.nodes
                    .iter()
                    .filter(|n| n.kind == ArchNodeKind::External)
                    .map(|n| n.dependencies.len())
                    .sum::<usize>(),
            )
            .unwrap_or(u32::MAX),
            edges: u32::try_from(self.edges.len()).unwrap_or(u32::MAX),
            ..ArchStats::default()
        }
    }
}

/// Parent directory of a relative path (`.` for top level).
#[must_use]
pub fn parent_dir(path: &str) -> String {
    path.rsplit_once('/').map_or_else(|| ".".to_owned(), |(d, _)| d.to_owned())
}

/// Normalises `dir/../x/./y` (relative, `/` separators). `None` when it escapes the root.
#[must_use]
pub fn normalize(path: &str) -> Option<String> {
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
    Some(if parts.is_empty() { ".".to_owned() } else { parts.join("/") })
}

fn join(dir: &str, rel: &str) -> Option<String> {
    if dir == "." {
        normalize(rel)
    } else {
        normalize(&format!("{dir}/{rel}"))
    }
}

fn is_under(path: &str, dir: &str) -> bool {
    dir == "." || path == dir || (path.starts_with(dir) && path.as_bytes().get(dir.len()) == Some(&b'/'))
}

/// Database engine implied by a dependency name or image.
fn engine_of(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    let n = n.rsplit('/').next().unwrap_or(&n);
    let n = n.split([':', '@']).next().unwrap_or(n);
    Some(match n {
        "postgres"
        | "postgis"
        | "pg"
        | "tokio-postgres"
        | "postgresql"
        | "psycopg2"
        | "psycopg2-binary"
        | "psycopg"
        | "asyncpg"
        | "pgx"
        | "npgsql"
        | "npgsql.entityframeworkcore.postgresql"
        | "pq"
        | "pgvector"
        | "timescaledb" => "postgres",
        "mysql" | "mysql2" | "mariadb" | "mysqlclient" | "pymysql" | "mysql_async" | "go-sql-driver" => "mysql",
        "sqlite"
        | "sqlite3"
        | "rusqlite"
        | "better-sqlite3"
        | "libsql"
        | "@libsql"
        | "sql.js"
        | "aiosqlite"
        | "microsoft.data.sqlite"
        | "go-sqlite3" => "sqlite",
        "mongo" | "mongodb" | "mongoose" | "pymongo" | "motor" | "mongo-driver" => "mongodb",
        "redis" | "ioredis" | "redis-py" | "valkey" | "fred" | "go-redis" | "stackexchange.redis" => "redis",
        "elasticsearch" | "opensearch" | "@elastic" => "elasticsearch",
        "memcached" => "memcached",
        "cassandra" | "scylla" | "scylladb" => "cassandra",
        "clickhouse" | "clickhouse-server" => "clickhouse",
        "neo4j" => "neo4j",
        "influxdb" => "influxdb",
        "mssql" | "tedious" | "mcr.microsoft.com" | "sqlserver" | "microsoft.entityframeworkcore.sqlserver" => {
            "sqlserver"
        }
        "couchdb" => "couchdb",
        "dynamodb-local" => "dynamodb",
        "rabbitmq" | "amqplib" | "lapin" | "pika" => "rabbitmq",
        "kafka" | "kafkajs" | "rdkafka" | "confluent-kafka" | "cp-kafka" => "kafka",
        "nats" => "nats",
        _ => return None,
    })
}

fn is_queue(engine: &str) -> bool {
    matches!(engine, "rabbitmq" | "kafka" | "nats")
}

fn external_group_name(ecosystem: &str) -> &'static str {
    match ecosystem {
        eco::CARGO => "crates.io",
        eco::NPM => "npm",
        eco::PYTHON => "PyPI",
        eco::GO => "Go modules",
        eco::DOTNET => "NuGet",
        _ => "external",
    }
}

#[derive(Debug, Clone)]
struct ModuleInfo {
    id: String,
    dir: String,
    ecosystem: &'static str,
    name: String,
    import_name: Option<String>,
    deps: Vec<Dep>,
    manifest_path: Option<String>,
}

#[derive(Default)]
struct Builder {
    nodes: BTreeMap<String, ArchNode>,
    edges: BTreeMap<String, ArchEdge>,
    warnings: Vec<String>,
}

impl Builder {
    fn add_node(&mut self, node: ArchNode) -> String {
        let id = node.id.clone();
        self.nodes.entry(id.clone()).or_insert(node);
        id
    }

    fn add_edge(&mut self, from: &str, to: &str, kind: ArchEdgeKind, label: Option<&str>, weight: u32) {
        if from == to || !self.nodes.contains_key(from) || !self.nodes.contains_key(to) {
            return;
        }
        let id = format!("{from}->{to}:{}", edge_kind_tag(kind));
        let e = self.edges.entry(id.clone()).or_insert_with(|| ArchEdge {
            id,
            from: from.to_owned(),
            to: to.to_owned(),
            kind,
            weight: 0,
            labels: Vec::new(),
        });
        e.weight = e.weight.saturating_add(weight.max(1));
        if let Some(l) = label {
            if e.labels.len() < MAX_EDGE_LABELS && !e.labels.iter().any(|x| x == l) {
                e.labels.push(l.to_owned());
            }
        }
    }
}

fn edge_kind_tag(kind: ArchEdgeKind) -> &'static str {
    match kind {
        ArchEdgeKind::DependsOn => "dependsOn",
        ArchEdgeKind::Imports => "imports",
        ArchEdgeKind::Uses => "uses",
        ArchEdgeKind::Exposes => "exposes",
        ArchEdgeKind::Runs => "runs",
        ArchEdgeKind::External => "external",
    }
}

fn node(id: String, kind: ArchNodeKind, name: &str, ecosystem: &str, path: Option<String>) -> ArchNode {
    ArchNode {
        id,
        kind,
        name: name.to_owned(),
        ecosystem: ecosystem.to_owned(),
        path,
        detail: None,
        files: Vec::new(),
        file_count: 0,
        tags: Vec::new(),
        routes: Vec::new(),
        dependencies: Vec::new(),
    }
}

fn push_file(n: &mut ArchNode, f: &str) {
    if n.files.len() < MAX_NODE_FILES && !n.files.iter().any(|x| x == f) {
        n.files.push(f.to_owned());
    }
}

fn test_like(dir: &str) -> bool {
    dir.split('/')
        .any(|s| matches!(s, "tests" | "test" | "fixtures" | "__tests__" | "examples" | "example" | "testdata"))
}

/// Builds the model.
#[must_use]
pub fn build(files: &[String], facts: &BTreeMap<String, Arc<FileFacts>>, root_name: &str) -> Model {
    let mut b = Builder::default();
    let file_set: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    for f in facts.values() {
        b.warnings.extend(f.warnings.iter().cloned());
    }

    // ---- modules from manifests ------------------------------------------
    let mut modules: Vec<ModuleInfo> = Vec::new();
    let mut used_ids: HashMap<String, usize> = HashMap::new();
    let mut python_dirs_with_manifest: BTreeSet<String> = BTreeSet::new();
    for (path, f) in facts {
        let Some(m) = &f.manifest else { continue };
        let dir = parent_dir(path);
        if m.ecosystem == eco::PYTHON {
            python_dirs_with_manifest.insert(dir.clone());
        }
        if !m.has_package {
            continue;
        }
        let mut id = format!("module:{}:{}", m.ecosystem, m.name);
        let n = used_ids.entry(id.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            id = format!("{id}@{dir}");
        }
        // pyproject + setup.py in the same dir describe one package.
        if modules.iter().any(|x| x.dir == dir && x.ecosystem == m.ecosystem) {
            continue;
        }
        let mut n = node(id.clone(), ArchNodeKind::Module, &m.name, m.ecosystem, Some(dir.clone()));
        n.detail = m.version.clone();
        push_file(&mut n, path);
        n.tags.extend(m.tags.iter().cloned());
        if m.workspace_root {
            n.tags.push("workspace-root".into());
        }
        if test_like(&dir) {
            n.tags.push("test".into());
        }
        b.add_node(n);
        modules.push(ModuleInfo {
            id,
            dir,
            ecosystem: m.ecosystem,
            name: m.name.clone(),
            import_name: m.import_name.clone(),
            deps: m.deps.clone(),
            manifest_path: Some(path.clone()),
        });
    }
    // requirements.txt without another Python manifest → a module named after its dir.
    for (path, f) in facts {
        if f.role != Role::Requirements {
            continue;
        }
        let dir = parent_dir(path);
        if python_dirs_with_manifest.contains(&dir) {
            continue;
        }
        let name = if dir == "." { root_name.to_owned() } else { dir.rsplit('/').next().unwrap_or(&dir).to_owned() };
        let id = format!("module:python:{name}@{dir}");
        let mut n = node(id.clone(), ArchNodeKind::Module, &name, eco::PYTHON, Some(dir.clone()));
        push_file(&mut n, path);
        b.add_node(n);
        python_dirs_with_manifest.insert(dir.clone());
        modules.push(ModuleInfo {
            id,
            dir,
            ecosystem: eco::PYTHON,
            import_name: Some(python_import_name(&name)),
            name,
            deps: f.requirements.clone(),
            manifest_path: None,
        });
    }

    // ---- directory fallback -----------------------------------------------
    let source_files: Vec<(&str, Lang)> = facts
        .iter()
        .filter_map(|(p, f)| match f.role {
            Role::Source(l) => Some((p.as_str(), l)),
            _ => None,
        })
        .collect();
    if modules.is_empty() {
        let mut dirs: BTreeSet<String> = BTreeSet::new();
        for (p, _) in &source_files {
            dirs.insert(p.split_once('/').map_or_else(|| ".".to_owned(), |(d, _)| d.to_owned()));
        }
        for dir in dirs {
            let name = if dir == "." { root_name.to_owned() } else { dir.clone() };
            let id = format!("module:dir:{dir}");
            let mut n = node(id.clone(), ArchNodeKind::Module, &name, eco::DIR, Some(dir.clone()));
            if test_like(&dir) {
                n.tags.push("test".into());
            }
            b.add_node(n);
            modules.push(ModuleInfo {
                id,
                import_name: Some(python_import_name(&name)),
                name,
                dir,
                ecosystem: eco::DIR,
                deps: Vec::new(),
                manifest_path: None,
            });
        }
    }
    // Deepest module first, for ownership lookups.
    let mut by_depth: Vec<usize> = (0..modules.len()).collect();
    by_depth.sort_by_key(|&i| {
        std::cmp::Reverse(if modules[i].dir == "." { 0 } else { modules[i].dir.matches('/').count() + 1 })
    });
    let owner = |path: &str| -> Option<usize> { by_depth.iter().copied().find(|&i| is_under(path, &modules[i].dir)) };

    // File counts.
    let mut counts: HashMap<usize, u32> = HashMap::new();
    for (p, _) in &source_files {
        if let Some(i) = owner(p) {
            *counts.entry(i).or_default() += 1;
        }
    }
    for (i, m) in modules.iter().enumerate() {
        if let Some(n) = b.nodes.get_mut(&m.id) {
            n.file_count = counts.get(&i).copied().unwrap_or(0);
        }
    }

    // Python package dirs (`pkg/__init__.py`) → import names of their owner.
    let mut python_packages: HashMap<String, usize> = HashMap::new();
    for f in files {
        if let Some(dir) = f.strip_suffix("/__init__.py") {
            let pkg = dir.rsplit('/').next().unwrap_or(dir);
            if let Some(i) = owner(f) {
                python_packages.entry(pkg.to_owned()).or_insert(i);
            }
        }
    }

    // ---- services: binaries ------------------------------------------------
    for m in &modules {
        let manifest = m
            .manifest_path
            .as_ref()
            .and_then(|p| facts.get(p).and_then(|f| f.manifest.as_ref().map(|x| (p.clone(), x))));
        let mut bins: Vec<(String, Option<String>)> = Vec::new();
        if let Some((_, man)) = &manifest {
            bins.extend(man.bins.iter().map(|b| (b.clone(), None)));
        }
        if m.ecosystem == eco::CARGO {
            let prefix = if m.dir == "." { String::new() } else { format!("{}/", m.dir) };
            let main = format!("{prefix}src/main.rs");
            if file_set.contains(main.as_str()) && !bins.iter().any(|(b, _)| b == &m.name) {
                bins.push((m.name.clone(), Some(main)));
            }
            let bin_dir = format!("{prefix}src/bin/");
            for f in file_set.range(bin_dir.as_str()..) {
                if !f.starts_with(&bin_dir) {
                    break;
                }
                let rest = &f[bin_dir.len()..];
                let name = rest.strip_suffix(".rs").or_else(|| rest.strip_suffix("/main.rs"));
                if let Some(name) = name.filter(|n| !n.contains('/')) {
                    if let Some(existing) = bins.iter_mut().find(|(b, _)| b == name) {
                        existing.1 = Some((*f).to_owned());
                    } else {
                        bins.push((name.to_owned(), Some((*f).to_owned())));
                    }
                }
            }
        }
        let is_app = manifest.as_ref().is_some_and(|(_, man)| man.tags.iter().any(|t| t == "app"));
        if bins.is_empty() && is_app {
            bins.push((m.name.clone(), None));
        }
        for (bin, entry) in bins {
            let id = format!("service:{}:{}:{}", m.ecosystem, m.name, bin);
            let mut n = node(id.clone(), ArchNodeKind::Service, &bin, m.ecosystem, Some(m.dir.clone()));
            n.tags.push(if is_app && m.ecosystem == eco::NPM { "app".into() } else { "binary".into() });
            if let Some(e) = entry {
                push_file(&mut n, &e);
            }
            if let Some((p, man)) = &manifest {
                push_file(&mut n, p);
                if man.tags.iter().any(|t| t == "web") {
                    n.tags.push("web".into());
                }
            }
            b.add_node(n);
            b.add_edge(&id, &m.id, ArchEdgeKind::Runs, None, 1);
            if let Some(mn) = b.nodes.get_mut(&m.id) {
                if !mn.tags.iter().any(|t| t == "binary") {
                    mn.tags.push("binary".into());
                }
            }
        }
    }
    // Go `package main` directories.
    let mut go_mains: BTreeSet<String> = BTreeSet::new();
    for (p, f) in facts {
        if f.go_main {
            go_mains.insert(parent_dir(p));
        }
    }
    for dir in go_mains {
        let name = if dir == "." { root_name.to_owned() } else { dir.rsplit('/').next().unwrap_or(&dir).to_owned() };
        let id = format!("service:go:{dir}");
        let mut n = node(id.clone(), ArchNodeKind::Service, &name, eco::GO, Some(dir.clone()));
        n.tags.push("binary".into());
        b.add_node(n);
        if let Some(i) = owner(&format!("{dir}/main.go")) {
            let mid = modules[i].id.clone();
            b.add_edge(&id, &mid, ArchEdgeKind::Runs, None, 1);
        }
    }
    // Dockerfiles.
    let mut docker_by_dir: HashMap<String, String> = HashMap::new();
    for (p, f) in facts {
        if f.role != Role::Dockerfile {
            continue;
        }
        let dir = parent_dir(p);
        let fname = p.rsplit('/').next().unwrap_or(p);
        let base = if dir == "." { root_name.to_owned() } else { dir.rsplit('/').next().unwrap_or(&dir).to_owned() };
        let name = match fname.to_ascii_lowercase().as_str() {
            "dockerfile" | "containerfile" => base,
            other => other.trim_start_matches("dockerfile.").trim_end_matches(".dockerfile").to_owned(),
        };
        let id = format!("service:docker:{p}");
        let mut n = node(id.clone(), ArchNodeKind::Service, &name, "docker", Some(dir.clone()));
        n.detail.clone_from(&f.docker_from);
        n.tags.push("container".into());
        push_file(&mut n, p);
        b.add_node(n);
        docker_by_dir.entry(dir.clone()).or_insert_with(|| id.clone());
        let target = modules.iter().find(|m| m.dir == dir).map(|m| m.id.clone()).or_else(|| {
            // A root Dockerfile builds the root module only when one exists.
            if dir == "." {
                None
            } else {
                owner(&format!("{dir}/Dockerfile")).map(|i| modules[i].id.clone())
            }
        });
        if let Some(t) = target {
            b.add_edge(&id, &t, ArchEdgeKind::Runs, None, 1);
        }
    }

    // ---- databases ----------------------------------------------------------
    let mut engine_nodes: HashMap<&'static str, String> = HashMap::new();
    let mut compose_ids: HashMap<String, String> = HashMap::new();
    let mut compose_pending: Vec<(String, Vec<String>, Option<String>, String)> = Vec::new();
    for (p, f) in facts {
        if f.role != Role::Compose {
            continue;
        }
        let dir = parent_dir(p);
        for s in &f.compose {
            let engine = s.image.as_deref().and_then(engine_of);
            let (id, kind) = match engine {
                Some(_) if s.build.is_none() => (format!("db:compose:{}", s.name), ArchNodeKind::Database),
                _ => (format!("service:compose:{}", s.name), ArchNodeKind::Service),
            };
            let mut n = node(id.clone(), kind, &s.name, "compose", Some(dir.clone()));
            n.detail = s.image.clone().or_else(|| s.build.as_ref().map(|b| format!("build: {b}")));
            push_file(&mut n, p);
            if let Some(e) = engine {
                n.tags.push(e.to_owned());
                if is_queue(e) {
                    n.tags.push("queue".into());
                }
                if kind == ArchNodeKind::Database {
                    engine_nodes.entry(e).or_insert_with(|| id.clone());
                }
            }
            if !s.ports.is_empty() {
                n.tags.push(format!("ports: {}", s.ports.join(", ")));
            }
            b.add_node(n);
            compose_ids.insert(s.name.clone(), id.clone());
            compose_pending.push((
                id,
                s.depends_on.clone(),
                s.build.as_ref().and_then(|bd| join(&dir, bd)),
                dir.clone(),
            ));
        }
    }
    for (id, deps, build_dir, _) in &compose_pending {
        for d in deps {
            if let Some(t) = compose_ids.get(d).cloned() {
                let kind = if t.starts_with("db:") { ArchEdgeKind::Uses } else { ArchEdgeKind::DependsOn };
                b.add_edge(id, &t, kind, None, 1);
            }
        }
        if let Some(bd) = build_dir {
            let target = docker_by_dir
                .get(bd)
                .cloned()
                .or_else(|| modules.iter().find(|m| &m.dir == bd).map(|m| m.id.clone()))
                .or_else(|| owner(&format!("{bd}/x")).filter(|_| bd != ".").map(|i| modules[i].id.clone()));
            if let Some(t) = target {
                b.add_edge(id, &t, ArchEdgeKind::Runs, None, 1);
            }
        }
    }
    let mut ensure_engine = |b: &mut Builder, engine: &'static str| -> String {
        engine_nodes
            .entry(engine)
            .or_insert_with(|| {
                let id = format!("db:engine:{engine}");
                let mut n = node(id.clone(), ArchNodeKind::Database, engine, "inferred", None);
                n.tags.push("inferred".into());
                if is_queue(engine) {
                    n.tags.push("queue".into());
                }
                b.add_node(n);
                id
            })
            .clone()
    };
    for m in &modules {
        for d in m.deps.iter().filter(|d| !d.dev) {
            if let Some(engine) = engine_of(&d.name) {
                let id = ensure_engine(&mut b, engine);
                b.add_edge(&m.id, &id, ArchEdgeKind::Uses, Some(&d.name), 1);
            }
        }
    }
    for (p, f) in facts {
        let engine = if f.uses_sqlite {
            Some("sqlite")
        } else {
            f.prisma_provider.as_deref().and_then(|pp| engine_of(if pp == "postgresql" { "postgres" } else { pp }))
        };
        if let (Some(engine), Some(i)) = (engine, owner(p)) {
            let id = ensure_engine(&mut b, engine);
            let mid = modules[i].id.clone();
            b.add_edge(&mid, &id, ArchEdgeKind::Uses, Some(p), 1);
        }
    }
    // Migration folders.
    let mut migration_dirs: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (p, f) in facts {
        if f.role != Role::Migration {
            continue;
        }
        let segs: Vec<&str> = p.split('/').collect();
        let cut = segs.iter().position(|s| {
            matches!(s.to_ascii_lowercase().as_str(), "migrations" | "migration" | "migrate" | "alembic")
        });
        if let Some(cut) = cut {
            migration_dirs.entry(segs[..=cut].join("/")).or_default().push(p);
        }
    }
    for (dir, list) in migration_dirs {
        let id = format!("db:migrations:{dir}");
        let mut n = node(id.clone(), ArchNodeKind::Database, &dir, "migrations", Some(dir.clone()));
        n.file_count = u32::try_from(list.len()).unwrap_or(u32::MAX);
        for f in list.iter().take(MAX_NODE_FILES) {
            push_file(&mut n, f);
        }
        n.tags.push("migrations".into());
        b.add_node(n);
        if let Some(i) = owner(&format!("{dir}/x")) {
            let mid = modules[i].id.clone();
            b.add_edge(&mid, &id, ArchEdgeKind::Uses, None, 1);
        }
    }

    // ---- APIs -----------------------------------------------------------------
    let mut routes_by_module: BTreeMap<usize, Vec<ApiRoute>> = BTreeMap::new();
    let mut orphan_routes: Vec<ApiRoute> = Vec::new();
    for (p, f) in facts {
        if f.routes.is_empty() {
            continue;
        }
        if f.role == Role::OpenApi {
            let id = format!("api:openapi:{p}");
            let name = f.title.clone().unwrap_or_else(|| p.rsplit('/').next().unwrap_or(p).to_owned());
            let mut n = node(id.clone(), ArchNodeKind::Api, &name, "openapi", Some(parent_dir(p)));
            n.routes = f.routes.clone();
            n.routes.sort();
            n.routes.dedup();
            push_file(&mut n, p);
            n.detail = Some("OpenAPI".into());
            b.add_node(n);
            if let Some(i) = owner(p) {
                let mid = modules[i].id.clone();
                b.add_edge(&mid, &id, ArchEdgeKind::Exposes, None, 1);
            }
            continue;
        }
        match owner(p) {
            Some(i) => routes_by_module.entry(i).or_default().extend(f.routes.iter().cloned()),
            None => orphan_routes.extend(f.routes.iter().cloned()),
        }
    }
    let add_api = |b: &mut Builder,
                   id: String,
                   name: &str,
                   dir: Option<String>,
                   mut routes: Vec<ApiRoute>,
                   owner_id: Option<String>| {
        routes.sort();
        routes.dedup();
        routes.truncate(MAX_API_ROUTES);
        let mut frameworks: Vec<String> = routes.iter().map(|r| r.framework.clone()).collect();
        frameworks.sort();
        frameworks.dedup();
        let mut n = node(id.clone(), ArchNodeKind::Api, name, "http", dir);
        n.detail = Some(frameworks.join(", "));
        let mut files: Vec<String> = routes.iter().map(|r| r.file.clone()).collect();
        files.sort();
        files.dedup();
        for f in files {
            push_file(&mut n, &f);
        }
        n.routes = routes;
        b.add_node(n);
        if let Some(o) = owner_id {
            b.add_edge(&o, &id, ArchEdgeKind::Exposes, None, 1);
        }
    };
    for (i, routes) in routes_by_module {
        let m = &modules[i];
        add_api(&mut b, format!("api:{}", m.id), &m.name, Some(m.dir.clone()), routes, Some(m.id.clone()));
    }
    if !orphan_routes.is_empty() {
        add_api(&mut b, "api:root".into(), root_name, Some(".".into()), orphan_routes, None);
    }

    // ---- internal dependencies ------------------------------------------------
    let by_dir: HashMap<(&str, &str), usize> =
        modules.iter().enumerate().map(|(i, m)| ((m.dir.as_str(), m.ecosystem), i)).collect();
    let mut by_name: HashMap<(&str, String), usize> = HashMap::new();
    for (i, m) in modules.iter().enumerate() {
        let key = match m.ecosystem {
            eco::PYTHON => python_import_name(&m.name),
            eco::CARGO => m.name.replace('_', "-"),
            _ => m.name.clone(),
        };
        by_name.entry((m.ecosystem, key)).or_insert(i);
    }
    let mut externals: BTreeMap<&'static str, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let mut declared: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (i, m) in modules.iter().enumerate() {
        for d in &m.deps {
            let target = d
                .path
                .as_deref()
                .and_then(|p| join(&m.dir, p))
                .and_then(|dir| by_dir.get(&(dir.as_str(), m.ecosystem)).copied())
                .or_else(|| {
                    let key = match m.ecosystem {
                        eco::PYTHON => python_import_name(&d.name),
                        eco::CARGO => d.name.replace('_', "-"),
                        _ => d.name.clone(),
                    };
                    by_name.get(&(m.ecosystem, key)).copied()
                });
            match target {
                Some(t) if t != i => {
                    declared.insert((i, t));
                    let tid = modules[t].id.clone();
                    b.add_edge(&m.id, &tid, ArchEdgeKind::DependsOn, Some(&d.name), 1);
                }
                Some(_) => {}
                None => {
                    if m.ecosystem != eco::DIR {
                        externals.entry(m.ecosystem).or_default().entry(m.id.clone()).or_default().push(d.name.clone());
                    }
                }
            }
        }
    }

    // ---- imports -----------------------------------------------------------------
    let mut import_hits: BTreeMap<(usize, usize), (u32, Vec<String>)> = BTreeMap::new();
    let go_modules: Vec<(String, usize)> = modules
        .iter()
        .enumerate()
        .filter(|(_, m)| m.ecosystem == eco::GO)
        .filter_map(|(i, m)| m.import_name.clone().map(|n| (n, i)))
        .collect();
    let rust_crates: HashMap<String, usize> = modules
        .iter()
        .enumerate()
        .filter(|(_, m)| m.ecosystem == eco::CARGO)
        .filter_map(|(i, m)| m.import_name.clone().map(|n| (n, i)))
        .collect();
    let npm_names: HashMap<&str, usize> = modules
        .iter()
        .enumerate()
        .filter(|(_, m)| m.ecosystem == eco::NPM)
        .map(|(i, m)| (m.name.as_str(), i))
        .collect();
    let dotnet: Vec<(&str, usize)> = modules
        .iter()
        .enumerate()
        .filter(|(_, m)| m.ecosystem == eco::DOTNET)
        .map(|(i, m)| (m.name.as_str(), i))
        .collect();
    let python_names: HashMap<String, usize> = modules
        .iter()
        .enumerate()
        .filter(|(_, m)| matches!(m.ecosystem, eco::PYTHON | eco::DIR))
        .filter_map(|(i, m)| m.import_name.clone().map(|n| (n, i)))
        .collect();
    for (p, f) in facts {
        let Role::Source(lang) = f.role else { continue };
        if f.imports.is_empty() {
            continue;
        }
        let Some(from) = owner(p) else { continue };
        let dir = parent_dir(p);
        for imp in &f.imports {
            let target = match lang {
                Lang::Rust => rust_crates.get(imp.as_str()).copied(),
                Lang::JsTs => {
                    if imp.starts_with("./") || imp.starts_with("../") {
                        join(&dir, imp).and_then(|resolved| owner(&resolved))
                    } else {
                        let pkg = if imp.starts_with('@') {
                            imp.splitn(3, '/').take(2).collect::<Vec<_>>().join("/")
                        } else {
                            imp.split('/').next().unwrap_or(imp).to_owned()
                        };
                        npm_names.get(pkg.as_str()).copied()
                    }
                }
                Lang::Python => {
                    if imp.starts_with('.') {
                        None
                    } else {
                        let first = imp.split('.').next().unwrap_or(imp);
                        python_packages.get(first).copied().or_else(|| python_names.get(first).copied())
                    }
                }
                Lang::Go => go_modules
                    .iter()
                    .filter(|(name, _)| imp == name || imp.starts_with(&format!("{name}/")))
                    .max_by_key(|(name, _)| name.len())
                    .map(|(_, i)| *i),
                Lang::CSharp => dotnet
                    .iter()
                    .filter(|(name, _)| imp == name || imp.starts_with(&format!("{name}.")))
                    .max_by_key(|(name, _)| name.len())
                    .map(|(_, i)| *i),
                Lang::Java | Lang::Other => None,
            };
            if let Some(t) = target.filter(|&t| t != from) {
                let e = import_hits.entry((from, t)).or_default();
                e.0 += 1;
                if e.1.len() < 5 && !e.1.contains(imp) {
                    e.1.push(imp.clone());
                }
            }
        }
    }
    for ((from, to), (count, samples)) in import_hits {
        let fid = modules[from].id.clone();
        let tid = modules[to].id.clone();
        if declared.contains(&(from, to)) {
            // Declared dependency: imports only strengthen the edge.
            if let Some(e) = b.edges.get_mut(&format!("{fid}->{tid}:dependsOn")) {
                e.weight = e.weight.saturating_add(count);
            }
            continue;
        }
        b.add_edge(&fid, &tid, ArchEdgeKind::Imports, None, count);
        if let Some(e) = b.edges.get_mut(&format!("{fid}->{tid}:imports")) {
            for s in samples {
                if e.labels.len() < MAX_EDGE_LABELS && !e.labels.contains(&s) {
                    e.labels.push(s);
                }
            }
        }
    }

    // ---- external groups -----------------------------------------------------------
    for (ecosystem, per_module) in externals {
        let id = format!("external:{ecosystem}");
        let mut all: BTreeSet<String> = BTreeSet::new();
        for deps in per_module.values() {
            all.extend(deps.iter().cloned());
        }
        let mut n = node(id.clone(), ArchNodeKind::External, external_group_name(ecosystem), ecosystem, None);
        n.dependencies = all.into_iter().collect();
        b.add_node(n);
        for (mid, mut deps) in per_module {
            deps.sort();
            deps.dedup();
            let weight = u32::try_from(deps.len()).unwrap_or(u32::MAX);
            b.add_edge(&mid, &id, ArchEdgeKind::External, None, weight);
            if let Some(e) = b.edges.get_mut(&format!("{mid}->{id}:external")) {
                e.labels = deps.into_iter().take(MAX_EDGE_LABELS).collect();
            }
        }
    }

    for n in b.nodes.values_mut() {
        n.tags.sort();
        n.tags.dedup();
    }
    let mut nodes: Vec<ArchNode> = b.nodes.into_values().collect();
    nodes.sort_by(|a, c| a.kind.cmp(&c.kind).then_with(|| a.id.cmp(&c.id)));
    let edges: Vec<ArchEdge> = b.edges.into_values().collect();
    let mut warnings = b.warnings;
    warnings.sort();
    warnings.dedup();
    Model { nodes, edges, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_helpers() {
        assert_eq!(normalize("a/./b/../c").as_deref(), Some("a/c"));
        assert_eq!(normalize("a/..").as_deref(), Some("."));
        assert_eq!(normalize("../x"), None);
        assert_eq!(join("crates/core", "../domain").as_deref(), Some("crates/domain"));
        assert_eq!(parent_dir("Cargo.toml"), ".");
        assert!(is_under("crates/core/src/lib.rs", "crates/core"));
        assert!(!is_under("crates/core2/src/lib.rs", "crates/core"));
        assert!(is_under("anything", "."));
    }

    #[test]
    fn engines() {
        assert_eq!(engine_of("postgres:16-alpine"), Some("postgres"));
        assert_eq!(engine_of("bitnami/redis:7"), Some("redis"));
        assert_eq!(engine_of("rusqlite"), Some("sqlite"));
        assert_eq!(engine_of("serde"), None);
    }
}
