/**
 * The VS Code workbench embedded in an Orkestr panel (ADR-0043).
 *
 * **Why a `vscode.<app host>` name:** the VS Code server swaps the `?tkn=`
 * connection token for a `SameSite=Lax` cookie. An `<iframe>` on
 * `http://127.0.0.1:<port>` inside `http://tauri.localhost` is a *cross-site*
 * frame, so that cookie never reaches the server's sub-requests and web
 * sockets. `vscode.tauri.localhost` is the same site as `tauri.localhost`
 * (ports do not matter for sites), and Chromium/WebView2 resolve every
 * `*.localhost` name to the loopback interface itself (RFC 6761) — so the
 * frame reaches the same `127.0.0.1` server with a first-party cookie.
 * The main window CSP allows exactly that host in `frame-src`.
 *
 * Pure helpers here (no React, no RPC): URL rewriting and the mapping of
 * Orkestr theme tokens to VS Code workbench colors.
 */

/** Host prefix of the embedded workbench (`vscode.` + the app's host). */
export const EMBED_HOST_PREFIX = "vscode.";

/** Sandbox of the workbench frame: no top-level navigation of the Orkestr window. */
export const EMBED_SANDBOX =
  "allow-scripts allow-same-origin allow-forms allow-popups allow-popups-to-escape-sandbox allow-modals allow-downloads allow-pointer-lock";

/** Permissions delegated to the frame (VS Code copy / paste). */
export const EMBED_ALLOW = "clipboard-read; clipboard-write";

/**
 * Host name under which the frame is same-site with the page at `appHost`,
 * or `null` when the workbench cannot be embedded there (a remote web
 * client, a `file:` page…).
 */
export function embedHostFor(appHost: string): string | null {
  const host = appHost.toLowerCase();
  // Dev server on the loopback itself: the server's own address is same-site.
  if (host === "127.0.0.1") return "127.0.0.1";
  if (host === "localhost") return "localhost";
  // Packaged app (`tauri.localhost`) or a detached window of it.
  if (host.endsWith(".localhost") && /^[a-z0-9.-]+$/.test(host)) return `${EMBED_HOST_PREFIX}${host}`;
  return null;
}

/**
 * Rewrites the server URL from `vscode.start` (`http://127.0.0.1:<port>/?tkn=…`)
 * for the frame; `null` when it cannot be embedded on `appHost`.
 */
export function embedUrlFor(serverUrl: string, appHost: string): string | null {
  let url: URL;
  try {
    url = new URL(serverUrl);
  } catch {
    return null;
  }
  if (url.protocol !== "http:" || url.hostname !== "127.0.0.1" || !url.port) return null;
  const host = embedHostFor(appHost);
  if (!host) return null;
  url.hostname = host;
  return url.toString();
}

/** Origin of an embed URL (`http://vscode.tauri.localhost:<port>`). */
export function embedOrigin(embedUrl: string): string | null {
  try {
    return new URL(embedUrl).origin;
  } catch {
    return null;
  }
}

// ── theme ───────────────────────────────────────────────────────────────

interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

/** Parses `#rgb[a]`, `#rrggbb[aa]`, `rgb()` / `rgba()` (legacy and space syntax). */
export function parseColor(value: string | null | undefined): Rgba | null {
  const v = (value ?? "").trim().toLowerCase();
  if (!v) return null;
  const hex = /^#([0-9a-f]{3,8})$/.exec(v);
  if (hex) {
    let h = hex[1] ?? "";
    if (h.length === 3 || h.length === 4) h = [...h].map((c) => c + c).join("");
    if (h.length !== 6 && h.length !== 8) return null;
    const n = (i: number) => parseInt(h.slice(i, i + 2), 16);
    return { r: n(0), g: n(2), b: n(4), a: h.length === 8 ? n(6) / 255 : 1 };
  }
  const m = /^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)(?:[\s,/]+([\d.]+%?))?\s*\)$/.exec(v);
  if (!m) return null;
  const alpha = m[4] === undefined ? 1 : m[4].endsWith("%") ? Number(m[4].slice(0, -1)) / 100 : Number(m[4]);
  const c = { r: Number(m[1]), g: Number(m[2]), b: Number(m[3]), a: alpha };
  return [c.r, c.g, c.b, c.a].every(Number.isFinite) ? c : null;
}

const byte = (n: number) => Math.max(0, Math.min(255, Math.round(n))).toString(16).padStart(2, "0");

