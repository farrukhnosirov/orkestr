/**
 * VS Code file icon themes in the Files panel and the Code tabs (ADR-0025).
 *
 * The core resolves the theme (`extensions.iconTheme`): icon definitions
 * with extension-relative image paths or icon-font glyphs, plus name → icon
 * associations. Icons are loaded lazily through `extensions.readAsset` and
 * shown as `data:` URLs in `<img>` (an SVG in an image context never runs
 * scripts); icon fonts are registered with the FontFace API from bytes.
 */
import type { IconAssociations, IconDefinition, InstalledExtension, ResolvedIconTheme } from "@orkestr/protocol";
import { create } from "zustand";

import { maybeClient } from "../../core/client";

/** What to draw for one file / folder. */
export type ThemedIcon =
  | { kind: "image"; src: string }
  | { kind: "glyph"; char: string; family: string; color: string | null; size: string | null };

/** Built-in extension → language id (for `languageIds` associations). */
const BUILTIN_LANGUAGES: Record<string, string> = {
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "typescriptreact",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "javascriptreact",
  json: "json",
  jsonc: "jsonc",
  md: "markdown",
  rs: "rust",
  py: "python",
  go: "go",
  java: "java",
  kt: "kotlin",
  cs: "csharp",
  c: "c",
  h: "c",
  cpp: "cpp",
  cc: "cpp",
  hpp: "cpp",
  html: "html",
  htm: "html",
  css: "css",
  scss: "scss",
  less: "less",
  yaml: "yaml",
  yml: "yaml",
  toml: "toml",
  xml: "xml",
  sh: "shellscript",
  ps1: "powershell",
  sql: "sql",
  rb: "ruby",
  php: "php",
  swift: "swift",
  dart: "dart",
  lua: "lua",
  vue: "vue",
  svelte: "svelte",
};

/** Language id of a file name from built-ins plus contributed languages. */
export function languageOfName(name: string, extensions: readonly InstalledExtension[] = []): string | null {
  const lower = name.toLowerCase();
  for (const ext of extensions) {
    if (!ext.enabled) continue;
    for (const lang of ext.contributions.languages) {
      if (lang.filenames.some((f) => f.toLowerCase() === lower)) return lang.id;
      if (lang.extensions.some((e) => lower.endsWith(e.toLowerCase()))) return lang.id;
    }
  }
  const dot = lower.lastIndexOf(".");
  return dot >= 0 ? BUILTIN_LANGUAGES[lower.slice(dot + 1)] ?? null : null;
}

/** Light-theme associations override the dark ones key by key. */
export function mergeAssociations(dark: IconAssociations, light: IconAssociations | null): IconAssociations {
  if (!light) return dark;
  return {
    file: light.file ?? dark.file,
    folder: light.folder ?? dark.folder,
    folderExpanded: light.folderExpanded ?? dark.folderExpanded,
    fileExtensions: { ...dark.fileExtensions, ...light.fileExtensions },
    fileNames: { ...dark.fileNames, ...light.fileNames },
    folderNames: { ...dark.folderNames, ...light.folderNames },
    folderNamesExpanded: { ...dark.folderNamesExpanded, ...light.folderNamesExpanded },
    languageIds: { ...dark.languageIds, ...light.languageIds },
  };
}

/**
 * Icon id for a name, following VS Code's precedence: exact file name,
 * then the longest matching extension (`d.ts` before `ts`), then the
 * language id, then the default file icon. Folders: expanded / plain folder
 * names, then the default folder icons.
 */
export function resolveIconId(
  a: IconAssociations,
  name: string,
  kind: "file" | "dir",
  open: boolean,
  languageId: string | null,
): string | null {
  const lower = name.toLowerCase();
  if (kind === "dir") {
    if (open) return a.folderNamesExpanded[lower] ?? a.folderNames[lower] ?? a.folderExpanded ?? a.folder ?? null;
    return a.folderNames[lower] ?? a.folder ?? null;
  }
  const byName = a.fileNames[lower];
  if (byName) return byName;
  let rest = lower;
  for (let i = rest.indexOf("."); i >= 0; i = rest.indexOf(".")) {
    rest = rest.slice(i + 1);
    const hit = a.fileExtensions[rest];
    if (hit) return hit;
  }
  if (languageId) {
    const hit = a.languageIds[languageId];
    if (hit) return hit;
  }
  return a.file ?? null;
}

