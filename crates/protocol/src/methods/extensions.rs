//! `extensions.*` — VS Code extensions in Orkestr's own editor (ADR-0025).
//!
//! Extensions come from **Open VSX** (`https://open-vsx.org`) or from a local
//! `.vsix` file chosen by the user; the Microsoft Visual Studio Marketplace is
//! never contacted (its terms forbid non-Microsoft products).
//!
//! Extensions are **untrusted content**. Orkestr reads their declarative
//! contributions as data — color themes, file icon themes, TextMate grammars,
//! language configurations and snippets — and never executes their
//! JavaScript. Everything else (commands, views, debuggers, activation code,
//! keybindings, settings) is reported as "needs full VS Code". Language
//! servers bundled with an extension run only through `lsp.*` after an
//! explicit user approval.
//!
//! Paths inside contributions are relative to the extension root and use `/`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::Timestamp;

use super::method;

/// An extension found on Open VSX (search hit / details header).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MarketplaceExtension {
    /// Publisher namespace (`dracula-theme`).
    pub namespace: String,
    /// Extension name (`theme-dracula`).
    pub name: String,
    /// `namespace.name`, lowercase.
    pub id: String,
    /// Display name (falls back to `name`).
    pub display_name: String,
    /// Short description.
    pub description: Option<String>,
    /// Latest version.
    pub version: String,
    /// Namespace display name, when Open VSX reports one.
    pub publisher_display_name: Option<String>,
    /// Whether the publisher is a verified owner of the namespace on Open VSX.
    pub verified: bool,
    /// Total downloads.
    pub downloads: u64,
    /// Average rating (0–5), when rated.
    pub rating: Option<f64>,
    /// Number of reviews.
    pub review_count: u32,
    /// Icon URL on open-vsx.org (fetch it through `extensions.icon`).
    pub icon_url: Option<String>,
    /// SPDX license id.
    pub license: Option<String>,
    /// Publication time of the latest version (RFC 3339, as reported).
    pub published_at: Option<String>,
    /// Installed version, when this extension is installed.
    pub installed_version: Option<String>,
}

/// Sort order of a search.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExtensionSort {
    /// Open VSX relevance.
    #[default]
    Relevance,
    /// Most downloaded.
    Downloads,
    /// Best rated.
    Rating,
    /// Most recently published.
    Recent,
}

/// `extensions.search` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchExtensionsParams {
    /// Free text (may be empty: popular extensions).
    pub query: String,
    /// Page offset (default 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub offset: Option<u32>,
    /// Page size (default 20, max 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub size: Option<u32>,
    /// Sort order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sort: Option<ExtensionSort>,
    /// Open VSX category filter (`Themes`, `Programming Languages`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub category: Option<String>,
}

/// `extensions.search` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchExtensionsResult {
    /// Total matches.
    pub total: u64,
    /// Offset of this page.
    pub offset: u32,
    /// Hits.
    pub items: Vec<MarketplaceExtension>,
}

/// `namespace` + `name` of an Open VSX extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MarketplaceRef {
    /// Namespace.
    pub namespace: String,
    /// Name.
    pub name: String,
}

/// `extensions.details` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MarketplaceDetails {
    /// Header.
    pub extension: MarketplaceExtension,
    /// Known versions, newest first (at most 50).
    pub versions: Vec<String>,
    /// Categories.
    pub categories: Vec<String>,
    /// Tags.
    pub tags: Vec<String>,
    /// Repository URL.
    pub repository: Option<String>,
    /// Homepage URL.
    pub homepage: Option<String>,
    /// `engines.vscode` range of the latest version.
    pub engine: Option<String>,
    /// Platform-specific builds offered (`win32-x64`, `universal`, …).
    pub target_platforms: Vec<String>,
    /// The build this machine would install.
    pub install_target: String,
    /// Whether a build for this machine exists.
    pub installable: bool,
    /// Contributions of the latest version (from its `package.json`).
    pub contributions: Option<ContributionSummary>,
    /// README (markdown, untrusted — render as text, no HTML).
    pub readme: Option<String>,
    /// Whether the version is marked as a pre-release.
    pub pre_release: bool,
}

/// `extensions.icon` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExtensionIconParams {
    /// An icon URL returned by `extensions.search` (only `https://open-vsx.org` is fetched).
    pub url: String,
}

/// `extensions.icon` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExtensionIconResult {
    /// `data:` URL (PNG / JPEG / SVG / WebP, ≤ 512 KiB), or `None` when unavailable.
    pub data_url: Option<String>,
}

/// `extensions.install` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstallExtensionParams {
    /// Namespace.
    pub namespace: String,
    /// Name.
    pub name: String,
    /// Version (default: latest for this platform).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version: Option<String>,
}

