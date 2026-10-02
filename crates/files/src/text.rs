//! Text handling: binary detection, encoding detection/encoding, line
//! endings, language ids and hashing.

use orkestr_protocol::methods::files::{LineEnding, TextEncoding};

use crate::error::{FilesError, FilesResult};

/// Bytes inspected for binary detection.
const SNIFF_LEN: usize = 8000;

/// Decoded file content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    /// Text with its encoding.
    Text {
        /// Content.
        content: String,
        /// Encoding found.
        encoding: TextEncoding,
    },
    /// Binary content.
    Binary,
}

/// blake3 hex digest of raw bytes.
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Detects the encoding (BOM → UTF-8 → Latin-1 fallback) or binary content.
#[must_use]
pub fn decode(bytes: &[u8]) -> Decoded {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return match std::str::from_utf8(rest) {
            Ok(s) => Decoded::Text { content: s.to_owned(), encoding: TextEncoding::Utf8Bom },
            Err(_) => Decoded::Binary,
        };
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, u16::from_le_bytes, TextEncoding::Utf16Le);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, u16::from_be_bytes, TextEncoding::Utf16Be);
    }
    let sniff = &bytes[..bytes.len().min(SNIFF_LEN)];
    if sniff.contains(&0) {
        return Decoded::Binary;
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Decoded::Text { content: s.to_owned(), encoding: TextEncoding::Utf8 };
    }
    // Not UTF-8: treat as single-byte text unless it looks like binary data.
    let suspicious = sniff.iter().filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0C | 0x1B)).count();
    if sniff.is_empty() || suspicious * 100 / sniff.len() > 5 {
        return Decoded::Binary;
    }
    Decoded::Text { content: bytes.iter().map(|&b| char::from(b)).collect(), encoding: TextEncoding::Latin1 }
}

fn decode_utf16(rest: &[u8], conv: fn([u8; 2]) -> u16, encoding: TextEncoding) -> Decoded {
    if rest.len() % 2 != 0 {
        return Decoded::Binary;
    }
    let units: Vec<u16> = rest.chunks_exact(2).map(|c| conv([c[0], c[1]])).collect();
    match String::from_utf16(&units) {
        Ok(content) => Decoded::Text { content, encoding },
        Err(_) => Decoded::Binary,
    }
}

/// Encodes text for writing (BOMs included where the encoding has one).
pub fn encode(content: &str, encoding: TextEncoding) -> FilesResult<Vec<u8>> {
    Ok(match encoding {
        TextEncoding::Utf8 => content.as_bytes().to_vec(),
        TextEncoding::Utf8Bom => {
            let mut v = Vec::with_capacity(content.len() + 3);
            v.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            v.extend_from_slice(content.as_bytes());
            v
        }
        TextEncoding::Utf16Le => {
            let mut v = vec![0xFF, 0xFE];
            for u in content.encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        }
        TextEncoding::Utf16Be => {
            let mut v = vec![0xFE, 0xFF];
            for u in content.encode_utf16() {
                v.extend_from_slice(&u.to_be_bytes());
            }
            v
        }
        TextEncoding::Latin1 => content
            .chars()
            .map(|c| u8::try_from(u32::from(c)).map_err(|_| FilesError::Encoding("Latin-1")))
            .collect::<FilesResult<Vec<u8>>>()?,
    })
}

/// Dominant line ending (CRLF when at least half of the line breaks are CRLF).
#[must_use]
pub fn line_ending(content: &str) -> LineEnding {
    let lf = content.matches('\n').count();
    if lf == 0 {
        return LineEnding::Lf;
    }
    let crlf = content.matches("\r\n").count();
    if crlf * 2 >= lf {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    }
}

