/**
 * Monaco bootstrap (loaded lazily by the Code panel so the editor bundle is
 * not part of the shell's startup path).
 *
 * - Web workers via Vite `?worker` imports (editor, JSON, CSS, HTML, TS/JS)
 *   — no CDN, works offline inside Tauri.
 * - Built-in language services provide diagnostics; module-resolution
 *   errors are ignored because the editor sees one file at a time.
 * - Theme `orkestr-dark` generated from the live `--ok-*` tokens.
 */
import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import CssWorker from "monaco-editor/language/css/css.worker?worker";
import HtmlWorker from "monaco-editor/language/html/html.worker?worker";
import JsonWorker from "monaco-editor/language/json/json.worker?worker";
import TsWorker from "monaco-editor/language/typescript/ts.worker?worker";

import { buildTheme, MONACO_THEME } from "./theme";

export type MonacoApi = typeof monaco;

let configured = false;

function configure(): void {
  if (configured) return;
  configured = true;
  globalThis.MonacoEnvironment = {
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
  const ts = monaco.typescript;
  const compilerOptions = {
    target: ts.ScriptTarget.ESNext,
    module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.NodeJs,
    jsx: ts.JsxEmit.ReactJSX,
    allowJs: true,
    allowNonTsExtensions: true,
    esModuleInterop: true,
    strict: true,
  };
  ts.typescriptDefaults.setCompilerOptions(compilerOptions);
  ts.javascriptDefaults.setCompilerOptions(compilerOptions);
  // 2307/2792: cannot find module; 7016: no declaration file — expected for single-file editing.
  const diagnostics = { noSemanticValidation: false, noSyntaxValidation: false, diagnosticCodesToIgnore: [2307, 2792, 7016] };
  ts.typescriptDefaults.setDiagnosticsOptions(diagnostics);
  ts.javascriptDefaults.setDiagnosticsOptions(diagnostics);
  applyTokensTheme();
}

/** (Re)defines `orkestr-dark` from the current CSS tokens and activates it. */
export function applyTokensTheme(): void {
  const style = getComputedStyle(document.documentElement);
  monaco.editor.defineTheme(MONACO_THEME, buildTheme((t) => style.getPropertyValue(t)));
  monaco.editor.setTheme(MONACO_THEME);
}

/** Configured Monaco API. */
export function getMonaco(): MonacoApi {
  configure();
  return monaco;
}

/** Model URI for a workspace file (one model per project + path). */
export function modelUri(projectId: string, path: string): monaco.Uri {
  return monaco.Uri.from({ scheme: "orkestr", authority: projectId, path: `/${path}` });
}
