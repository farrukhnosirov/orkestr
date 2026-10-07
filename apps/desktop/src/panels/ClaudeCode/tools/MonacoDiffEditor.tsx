/**
 * Read-only Monaco inline diff of one edit. Loaded lazily (its own chunk) and
 * mounted only when a card is expanded with the editor view on. The theme is
 * the editor's global one (`getMonaco()` applies the Orkestr / VS Code theme),
 * so it follows the app theme like the Code panel.
 */
import type * as Monaco from "monaco-editor";
import { useEffect, useRef } from "react";

import { getMonaco } from "../../Code/monaco";

/** Monaco language id of a path (by extension / file name), `plaintext` when unknown. */
export function languageOf(monaco: typeof Monaco, path: string): string {
  const lower = path.toLowerCase().replace(/\\/g, "/");
  const base = lower.slice(lower.lastIndexOf("/") + 1);
  const ext = base.match(/\.[^.]+$/)?.[0];
  const lang = monaco.languages
    .getLanguages()
    .find((l) => (ext !== undefined && l.extensions?.includes(ext)) || l.filenames?.some((f) => f.toLowerCase() === base));
  return lang?.id ?? "plaintext";
}

const LINE_HEIGHT = 19;
const MAX_LINES = 32;

export interface MonacoDiffEditorProps {
  before: string;
  after: string;
  path: string;
  /** Accessible name of the editor region. */
  label: string;
}

export default function MonacoDiffEditor({ before, after, path, label }: MonacoDiffEditorProps) {
  const el = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const host = el.current;
    if (!host) return;
    const monaco = getMonaco();
    const lines = Math.min(MAX_LINES, Math.max(before.split("\n").length, after.split("\n").length) + 2);
    host.style.height = `${lines * LINE_HEIGHT + 8}px`;
    const editor = monaco.editor.createDiffEditor(host, {
      readOnly: true,
      originalEditable: false,
      renderSideBySide: false,
      automaticLayout: true,
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      renderOverviewRuler: false,
      lineNumbers: "off",
      folding: false,
      glyphMargin: false,
      fontSize: 12,
      lineHeight: LINE_HEIGHT,
      ariaLabel: label,
      hideUnchangedRegions: { enabled: true, contextLineCount: 3, minimumLineCount: 6 },
    });
    const lang = languageOf(monaco, path);
    const original = monaco.editor.createModel(before, lang);
    const modified = monaco.editor.createModel(after, lang);
    editor.setModel({ original, modified });
    return () => {
      editor.dispose();
      original.dispose();
      modified.dispose();
    };
  }, [before, after, path, label]);
  return <div className="cct-monaco" ref={el} />;
}
