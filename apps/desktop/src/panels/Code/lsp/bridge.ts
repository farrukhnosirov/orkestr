/**
 * Language servers in the Code panel (ADR-0025): one LSP session per
 * (project, server), started on demand when a document of a served language
 * is open. The core asks the user to approve running a server once; this
 * module surfaces that prompt (see `LspBanner`) and wires the session into
 * Monaco: document sync, diagnostics, completion, hover, go to definition,
 * references, rename, formatting and signature help.
 */
import type { LspServerInfo, LspSessionInfo, LspState } from "@orkestr/protocol";
import type * as Monaco from "monaco-editor";
import { create } from "zustand";

import { getClient, maybeClient } from "../../../core/client";
import { errorInfo } from "../../../features/files/rpc";
import { describeError } from "../../../i18n/errors";
import { useEditorStore } from "../../../store/editor";
import { currentRegistry } from "../documents";
import { type MonacoApi, modelUri } from "../monaco";
import { LspConnection } from "./connection";
import {
  completionKind,
  fromFileUri,
  type LspRange,
  markerSeverity,
  markupToMarkdown,
  toFileUri,
  toLspPosition,
  toMonacoRange,
} from "./convert";

/** UI-facing state of one (project, server). */
export interface LspEntry {
  projectId: string;
  serverId: string;
  serverName: string;
  state: LspState | "needsApproval" | "denied" | "error";
  approvalId?: string;
  command?: string;
  error?: string;
}

interface LspUiState {
  entries: Record<string, LspEntry>;
  servers: LspServerInfo[];
}

export const useLspStore = create<LspUiState>(() => ({ entries: {}, servers: [] }));

const keyOf = (projectId: string, serverId: string) => `${projectId}:${serverId}`;

function setEntry(e: LspEntry): void {
  useLspStore.setState((s) => ({ entries: { ...s.entries, [keyOf(e.projectId, e.serverId)]: e } }));
}

interface OpenDoc {
  version: number;
  timer: ReturnType<typeof setTimeout> | null;
  sub: Monaco.IDisposable;
}

/** One live session wired into Monaco. */
class Session {
  readonly docs = new Map<string, OpenDoc>();
  private providers: Monaco.IDisposable[] = [];
  readonly owner: string;

  constructor(
    readonly monaco: MonacoApi,
    readonly projectId: string,
    readonly server: LspServerInfo,
    readonly conn: LspConnection,
  ) {
    this.owner = `lsp:${server.id}`;
  }

  get rootUri(): string {
    return this.conn.session.rootUri;
  }

  serves(model: Monaco.editor.ITextModel): boolean {
    return model.uri.scheme === "orkestr" && model.uri.authority === this.projectId && this.server.languages.includes(model.getLanguageId());
  }

