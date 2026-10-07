//! Built-in launch recipes for common language servers.
//!
//! A server is found either inside an installed, enabled extension (a
//! bundled binary or a bundled Node.js server script) or on `PATH`.
//! Orkestr never downloads a server by itself and never runs extension
//! activation code; it only launches a known server program, and only after
//! the user approved it (see `lsp.start`).

use std::path::{Path, PathBuf};

use orkestr_protocol::methods::lsp::LspServerSource;

/// How to find a server.
#[derive(Debug, Clone, Copy)]
pub enum Candidate {
    /// A native binary bundled in an extension (`exe` suffix added on Windows).
    ExtensionBinary {
        /// Extension id.
        extension: &'static str,
        /// Path inside the extension (without `.exe`).
        path: &'static str,
        /// Arguments.
        args: &'static [&'static str],
    },
    /// A Node.js server script bundled in an extension (needs `node` on `PATH`).
    ExtensionNodeScript {
        /// Extension id.
        extension: &'static str,
        /// Script path inside the extension.
        path: &'static str,
        /// Arguments after the script.
        args: &'static [&'static str],
    },
    /// A program on `PATH`.
    PathProgram {
        /// Program name.
        name: &'static str,
        /// Arguments.
        args: &'static [&'static str],
    },
}

/// A server recipe.
#[derive(Debug, Clone, Copy)]
pub struct Recipe {
    /// Id.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Language ids served.
    pub languages: &'static [&'static str],
    /// Candidates in priority order.
    pub candidates: &'static [Candidate],
    /// Install hint.
    pub install_hint: &'static str,
}

const STDIO: &[&str] = &["--stdio"];

/// All recipes.
pub const RECIPES: &[Recipe] = &[
    Recipe {
        id: "rust-analyzer",
        name: "rust-analyzer",
        languages: &["rust"],
        candidates: &[
            Candidate::ExtensionBinary { extension: "rust-lang.rust-analyzer", path: "server/rust-analyzer", args: &[] },
            Candidate::PathProgram { name: "rust-analyzer", args: &[] },
        ],
        install_hint: "rust-lang.rust-analyzer (Open VSX) or `rustup component add rust-analyzer`",
    },
    Recipe {
        id: "pyright",
        name: "Pyright / basedpyright",
        languages: &["python"],
        candidates: &[
            Candidate::PathProgram { name: "basedpyright-langserver", args: STDIO },
            Candidate::PathProgram { name: "pyright-langserver", args: STDIO },
            Candidate::ExtensionNodeScript { extension: "detachhead.basedpyright", path: "dist/server.js", args: STDIO },
            Candidate::ExtensionNodeScript { extension: "ms-pyright.pyright", path: "dist/server.js", args: STDIO },
        ],
        install_hint: "`pip install basedpyright` or `npm install -g pyright`",
    },
    Recipe {
        id: "typescript",
        name: "TypeScript Language Server",
        languages: &["typescript", "javascript", "typescriptreact", "javascriptreact"],
        candidates: &[Candidate::PathProgram { name: "typescript-language-server", args: STDIO }],
        install_hint: "`npm install -g typescript-language-server typescript`",
    },
    Recipe {
        id: "gopls",
        name: "gopls",
        languages: &["go"],
        candidates: &[Candidate::PathProgram { name: "gopls", args: &[] }],
        install_hint: "`go install golang.org/x/tools/gopls@latest`",
    },
    Recipe {
        id: "clangd",
        name: "clangd",
        languages: &["c", "cpp", "objective-c", "objective-cpp"],
        candidates: &[Candidate::PathProgram { name: "clangd", args: &[] }],
        install_hint: "LLVM / clangd from your package manager or https://clangd.llvm.org",
    },
    Recipe {
        id: "csharp",
        name: "C# (csharp-ls / OmniSharp, best effort)",
        languages: &["csharp"],
        candidates: &[
            Candidate::PathProgram { name: "csharp-ls", args: &[] },
            Candidate::PathProgram { name: "OmniSharp", args: &["-lsp"] },
        ],
        install_hint: "`dotnet tool install -g csharp-ls`",
    },
    Recipe {
        id: "json",
        name: "JSON (vscode-langservers-extracted)",
        languages: &["json", "jsonc"],
        candidates: &[Candidate::PathProgram { name: "vscode-json-language-server", args: STDIO }],
        install_hint: "`npm install -g vscode-langservers-extracted`",
    },
    Recipe {
        id: "css",
        name: "CSS / SCSS / Less (vscode-langservers-extracted)",
        languages: &["css", "scss", "less"],
        candidates: &[Candidate::PathProgram { name: "vscode-css-language-server", args: STDIO }],
        install_hint: "`npm install -g vscode-langservers-extracted`",
    },
    Recipe {
        id: "html",
        name: "HTML (vscode-langservers-extracted)",
        languages: &["html"],
        candidates: &[Candidate::PathProgram { name: "vscode-html-language-server", args: STDIO }],
        install_hint: "`npm install -g vscode-langservers-extracted`",
    },
];

/// Recipe by id.
#[must_use]
pub fn recipe(id: &str) -> Option<&'static Recipe> {
    RECIPES.iter().find(|r| r.id == id)
}