/// `extensions.installVsix` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstallVsixParams {
    /// Absolute path of a `.vsix` file on this machine.
    pub path: String,
}

/// Where an installed extension came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExtensionSource {
    /// Downloaded from Open VSX (checksum verified).
    OpenVsx,
    /// Installed from a local `.vsix` file.
    Vsix,
}

/// A color theme contribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ThemeContribution {
    /// Theme id (`id` or, as VS Code does, the label).
    pub id: String,
    /// Label.
    pub label: String,
    /// `vs`, `vs-dark`, `hc-black` or `hc-light`.
    pub ui_theme: String,
    /// Theme file.
    pub path: String,
}

/// A file icon theme contribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IconThemeContribution {
    /// Id.
    pub id: String,
    /// Label.
    pub label: String,
    /// Icon theme file.
    pub path: String,
}

/// A TextMate grammar contribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GrammarContribution {
    /// Language id (absent for injection grammars).
    pub language: Option<String>,
    /// Root scope (`source.rust`).
    pub scope_name: String,
    /// Grammar file (`.tmLanguage.json`, `.json`, `.tmLanguage`, `.plist`).
    pub path: String,
    /// Embedded scope → language id.
    pub embedded_languages: BTreeMap<String, String>,
    /// Scopes this grammar is injected into.
    pub injection_to: Vec<String>,
    /// Scope → standard token type (`string`, `comment`, `other`).
    pub token_types: BTreeMap<String, String>,
}

/// A language contribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LanguageContribution {
    /// Language id.
    pub id: String,
    /// Aliases (first = display name).
    pub aliases: Vec<String>,
    /// File extensions incl. dot (`.rs`).
    pub extensions: Vec<String>,
    /// Exact file names.
    pub filenames: Vec<String>,
    /// File name globs.
    pub filename_patterns: Vec<String>,
    /// First-line regex.
    pub first_line: Option<String>,
    /// `language-configuration.json` file.
    pub configuration: Option<String>,
}

/// A snippets contribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnippetContribution {
    /// Language id.
    pub language: String,
    /// Snippets file.
    pub path: String,
}

/// A contribution point Orkestr's editor does not support (needs full VS Code).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UnsupportedContribution {
    /// Contribution point (`commands`, `views`, `debuggers`, `keybindings`,
    /// `configuration`, `activation` for JavaScript code, …).
    pub kind: String,
    /// Number of entries.
    pub count: u32,
}

/// What an extension contributes and how much of it Orkestr's editor supports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContributionSummary {
    /// Color themes.
    pub themes: Vec<ThemeContribution>,
    /// File icon themes.
    pub icon_themes: Vec<IconThemeContribution>,
    /// TextMate grammars.
    pub grammars: Vec<GrammarContribution>,
    /// Languages.
    pub languages: Vec<LanguageContribution>,
    /// Snippets.
    pub snippets: Vec<SnippetContribution>,
    /// Built-in language server recipes this extension can provide a server for
    /// (`rust-analyzer`, …). Started only through `lsp.start` with approval.
    pub language_servers: Vec<String>,
    /// Contribution points that need the full VS Code extension host.
    pub unsupported: Vec<UnsupportedContribution>,
    /// Whether the extension ships activation code (`main` / `browser`); it is never run.
    pub has_code: bool,
}

/// An installed extension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstalledExtension {
    /// `publisher.name`, lowercase.
    pub id: String,
    /// Publisher.
    pub publisher: String,
    /// Name.
    pub name: String,
    /// Display name.
    pub display_name: String,
    /// Version.
    pub version: String,
    /// Description.
    pub description: Option<String>,
    /// License (SPDX id or text from `package.json`).
    pub license: Option<String>,
    /// Origin.
    pub source: ExtensionSource,
    /// Open VSX verified-publisher flag at install time (`None` for `.vsix`).
    pub verified: Option<bool>,
    /// Whether contributions are applied.
    pub enabled: bool,
    /// Install time.
    pub installed_at: Timestamp,
    /// SHA-256 of the installed `.vsix` (hex).
    pub sha256: String,
    /// `engines.vscode` range.
    pub engine: Option<String>,
    /// Whether the range accepts the VS Code API level Orkestr emulates for declarative contributions.
    pub engine_compatible: bool,
    /// Platform build (`win32-x64`, `universal`, …).
    pub target_platform: Option<String>,
    /// Extension icon (path inside the extension).
    pub icon: Option<String>,
    /// Contributions.
    pub contributions: ContributionSummary,
}

/// `extensions.list` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListExtensionsResult {
    /// Installed extensions, by display name.
    pub extensions: Vec<InstalledExtension>,
}

