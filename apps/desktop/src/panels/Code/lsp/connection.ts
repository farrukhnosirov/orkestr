/**
 * A lean LSP client over Orkestr's RPC channel (ADR-0025): requests and
 * notifications go out through `lsp.send`, server messages arrive as
 * `LspMessage` events on `lsp:<sessionId>`. Server → client requests the
 * editor does not implement get an explicit `MethodNotFound` error.
 */
import type { Event, LspSessionInfo, LspState } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";

import { getClient } from "../../../core/client";

type JsonRpcId = number | string;

interface Pending {
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

export class LspRequestError extends Error {
  constructor(
    readonly code: number,
    message: string,
  ) {
    super(message);
    this.name = "LspRequestError";
  }
}

export interface ConnectionHandlers {
  onNotification: (method: string, params: unknown) => void;
  onStatus: (session: LspSessionInfo, previous: LspState) => void;
}

/** Client capabilities Orkestr's editor implements. */
export const CLIENT_CAPABILITIES = {
  general: { positionEncodings: ["utf-16"] },
  workspace: { configuration: true, workspaceFolders: true, applyEdit: false },
  textDocument: {
    synchronization: { dynamicRegistration: false, didSave: true, willSave: false },
    completion: {
      contextSupport: true,
      completionItem: {
        snippetSupport: true,
        documentationFormat: ["markdown", "plaintext"],
        deprecatedSupport: true,
        labelDetailsSupport: true,
        resolveSupport: { properties: ["documentation", "detail"] },
      },
    },
    hover: { contentFormat: ["markdown", "plaintext"] },
    signatureHelp: {
      signatureInformation: { documentationFormat: ["markdown", "plaintext"], parameterInformation: { labelOffsetSupport: true } },
    },
    definition: { linkSupport: false },
    references: {},
    rename: { prepareSupport: false },
    formatting: {},
    rangeFormatting: {},
    publishDiagnostics: { relatedInformation: false, versionSupport: false },
  },
} as const;

export class LspConnection {
  private nextId = 1;
  private readonly pending = new Map<JsonRpcId, Pending>();
  private off: (() => void) | null = null;
  private state: LspState;
  capabilities: Record<string, unknown> = {};

  constructor(
    public session: LspSessionInfo,
    private readonly handlers: ConnectionHandlers,
  ) {
    this.state = session.state;
    this.off = getClient().subscribe([topic.lsp(session.sessionId)], (ev) => this.onEvent(ev));
  }

  get sessionId(): string {
    return this.session.sessionId;
  }

  get running(): boolean {
    return this.state === "running";
  }

  /** Re-reads the session state (covers events emitted before we subscribed). */
  async sync(): Promise<void> {
    const res = await getClient().call("lsp.servers", {});
    const s = res.sessions.find((x) => x.sessionId === this.session.sessionId);
    if (s) this.applyStatus(s);
    else this.applyStatus({ ...this.session, state: "stopped" });
  }

  private applyStatus(s: LspSessionInfo): void {
    const prev = this.state;
    this.session = s;
    this.state = s.state;
    if (s.state !== "running") this.failPending(new Error(`language server ${s.state}`));
    if (prev !== s.state || s.state === "running") this.handlers.onStatus(s, prev);
  }

  private onEvent(ev: Event): void {
    if (ev.kind === "LspStatus") {
      this.applyStatus(ev.payload.session);
      return;
    }
    if (ev.kind !== "LspMessage") return;
    let msg: { id?: JsonRpcId; method?: string; params?: unknown; result?: unknown; error?: { code: number; message: string } };
    try {
      msg = JSON.parse(ev.payload.message) as typeof msg;
    } catch {
      return;
    }
    if (msg.method !== undefined && msg.id !== undefined) {
      void this.answerServerRequest(msg.id, msg.method, msg.params);
    } else if (msg.method !== undefined) {
      this.handlers.onNotification(msg.method, msg.params);
    } else if (msg.id !== undefined) {
      const p = this.pending.get(msg.id);
      if (!p) return;
      this.pending.delete(msg.id);
      clearTimeout(p.timer);
      if (msg.error) p.reject(new LspRequestError(msg.error.code, msg.error.message));
      else p.resolve(msg.result ?? null);
    }
  }

  private async answerServerRequest(id: JsonRpcId, method: string, params: unknown): Promise<void> {
    let result: unknown = null;
    let error: { code: number; message: string } | null = null;
    switch (method) {
      case "workspace/configuration": {
        const items = (params as { items?: unknown[] } | undefined)?.items ?? [];
        result = items.map(() => null);
        break;
      }
      case "workspace/workspaceFolders":
        result = [{ uri: this.session.rootUri, name: this.session.rootUri.split("/").pop() ?? "workspace" }];
        break;
      case "client/registerCapability":
      case "client/unregisterCapability":
      case "window/workDoneProgress/create":
        result = null;
        break;
      case "window/showMessageRequest":
        result = null;
        break;
      case "workspace/applyEdit":
        result = { applied: false, failureReason: "Orkestr does not apply server-initiated workspace edits yet" };
        break;
      default:
        error = { code: -32601, message: `Orkestr's editor does not implement ${method}` };
    }
    await this.send(error ? { jsonrpc: "2.0", id, error } : { jsonrpc: "2.0", id, result }).catch(() => undefined);
  }

  private async send(msg: object): Promise<void> {
    await getClient().call("lsp.send", { sessionId: this.session.sessionId, message: JSON.stringify(msg) });
  }

  /** Sends a request; rejects on error / timeout / server stop. */
  request<T = unknown>(method: string, params: unknown, timeoutMs = 20_000): Promise<T> {
    if (!this.running) return Promise.reject(new Error("language server is not running"));
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} timed out`));
        void this.notify("$/cancelRequest", { id });
      }, timeoutMs);
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject, timer });
      this.send({ jsonrpc: "2.0", id, method, params }).catch((e: unknown) => {
        clearTimeout(timer);
        this.pending.delete(id);
        reject(e instanceof Error ? e : new Error(String(e)));
      });
    });
  }

  /** Sends a notification (dropped while the server is not running). */
  async notify(method: string, params: unknown): Promise<void> {
    if (!this.running) return;
    await this.send({ jsonrpc: "2.0", method, params });
  }

  /** `initialize` + `initialized`. */
  async initialize(): Promise<void> {
    const name = this.session.rootUri.split("/").pop() ?? "workspace";
    const res = await this.request<{ capabilities?: Record<string, unknown> }>(
      "initialize",
      {
        processId: null,
        clientInfo: { name: "Orkestr" },
        locale: document.documentElement.lang || "en",
        rootUri: this.session.rootUri,
        workspaceFolders: [{ uri: this.session.rootUri, name: decodeURIComponent(name) }],
        capabilities: CLIENT_CAPABILITIES,
      },
      60_000,
    );
    this.capabilities = res.capabilities ?? {};
    await this.notify("initialized", {});
  }

  private failPending(e: Error): void {
    for (const p of this.pending.values()) {
      clearTimeout(p.timer);
      p.reject(e);
    }
    this.pending.clear();
  }

  /** Graceful `shutdown`/`exit`, then `lsp.stop`. */
  async dispose(): Promise<void> {
    if (this.running) {
      await this.request("shutdown", null, 2_000).catch(() => undefined);
      await this.notify("exit", null).catch(() => undefined);
    }
    this.failPending(new Error("disposed"));
    this.off?.();
    this.off = null;
    await getClient()
      .call("lsp.stop", { sessionId: this.session.sessionId })
      .catch(() => undefined);
  }
}
