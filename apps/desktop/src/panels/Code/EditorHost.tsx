/**
 * Monaco hosts: the code editor (one instance, models swapped per tab) and
 * the diff editor (last saved / disk version vs. the live model).
 */
import type * as Monaco from "monaco-editor";
import { ClipboardPaste, Copy, FileCode2, ListTree, PencilLine, Replace, Scissors, Sparkles, SquareTerminal, TextSelect, WandSparkles } from "lucide-react";
import { useEffect, useRef } from "react";

import { type MenuEntry, useContextMenu } from "../../features/contextMenu";

import { toRanges } from "../../features/files/lineDiff";
import { t, useLanguage } from "../../i18n";
import { useEditorStore } from "../../store/editor";
import { notifySelectionChanged, setSelectionSource } from "../../features/claudeCode/bridge";
import type { Doc } from "./documents";
import { editorThemeName } from "./ext/editorExtensions";
import { tokenHex } from "./theme";
import type { MonacoApi } from "./monaco";

export interface EditorCallbacks {
  onSave: () => void;
  onQuickOpen: () => void;
  onSearch: () => void;
  onAskManager: (text: string, startLine: number, endLine: number) => void;
  /** "Ask Claude about selection" (Claude Code panel). */
  onAskClaude: (text: string, startLine: number, endLine: number) => void;
  onCursor: (line: number, column: number, selected: number) => void;
  /** "Open in VS Code" at the cursor (the embedded VS Code, ADR-0043). */
  onOpenInVsCode?: (line: number, column: number) => void;
}

function baseOptions(): Monaco.editor.IStandaloneEditorConstructionOptions {
  const style = getComputedStyle(document.documentElement);
  const mono = style.getPropertyValue("--ok-font-mono").trim();
  return {
    theme: editorThemeName(),
    automaticLayout: true,
    fontFamily: mono || "Consolas, monospace",
    fontSize: 13,
    lineHeight: 20,
    fontLigatures: true,
    minimap: { enabled: true, renderCharacters: false, scale: 1 },
    smoothScrolling: true,
    cursorBlinking: "smooth",
    cursorSmoothCaretAnimation: "on",
    renderLineHighlight: "all",
    scrollBeyondLastLine: false,
    padding: { top: 10, bottom: 10 },
    bracketPairColorization: { enabled: true },
    guides: { bracketPairs: "active", indentation: true },
    stickyScroll: { enabled: true },
    fixedOverflowWidgets: true,
    // Monaco's built-in menu is replaced by Orkestr's (themed, localized):
    // see `editorMenu` below.
    contextmenu: false,
    glyphMargin: false,
    lineDecorationsWidth: 12,
    tabSize: 2,
    detectIndentation: true,
  };
}

function registerCommands(monaco: MonacoApi, editor: Monaco.editor.IStandaloneCodeEditor, cb: { current: EditorCallbacks }): void {
  const { KeyMod, KeyCode } = monaco;
  editor.addCommand(KeyMod.CtrlCmd | KeyCode.KeyS, () => cb.current.onSave());
  editor.addCommand(KeyMod.CtrlCmd | KeyCode.KeyP, () => cb.current.onQuickOpen());
  editor.addCommand(KeyMod.CtrlCmd | KeyMod.Shift | KeyCode.KeyF, () => cb.current.onSearch());
  editor.addAction({
    id: "orkestr.askManager",
    label: t("code:editor.askManager"),
    contextMenuGroupId: "navigation",
    contextMenuOrder: 0,
    precondition: "editorHasSelection",
    run: (ed) => {
      const model = ed.getModel();
      const sel = ed.getSelection();
      if (!model || !sel || sel.isEmpty()) return;
      cb.current.onAskManager(model.getValueInRange(sel), sel.startLineNumber, sel.endLineNumber);
    },
  });
  editor.addAction({
    id: "orkestr.openInVsCode",
    label: t("vscode:openFile.action"),
    contextMenuGroupId: "navigation",
    contextMenuOrder: 0.8,
    run: (ed) => {
      const pos = ed.getPosition();
      cb.current.onOpenInVsCode?.(pos?.lineNumber ?? 1, pos?.column ?? 1);
    },
  });
  editor.addAction({
    id: "orkestr.askClaude",
    label: t("code:editor.askClaude"),
    contextMenuGroupId: "navigation",
    contextMenuOrder: 0.5,
    precondition: "editorHasSelection",
    run: (ed) => {
      const model = ed.getModel();
      const sel = ed.getSelection();
      if (!model || !sel || sel.isEmpty()) return;
      cb.current.onAskClaude(model.getValueInRange(sel), sel.startLineNumber, sel.endLineNumber);
    },
  });
}

