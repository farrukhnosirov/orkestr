/** Monaco inline diff editor for one edit (lazy chunk of the Claude Code panel). */
import type * as Monaco from "monaco-editor";
import { useEffect, useRef } from "react";

import { getMonaco } from "../Code/monaco";

function languageOf(monaco: typeof Monaco, path: string): string {
  const ext = path.toLowerCase().match(/\.[^./\\]+$/)?.[0];
  if (!ext) return "plaintext";
  const lang = monaco.languages.getLanguages().find((l) => l.extensions?.includes(ext));
  return lang?.id ?? "plaintext";
}

export default function MonacoDiff({ before, after, path }: { before: string; after: string; path: string }) {
  const el = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!el.current) return;
    const monaco = getMonaco();
    const lines = Math.min(30, Math.max(before.split("\n").length, after.split("\n").length) + 2);
    el.current.style.height = `${lines * 19 + 8}px`;
    const editor = monaco.editor.createDiffEditor(el.current, {
      readOnly: true,
      originalEditable: false,
      renderSideBySide: false,
      automaticLayout: true,
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      renderOverviewRuler: false,
      lineNumbers: "off",
      fontSize: 12,
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
  }, [before, after, path]);
  return <div className="cc-monaco-diff" ref={el} />;
}
