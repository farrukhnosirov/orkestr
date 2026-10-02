/**
 * Monaco bootstrap for Vite: web workers via `?worker` imports and an
 * `orkestr` theme derived from the active `--ok-*` design tokens.
 * Loaded lazily (only when a diff is shown), so Monaco never lands in the
 * initial bundle or in unit tests.
 *
 * Integration note: if another panel configures `MonacoEnvironment` too, the
 * last one wins — both map the same labels to the same workers.
 */
import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import CssWorker from "monaco-editor/language/css/css.worker?worker";
import HtmlWorker from "monaco-editor/language/html/html.worker?worker";
import JsonWorker from "monaco-editor/language/json/json.worker?worker";
import TsWorker from "monaco-editor/language/typescript/ts.worker?worker";

import { cssColorToHex } from "./model";

declare global {
  interface Window {
    MonacoEnvironment?: monaco.Environment;
  }
}

if (!window.MonacoEnvironment) {
  window.MonacoEnvironment = {
    getWorker(_workerId: string, label: string): Worker {
      switch (label) {
        case "json":
          return new JsonWorker();
        case "css":
        case "scss":
        case "less":
          return new CssWorker();
        case "html":
        case "handlebars":
        case "razor":
          return new HtmlWorker();
        case "typescript":
        case "javascript":
          return new TsWorker();
        default:
          return new EditorWorker();
      }
    },
  };
}

export const THEME = "orkestr";

/** (Re)defines the Monaco theme from the current CSS tokens. */
export function applyTheme(): void {
  const css = getComputedStyle(document.documentElement);
  const token = (name: string, fallback: string) => cssColorToHex(css.getPropertyValue(name), fallback);
  const light = css.getPropertyValue("color-scheme").includes("light");
  const text = token("--ok-color-text", "#e8ecff");
  monaco.editor.defineTheme(THEME, {
    base: light ? "vs" : "vs-dark",
    inherit: true,
    rules: [],
    colors: {
      "editor.background": token("--ok-color-bg-elevated", "#0a0d1c"),
      "editor.foreground": text,
      "editorLineNumber.foreground": token("--ok-color-text-subtle", "#646d96"),
      "editorLineNumber.activeForeground": token("--ok-color-text-muted", "#9aa3c8"),
      "editor.selectionBackground": token("--ok-color-surface-active", "#26326480"),
      "editorGutter.background": token("--ok-color-bg-elevated", "#0a0d1c"),
      "diffEditor.insertedTextBackground": token("--ok-color-success-soft", "#34d39924"),
      "diffEditor.removedTextBackground": token("--ok-color-danger-soft", "#fb718524"),
      "diffEditor.insertedLineBackground": token("--ok-color-success-soft", "#34d39924"),
      "diffEditor.removedLineBackground": token("--ok-color-danger-soft", "#fb718524"),
      "diffEditor.border": token("--ok-color-border", "#889cff1f"),
      "scrollbarSlider.background": token("--ok-color-surface-hover", "#465aaa29"),
    },
  });
}

applyTheme();

export { monaco };