/** Runs a Monaco action / command by id (no-op when unsupported for the model). */
function runEditorAction(editor: Monaco.editor.IStandaloneCodeEditor, id: string): void {
  editor.focus();
  const action = editor.getAction(id);
  if (action) void action.run();
  else editor.trigger("contextmenu", id, null);
}

/** Orkestr context menu for the code editor (exported for tests). */
export function editorMenu(editor: Monaco.editor.IStandaloneCodeEditor, cb: EditorCallbacks): MenuEntry[] {
  const model = editor.getModel();
  const sel = editor.getSelection();
  const hasSel = !!model && !!sel && !sel.isEmpty();
  const readOnly = editor.getRawOptions().readOnly === true;
  const selected = () => (model && sel ? model.getValueInRange(sel) : "");
  const run = (id: string) => () => runEditorAction(editor, id);
  return [
    { id: "ed.askManager", label: t("code:editor.askManager"), icon: <Sparkles size={14} />, hidden: !hasSel, onSelect: () => void (sel && cb.onAskManager(selected(), sel.startLineNumber, sel.endLineNumber)) },
    { id: "ed.askClaude", label: t("code:editor.askClaude"), icon: <Sparkles size={14} />, hidden: !hasSel, onSelect: () => void (sel && cb.onAskClaude(selected(), sel.startLineNumber, sel.endLineNumber)) },
    { kind: "separator", id: "ed.s0" },
    { id: "ed.def", label: t("contextMenu:editor.goToDefinition"), icon: <FileCode2 size={14} />, shortcut: "F12", onSelect: run("editor.action.revealDefinition") },
    { id: "ed.refs", label: t("contextMenu:editor.goToReferences"), icon: <ListTree size={14} />, shortcut: "Shift+F12", onSelect: run("editor.action.goToReferences") },
    { kind: "separator", id: "ed.s1" },
    { id: "ed.rename", label: t("contextMenu:editor.renameSymbol"), icon: <PencilLine size={14} />, shortcut: "F2", disabled: readOnly, onSelect: run("editor.action.rename") },
    { id: "ed.changeAll", label: t("contextMenu:editor.changeAll"), icon: <Replace size={14} />, shortcut: "Ctrl+F2", disabled: readOnly, onSelect: run("editor.action.changeAll") },
    { id: "ed.format", label: t("contextMenu:editor.formatDocument"), icon: <WandSparkles size={14} />, shortcut: "Shift+Alt+F", disabled: readOnly, onSelect: run("editor.action.formatDocument") },
    { kind: "separator", id: "ed.s2" },
    { id: "ed.cut", label: t("contextMenu:edit.cut"), icon: <Scissors size={14} />, shortcut: "Ctrl+X", disabled: readOnly || !hasSel, onSelect: run("editor.action.clipboardCutAction") },
    { id: "ed.copy", label: t("contextMenu:edit.copy"), icon: <Copy size={14} />, shortcut: "Ctrl+C", disabled: !hasSel, onSelect: run("editor.action.clipboardCopyAction") },
    { id: "ed.paste", label: t("contextMenu:edit.paste"), icon: <ClipboardPaste size={14} />, shortcut: "Ctrl+V", disabled: readOnly, onSelect: run("editor.action.clipboardPasteAction") },
    { id: "ed.selectAll", label: t("contextMenu:edit.selectAll"), icon: <TextSelect size={14} />, shortcut: "Ctrl+A", onSelect: run("editor.action.selectAll") },
    { kind: "separator", id: "ed.s3" },
    { id: "ed.palette", label: t("contextMenu:editor.commandPalette"), icon: <SquareTerminal size={14} />, shortcut: "F1", onSelect: run("editor.action.quickCommand") },
  ];
}

