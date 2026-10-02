/**
 * File-type icons for the Files tree, tabs and quick open. Colors are token
 * names (`--ok-*`), never hardcoded values (contract §8).
 */
import {
  Database,
  File,
  FileArchive,
  FileAudio,
  FileBraces,
  FileCode,
  FileCog,
  FileImage,
  FileKey,
  FileLock,
  FileSpreadsheet,
  FileTerminal,
  FileText,
  FileType,
  FileVideo,
  Folder,
  FolderOpen,
  type LucideIcon,
} from "lucide-react";

import { extension } from "./paths";

export interface FileIcon {
  icon: LucideIcon;
  /** CSS custom property used as the icon color. */
  color: string;
}

const C = {
  accent: "var(--ok-color-accent)",
  violet: "var(--ok-color-violet)",
  magenta: "var(--ok-color-magenta)",
  success: "var(--ok-color-success)",
  warning: "var(--ok-color-warning)",
  danger: "var(--ok-color-danger)",
  info: "var(--ok-color-info)",
  muted: "var(--ok-color-text-muted)",
  subtle: "var(--ok-color-text-subtle)",
} as const;

const BY_EXT: Record<string, FileIcon> = {};
function map(exts: string[], icon: LucideIcon, color: string): void {
  for (const e of exts) BY_EXT[e] = { icon, color };
}
map(["ts", "tsx", "mts", "cts"], FileCode, C.info);
map(["js", "jsx", "mjs", "cjs"], FileCode, C.warning);
map(["rs"], FileCode, C.danger);
map(["py", "pyi", "go", "java", "kt", "cs", "c", "h", "cpp", "hpp", "cc", "swift", "rb", "php", "lua", "dart", "scala"], FileCode, C.accent);
map(["html", "htm", "vue", "svelte", "astro", "xml", "svg"], FileCode, C.magenta);
map(["css", "scss", "less"], FileType, C.violet);
map(["json", "jsonc", "json5", "yaml", "yml", "toml"], FileBraces, C.warning);
map(["md", "mdx", "markdown", "txt", "rst", "adoc"], FileText, C.muted);
map(["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "avif"], FileImage, C.success);
map(["mp4", "mov", "webm", "mkv", "avi"], FileVideo, C.violet);
map(["mp3", "wav", "ogg", "flac", "m4a"], FileAudio, C.violet);
map(["zip", "gz", "tgz", "tar", "7z", "rar", "xz", "bz2"], FileArchive, C.subtle);
map(["sh", "bash", "zsh", "fish", "ps1", "psm1", "bat", "cmd"], FileTerminal, C.success);
map(["sql", "db", "sqlite", "sqlite3"], Database, C.info);
map(["csv", "tsv", "xlsx", "xls"], FileSpreadsheet, C.success);
map(["pem", "key", "crt", "cer", "p12", "pfx"], FileKey, C.danger);
map(["lock"], FileLock, C.subtle);
map(["ini", "cfg", "conf", "properties", "env"], FileCog, C.subtle);

const BY_NAME: Record<string, FileIcon> = {
  "package.json": { icon: FileBraces, color: C.success },
  "cargo.toml": { icon: FileCog, color: C.danger },
  "cargo.lock": { icon: FileLock, color: C.subtle },
  dockerfile: { icon: FileCog, color: C.info },
  makefile: { icon: FileTerminal, color: C.warning },
  ".gitignore": { icon: FileCog, color: C.subtle },
  ".editorconfig": { icon: FileCog, color: C.subtle },
  license: { icon: FileText, color: C.warning },
  "readme.md": { icon: FileText, color: C.accent },
};

/** Icon for an entry. */
export function fileIcon(name: string, kind: "file" | "dir", open = false): FileIcon {
  if (kind === "dir") return { icon: open ? FolderOpen : Folder, color: open ? C.accent : C.muted };
  const lower = name.toLowerCase();
  const byName = BY_NAME[lower];
  if (byName) return byName;
  if (lower === ".env" || lower.startsWith(".env.")) return { icon: FileKey, color: C.warning };
  return BY_EXT[extension(lower)] ?? { icon: File, color: C.subtle };
}
