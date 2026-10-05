//! HTTP route heuristics per language / framework and OpenAPI documents.
//!
//! Heuristics are line based and deliberately conservative: a route needs a
//! string literal path that starts with `/` (or is empty for prefixed
//! controller methods) next to a recognisable router / decorator call.

use once_cell::sync::Lazy;
use orkestr_protocol::methods::architecture::ApiRoute;
use regex::Regex;

use super::facts::Lang;
use super::yaml::{self, Yaml};

/// Max routes kept per file.
pub const MAX_ROUTES_PER_FILE: usize = 500;

const HTTP_METHODS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options"];

fn re(s: &str) -> Regex {
    Regex::new(s).expect("valid route regex")
}

fn route(method: &str, path: &str, file: &str, line: usize, framework: &str) -> ApiRoute {
    ApiRoute {
        method: method.to_ascii_uppercase(),
        path: path.to_owned(),
        file: file.to_owned(),
        line: u32::try_from(line + 1).unwrap_or(u32::MAX),
        framework: framework.to_owned(),
    }
}

fn join_prefix(prefix: &str, path: &str) -> String {
    let p = prefix.trim_end_matches('/');
    let s = path.trim_start_matches('/');
    match (p.is_empty(), s.is_empty()) {
        (true, true) => "/".into(),
        (true, false) => format!("/{s}"),
        (false, true) => if p.starts_with('/') { p.to_owned() } else { format!("/{p}") },
        (false, false) => {
            if p.starts_with('/') {
                format!("{p}/{s}")
            } else {
                format!("/{p}/{s}")
            }
        }
    }
}

/// Routes declared in a source file.
#[must_use]
pub fn extract(lang: Lang, file: &str, text: &str) -> Vec<ApiRoute> {
    let mut out = match lang {
        Lang::Rust => rust(file, text),
        Lang::JsTs => js(file, text),
        Lang::Python => python(file, text),
        Lang::CSharp => csharp(file, text),
        Lang::Go => go(file, text),
        Lang::Java => java(file, text),
        Lang::Other => Vec::new(),
    };
    out.truncate(MAX_ROUTES_PER_FILE);
    out
}

