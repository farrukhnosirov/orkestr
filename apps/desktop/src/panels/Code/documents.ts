/**
 * Open documents of the Code panel: one Monaco model per project file plus
 * what the core last told us about it (hash for optimistic concurrency,
 * encoding, last saved text for the diff view). Kept at module level so
 * closing and re-opening the panel does not lose unsaved edits.
 */
import type { LineEnding, ReadFileResult, TextEncoding, WriteFileParams } from "@orkestr/protocol";
import type * as Monaco from "monaco-editor";

import { getClient } from "../../core/client";
import { changedLines } from "../../features/files/lineDiff";
import { errorInfo } from "../../features/files/rpc";
import { describeError } from "../../i18n/errors";
import { type EditorTab, useEditorStore } from "../../store/editor";
import { type MonacoApi, modelUri } from "./monaco";

export interface Doc {
  path: string;
  model: Monaco.editor.ITextModel;
  /** Text as last loaded from / saved to disk. */
  savedText: string;
  /** blake3 of the on-disk bytes we last saw. */
  hash: string;
  encoding: TextEncoding;
  /** Model version matching `savedText`. */
  savedVersion: number;
  viewState: Monaco.editor.ICodeEditorViewState | null;
  readonly: boolean;
}

/** Thrown by `save` on a stale write. */
export class SaveConflictError extends Error {
  constructor(
    readonly path: string,
    readonly actualHash: string | null,
  ) {
    super(`${path} changed on disk`);
    this.name = "SaveConflictError";
  }
}

export class DocumentRegistry {
  private readonly docs = new Map<string, Doc>();
  private readonly loading = new Map<string, Promise<Doc | null>>();
  private readonly listeners = new Set<() => void>();

  constructor(
    private readonly monaco: MonacoApi,
    readonly projectId: string,
  ) {}

  get(path: string): Doc | undefined {
    return this.docs.get(path);
  }

