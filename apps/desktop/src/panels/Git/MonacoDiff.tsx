/**
 * Monaco DiffEditor (lazy-loaded chunk). Read-only, side-by-side or inline.
 */
import { useEffect, useRef } from "react";

import { applyTheme, monaco, THEME } from "./monacoSetup";

export interface MonacoDiffProps {
  original: string;
  modified: string;
  language: string;
  sideBySide: boolean;
}

export default function MonacoDiff({ original, modified, language, sideBySide }: MonacoDiffProps) {
  const host = useRef<HTMLDivElement>(null);
  const editor = useRef<monaco.editor.IStandaloneDiffEditor | null>(null);

  useEffect(() => {
    if (!host.current) return;
    applyTheme();
    const e = monaco.editor.createDiffEditor(host.current, {
      theme: THEME,
      readOnly: true,
      originalEditable: false,
      automaticLayout: true,
      renderSideBySide: sideBySide,
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--ok-font-mono") || undefined,
      fontSize: 12,
      renderOverviewRuler: false,
      ignoreTrimWhitespace: false,
    });
    editor.current = e;
    return () => {
      const model = e.getModel();
      e.dispose();
      model?.original.dispose();
      model?.modified.dispose();
      editor.current = null;
    };
    // created once; options/models are updated below
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const e = editor.current;
    if (!e) return;
    const prev = e.getModel();
    e.setModel({
      original: monaco.editor.createModel(original, language),
      modified: monaco.editor.createModel(modified, language),
    });
    prev?.original.dispose();
    prev?.modified.dispose();
  }, [original, modified, language]);

  useEffect(() => {
    editor.current?.updateOptions({ renderSideBySide: sideBySide });
  }, [sideBySide]);

  return <div ref={host} className="git-monaco" />;
}