fn rust(file: &str, text: &str) -> Vec<ApiRoute> {
    static AXUM: Lazy<Regex> = Lazy::new(|| re(r#"\.route\(\s*"(/[^"]*)"\s*,(.*)"#));
    static AXUM_METHOD: Lazy<Regex> =
        Lazy::new(|| re(r"\b(get|post|put|delete|patch|head|options|any)(?:_service)?\s*\("));
    static ACTIX: Lazy<Regex> = Lazy::new(|| re(r#"#\[(get|post|put|delete|patch|head|options)\(\s*"(/[^"]*)""#));
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if let Some(c) = AXUM.captures(line) {
            let rest = c.get(2).map_or("", |m| m.as_str());
            let methods: Vec<&str> = AXUM_METHOD.captures_iter(rest).filter_map(|m| m.get(1)).map(|m| m.as_str()).collect();
            if methods.is_empty() {
                out.push(route("ANY", &c[1], file, i, "axum"));
            }
            for m in methods {
                out.push(route(if m == "any" { "ANY" } else { m }, &c[1], file, i, "axum"));
            }
        } else if let Some(c) = ACTIX.captures(line) {
            out.push(route(&c[1], &c[2], file, i, "actix"));
        }
    }
    out
}

fn js(file: &str, text: &str) -> Vec<ApiRoute> {
    static EXPRESS: Lazy<Regex> = Lazy::new(|| {
        re(r#"\b(?:app|router|server|api|routes|r|fastify|hono)\.(get|post|put|delete|patch|head|options|all)\(\s*['"`](/[^'"`]*)['"`]"#)
    });
    static NEST_CTRL: Lazy<Regex> = Lazy::new(|| re(r#"@Controller\(\s*(?:['"`]([^'"`]*)['"`])?\s*\)"#));
    static NEST: Lazy<Regex> = Lazy::new(|| re(r#"@(Get|Post|Put|Delete|Patch|Head|Options|All)\(\s*(?:['"`]([^'"`]*)['"`])?\s*\)"#));
    static NEXT_HANDLER: Lazy<Regex> =
        Lazy::new(|| re(r"export\s+(?:async\s+)?(?:function|const)\s+(GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS)\b"));
    let mut out = Vec::new();
    let mut prefix = String::new();
    // Next.js app router: `app/**/route.ts` exports GET/POST/… handlers.
    let next_path = next_route_path(file);
    for (i, line) in text.lines().enumerate() {
        if let Some(c) = EXPRESS.captures(line) {
            let m = if &c[1] == "all" { "ANY" } else { &c[1] };
            out.push(route(m, &c[2], file, i, "express"));
            continue;
        }
        if let Some(c) = NEST_CTRL.captures(line) {
            prefix = c.get(1).map_or(String::new(), |m| m.as_str().to_owned());
            continue;
        }
        if let Some(c) = NEST.captures(line) {
            let m = if &c[1] == "All" { "ANY" } else { &c[1] };
            let path = c.get(2).map_or("", |m| m.as_str());
            out.push(route(m, &join_prefix(&prefix, path), file, i, "nestjs"));
            continue;
        }
        if let (Some(path), Some(c)) = (next_path.as_deref(), NEXT_HANDLER.captures(line)) {
            out.push(route(&c[1], path, file, i, "nextjs"));
        }
    }
    out
}

/// `app/users/[id]/route.ts` → `/users/[id]`.
fn next_route_path(file: &str) -> Option<String> {
    let stem = file.rsplit('/').next()?;
    if !(stem.starts_with("route.") && (stem.ends_with(".ts") || stem.ends_with(".js"))) {
        return None;
    }
    let dir = file.rsplit_once('/')?.0;
    let idx = dir.find("app/").map(|i| i + 4).or_else(|| (dir == "app" || dir.ends_with("/app")).then_some(dir.len()))?;
    let rest = &dir[idx.min(dir.len())..];
    // Route groups `(group)` do not appear in URLs.
    let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty() && !(s.starts_with('(') && s.ends_with(')'))).collect();
    Some(format!("/{}", segs.join("/")))
}

fn python(file: &str, text: &str) -> Vec<ApiRoute> {
    static FASTAPI: Lazy<Regex> = Lazy::new(|| {
        re(r#"@\w+\.(get|post|put|delete|patch|head|options|api_route|websocket)\(\s*['"]([^'"]*)['"]"#)
    });
    static FLASK: Lazy<Regex> = Lazy::new(|| re(r#"@\w+\.route\(\s*['"]([^'"]*)['"](.*)"#));
    static FLASK_METHODS: Lazy<Regex> = Lazy::new(|| re(r#"['"](GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS)['"]"#));
    static ROUTER_PREFIX: Lazy<Regex> = Lazy::new(|| re(r#"APIRouter\(.*prefix\s*=\s*['"]([^'"]*)['"]"#));
    static DJANGO: Lazy<Regex> = Lazy::new(|| re(r#"\b(?:re_)?path\(\s*r?['"]([^'"]*)['"]"#));
    let mut out = Vec::new();
    let mut prefix = String::new();
    let django = file.ends_with("urls.py");
    for (i, line) in text.lines().enumerate() {
        if let Some(c) = ROUTER_PREFIX.captures(line) {
            prefix = c[1].to_owned();
        }
        if let Some(c) = FASTAPI.captures(line) {
            let m = match &c[1] {
                "api_route" => "ANY",
                "websocket" => "WS",
                other => other,
            };
            out.push(route(m, &join_prefix(&prefix, &c[2]), file, i, "fastapi"));
        } else if let Some(c) = FLASK.captures(line) {
            let rest = c.get(2).map_or("", |m| m.as_str());
            let methods: Vec<String> = FLASK_METHODS.captures_iter(rest).map(|m| m[1].to_owned()).collect();
            if methods.is_empty() {
                out.push(route("GET", &c[1], file, i, "flask"));
            }
            for m in methods {
                out.push(route(&m, &c[1], file, i, "flask"));
            }
        } else if django {
            if let Some(c) = DJANGO.captures(line) {
                out.push(route("ANY", &join_prefix("", &c[1]), file, i, "django"));
            }
        }
    }
    out
}

fn csharp(file: &str, text: &str) -> Vec<ApiRoute> {
    static CLASS: Lazy<Regex> = Lazy::new(|| re(r"\bclass\s+(\w+?)(?:Controller)?\b"));
    static ROUTE_ATTR: Lazy<Regex> = Lazy::new(|| re(r#"^\s*\[Route\(\s*"([^"]*)"\s*\)\]"#));
    static HTTP_ATTR: Lazy<Regex> = Lazy::new(|| re(r#"\[Http(Get|Post|Put|Delete|Patch|Head|Options)(?:\(\s*"([^"]*)"\s*\))?\]"#));
    static MINIMAL: Lazy<Regex> = Lazy::new(|| re(r#"\.Map(Get|Post|Put|Delete|Patch)\(\s*"([^"]*)""#));
    let mut out = Vec::new();
    let mut pending_route: Option<String> = None;
    let mut prefix = String::new();
    for (i, line) in text.lines().enumerate() {
        if let Some(c) = MINIMAL.captures(line) {
            out.push(route(&c[1], &join_prefix("", &c[2]), file, i, "aspnet"));
            continue;
        }
        if let Some(c) = ROUTE_ATTR.captures(line) {
            pending_route = Some(c[1].to_owned());
            continue;
        }
        if let Some(c) = CLASS.captures(line) {
            let controller = c[1].to_owned();
            prefix = pending_route.take().unwrap_or_default().replace("[controller]", &controller.to_ascii_lowercase());
            continue;
        }
        if let Some(c) = HTTP_ATTR.captures(line) {
            let path = c.get(2).map_or("", |m| m.as_str());
            let full = if path.starts_with('/') { join_prefix("", path) } else { join_prefix(&prefix, path) };
            out.push(route(&c[1], &full, file, i, "aspnet"));
        }
    }
    out
}

fn go(file: &str, text: &str) -> Vec<ApiRoute> {
    static GIN: Lazy<Regex> =
        Lazy::new(|| re(r#"\.(GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS|Any|Get|Post|Put|Delete|Patch)\(\s*"(/[^"]*)""#));
    static STD: Lazy<Regex> = Lazy::new(|| re(r#"\b(?:http\.)?Handle(?:Func)?\(\s*"((?:[A-Z]+ )?/[^"]*)""#));
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if let Some(c) = GIN.captures(line) {
            let m = if &c[1] == "Any" { "ANY" } else { &c[1] };
            out.push(route(m, &c[2], file, i, "gin"));
        } else if let Some(c) = STD.captures(line) {
            let pat = &c[1];
            match pat.split_once(' ') {
                Some((m, p)) => out.push(route(m, p, file, i, "net/http")),
                None => out.push(route("ANY", pat, file, i, "net/http")),
            }
        }
    }
    out
}

fn java(file: &str, text: &str) -> Vec<ApiRoute> {
    static MAPPING: Lazy<Regex> = Lazy::new(|| {
        re(r#"@(Get|Post|Put|Delete|Patch|Request)Mapping\(\s*(?:(?:value|path)\s*=\s*)?\{?\s*"([^"]*)""#)
    });
    static CLASS: Lazy<Regex> = Lazy::new(|| re(r"\bclass\s+\w+"));
    let mut out = Vec::new();
    let mut prefix = String::new();
    let mut pending: Option<String> = None;
    let mut seen_class = false;
    for (i, line) in text.lines().enumerate() {
        if let Some(c) = MAPPING.captures(line) {
            if &c[1] == "Request" && !seen_class {
                pending = Some(c[2].to_owned());
                continue;
            }
            let m = if &c[1] == "Request" { "ANY" } else { &c[1] };
            out.push(route(m, &join_prefix(&prefix, &c[2]), file, i, "spring"));
        } else if !seen_class && CLASS.is_match(line) {
            seen_class = true;
            prefix = pending.take().unwrap_or_default();
        }
    }
    out
}

/// Routes and title of an OpenAPI / Swagger document (JSON or YAML).
#[must_use]
pub fn openapi(file: &str, text: &str) -> (Option<String>, Vec<ApiRoute>) {
    let mut routes = Vec::new();
    let lower = file.to_ascii_lowercase();
    if lower.ends_with(".json") {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return (None, routes) };
        let title = v.pointer("/info/title").and_then(|t| t.as_str()).map(str::to_owned);
        if let Some(serde_json::Value::Object(paths)) = v.get("paths") {
            for (path, ops) in paths {
                let line = find_line(text, &format!("\"{path}\""));
                for m in ops.as_object().into_iter().flat_map(|o| o.keys()) {
                    if HTTP_METHODS.contains(&m.as_str()) {
                        routes.push(route(m, path, file, line, "openapi"));
                    }
                }
            }
        }
        routes.truncate(MAX_ROUTES_PER_FILE);
        return (title, routes);
    }
    let doc = yaml::parse(text);
    let title = doc.get("info").and_then(|i| i.get("title")).and_then(Yaml::as_str).map(str::to_owned);
    if let Some(paths) = doc.get("paths") {
        for (path, ops) in paths.entries() {
            let line = find_line(text, &format!("{path}:"));
            for (m, _) in ops.entries() {
                if HTTP_METHODS.contains(&m.as_str()) {
                    routes.push(route(m, path, file, line, "openapi"));
                }
            }
        }
    }
    routes.truncate(MAX_ROUTES_PER_FILE);
    (title, routes)
}

fn find_line(text: &str, needle: &str) -> usize {
    text.lines().position(|l| l.contains(needle)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(r: &[ApiRoute]) -> Vec<(String, String)> {
        r.iter().map(|r| (r.method.clone(), r.path.clone())).collect()
    }

    fn p(m: &str, path: &str) -> (String, String) {
        (m.to_owned(), path.to_owned())
    }

    #[test]
    fn rust_axum_and_actix() {
        let src = r#"
let app = Router::new()
    .route("/health", get(health))
    .route("/users/{id}", get(get_user).delete(del_user))
    .route("/rpc", any(ws));
#[post("/login")]
async fn login() {}
"#;
        let r = extract(Lang::Rust, "src/main.rs", src);
        assert_eq!(
            pairs(&r),
            vec![p("GET", "/health"), p("GET", "/users/{id}"), p("DELETE", "/users/{id}"), p("ANY", "/rpc"), p("POST", "/login")]
        );
        assert_eq!(r[0].line, 3);
        assert_eq!(r[0].framework, "axum");
    }

    #[test]
    fn js_express_nest_next() {
        let r = extract(Lang::JsTs, "src/server.ts", "app.get('/a', h)\nrouter.post(\"/b\", h)\nfoo.get('/no')\n");
        assert_eq!(pairs(&r), vec![p("GET", "/a"), p("POST", "/b")]);
        let r = extract(Lang::JsTs, "src/cats.controller.ts", "@Controller('cats')\nclass C {\n @Get()\n a(){}\n @Post(':id')\n b(){}\n}");
        assert_eq!(pairs(&r), vec![p("GET", "/cats"), p("POST", "/cats/:id")]);
        let r = extract(Lang::JsTs, "web/app/api/(admin)/users/route.ts", "export async function GET(req) {}\nexport const POST = 1;");
        assert_eq!(pairs(&r), vec![p("GET", "/api/users"), p("POST", "/api/users")]);
    }

    #[test]
    fn python_fastapi_flask_django() {
        let r = extract(
            Lang::Python,
            "api/main.py",
            "router = APIRouter(prefix=\"/v1\")\n@router.get(\"/items/{id}\")\ndef a(): ...\n@app.route('/x', methods=['POST', 'PUT'])\n@bp.route('/y')\n",
        );
        assert_eq!(pairs(&r), vec![p("GET", "/v1/items/{id}"), p("POST", "/x"), p("PUT", "/x"), p("GET", "/y")]);
        let r = extract(Lang::Python, "site/urls.py", "urlpatterns = [path('admin/', admin.site.urls)]");
        assert_eq!(pairs(&r), vec![p("ANY", "/admin/")]);
    }

    #[test]
    fn csharp_go_java() {
        let r = extract(
            Lang::CSharp,
            "Api/UsersController.cs",
            "[ApiController]\n[Route(\"api/[controller]\")]\npublic class UsersController : ControllerBase {\n [HttpGet]\n public X A() {}\n [HttpGet(\"{id}\")]\n public X B() {}\n}\napp.MapPost(\"/login\", h);",
        );
        assert_eq!(pairs(&r), vec![p("GET", "/api/users"), p("GET", "/api/users/{id}"), p("POST", "/login")]);
        let r = extract(Lang::Go, "main.go", "r.GET(\"/ping\", h)\nhttp.HandleFunc(\"/health\", h)\nmux.HandleFunc(\"POST /items\", h)\n");
        assert_eq!(pairs(&r), vec![p("GET", "/ping"), p("ANY", "/health"), p("POST", "/items")]);
        let r = extract(
            Lang::Java,
            "src/A.java",
            "@RequestMapping(\"/api\")\npublic class A {\n @GetMapping(\"/x\")\n void x(){}\n @PostMapping(value = \"/y\")\n void y(){}\n}",
        );
        assert_eq!(pairs(&r), vec![p("GET", "/api/x"), p("POST", "/api/y")]);
    }

    #[test]
    fn openapi_yaml_and_json() {
        let (title, r) = openapi(
            "openapi.yaml",
            "openapi: 3.0.0\ninfo:\n  title: Pets\npaths:\n  /pets:\n    get:\n      summary: l\n    post:\n      summary: c\n  /pets/{id}:\n    parameters: []\n    get:\n      summary: g\n",
        );
        assert_eq!(title.as_deref(), Some("Pets"));
        assert_eq!(pairs(&r), vec![p("GET", "/pets"), p("POST", "/pets"), p("GET", "/pets/{id}")]);
        let (title, r) = openapi("swagger.json", r#"{"info":{"title":"T"},"paths":{"/a":{"get":{},"parameters":[]}}}"#);
        assert_eq!(title.as_deref(), Some("T"));
        assert_eq!(pairs(&r), vec![p("GET", "/a")]);
    }
}