  /** Re-render hook for consumers. */
  onChange(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit(): void {
    for (const l of this.listeners) l();
  }

  isDirty(doc: Doc): boolean {
    return doc.model.getAlternativeVersionId() !== doc.savedVersion;
  }

  private updateStore(path: string, patch: Partial<Omit<EditorTab, "path">>): void {
    useEditorStore.getState().updateTab(path, patch);
  }

  /** Loads a file into a model (idempotent). Binary/too-large/error states go to the tab. */
  load(path: string): Promise<Doc | null> {
    const existing = this.docs.get(path);
    if (existing) return Promise.resolve(existing);
    const pending = this.loading.get(path);
    if (pending) return pending;
    const p = this.read(path)
      .then((res) => {
        if (res.binary || res.content === null) {
          this.updateStore(path, { status: "binary", language: "plaintext" });
          return null;
        }
        const doc = this.createDoc(path, res.content, res);
        this.updateStore(path, { status: "ready", language: res.language, dirty: false, stale: false, deleted: false });
        this.emit();
        return doc;
      })
      .catch((e: unknown) => {
        const info = errorInfo(e);
        if (info.reason === "too_large") this.updateStore(path, { status: "tooLarge", error: describeError(e) });
        else this.updateStore(path, { status: "error", error: describeError(e) });
        return null;
      })
      .finally(() => this.loading.delete(path));
    this.loading.set(path, p);
    return p;
  }

  private read(path: string): Promise<ReadFileResult> {
    return getClient().call("files.read", { projectId: this.projectId, path });
  }

  private createDoc(path: string, content: string, res: ReadFileResult): Doc {
    const uri = modelUri(this.projectId, path);
    this.monaco.editor.getModel(uri)?.dispose();
    const model = this.monaco.editor.createModel(content, res.language, uri);
    this.applyEol(model, res.lineEnding);
    const doc: Doc = {
      path,
      model,
      savedText: model.getValue(),
      hash: res.hash,
      encoding: res.encoding,
      savedVersion: model.getAlternativeVersionId(),
      viewState: null,
      readonly: res.readonly,
    };
    model.onDidChangeContent(() => {
      const dirty = this.isDirty(doc);
      const tab = useEditorStore.getState().tabs.find((t) => t.path === doc.path);
      if (tab && tab.dirty !== dirty) this.updateStore(doc.path, { dirty });
    });
    this.docs.set(path, doc);
    return doc;
  }

  private applyEol(model: Monaco.editor.ITextModel, eol: LineEnding): void {
    model.setEOL(eol === "crlf" ? this.monaco.editor.EndOfLineSequence.CRLF : this.monaco.editor.EndOfLineSequence.LF);
  }

  /** Saves; `overwriteHash` forces a write over a known newer disk version. */
  async save(path: string, overwriteHash?: string | null): Promise<void> {
    const doc = this.docs.get(path);
    if (!doc) return;
    const content = doc.model.getValue();
    const version = doc.model.getAlternativeVersionId();
    const expectedHash = overwriteHash === undefined ? doc.hash : (overwriteHash ?? undefined);
    try {
      const params: WriteFileParams = {
        projectId: this.projectId,
        path,
        content,
        encoding: doc.encoding,
        create: true,
      };
      if (expectedHash) params.expectedHash = expectedHash;
      const res = await getClient().call("files.write", params);
      doc.hash = res.hash;
      doc.savedText = content;
      doc.savedVersion = version;
      this.updateStore(path, { dirty: this.isDirty(doc), stale: false, deleted: false });
      this.emit();
    } catch (e) {
      const info = errorInfo(e);
      if (info.reason === "stale") {
        const actual = info.data?.actualHash;
        throw new SaveConflictError(path, typeof actual === "string" ? actual : null);
      }
      throw e;
    }
  }

  /** Reads the current disk text (for compare). */
  async diskText(path: string): Promise<{ text: string; hash: string } | null> {
    try {
      const res = await this.read(path);
      return res.content === null ? null : { text: res.content, hash: res.hash };
    } catch {
      return null;
    }
  }

  /**
   * Replaces the model content with the disk version (keeps undo history).
   * Returns the 1-based changed lines, or null when nothing changed.
   */
  async reloadFromDisk(path: string): Promise<number[] | null> {
    const doc = this.docs.get(path);
    if (!doc) return null;
    const res = await this.read(path);
    if (res.content === null) return null;
    if (res.hash === doc.hash && !this.isDirty(doc)) return null;
    const before = doc.model.getValue();
    const lines = changedLines(before, res.content);
    doc.model.pushEditOperations([], [{ range: doc.model.getFullModelRange(), text: res.content }], () => null);
    this.applyEol(doc.model, res.lineEnding);
    doc.hash = res.hash;
    doc.encoding = res.encoding;
    doc.savedText = doc.model.getValue();
    doc.savedVersion = doc.model.getAlternativeVersionId();
    this.updateStore(path, { dirty: false, stale: false, deleted: false });
    this.emit();
    return lines;
  }

  /** Moves documents after a rename (models are re-created at the new URI). */
  move(from: string, to: string): void {
    for (const [path, doc] of [...this.docs]) {
      const target = path === from ? to : path.startsWith(`${from}/`) ? `${to}${path.slice(from.length)}` : null;
      if (target === null) continue;
      const dirtyText = doc.model.getValue();
      const wasDirty = this.isDirty(doc);
      const uri = modelUri(this.projectId, target);
      this.monaco.editor.getModel(uri)?.dispose();
      const model = this.monaco.editor.createModel(doc.savedText, doc.model.getLanguageId(), uri);
      model.setEOL(doc.model.getEndOfLineSequence());
      const moved: Doc = { ...doc, path: target, model, savedVersion: model.getAlternativeVersionId(), viewState: null };
      if (wasDirty) model.setValue(dirtyText);
      model.onDidChangeContent(() => {
        const dirty = this.isDirty(moved);
        this.updateStore(moved.path, { dirty });
      });
      doc.model.dispose();
      this.docs.delete(path);
      this.docs.set(target, moved);
    }
    this.emit();
  }

  close(path: string): void {
    const doc = this.docs.get(path);
    if (!doc) return;
    doc.model.dispose();
    this.docs.delete(path);
    this.emit();
  }

  dispose(): void {
    for (const doc of this.docs.values()) doc.model.dispose();
    this.docs.clear();
    this.listeners.clear();
  }
}

let current: DocumentRegistry | null = null;

/** The registry for a project (disposing the previous project's documents). */
export function registryFor(monaco: MonacoApi, projectId: string): DocumentRegistry {
  if (current && current.projectId === projectId) return current;
  current?.dispose();
  current = new DocumentRegistry(monaco, projectId);
  return current;
}

/** The current registry, if the Code panel has loaded one. */
export function currentRegistry(): DocumentRegistry | null {
  return current;
}