/// Monaco language id for a file name.
#[must_use]
pub fn language_for(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "dockerfile" | "containerfile" => return "dockerfile",
        "makefile" | "gnumakefile" => return "plaintext",
        "cmakelists.txt" => return "plaintext",
        ".gitignore" | ".dockerignore" | ".npmignore" | ".prettierignore" | ".eslintignore" => return "plaintext",
        ".editorconfig" | ".npmrc" | ".gitattributes" | ".gitconfig" => return "ini",
        ".env" => return "ini",
        _ => {}
    }
    if lower.starts_with(".env.") {
        return "ini";
    }
    if lower.starts_with("dockerfile.") || lower.ends_with(".dockerfile") {
        return "dockerfile";
    }
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    match ext {
        "ts" | "mts" | "cts" | "tsx" => "typescript",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "json" | "jsonc" | "json5" | "code-workspace" => "json",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "html" | "htm" | "xhtml" | "vue" | "svelte" | "astro" => "html",
        "md" | "markdown" | "mdx" => "markdown",
        "rs" => "rust",
        "py" | "pyw" | "pyi" => "python",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "scala" | "sc" => "scala",
        "cs" | "csx" => "csharp",
        "fs" | "fsi" | "fsx" => "fsharp",
        "vb" => "vb",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "ino" => "cpp",
        "m" | "mm" => "objective-c",
        "swift" => "swift",
        "dart" => "dart",
        "rb" | "gemspec" => "ruby",
        "php" | "phtml" => "php",
        "lua" => "lua",
        "r" => "r",
        "pl" | "pm" => "perl",
        "sh" | "bash" | "zsh" | "fish" => "shell",
        "ps1" | "psm1" | "psd1" => "powershell",
        "bat" | "cmd" => "bat",
        "yml" | "yaml" => "yaml",
        "toml" | "ini" | "cfg" | "conf" | "properties" => "ini",
        "xml" | "xsd" | "xsl" | "svg" | "plist" | "csproj" | "props" | "targets" | "xaml" => "xml",
        "sql" => "sql",
        "graphql" | "gql" => "graphql",
        "proto" => "proto",
        "tf" | "hcl" => "hcl",
        "sol" => "sol",
        "ex" | "exs" => "elixir",
        "clj" | "cljs" | "edn" => "clojure",
        "coffee" => "coffeescript",
        "pug" => "pug",
        "hbs" | "handlebars" => "handlebars",
        "twig" => "twig",
        "dockerfile" => "dockerfile",
        _ => "plaintext",
    }
}

/// Number of UTF-16 code units of a string (Monaco column arithmetic).
#[must_use]
pub fn utf16_len(s: &str) -> u32 {
    u32::try_from(s.encode_utf16().count()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_and_bom_roundtrip() {
        let d = decode("héllo\n".as_bytes());
        assert_eq!(d, Decoded::Text { content: "héllo\n".into(), encoding: TextEncoding::Utf8 });
        let bytes = encode("héllo", TextEncoding::Utf8Bom).unwrap();
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
        assert_eq!(decode(&bytes), Decoded::Text { content: "héllo".into(), encoding: TextEncoding::Utf8Bom });
    }

    #[test]
    fn utf16_roundtrip_both_endians() {
        for enc in [TextEncoding::Utf16Le, TextEncoding::Utf16Be] {
            let bytes = encode("añ€😀\r\n", enc).unwrap();
            assert_eq!(decode(&bytes), Decoded::Text { content: "añ€😀\r\n".into(), encoding: enc });
        }
        assert_eq!(decode(&[0xFF, 0xFE, 0x41]), Decoded::Binary, "odd UTF-16 length is binary");
    }

    #[test]
    fn binary_detection() {
        assert_eq!(decode(&[0x89, b'P', b'N', b'G', 0, 0, 0, 13]), Decoded::Binary);
        assert_eq!(decode(&[1, 2, 3, 4, 5, 6, 0x80, 0x81]), Decoded::Binary);
        let latin = decode(&[b'c', b'a', b'f', 0xE9]);
        assert_eq!(latin, Decoded::Text { content: "café".into(), encoding: TextEncoding::Latin1 });
        assert_eq!(decode(b""), Decoded::Text { content: String::new(), encoding: TextEncoding::Utf8 });
    }

    #[test]
    fn latin1_encoding_rejects_wide_chars() {
        assert_eq!(encode("café", TextEncoding::Latin1).unwrap(), vec![b'c', b'a', b'f', 0xE9]);
        assert!(matches!(encode("€", TextEncoding::Latin1), Err(FilesError::Encoding(_))));
    }

    #[test]
    fn line_endings() {
        assert_eq!(line_ending("a\nb\n"), LineEnding::Lf);
        assert_eq!(line_ending("a\r\nb\r\n"), LineEnding::Crlf);
        assert_eq!(line_ending("single line"), LineEnding::Lf);
        assert_eq!(line_ending("a\r\nb\nc\n"), LineEnding::Lf);
    }

    #[test]
    fn languages() {
        assert_eq!(language_for("main.rs"), "rust");
        assert_eq!(language_for("App.TSX"), "typescript");
        assert_eq!(language_for("Dockerfile"), "dockerfile");
        assert_eq!(language_for(".env.local"), "ini");
        assert_eq!(language_for("Cargo.toml"), "ini");
        assert_eq!(language_for("README"), "plaintext");
        assert_eq!(language_for("style.module.scss"), "scss");
    }

    #[test]
    fn hashing_and_utf16_len() {
        assert_eq!(hash_bytes(b"abc").len(), 64);
        assert_ne!(hash_bytes(b"abc"), hash_bytes(b"abd"));
        assert_eq!(utf16_len("a😀"), 3);
    }
}