  relPath(model: Monaco.editor.ITextModel): string {
    return model.uri.path.replace(/^\//, "");
  }

  fileUri(model: Monaco.editor.ITextModel): string {
    return toFileUri(this.rootUri, this.relPath(model));
  }

  docId(model: Monaco.editor.ITextModel): { uri: string } {
    return { uri: this.fileUri(model) };
  }

  async start(): Promise<void> {
    await this.conn.initialize();
    this.registerProviders();
    for (const m of this.monaco.editor.getModels()) if (this.serves(m)) this.open(m);
  }

  open(model: Monaco.editor.ITextModel): void {
    const key = model.uri.toString();
    if (this.docs.has(key) || !this.conn.running) return;
    const doc: OpenDoc = {
      version: 1,
      timer: null,
      sub: model.onDidChangeContent(() => {
        if (doc.timer !== null) clearTimeout(doc.timer);
        doc.timer = setTimeout(() => {
          doc.timer = null;
          doc.version += 1;
          void this.conn.notify("textDocument/didChange", {
            textDocument: { uri: this.fileUri(model), version: doc.version },
            contentChanges: [{ text: model.getValue() }],
          });
        }, 120);
      }),
    };
    this.docs.set(key, doc);
    void this.conn.notify("textDocument/didOpen", {
      textDocument: { uri: this.fileUri(model), languageId: model.getLanguageId(), version: doc.version, text: model.getValue() },
    });
  }

  /** Flushes a pending change so a request sees the current text. */
  flush(model: Monaco.editor.ITextModel): void {
    const doc = this.docs.get(model.uri.toString());
    if (!doc || doc.timer === null) return;
    clearTimeout(doc.timer);
    doc.timer = null;
    doc.version += 1;
    void this.conn.notify("textDocument/didChange", {
      textDocument: { uri: this.fileUri(model), version: doc.version },
      contentChanges: [{ text: model.getValue() }],
    });
  }

  close(model: Monaco.editor.ITextModel): void {
    const key = model.uri.toString();
    const doc = this.docs.get(key);
    if (!doc) return;
    if (doc.timer !== null) clearTimeout(doc.timer);
    doc.sub.dispose();
    this.docs.delete(key);
    this.monaco.editor.setModelMarkers(model, this.owner, []);
    void this.conn.notify("textDocument/didClose", { textDocument: { uri: this.fileUri(model) } });
  }

  saved(model: Monaco.editor.ITextModel): void {
    if (!this.docs.has(model.uri.toString())) return;
    this.flush(model);
    void this.conn.notify("textDocument/didSave", { textDocument: this.docId(model) });
  }

  /** Forgets document state (the server restarted and must be re-initialized). */
  reset(): void {
    for (const d of this.docs.values()) {
      if (d.timer !== null) clearTimeout(d.timer);
      d.sub.dispose();
    }
    this.docs.clear();
    for (const p of this.providers) p.dispose();
    this.providers = [];
  }

  diagnostics(params: { uri: string; diagnostics: { range: LspRange; severity?: number; message: string; source?: string; code?: unknown }[] }): void {
    const rel = fromFileUri(this.rootUri, params.uri);
    if (rel === null) return;
    const model = this.monaco.editor.getModel(modelUri(this.projectId, rel));
    if (!model) return;
    this.monaco.editor.setModelMarkers(
      model,
      this.owner,
      params.diagnostics.map((d) => {
        const code = typeof d.code === "string" || typeof d.code === "number" ? String(d.code) : undefined;
        return {
          ...toMonacoRange(d.range),
          severity: markerSeverity(d.severity),
          message: d.message,
          ...(d.source ? { source: d.source } : {}),
          ...(code ? { code } : {}),
        };
      }),
    );
  }

  /** Monaco location for a `file://` URI inside the project (loads the model for peeks). */
  private async location(uri: string, range: LspRange): Promise<Monaco.languages.Location | null> {
    const rel = fromFileUri(this.rootUri, uri);
    if (rel === null) return null;
    const target = modelUri(this.projectId, rel);
    if (!this.monaco.editor.getModel(target)) {
      const registry = currentRegistry();
      if (registry?.projectId === this.projectId) await registry.load(rel).catch(() => null);
    }
    return { uri: target, range: toMonacoRange(range) };
  }

  private async locations(result: unknown): Promise<Monaco.languages.Location[]> {
    if (!result) return [];
    const list = (Array.isArray(result) ? result : [result]) as {
      uri?: string;
      range?: LspRange;
      targetUri?: string;
      targetSelectionRange?: LspRange;
    }[];
    const out: Monaco.languages.Location[] = [];
    for (const l of list.slice(0, 200)) {
      const uri = l.uri ?? l.targetUri;
      const range = l.range ?? l.targetSelectionRange;
      if (!uri || !range) continue;
      const loc = await this.location(uri, range);
      if (loc) out.push(loc);
    }
    return out;
  }

  private registerProviders(): void {
    const monaco = this.monaco;
    const langs = this.server.languages;
    const caps = this.conn.capabilities as {
      completionProvider?: { triggerCharacters?: string[]; resolveProvider?: boolean };
      hoverProvider?: unknown;
      definitionProvider?: unknown;
      referencesProvider?: unknown;
      renameProvider?: unknown;
      documentFormattingProvider?: unknown;
      documentRangeFormattingProvider?: unknown;
      signatureHelpProvider?: { triggerCharacters?: string[]; retriggerCharacters?: string[] };
    };
    const mine = (model: Monaco.editor.ITextModel) => this.serves(model) && this.conn.running;
    const pos = (p: Monaco.Position) => toLspPosition(p.lineNumber, p.column);

    if (caps.completionProvider) {
      this.providers.push(
        monaco.languages.registerCompletionItemProvider(langs, {
          triggerCharacters: caps.completionProvider.triggerCharacters ?? [],
          provideCompletionItems: async (model, position, context) => {
            if (!mine(model)) return { suggestions: [] };
            this.flush(model);
            const res = await this.conn
              .request<unknown>("textDocument/completion", {
                textDocument: this.docId(model),
                position: pos(position),
                context: context.triggerCharacter ? { triggerKind: 2, triggerCharacter: context.triggerCharacter } : { triggerKind: 1 },
              })
              .catch(() => null);
            const items = (Array.isArray(res) ? res : ((res as { items?: unknown[] } | null)?.items ?? [])) as {
              label: string;
              kind?: number;
              detail?: string;
              documentation?: unknown;
              insertText?: string;
              insertTextFormat?: number;
              textEdit?: { range?: LspRange; insert?: LspRange; replace?: LspRange; newText: string };
              sortText?: string;
              filterText?: string;
              preselect?: boolean;
              additionalTextEdits?: { range: LspRange; newText: string }[];
            }[];
            const word = model.getWordUntilPosition(position);
            const fallback = new monaco.Range(position.lineNumber, word.startColumn, position.lineNumber, word.endColumn);
            const suggestions: Monaco.languages.CompletionItem[] = items.slice(0, 2000).map((it) => {
              const editRange = it.textEdit?.range ?? it.textEdit?.replace;
              const item: Monaco.languages.CompletionItem = {
                label: it.label,
                kind: completionKind(it.kind),
                insertText: it.textEdit?.newText ?? it.insertText ?? it.label,
                range: editRange ? toMonacoRange(editRange) : fallback,
              };
              if (it.detail) item.detail = it.detail;
              const doc = markupToMarkdown(it.documentation);
              if (doc) item.documentation = { value: doc };
              if (it.insertTextFormat === 2) item.insertTextRules = monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet;
              if (it.sortText) item.sortText = it.sortText;
              if (it.filterText) item.filterText = it.filterText;
              if (it.preselect) item.preselect = true;
              if (it.additionalTextEdits?.length) {
                item.additionalTextEdits = it.additionalTextEdits.map((e) => ({ range: toMonacoRange(e.range), text: e.newText }));
              }
              return item;
            });
            const incomplete = !Array.isArray(res) && Boolean((res as { isIncomplete?: boolean } | null)?.isIncomplete);
            return { suggestions, incomplete };
          },
        }),
      );
    }
    if (caps.hoverProvider) {
      this.providers.push(
        monaco.languages.registerHoverProvider(langs, {
          provideHover: async (model, position) => {
            if (!mine(model)) return null;
            this.flush(model);
            const res = await this.conn
              .request<{ contents?: unknown; range?: LspRange } | null>("textDocument/hover", { textDocument: this.docId(model), position: pos(position) })
              .catch(() => null);
            const md = markupToMarkdown(res?.contents);
            if (!md) return null;
            const hover: Monaco.languages.Hover = { contents: [{ value: md }] };
            if (res?.range) hover.range = toMonacoRange(res.range);
            return hover;
          },
        }),
      );
    }
    if (caps.definitionProvider) {
      this.providers.push(
        monaco.languages.registerDefinitionProvider(langs, {
          provideDefinition: async (model, position) => {
            if (!mine(model)) return null;
            this.flush(model);
            const res = await this.conn.request("textDocument/definition", { textDocument: this.docId(model), position: pos(position) }).catch(() => null);
            return this.locations(res);
          },
        }),
      );
    }
    if (caps.referencesProvider) {
      this.providers.push(
        monaco.languages.registerReferenceProvider(langs, {
          provideReferences: async (model, position, ctx) => {
            if (!mine(model)) return null;
            this.flush(model);
            const res = await this.conn
              .request("textDocument/references", {
                textDocument: this.docId(model),
                position: pos(position),
                context: { includeDeclaration: ctx.includeDeclaration },
              })
              .catch(() => null);
            return this.locations(res);
          },
        }),
      );
    }
    if (caps.renameProvider) {
      this.providers.push(
        monaco.languages.registerRenameProvider(langs, {
          provideRenameEdits: async (model, position, newName) => {
            if (!mine(model)) return null;
            this.flush(model);
            const res = await this.conn.request<{
              changes?: Record<string, { range: LspRange; newText: string }[]>;
              documentChanges?: { textDocument?: { uri: string }; edits?: { range: LspRange; newText: string }[] }[];
            } | null>("textDocument/rename", { textDocument: this.docId(model), position: pos(position), newName });
            const byUri: [string, { range: LspRange; newText: string }[]][] = [
              ...Object.entries(res?.changes ?? {}),
              ...(res?.documentChanges ?? []).flatMap((d) => (d.textDocument && d.edits ? [[d.textDocument.uri, d.edits] as [string, { range: LspRange; newText: string }[]]] : [])),
            ];
            const edits: Monaco.languages.IWorkspaceTextEdit[] = [];
            for (const [uri, list] of byUri) {
              const first = list[0];
              if (!first) continue;
              const loc = await this.location(uri, first.range);
              if (!loc) continue;
              for (const e of list) edits.push({ resource: loc.uri, textEdit: { range: toMonacoRange(e.range), text: e.newText }, versionId: undefined });
            }
            return { edits };
          },
        }),
      );
    }
    if (caps.documentFormattingProvider) {
      this.providers.push(
        monaco.languages.registerDocumentFormattingEditProvider(langs, {
          provideDocumentFormattingEdits: async (model, options) => {
            if (!mine(model)) return [];
            this.flush(model);
            const res = await this.conn
              .request<{ range: LspRange; newText: string }[] | null>("textDocument/formatting", {
                textDocument: this.docId(model),
                options: { tabSize: options.tabSize, insertSpaces: options.insertSpaces },
              })
              .catch(() => null);
            return (res ?? []).map((e) => ({ range: toMonacoRange(e.range), text: e.newText }));
          },
        }),
      );
    }
    if (caps.signatureHelpProvider) {
      this.providers.push(
        monaco.languages.registerSignatureHelpProvider(langs, {
          signatureHelpTriggerCharacters: caps.signatureHelpProvider.triggerCharacters ?? ["(", ","],
          signatureHelpRetriggerCharacters: caps.signatureHelpProvider.retriggerCharacters ?? [],
          provideSignatureHelp: async (model, position) => {
            if (!mine(model)) return null;
            this.flush(model);
            const res = await this.conn
              .request<{
                signatures?: { label: string; documentation?: unknown; parameters?: { label: string | [number, number]; documentation?: unknown }[] }[];
                activeSignature?: number;
                activeParameter?: number;
              } | null>("textDocument/signatureHelp", { textDocument: this.docId(model), position: pos(position) })
              .catch(() => null);
            if (!res?.signatures?.length) return null;
            return {
              value: {
                signatures: res.signatures.map((s) => ({
                  label: s.label,
                  documentation: { value: markupToMarkdown(s.documentation) },
                  parameters: (s.parameters ?? []).map((p) => ({ label: p.label, documentation: { value: markupToMarkdown(p.documentation) } })),
                })),
                activeSignature: res.activeSignature ?? 0,
                activeParameter: res.activeParameter ?? 0,
              },
              dispose: () => undefined,
            };
          },
        }),
      );
    }
  }

  async dispose(): Promise<void> {
    for (const m of this.monaco.editor.getModels()) {
      if (this.docs.has(m.uri.toString())) this.monaco.editor.setModelMarkers(m, this.owner, []);
    }
    this.reset();
    await this.conn.dispose();
  }
}

/** Starts / tracks sessions for the documents open in Monaco. */
class LspCoordinator {
  private monaco: MonacoApi | null = null;
  private readonly sessions = new Map<string, Session>();
  private readonly starting = new Set<string>();
  /** (project, server) the user declined this run. */
  private readonly declined = new Set<string>();
  private serversLoaded: Promise<void> | null = null;

