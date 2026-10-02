//! Regex-based symbol extraction (no parser dependency).
//!
//! Line-oriented patterns per language catch top-level and nested
//! declarations well enough for relevance ranking and excerpt anchoring;
//! they are not a substitute for a real parser (tree-sitter is a Phase 2
//! candidate).

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Source language of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    /// Rust.
    Rust,
    /// TypeScript / TSX.
    TypeScript,
    /// JavaScript / JSX.
    JavaScript,
    /// Python.
    Python,
    /// Go.
    Go,
    /// C#.
    CSharp,
    /// Java.
    Java,
    /// Markdown.
    Markdown,
    /// Any other indexed text file.
    Other,
}

impl Language {
    /// Language from a path's extension.
    #[must_use]
    pub fn from_path(path: &str) -> Self {
        let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "rs" => Self::Rust,
            "ts" | "tsx" | "mts" | "cts" => Self::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "py" | "pyi" => Self::Python,
            "go" => Self::Go,
            "cs" => Self::CSharp,
            "java" => Self::Java,
            "md" | "markdown" | "mdx" => Self::Markdown,
            _ => Self::Other,
        }
    }

    /// Fence tag used when rendering excerpts.
    #[must_use]
    pub fn fence(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "ts",
            Self::JavaScript => "js",
            Self::Python => "python",
            Self::Go => "go",
            Self::CSharp => "csharp",
            Self::Java => "java",
            Self::Markdown => "markdown",
            Self::Other => "",
        }
    }

    /// Whether token estimates use the code factor.
    #[must_use]
    pub fn is_code(self) -> bool {
        !matches!(self, Self::Markdown)
    }
}

/// Kind of a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    /// Function.
    Function,
    /// Method.
    Method,
    /// Struct.
    Struct,
    /// Class.
    Class,
    /// Interface.
    Interface,
    /// Trait.
    Trait,
    /// Enum.
    Enum,
    /// Type alias.
    Type,
    /// Module.
    Module,
    /// Constant / static.
    Const,
    /// `impl` block (Rust).
    Impl,
    /// Markdown heading.
    Heading,
}

/// One extracted symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Symbol {
    /// Name (heading text for markdown).
    pub name: String,
    /// Kind.
    pub kind: SymbolKind,
    /// 1-based line.
    pub line: u32,
}

struct Pattern {
    re: Regex,
    /// Capture group holding the name.
    group: usize,
    kind: Option<SymbolKind>,
    /// Group holding a keyword that decides the kind (`struct`/`enum`/…).
    kind_group: Option<usize>,
}

fn p(re: &str, group: usize, kind: SymbolKind) -> Pattern {
    // Patterns are compile-time constants covered by tests.
    #[allow(clippy::expect_used)]
    let re = Regex::new(re).expect("valid symbol regex");
    Pattern { re, group, kind: Some(kind), kind_group: None }
}

fn pk(re: &str, group: usize, kind_group: usize) -> Pattern {
    #[allow(clippy::expect_used)]
    let re = Regex::new(re).expect("valid symbol regex");
    Pattern { re, group, kind: None, kind_group: Some(kind_group) }
}

fn keyword_kind(k: &str) -> SymbolKind {
    match k {
        "struct" | "union" | "record" => SymbolKind::Struct,
        "enum" => SymbolKind::Enum,
        "trait" => SymbolKind::Trait,
        "type" => SymbolKind::Type,
        "mod" => SymbolKind::Module,
        "interface" => SymbolKind::Interface,
        _ => SymbolKind::Class,
    }
}

const VIS_RS: &str = r"(?:pub(?:\([^)]*\))?\s+)?";

static RUST: Lazy<Vec<Pattern>> = Lazy::new(|| {
    vec![
        p(
            &format!(
                r#"^\s*{VIS_RS}(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"#
            ),
            1,
            SymbolKind::Function,
        ),
        pk(&format!(r"^\s*{VIS_RS}(struct|enum|trait|type|mod|union)\s+([A-Za-z_][A-Za-z0-9_]*)"), 2, 1),
        p(
            r"^\s*(?:unsafe\s+)?impl(?:<[^>]*>)?\s+(?:[A-Za-z0-9_:<>, &']+\s+for\s+)?([A-Za-z_][A-Za-z0-9_]*)",
            1,
            SymbolKind::Impl,
        ),
        p(&format!(r"^\s*{VIS_RS}(?:const|static)\s+(?:mut\s+)?([A-Z_][A-Z0-9_]*)\s*:"), 1, SymbolKind::Const),
    ]
});