function utf8ToBase64(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

function base64ToBytes(b64: string): Uint8Array<ArrayBuffer> {
  const bin = atob(b64);
  const out = new Uint8Array(new ArrayBuffer(bin.length));
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** CSS-escaped glyph (`\E001` → the character). */
export function glyphOf(fontCharacter: string): string {
  const m = /^\\([0-9a-fA-F]{1,6})$/.exec(fontCharacter.trim());
  if (m?.[1]) return String.fromCodePoint(parseInt(m[1], 16));
  return fontCharacter;
}

interface IconThemeState {
  theme: ResolvedIconTheme | null;
  associations: IconAssociations | null;
  /** Icon id → loaded icon (or null = failed). */
  icons: Record<string, ThemedIcon | null>;
  /** Font id → CSS family. */
  fonts: Record<string, string>;
  loadedKey: string | null;
}

export const useIconThemeStore = create<IconThemeState>(() => ({
  theme: null,
  associations: null,
  icons: {},
  fonts: {},
  loadedKey: null,
}));

const pending = new Set<string>();

/** Loads (or clears) the icon theme named by the preferences. */
export async function applyIconTheme(ref: { extensionId: string; themeId: string } | null, light: boolean): Promise<void> {
  const key = ref ? `${ref.extensionId}/${ref.themeId}/${light ? "l" : "d"}` : null;
  if (useIconThemeStore.getState().loadedKey === key) return;
  if (!ref) {
    useIconThemeStore.setState({ theme: null, associations: null, icons: {}, fonts: {}, loadedKey: null });
    return;
  }
  const client = maybeClient();
  if (!client) return;
  const theme = await client.call("extensions.iconTheme", ref);
  const fonts: Record<string, string> = {};
  for (const f of theme.fonts) {
    const family = `orkestr-icon-${theme.theme.extensionId}-${f.id}`.replace(/[^a-zA-Z0-9-]/g, "-");
    try {
      const asset = await client.call("extensions.readAsset", { extensionId: theme.theme.extensionId, path: f.path });
      if (asset.base64) {
        const face = new FontFace(family, base64ToBytes(asset.base64), {
          weight: f.weight ?? "normal",
          style: f.style ?? "normal",
        });
        await face.load();
        document.fonts.add(face);
        fonts[f.id] = family;
      }
    } catch (e) {
      console.warn("icon font failed to load", f.path, e);
    }
  }
  pending.clear();
  useIconThemeStore.setState({
    theme,
    associations: mergeAssociations(theme.dark, light ? theme.light : null),
    icons: {},
    fonts,
    loadedKey: key,
  });
}

function glyphIcon(def: IconDefinition, theme: ResolvedIconTheme, fonts: Record<string, string>): ThemedIcon | null {
  if (!def.fontCharacter) return null;
  const fontId = def.fontId ?? theme.fonts[0]?.id;
  const family = fontId ? fonts[fontId] : undefined;
  if (!family) return null;
  const size = def.fontSize ?? (fontId ? theme.fonts.find((f) => f.id === fontId)?.size ?? null : null);
  return { kind: "glyph", char: glyphOf(def.fontCharacter), family, color: def.fontColor, size };
}

/** Requests an icon's image (async; the store updates when it arrives). */
export function requestIcon(id: string): void {
  const st = useIconThemeStore.getState();
  const theme = st.theme;
  if (!theme || id in st.icons || pending.has(id)) return;
  const def = theme.definitions[id];
  if (!def) {
    useIconThemeStore.setState({ icons: { ...st.icons, [id]: null } });
    return;
  }
  if (!def.iconPath) {
    useIconThemeStore.setState({ icons: { ...st.icons, [id]: glyphIcon(def, theme, st.fonts) } });
    return;
  }
  const client = maybeClient();
  if (!client) return;
  pending.add(id);
  const key = st.loadedKey;
  client
    .call("extensions.readAsset", { extensionId: theme.theme.extensionId, path: def.iconPath })
    .then((a) => {
      const src = a.text !== null ? `data:${a.mime};base64,${utf8ToBase64(a.text)}` : a.base64 !== null ? `data:${a.mime};base64,${a.base64}` : null;
      return src ? ({ kind: "image", src } as const) : null;
    })
    .catch(() => null)
    .then((icon) => {
      pending.delete(id);
      const now = useIconThemeStore.getState();
      if (now.loadedKey !== key) return;
      useIconThemeStore.setState({ icons: { ...now.icons, [id]: icon } });
    });
}