/// One installed extension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstalledExtensionResult {
    /// The extension.
    pub extension: InstalledExtension,
}

/// `{ id }` of an installed extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExtensionIdParams {
    /// `publisher.name`.
    pub id: String,
}

/// `extensions.setEnabled` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetExtensionEnabledParams {
    /// `publisher.name`.
    pub id: String,
    /// Enable or disable.
    pub enabled: bool,
}

/// A theme reference (`extensionId` + theme id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ThemeRef {
    /// Extension id.
    pub extension_id: String,
    /// Theme id within the extension.
    pub theme_id: String,
}

/// One TextMate token color rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TokenColorRule {
    /// Scope selectors (empty = default colors).
    pub scope: Vec<String>,
    /// Foreground `#rrggbb[aa]`.
    pub foreground: Option<String>,
    /// Background.
    pub background: Option<String>,
    /// `italic`, `bold`, `underline`, `strikethrough` (space separated), or `""` to reset.
    pub font_style: Option<String>,
}

/// A fully resolved color theme (`include` chain merged, JSONC / plist parsed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResolvedTheme {
    /// Reference.
    pub theme: ThemeRef,
    /// Label.
    pub label: String,
    /// `vs`, `vs-dark`, `hc-black`, `hc-light`.
    pub ui_theme: String,
    /// Workbench colors (`editor.background`, …), normalized to `#rrggbb[aa]`.
    pub colors: BTreeMap<String, String>,
    /// Token colors in order (later rules win, as in VS Code).
    pub token_colors: Vec<TokenColorRule>,
    /// Whether the theme enables semantic highlighting.
    pub semantic_highlighting: bool,
}

/// An icon font of an icon theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IconFont {
    /// Font id.
    pub id: String,
    /// Font file (path inside the extension).
    pub path: String,
    /// CSS format (`woff2`, `woff`, `truetype`).
    pub format: String,
    /// CSS font-weight.
    pub weight: Option<String>,
    /// CSS font-style.
    pub style: Option<String>,
    /// CSS font-size.
    pub size: Option<String>,
}

/// One icon of an icon theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IconDefinition {
    /// Image file (path inside the extension) — SVG / PNG.
    pub icon_path: Option<String>,
    /// Glyph of an icon font.
    pub font_character: Option<String>,
    /// Glyph color.
    pub font_color: Option<String>,
    /// Font id (default: the theme's first font).
    pub font_id: Option<String>,
    /// Glyph size.
    pub font_size: Option<String>,
}

/// Name → icon id associations of an icon theme (one variant: dark or light).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IconAssociations {
    /// Default file icon.
    pub file: Option<String>,
    /// Default folder icon.
    pub folder: Option<String>,
    /// Default expanded folder icon.
    pub folder_expanded: Option<String>,
    /// Extension (lowercase, no dot, may be compound `d.ts`) → icon.
    pub file_extensions: BTreeMap<String, String>,
    /// File name (lowercase) → icon.
    pub file_names: BTreeMap<String, String>,
    /// Folder name (lowercase) → icon.
    pub folder_names: BTreeMap<String, String>,
    /// Expanded folder name (lowercase) → icon.
    pub folder_names_expanded: BTreeMap<String, String>,
    /// Language id → icon.
    pub language_ids: BTreeMap<String, String>,
}

/// A resolved file icon theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResolvedIconTheme {
    /// Reference.
    pub theme: ThemeRef,
    /// Label.
    pub label: String,
    /// Icon id → definition.
    pub definitions: BTreeMap<String, IconDefinition>,
    /// Dark (default) associations.
    pub dark: IconAssociations,
    /// Light-theme overrides (merged over `dark` by the client when a light theme is active).
    pub light: Option<IconAssociations>,
    /// Icon fonts.
    pub fonts: Vec<IconFont>,
    /// Whether folders get no arrows (`hidesExplorerArrows`).
    pub hides_explorer_arrows: bool,
}

/// `extensions.readAsset` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReadExtensionAssetParams {
    /// Extension id.
    pub extension_id: String,
    /// Path inside the extension (`/` separated, no `..`).
    pub path: String,
}

/// `extensions.readAsset` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExtensionAssetResult {
    /// Media type (`application/json`, `image/svg+xml`, `font/woff2`, …).
    pub mime: String,
    /// Text content (JSON / plist / SVG).
    pub text: Option<String>,
    /// Base64 content (fonts, PNG).
    pub base64: Option<String>,
}

/// `extensions.languageConfig` / `extensions.snippets` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExtensionFileParams {
    /// Extension id.
    pub extension_id: String,
    /// Path inside the extension.
    pub path: String,
}