static TS_JS: Lazy<Vec<Pattern>> = Lazy::new(|| {
    vec![
        p(r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\*?\s+([A-Za-z_$][\w$]*)", 1, SymbolKind::Function),
        pk(
            r"^\s*(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:abstract\s+)?(class|interface|enum)\s+([A-Za-z_$][\w$]*)",
            2,
            1,
        ),
        p(r"^\s*(?:export\s+)?(?:declare\s+)?type\s+([A-Za-z_$][\w$]*)\s*(?:<[^=]*>)?\s*=", 1, SymbolKind::Type),
        p(
            r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::[^=]+)?=\s*(?:async\s+)?(?:function\b|\([^)]*\)\s*(?::[^=]+)?=>|[A-Za-z_$][\w$]*\s*=>)",
            1,
            SymbolKind::Function,
        ),
        p(r"^\s*export\s+(?:const|let|var)\s+([A-Za-z_$][\w$]*)", 1, SymbolKind::Const),
    ]
});

static PYTHON: Lazy<Vec<Pattern>> = Lazy::new(|| {
    vec![
        p(r"^(?:async\s+)?def\s+([A-Za-z_]\w*)", 1, SymbolKind::Function),
        p(r"^\s+(?:async\s+)?def\s+([A-Za-z_]\w*)", 1, SymbolKind::Method),
        p(r"^\s*class\s+([A-Za-z_]\w*)", 1, SymbolKind::Class),
    ]
});

static GO: Lazy<Vec<Pattern>> = Lazy::new(|| {
    vec![
        p(r"^func\s+\([^)]*\)\s*([A-Za-z_]\w*)", 1, SymbolKind::Method),
        p(r"^func\s+([A-Za-z_]\w*)", 1, SymbolKind::Function),
        pk(r"^type\s+([A-Za-z_]\w*)(?:\[[^\]]*\])?\s+(struct|interface)\b", 1, 2),
        p(r"^type\s+([A-Za-z_]\w*)", 1, SymbolKind::Type),
    ]
});

const MODS_CS: &str =
    r"(?:(?:public|private|protected|internal|static|sealed|abstract|partial|readonly|unsafe|new|file)\s+)*";
const MODS_JAVA: &str = r"(?:(?:public|private|protected|static|final|abstract|sealed|non-sealed|strictfp)\s+)*";

static CSHARP: Lazy<Vec<Pattern>> = Lazy::new(|| {
    vec![
        pk(&format!(r"^\s*{MODS_CS}(class|interface|struct|enum|record)\s+([A-Za-z_]\w*)"), 2, 1),
        p(
            r"^\s*(?:(?:public|private|protected|internal|static|virtual|override|async|abstract|sealed|extern|new)\s+)+[\w<>\[\],.?]+(?:\s*<[^>]*>)?\s+([A-Za-z_]\w*)\s*(?:<[^>]*>)?\s*\(",
            1,
            SymbolKind::Method,
        ),
    ]
});

static JAVA: Lazy<Vec<Pattern>> = Lazy::new(|| {
    vec![
        pk(&format!(r"^\s*{MODS_JAVA}(class|interface|enum|record)\s+([A-Za-z_]\w*)"), 2, 1),
        p(
            r"^\s*(?:(?:public|private|protected|static|final|synchronized|abstract|native|default)\s+)+(?:<[^>]*>\s+)?[\w<>\[\],.?]+\s+([A-Za-z_]\w*)\s*\(",
            1,
            SymbolKind::Method,
        ),
    ]
});

static MD_HEADING: Lazy<Regex> = Lazy::new(|| {
    #[allow(clippy::expect_used)]
    Regex::new(r"^ {0,3}#{1,6}\s+(.+?)\s*#*\s*$").expect("valid heading regex")
});

const NOT_NAMES: &[&str] =
    &["if", "for", "while", "switch", "return", "new", "catch", "foreach", "using", "lock", "else"];

/// Max symbols kept per file.
pub const MAX_SYMBOLS: usize = 400;

/// Extracts declarations / headings from `text`.
#[must_use]
pub fn extract_symbols(language: Language, text: &str) -> Vec<Symbol> {
    let patterns: &[Pattern] = match language {
        Language::Rust => &RUST,
        Language::TypeScript | Language::JavaScript => &TS_JS,
        Language::Python => &PYTHON,
        Language::Go => &GO,
        Language::CSharp => &CSHARP,
        Language::Java => &JAVA,
        Language::Markdown => return markdown_headings(text),
        Language::Other => return Vec::new(),
    };
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.len() > 1000 {
            continue;
        }
        for pat in patterns {
            let Some(caps) = pat.re.captures(line) else { continue };
            let Some(name) = caps.get(pat.group).map(|m| m.as_str()) else { continue };
            if NOT_NAMES.contains(&name) {
                continue;
            }
            let kind = match (pat.kind, pat.kind_group.and_then(|g| caps.get(g))) {
                (Some(k), _) => k,
                (None, Some(k)) => keyword_kind(k.as_str()),
                (None, None) => continue,
            };
            out.push(Symbol { name: name.to_owned(), kind, line: u32::try_from(i + 1).unwrap_or(u32::MAX) });
            break;
        }
        if out.len() >= MAX_SYMBOLS {
            break;
        }
    }
    out
}

