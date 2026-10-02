/**
 * Workspace-relative path helpers (`/`-separated, `""` = root), mirroring
 * the wire format of `files.*`.
 */

/** Last path component (`""` for the root). */
export function basename(path: string): string {
  const i = path.lastIndexOf("/");
  return i < 0 ? path : path.slice(i + 1);
}

/** Parent path (`""` for top-level entries and the root). */
export function dirname(path: string): string {
  const i = path.lastIndexOf("/");
  return i < 0 ? "" : path.slice(0, i);
}

/** Joins a directory and a child name. */
export function joinPath(dir: string, name: string): string {
  const clean = name.replace(/\\/g, "/").replace(/^\/+|\/+$/g, "");
  return dir ? `${dir}/${clean}` : clean;
}

/** Whether `path` equals `ancestor` or lies below it (`""` contains everything). */
export function isWithin(path: string, ancestor: string): boolean {
  return ancestor === "" || path === ancestor || path.startsWith(`${ancestor}/`);
}

/** Rewrites `path` when `from` (or one of its ancestors) moved to `to`; otherwise returns it unchanged. */
export function movePath(path: string, from: string, to: string): string {
  if (path === from) return to;
  if (from !== "" && path.startsWith(`${from}/`)) return `${to}${path.slice(from.length)}`;
  return path;
}

/** Path components for breadcrumbs: `a/b/c` → [`a`, `a/b`, `a/b/c`]. */
export function ancestry(path: string): { name: string; path: string }[] {
  if (!path) return [];
  const parts = path.split("/");
  return parts.map((name, i) => ({ name, path: parts.slice(0, i + 1).join("/") }));
}

/** File extension in lower case without the dot (`""` when none; dot-files have none). */
export function extension(name: string): string {
  const i = name.lastIndexOf(".");
  return i <= 0 ? "" : name.slice(i + 1).toLowerCase();
}

/** Client-side validation of a new file/folder name (the core re-validates). */
export function validateName(name: string): string | null {
  const n = name.trim();
  if (!n) return "Enter a name.";
  if (n === "." || n === "..") return "That name is reserved.";
  if (/[<>:"|?*\u0000-\u001f]/.test(n)) return "Names cannot contain < > : \" | ? * or control characters.";
  const parts = n.split(/[\\/]/).filter((p) => p !== "" && p !== ".");
  if (parts.some((p) => p === "..")) return "Names cannot contain `..`.";
  if (parts.some((p) => /[. ]$/.test(p))) return "Names cannot end with a dot or a space.";
  if (parts.some((p) => /^(con|prn|aux|nul|com[0-9]|lpt[0-9])( *\..*)?$/i.test(p))) return "That name is reserved on Windows.";
  return null;
}