/// `extensions.languageConfig` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LanguageConfigResult {
    /// The parsed `language-configuration.json` (JSONC → JSON, untouched otherwise).
    pub config: serde_json::Value,
}

/// One snippet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Snippet {
    /// Name.
    pub name: String,
    /// Prefixes that trigger it.
    pub prefixes: Vec<String>,
    /// Body in TextMate snippet syntax (lines joined with `\n`).
    pub body: String,
    /// Description.
    pub description: Option<String>,
}

/// `extensions.snippets` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnippetsResult {
    /// Snippets.
    pub snippets: Vec<Snippet>,
}

/// Editor appearance preferences (persisted by the core).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EditorPrefs {
    /// Editor color theme (`None` = follow the Orkestr UI theme).
    pub editor_theme: Option<ThemeRef>,
    /// File icon theme (`None` = Orkestr's built-in icons).
    pub icon_theme: Option<ThemeRef>,
}

/// `extensions.readme` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExtensionReadmeResult {
    /// README markdown (untrusted), when the extension has one.
    pub markdown: Option<String>,
}

/// `ExtensionsChanged` event payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExtensionsChangedPayload {
    /// `installed`, `uninstalled`, `enabled`, `disabled`, `prefs`.
    pub reason: String,
    /// Affected extension.
    pub extension_id: Option<String>,
}

/// Install phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExtensionInstallPhase {
    /// Downloading the `.vsix`.
    Downloading,
    /// Verifying the SHA-256 checksum.
    Verifying,
    /// Unpacking.
    Extracting,
    /// Installed.
    Done,
    /// Failed (see `error`).
    Failed,
}

/// `ExtensionInstallProgress` event payload (ephemeral).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExtensionInstallProgressPayload {
    /// `namespace.name` (lowercase) or the `.vsix` file name.
    pub key: String,
    /// Phase.
    pub phase: ExtensionInstallPhase,
    /// Bytes received.
    pub received: u64,
    /// Expected bytes, when known.
    pub total: Option<u64>,
    /// Error message for `failed`.
    pub error: Option<String>,
}

method!(
    /// `extensions.search` — Open VSX search.
    Search, "extensions.search", SearchExtensionsParams, SearchExtensionsResult
);
method!(
    /// `extensions.details` — Open VSX details + README + contributions.
    Details, "extensions.details", MarketplaceRef, MarketplaceDetails
);
method!(
    /// `extensions.icon` — Open VSX icon as a `data:` URL.
    Icon, "extensions.icon", ExtensionIconParams, ExtensionIconResult
);
method!(
    /// `extensions.install` — download from Open VSX (SHA-256 verified) and install.
    Install, "extensions.install", InstallExtensionParams, InstalledExtensionResult
);
method!(
    /// `extensions.installVsix` — install a local `.vsix`.
    InstallVsix, "extensions.installVsix", InstallVsixParams, InstalledExtensionResult
);
method!(
    /// `extensions.list` — installed extensions.
    List, "extensions.list", super::Empty, ListExtensionsResult
);
method!(
    /// `extensions.setEnabled`.
    SetEnabled, "extensions.setEnabled", SetExtensionEnabledParams, InstalledExtensionResult
);
method!(
    /// `extensions.uninstall`.
    Uninstall, "extensions.uninstall", ExtensionIdParams, super::Empty
);
method!(
    /// `extensions.readme` — README of an installed extension.
    Readme, "extensions.readme", ExtensionIdParams, ExtensionReadmeResult
);
method!(
    /// `extensions.theme` — resolved color theme.
    Theme, "extensions.theme", ThemeRef, ResolvedTheme
);
method!(
    /// `extensions.iconTheme` — resolved file icon theme.
    IconTheme, "extensions.iconTheme", ThemeRef, ResolvedIconTheme
);
method!(
    /// `extensions.readAsset` — grammar / icon / font file of an enabled extension.
    ReadAsset, "extensions.readAsset", ReadExtensionAssetParams, ExtensionAssetResult
);
method!(
    /// `extensions.languageConfig` — parsed language configuration.
    LanguageConfig, "extensions.languageConfig", ExtensionFileParams, LanguageConfigResult
);
method!(
    /// `extensions.snippets` — parsed snippets file.
    Snippets, "extensions.snippets", ExtensionFileParams, SnippetsResult
);
method!(
    /// `extensions.getPrefs` — editor theme / icon theme.
    GetPrefs, "extensions.getPrefs", super::Empty, EditorPrefs
);
method!(
    /// `extensions.setPrefs`.
    SetPrefs, "extensions.setPrefs", EditorPrefs, EditorPrefs
);
