/**
 * Editable Monaco editor for a conflict's merged result (lazy-loaded chunk).
 * `version` changes replace the content from `value` (e.g. "use ours",
 * "load AI suggestion"); typing reports through `onChange`.
 */
import { useEffect, useRef } from "react";

import { applyTheme, monaco, THEME } from "../monacoSetup";

export interface MonacoResultProps {
  value: string;
  version: number;
  language: string;
  ariaLabel: string;
  onChange: (value: string) => void;
}

export default function MonacoResult({ value, version, language, ariaLabel, onChange }: MonacoResultProps) {
  const host = useRef<HTMLDivElement>(null);
  const editor = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  useEffect(() => {
    if (!host.current) return;
    applyTheme();
    const model = monaco.editor.createModel(value, language);
    const e = monaco.editor.create(host.current, {
      model,
      theme: THEME,
      automaticLayout: true,
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--ok-font-mono") || undefined,
      fontSize: 12,
      ariaLabel,
    });
    const sub = e.onDidChangeModelContent(() => onChangeRef.current(e.getValue()));
    editor.current = e;
    return () => {
      sub.dispose();
      e.dispose();
      model.dispose();
      editor.current = null;
    };
    // created once; content is replaced below when `version` changes
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const e = editor.current;
    if (!e || e.getValue() === value) return;
    e.setValue(value);
    // only external replacements (version bumps) push content into the editor
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [version]);

  useEffect(() => {
    const model = editor.current?.getModel();
    if (model) monaco.editor.setModelLanguage(model, language);
  }, [language]);

  return <div ref={host} className="git-monaco" />;
}