/// Recipes an extension can provide a server for.
#[must_use]
pub fn servers_for_extension(extension_id: &str) -> Vec<String> {
    RECIPES
        .iter()
        .filter(|r| {
            r.candidates.iter().any(|c| match c {
                Candidate::ExtensionBinary { extension, .. } | Candidate::ExtensionNodeScript { extension, .. } => {
                    extension.eq_ignore_ascii_case(extension_id)
                }
                Candidate::PathProgram { .. } => false,
            })
        })
        .map(|r| r.id.to_owned())
        .collect()
}

/// A resolved command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Program.
    pub program: PathBuf,
    /// Arguments.
    pub args: Vec<String>,
    /// Where it was found.
    pub source: LspServerSource,
    /// Whether the program was unpacked from an extension (needs `chmod +x` on Unix).
    pub from_extension: bool,
}

impl Resolved {
    /// Display form of the command line.
    #[must_use]
    pub fn display(&self) -> String {
        std::iter::once(self.program.display().to_string()).chain(self.args.iter().cloned()).collect::<Vec<_>>().join(" ")
    }
}

fn binary_name(path: &str) -> String {
    if cfg!(windows) {
        format!("{path}.exe")
    } else {
        path.to_owned()
    }
}

fn in_extension(dir: &Path, rel: &str) -> Option<PathBuf> {
    let p = rel.split('/').fold(dir.to_path_buf(), |p, c| p.join(c));
    let meta = std::fs::symlink_metadata(&p).ok()?;
    meta.is_file().then_some(p)
}

/// Resolves a recipe against the enabled extensions and `PATH`.
pub fn resolve(
    recipe: &Recipe,
    extensions: &[(String, PathBuf)],
    which: &dyn Fn(&str) -> Option<PathBuf>,
) -> Option<Resolved> {
    let ext_dir = |id: &str| extensions.iter().find(|(e, _)| e.eq_ignore_ascii_case(id)).map(|(_, d)| d.clone());
    for c in recipe.candidates {
        match *c {
            Candidate::ExtensionBinary { extension, path, args } => {
                if let Some(p) = ext_dir(extension).and_then(|d| in_extension(&d, &binary_name(path))) {
                    return Some(Resolved {
                        program: p,
                        args: args.iter().map(|s| (*s).to_owned()).collect(),
                        source: LspServerSource::Extension { extension_id: extension.to_owned() },
                        from_extension: true,
                    });
                }
            }
            Candidate::ExtensionNodeScript { extension, path, args } => {
                if let (Some(script), Some(node)) = (ext_dir(extension).and_then(|d| in_extension(&d, path)), which("node")) {
                    return Some(Resolved {
                        program: node,
                        args: std::iter::once(script.display().to_string())
                            .chain(args.iter().map(|s| (*s).to_owned()))
                            .collect(),
                        source: LspServerSource::Extension { extension_id: extension.to_owned() },
                        from_extension: false,
                    });
                }
            }
            Candidate::PathProgram { name, args } => {
                if let Some(p) = which(name) {
                    return Some(Resolved {
                        program: p,
                        args: args.iter().map(|s| (*s).to_owned()).collect(),
                        source: LspServerSource::Path,
                        from_extension: false,
                    });
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_unique_and_extension_mapping() {
        let mut ids: Vec<_> = RECIPES.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), RECIPES.len());
        assert_eq!(servers_for_extension("rust-lang.rust-analyzer"), vec!["rust-analyzer".to_string()]);
        assert!(servers_for_extension("dracula-theme.theme-dracula").is_empty());
    }

    #[test]
    fn prefers_bundled_binary_then_path() {
        let dir = tempfile::tempdir().unwrap();
        let ext = dir.path().join("rust-lang.rust-analyzer-1.0.0");
        std::fs::create_dir_all(ext.join("server")).unwrap();
        let r = recipe("rust-analyzer").unwrap();
        let on_path = |n: &str| (n == "rust-analyzer").then(|| PathBuf::from("/usr/bin/rust-analyzer"));
        // Extension installed but without the binary → PATH.
        let exts = vec![("rust-lang.rust-analyzer".to_string(), ext.clone())];
        let got = resolve(r, &exts, &on_path).unwrap();
        assert_eq!(got.source, LspServerSource::Path);
        // With the bundled binary → extension.
        std::fs::write(ext.join("server").join(binary_name("rust-analyzer")), b"bin").unwrap();
        let got = resolve(r, &exts, &on_path).unwrap();
        assert!(matches!(got.source, LspServerSource::Extension { .. }));
        assert!(got.from_extension);
        // Nothing.
        assert!(resolve(r, &[], &|_| None).is_none());
    }

    #[test]
    fn node_script_needs_node() {
        let dir = tempfile::tempdir().unwrap();
        let ext = dir.path().join("p");
        std::fs::create_dir_all(ext.join("dist")).unwrap();
        std::fs::write(ext.join("dist/server.js"), b"//").unwrap();
        let r = recipe("pyright").unwrap();
        let exts = vec![("ms-pyright.pyright".to_string(), ext)];
        assert!(resolve(r, &exts, &|_| None).is_none());
        let got = resolve(r, &exts, &|n| (n == "node").then(|| PathBuf::from("/bin/node"))).unwrap();
        assert_eq!(got.program, PathBuf::from("/bin/node"));
        assert_eq!(got.args.last().map(String::as_str), Some("--stdio"));
    }
}
