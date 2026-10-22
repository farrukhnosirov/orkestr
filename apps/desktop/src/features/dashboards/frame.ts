/**
 * Sandboxed document for plugin widgets (ADR-0034): the iframe gets only
 * `allow-scripts` (opaque origin → no parent `__TAURI_INTERNALS__`, no parent DOM,
 * no app storage) and a CSP meta injected as the first element of `<head>`,
 * so no network and no external resources.
 */

/** Exact sandbox tokens of a widget iframe (never add `allow-same-origin`). */
export const WIDGET_SANDBOX = "allow-scripts";

/** CSP injected into every widget document (mirrors `WIDGET_CSP` in core `rpc/widgets.rs`). */
export const WIDGET_CSP =
  "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; " +
  "img-src data:; font-src data:; connect-src 'none'; media-src 'none'; object-src 'none'; frame-src 'none'; " +
  "worker-src 'none'; form-action 'none'; base-uri 'none'";

function escapeAttr(v: string): string {
  return v.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
}

/**
 * Builds the `srcdoc` document: the CSP meta goes first in `<head>` (before
 * any widget markup can run) and existing CSP metas of the widget are kept
 * (CSPs only ever intersect, so they can only tighten).
 */
export function buildWidgetDocument(html: string, csp: string = WIDGET_CSP): string {
  const meta = `<meta http-equiv="Content-Security-Policy" content="${escapeAttr(csp)}"><meta name="referrer" content="no-referrer">`;
  const head = /<head(\s[^>]*)?>/i.exec(html);
  if (head) return html.slice(0, head.index + head[0].length) + meta + html.slice(head.index + head[0].length);
  const htmlTag = /<html(\s[^>]*)?>/i.exec(html);
  if (htmlTag) {
    const at = htmlTag.index + htmlTag[0].length;
    return `${html.slice(0, at)}<head>${meta}</head>${html.slice(at)}`;
  }
  return `<!doctype html><html><head>${meta}</head><body>${html}</body></html>`;
}

/** Theme tokens forwarded to widgets (`--ok-<name>` → value). */
export const THEME_TOKEN_NAMES: readonly string[] = [
  "color-bg", "color-bg-elevated", "color-surface", "color-surface-hover", "color-surface-strong", "color-surface-sunken",
  "color-border", "color-border-strong", "color-text", "color-text-muted", "color-text-subtle", "color-text-inverse",
  "color-accent", "color-accent-soft", "color-accent-strong", "color-success", "color-success-soft", "color-warning",
  "color-warning-soft", "color-danger", "color-danger-soft", "color-info", "color-info-soft", "color-violet",
  "font-sans", "font-mono", "font-size-xs", "font-size-sm", "font-size-md", "font-size-lg",
];

/** Current theme tokens read from the app root. */
export function readThemeTokens(root: Element = document.documentElement): Record<string, string> {
  const style = getComputedStyle(root);
  const out: Record<string, string> = {};
  for (const name of THEME_TOKEN_NAMES) {
    const v = style.getPropertyValue(`--ok-${name}`).trim();
    if (v) out[name] = v;
  }
  return out;
}

/**
 * Optional loader serving the widget document from a dedicated URL instead of
 * `srcdoc`. Browsers make `srcdoc` documents inherit the embedder's CSP, so
 * inside the Tauri app (whose CSP has no `'unsafe-inline'` scripts) inline
 * widget scripts would be blocked; the app shell registers a loader that
 * serves the document from a custom scheme with {@link WIDGET_CSP} as its own
 * header (and allows that scheme in `frame-src`). The iframe stays
 * `sandbox="allow-scripts"`, so its origin is still opaque (`"null"`).
 */
export type WidgetDocumentLoader = (doc: { widgetId: string; instanceId: string; html: string; csp: string }) => Promise<string>;

let documentLoader: WidgetDocumentLoader | null = null;

export function setWidgetDocumentLoader(loader: WidgetDocumentLoader | null): void {
  documentLoader = loader;
}

export function widgetDocumentLoader(): WidgetDocumentLoader | null {
  return documentLoader;
}