  attach(monaco: MonacoApi): void {
    if (this.monaco) return;
    this.monaco = monaco;
    monaco.editor.onDidCreateModel((m) => this.onModel(m));
    monaco.editor.onWillDisposeModel((m) => {
      for (const s of this.sessions.values()) s.close(m);
    });
    monaco.editor.onDidChangeModelLanguage((e) => {
      for (const s of this.sessions.values()) s.close(e.model);
      this.onModel(e.model);
    });
    monaco.editor.registerEditorOpener({
      openCodeEditor: (_source, resource, selectionOrPosition) => {
        if (resource.scheme !== "orkestr") return false;
        if (useEditorStore.getState().projectId !== resource.authority) return false;
        const path = resource.path.replace(/^\//, "");
        const at =
          selectionOrPosition && "startLineNumber" in selectionOrPosition
            ? { line: selectionOrPosition.startLineNumber, column: selectionOrPosition.startColumn }
            : selectionOrPosition
              ? { line: selectionOrPosition.lineNumber, column: selectionOrPosition.column }
              : undefined;
        useEditorStore.getState().openFile(path, at);
        return true;
      },
    });
    for (const m of monaco.editor.getModels()) this.onModel(m);
  }

  /** Reloads the server list (after installs / settings changes). */
  refreshServers(): Promise<void> {
    const client = maybeClient();
    if (!client) return Promise.resolve();
    this.serversLoaded = client
      .call("lsp.servers", {})
      .then((r) => useLspStore.setState({ servers: r.servers }))
      .catch((e: unknown) => console.warn("lsp.servers failed", e));
    return this.serversLoaded;
  }

  private serverFor(languageId: string): LspServerInfo | null {
    return useLspStore.getState().servers.find((s) => s.available && s.languages.includes(languageId)) ?? null;
  }

  private onModel(model: Monaco.editor.ITextModel): void {
    if (model.uri.scheme !== "orkestr") return;
    void (this.serversLoaded ?? this.refreshServers()).then(() => {
      const server = this.serverFor(model.getLanguageId());
      if (!server) return;
      const key = keyOf(model.uri.authority, server.id);
      const session = this.sessions.get(key);
      if (session) session.open(model);
      else void this.start(model.uri.authority, server);
    });
  }

  /** Notifies servers that a document was saved. */
  saved(model: Monaco.editor.ITextModel): void {
    for (const s of this.sessions.values()) s.saved(model);
  }

  async start(projectId: string, server: LspServerInfo, approvalId?: string): Promise<void> {
    const monaco = this.monaco;
    const key = keyOf(projectId, server.id);
    if (!monaco || this.sessions.has(key) || this.starting.has(key) || (this.declined.has(key) && !approvalId)) return;
    this.starting.add(key);
    const base = { projectId, serverId: server.id, serverName: server.name };
    try {
      const res = await getClient().call("lsp.start", { projectId, serverId: server.id, ...(approvalId ? { approvalId } : {}) });
      setEntry({ ...base, state: "starting" });
      let session: Session | null = null;
      const conn = new LspConnection(res.session, {
        onNotification: (method, params) => {
          if (method === "textDocument/publishDiagnostics") session?.diagnostics(params as Parameters<Session["diagnostics"]>[0]);
        },
        onStatus: (info: LspSessionInfo, prev) => {
          setEntry({ ...base, state: info.state, ...(info.error ? { error: info.error } : {}) });
          if (!session) return;
          if (info.state === "running" && prev !== "running") {
            session.reset();
            session.start().catch((e: unknown) => setEntry({ ...base, state: "error", error: describeError(e) }));
          } else if (info.state === "stopped" || info.state === "failed") {
            void session.dispose();
            this.sessions.delete(key);
          }
        },
      });
      session = new Session(monaco, projectId, server, conn);
      this.sessions.set(key, session);
      await conn.sync();
    } catch (e) {
      const info = errorInfo(e);
      const data = (e as { data?: { approvalId?: string; command?: string } }).data;
      if (info.code === "approval_required" && data?.approvalId) {
        setEntry({ ...base, state: "needsApproval", approvalId: data.approvalId, ...(data.command ? { command: data.command } : {}) });
      } else if (info.code === "forbidden") {
        setEntry({ ...base, state: "denied", error: describeError(e) });
      } else {
        setEntry({ ...base, state: "error", error: describeError(e) });
      }
    } finally {
      this.starting.delete(key);
    }
  }

  /** User allowed the server: approve and start. */
  async approve(entry: LspEntry): Promise<void> {
    if (!entry.approvalId) return;
    await getClient().call("approvals.resolve", { approvalId: entry.approvalId, decision: "approve" });
    const server = useLspStore.getState().servers.find((s) => s.id === entry.serverId);
    this.declined.delete(keyOf(entry.projectId, entry.serverId));
    if (server) await this.start(entry.projectId, server, entry.approvalId);
  }

  /** User declined: reject the approval and do not ask again this run. */
  async decline(entry: LspEntry): Promise<void> {
    this.declined.add(keyOf(entry.projectId, entry.serverId));
    setEntry({ ...entry, state: "denied" });
    if (entry.approvalId) {
      await getClient()
        .call("approvals.resolve", { approvalId: entry.approvalId, decision: "reject" })
        .catch(() => undefined);
    }
  }

  /** Stops every session of a project (panel closed / project switched). */
  async stopProject(projectId: string): Promise<void> {
    for (const [key, s] of [...this.sessions]) {
      if (s.projectId !== projectId) continue;
      this.sessions.delete(key);
      await s.dispose();
      setEntry({ projectId, serverId: s.server.id, serverName: s.server.name, state: "stopped" });
    }
  }
}

export const lspCoordinator = new LspCoordinator();