/** `#rrggbb` (opaque) or `#rrggbbaa`. */
export function toHex(c: Rgba): string {
  const base = `#${byte(c.r)}${byte(c.g)}${byte(c.b)}`;
  return c.a >= 0.999 ? base : `${base}${byte(c.a * 255)}`;
}

/** `top` composited over an opaque `bottom` (glass surfaces → solid VS Code backgrounds). */
export function over(top: Rgba, bottom: Rgba): Rgba {
  const a = Math.max(0, Math.min(1, top.a));
  return {
    r: top.r * a + bottom.r * (1 - a),
    g: top.g * a + bottom.g * (1 - a),
    b: top.b * a + bottom.b * (1 - a),
    a: 1,
  };
}

/** Orkestr token (without `--ok-color-`) → raw CSS value. */
export type TokenReader = (token: string) => string | null;

/** VS Code color id → Orkestr token, and whether it must be opaque (a background). */
const MAP: readonly (readonly [string, string, "solid" | "keep"])[] = [
  // chrome
  ["foreground", "text", "solid"],
  ["descriptionForeground", "text-muted", "solid"],
  ["focusBorder", "accent", "keep"],
  ["contrastBorder", "border", "keep"],
  ["widget.border", "border", "keep"],
  ["selection.background", "accent-soft", "keep"],
  ["textLink.foreground", "accent", "solid"],
  ["textLink.activeForeground", "accent", "solid"],
  ["errorForeground", "danger", "solid"],
  ["icon.foreground", "text-muted", "solid"],
  ["sash.hoverBorder", "accent", "keep"],
  ["progressBar.background", "accent", "solid"],
  ["badge.background", "accent", "solid"],
  ["badge.foreground", "text-inverse", "solid"],
  ["titleBar.activeBackground", "bg", "solid"],
  ["titleBar.inactiveBackground", "bg", "solid"],
  ["titleBar.activeForeground", "text", "solid"],
  ["titleBar.inactiveForeground", "text-muted", "solid"],
  ["titleBar.border", "border", "keep"],
  ["activityBar.background", "bg", "solid"],
  ["activityBar.foreground", "text", "solid"],
  ["activityBar.inactiveForeground", "text-subtle", "solid"],
  ["activityBar.activeBorder", "accent", "solid"],
  ["activityBar.border", "border", "keep"],
  ["activityBarBadge.background", "accent", "solid"],
  ["activityBarBadge.foreground", "text-inverse", "solid"],
  ["sideBar.background", "bg", "solid"],
  ["sideBar.foreground", "text", "solid"],
  ["sideBar.border", "border", "keep"],
  ["sideBarTitle.foreground", "text-muted", "solid"],
  ["sideBarSectionHeader.background", "bg", "solid"],
  ["sideBarSectionHeader.border", "border", "keep"],
  ["panel.background", "bg", "solid"],
  ["panel.border", "border", "keep"],
  ["panelTitle.activeForeground", "text", "solid"],
  ["panelTitle.inactiveForeground", "text-muted", "solid"],
  ["panelTitle.activeBorder", "accent", "solid"],
  ["statusBar.background", "bg", "solid"],
  ["statusBar.foreground", "text-muted", "solid"],
  ["statusBar.border", "border", "keep"],
  ["statusBar.noFolderBackground", "bg", "solid"],
  ["statusBarItem.remoteBackground", "accent", "solid"],
  ["statusBarItem.remoteForeground", "text-inverse", "solid"],
  // editor
  ["editor.background", "bg-elevated", "solid"],
  ["editor.foreground", "text", "solid"],
  ["editorLineNumber.foreground", "text-subtle", "solid"],
  ["editorLineNumber.activeForeground", "text-muted", "solid"],
  ["editorCursor.foreground", "accent", "solid"],
  ["editor.selectionBackground", "accent-soft", "keep"],
  ["editor.lineHighlightBackground", "surface", "keep"],
  ["editorGutter.background", "bg-elevated", "solid"],
  ["editorWidget.background", "bg-elevated", "solid"],
  ["editorWidget.border", "border", "keep"],
  ["editorGroup.border", "border", "keep"],
  ["editorGroupHeader.tabsBackground", "bg", "solid"],
  ["editorGroupHeader.tabsBorder", "border", "keep"],
  ["editorError.foreground", "danger", "solid"],
  ["editorWarning.foreground", "warning", "solid"],
  ["editorInfo.foreground", "accent", "solid"],
  ["tab.activeBackground", "bg-elevated", "solid"],
  ["tab.inactiveBackground", "bg", "solid"],
  ["tab.activeForeground", "text", "solid"],
  ["tab.inactiveForeground", "text-muted", "solid"],
  ["tab.border", "border", "keep"],
  ["tab.activeBorderTop", "accent", "solid"],
  ["breadcrumb.background", "bg-elevated", "solid"],
  ["breadcrumb.foreground", "text-muted", "solid"],
  // lists, inputs, menus
  ["list.hoverBackground", "surface-hover", "solid"],
  ["list.activeSelectionBackground", "surface-active", "solid"],
  ["list.activeSelectionForeground", "text", "solid"],
  ["list.inactiveSelectionBackground", "surface-active", "solid"],
  ["list.focusOutline", "accent", "keep"],
  ["list.highlightForeground", "accent", "solid"],
  ["input.background", "surface-sunken", "solid"],
  ["input.foreground", "text", "solid"],
  ["input.border", "border", "keep"],
  ["input.placeholderForeground", "text-subtle", "solid"],
  ["inputOption.activeBorder", "accent", "solid"],
  ["dropdown.background", "bg-elevated", "solid"],
  ["dropdown.border", "border", "keep"],
  ["dropdown.foreground", "text", "solid"],
  ["quickInput.background", "bg-elevated", "solid"],
  ["quickInput.foreground", "text", "solid"],
  ["quickInputList.focusBackground", "surface-active", "solid"],
  ["menu.background", "bg-elevated", "solid"],
  ["menu.foreground", "text", "solid"],
  ["menu.selectionBackground", "surface-active", "solid"],
  ["menu.border", "border", "keep"],
  ["notifications.background", "bg-elevated", "solid"],
  ["notifications.foreground", "text", "solid"],
  ["notifications.border", "border", "keep"],
  ["button.background", "accent", "solid"],
  ["button.foreground", "text-inverse", "solid"],
  ["button.secondaryBackground", "surface-active", "solid"],
  ["button.secondaryForeground", "text", "solid"],
  ["checkbox.background", "surface-sunken", "solid"],
  ["checkbox.border", "border", "keep"],
  ["scrollbarSlider.background", "surface-hover", "keep"],
  ["scrollbarSlider.hoverBackground", "surface-active", "keep"],
  ["scrollbarSlider.activeBackground", "accent-soft", "keep"],
  // terminal
  ["terminal.background", "bg-elevated", "solid"],
  ["terminal.foreground", "text", "solid"],
  ["terminalCursor.foreground", "accent", "solid"],
];