fn markdown_headings(text: &str) -> Vec<Symbol> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (i, line) in text.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(c) = MD_HEADING.captures(line).and_then(|c| c.get(1)) {
            out.push(Symbol {
                name: c.as_str().to_owned(),
                kind: SymbolKind::Heading,
                line: u32::try_from(i + 1).unwrap_or(u32::MAX),
            });
            if out.len() >= MAX_SYMBOLS {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(lang: Language, src: &str) -> Vec<(String, SymbolKind)> {
        extract_symbols(lang, src).into_iter().map(|s| (s.name, s.kind)).collect()
    }

    #[test]
    fn rust() {
        let src = "pub struct Payroll;\nenum E { A }\npub(crate) async fn compute_net(x: u32) {}\nimpl Display for Payroll {}\nimpl<T> Foo<T> {}\ntrait T {}\npub const MAX_X: u32 = 1;\nmod tests {}\n    fn inner() {}\npub type Id = u32;";
        assert_eq!(
            names(Language::Rust, src),
            vec![
                ("Payroll".into(), SymbolKind::Struct),
                ("E".into(), SymbolKind::Enum),
                ("compute_net".into(), SymbolKind::Function),
                ("Payroll".into(), SymbolKind::Impl),
                ("Foo".into(), SymbolKind::Impl),
                ("T".into(), SymbolKind::Trait),
                ("MAX_X".into(), SymbolKind::Const),
                ("tests".into(), SymbolKind::Module),
                ("inner".into(), SymbolKind::Function),
                ("Id".into(), SymbolKind::Type),
            ]
        );
        let s = extract_symbols(Language::Rust, src);
        assert_eq!(s[2].line, 3);
    }

    #[test]
    fn typescript() {
        let src = "export function useMemory() {}\nexport default class Panel {}\ninterface Props { a: string }\nexport type Id = string;\nconst add = (a: number, b: number) => a + b;\nexport const handler = async (e) => {};\nexport const LIMIT = 5;\nenum Color { Red }\nconst x = 5;";
        assert_eq!(
            names(Language::TypeScript, src),
            vec![
                ("useMemory".into(), SymbolKind::Function),
                ("Panel".into(), SymbolKind::Class),
                ("Props".into(), SymbolKind::Interface),
                ("Id".into(), SymbolKind::Type),
                ("add".into(), SymbolKind::Function),
                ("handler".into(), SymbolKind::Function),
                ("LIMIT".into(), SymbolKind::Const),
                ("Color".into(), SymbolKind::Enum),
            ]
        );
    }

    #[test]
    fn python_go() {
        let py = "class Repo:\n    def save(self):\n        pass\ndef main():\n    pass\nasync def fetch(): ...";
        assert_eq!(
            names(Language::Python, py),
            vec![
                ("Repo".into(), SymbolKind::Class),
                ("save".into(), SymbolKind::Method),
                ("main".into(), SymbolKind::Function),
                ("fetch".into(), SymbolKind::Function),
            ]
        );
        let go = "func main() {}\nfunc (s *Server) Start() error {}\ntype Server struct {}\ntype Store interface {}\ntype ID string";
        assert_eq!(
            names(Language::Go, go),
            vec![
                ("main".into(), SymbolKind::Function),
                ("Start".into(), SymbolKind::Method),
                ("Server".into(), SymbolKind::Struct),
                ("Store".into(), SymbolKind::Interface),
                ("ID".into(), SymbolKind::Type),
            ]
        );
    }

    #[test]
    fn csharp_java() {
        let cs = "public sealed class PayrollService {\n  public async Task<int> ComputeAsync(int x) {\n    if (x) {}\n  }\n  internal record Line(int A);\n}";
        assert_eq!(
            names(Language::CSharp, cs),
            vec![
                ("PayrollService".into(), SymbolKind::Class),
                ("ComputeAsync".into(), SymbolKind::Method),
                ("Line".into(), SymbolKind::Struct),
            ]
        );
        let java = "public class App {\n  public static void main(String[] args) {}\n  private List<String> names() { return null; }\n}\ninterface Repo {}";
        assert_eq!(
            names(Language::Java, java),
            vec![
                ("App".into(), SymbolKind::Class),
                ("main".into(), SymbolKind::Method),
                ("names".into(), SymbolKind::Method),
                ("Repo".into(), SymbolKind::Interface),
            ]
        );
    }

    #[test]
    fn markdown_and_languages() {
        let md = "# Title\ntext\n```\n# not heading\n```\n## Sub ##\n";
        assert_eq!(
            names(Language::Markdown, md),
            vec![("Title".into(), SymbolKind::Heading), ("Sub".into(), SymbolKind::Heading)]
        );
        assert_eq!(Language::from_path("a/b.TSX"), Language::TypeScript);
        assert_eq!(Language::from_path("Makefile"), Language::Other);
        assert!(names(Language::Other, "fn x() {}").is_empty());
    }
}