/** Code editor bound to the active document. */
export function CodeEditor({
  monaco,
  doc,
  agentLines,
  callbacks,
}: {
  monaco: MonacoApi;
  doc: Doc;
  agentLines: readonly number[];
  callbacks: EditorCallbacks;
}) {
  const el = useRef<HTMLDivElement>(null);
  const editorRef = useRef<Monaco.editor.IStandaloneCodeEditor | null>(null);
  const decorations = useRef<Monaco.editor.IEditorDecorationsCollection | null>(null);
  const shownDoc = useRef<Doc | null>(null);
  const cb = useRef(callbacks);
  cb.current = callbacks;
  const reveal = useEditorStore((s) => s.reveal);
  const lng = useLanguage();

  useEffect(() => {
    if (!el.current) return;
    const editor = monaco.editor.create(el.current, { ...baseOptions(), model: null });
    editorRef.current = editor;
    decorations.current = editor.createDecorationsCollection();
    registerCommands(monaco, editor, cb);
    const sub = editor.onDidChangeCursorSelection((e) => {
      const model = editor.getModel();
      const selected = model ? model.getValueLengthInRange(e.selection) : 0;
      cb.current.onCursor(e.selection.positionLineNumber, e.selection.positionColumn, selected);
      notifySelectionChanged();
    });
    return () => {
      if (shownDoc.current) shownDoc.current.viewState = editor.saveViewState();
      shownDoc.current = null;
      sub.dispose();
      editor.dispose();
      editorRef.current = null;
    };
  }, [monaco]);

  // Swap models, keeping each document's scroll/cursor state.
  useEffect(() => {
    const editor = editorRef.current;
    if (!editor || shownDoc.current === doc) return;
    if (shownDoc.current) shownDoc.current.viewState = editor.saveViewState();
    editor.setModel(doc.model);
    if (doc.viewState) editor.restoreViewState(doc.viewState);
    editor.updateOptions({ readOnly: doc.readonly });
    shownDoc.current = doc;
    editor.focus();
    const pos = editor.getPosition();
    if (pos) cb.current.onCursor(pos.lineNumber, pos.column, 0);
  }, [doc]);

  // The current selection for «Claude Code: спросить о выделенном».
  useEffect(
    () =>
      setSelectionSource(() => {
        const editor = editorRef.current;
        const model = editor?.getModel();
        const sel = editor?.getSelection();
        if (!editor || !model || !sel || sel.isEmpty() || model !== doc.model) return null;
        return {
          path: doc.path,
          startLine: sel.startLineNumber,
          endLine: sel.endLineNumber,
          text: model.getValueInRange(sel),
          language: model.getLanguageId(),
        };
      }),
    [doc],
  );

  // Reveal requests (search hits, quick open with position).
  useEffect(() => {
    const editor = editorRef.current;
    if (!editor || !reveal || reveal.path !== doc.path) return;
    const col = reveal.column ?? 1;
    const range = new monaco.Range(reveal.line, col, reveal.line, col + (reveal.length ?? 0));
    editor.setSelection(range);
    editor.revealRangeInCenter(range, monaco.editor.ScrollType.Smooth);
    editor.focus();
    useEditorStore.getState().consumeReveal();
  }, [reveal, doc, monaco]);

  // Agent change markers (gutter bar + overview ruler + minimap).
  useEffect(() => {
    const coll = decorations.current;
    if (!coll) return;
    const lineCount = doc.model.getLineCount();
    const violet = tokenHex("--ok-color-violet");
    coll.set(
      toRanges(agentLines.filter((l) => l <= lineCount)).map(([from, to]) => ({
        range: new monaco.Range(from, 1, to, 1),
        options: {
          isWholeLine: true,
          className: "code-agent-line",
          linesDecorationsClassName: "code-agent-gutter",
          linesDecorationsTooltip: t("code:editor.agentChanged"),
          overviewRuler: { color: violet, position: monaco.editor.OverviewRulerLane.Left },
          minimap: { color: violet, position: monaco.editor.MinimapPosition.Gutter },
        },
      })),
    );
    // `lng`: the gutter tooltip follows a live language switch.
  }, [agentLines, doc, monaco, lng]);

  const menu = useContextMenu(() => (editorRef.current ? editorMenu(editorRef.current, cb.current) : []), { replaceDefaults: true });
  return <div className="code-editor" ref={el} {...menu} />;
}

/** Diff editor: `original` text (left, read-only) vs the live model (right, editable). */
export function DiffEditor({
  monaco,
  doc,
  original,
  callbacks,
}: {
  monaco: MonacoApi;
  doc: Doc;
  original: string;
  callbacks: EditorCallbacks;
}) {
  const el = useRef<HTMLDivElement>(null);
  const originalModel = useRef<Monaco.editor.ITextModel | null>(null);
  const cb = useRef(callbacks);
  cb.current = callbacks;

  useEffect(() => {
    if (!el.current) return;
    const diff = monaco.editor.createDiffEditor(el.current, {
      ...baseOptions(),
      renderSideBySide: true,
      originalEditable: false,
      enableSplitViewResizing: true,
      renderOverviewRuler: true,
      ignoreTrimWhitespace: false,
    });
    const orig = monaco.editor.createModel(original, doc.model.getLanguageId());
    originalModel.current = orig;
    diff.setModel({ original: orig, modified: doc.model });
    registerCommands(monaco, diff.getModifiedEditor(), cb);
    diff.getModifiedEditor().focus();
    return () => {
      diff.dispose();
      orig.dispose();
      originalModel.current = null;
    };
  }, [monaco, doc]);

  useEffect(() => {
    const m = originalModel.current;
    if (m && m.getValue() !== original) m.setValue(original);
  }, [original]);

  return <div className="code-editor" ref={el} />;
}