/** Every VS Code color id the theme sync sets. */
export const SYNCED_COLOR_IDS: readonly string[] = MAP.map(([id]) => id);

/**
 * Orkestr theme → `workbench.colorCustomizations`. Translucent ("glass")
 * backgrounds are composited over `--ok-color-bg` so VS Code gets solid
 * colors; tokens that are missing or unparsable are left to the base theme.
 */
export function workbenchColors(read: TokenReader): Record<string, string> {
  const bg = parseColor(read("bg"));
  const base = bg ? { ...bg, a: 1 } : null;
  const out: Record<string, string> = {};
  for (const [id, token, mode] of MAP) {
    const c = parseColor(read(token));
    if (!c) continue;
    if (mode === "solid") {
      if (c.a < 0.999 && !base) continue;
      out[id] = toHex(c.a < 0.999 && base ? over(c, base) : c);
    } else {
      out[id] = toHex(c);
    }
  }
  return out;
}

/** Whether the theme is light (`color-scheme` of the root, as Monaco uses it). */
export function isLightScheme(colorScheme: string): boolean {
  // The first keyword is the preferred scheme (`light`, `dark`, `light dark`…).
  return colorScheme.trim().toLowerCase().split(/\s+/)[0] === "light";
}

/** Reads the current theme from the document (browser only). */
export function documentThemeColors(doc: Document = document): { light: boolean; colors: Record<string, string> } {
  const css = getComputedStyle(doc.documentElement);
  const read: TokenReader = (token) => css.getPropertyValue(`--ok-color-${token}`) || null;
  return { light: isLightScheme(css.getPropertyValue("color-scheme").trim()), colors: workbenchColors(read) };
}
