import type {
  AuthResult,
  ClientFrame,
  ClientInfo,
  Event,
  JsonValue,
  MethodName,
  ParamsOf,
  ResFrame,
  ResultOf,
  ServerFrame,
  SubResult,
} from "@orkestr/protocol";
import { eventMatches, isValidTopic } from "@orkestr/protocol";

import { type BackoffOptions, backoffDelay } from "./backoff";
import { OrkestrError } from "./errors";

/** Minimal WebSocket surface (browser `WebSocket`, React Native, test fakes). */
export interface WebSocketLike {
  readonly readyState: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: { code: number; reason: string }) => void) | null;
  onerror: ((ev: unknown) => void) | null;
}

export type WebSocketFactory = (url: string) => WebSocketLike;

/** Connection lifecycle. */
export type ConnectionState =
  | "idle"
  | "connecting"
  | "authenticating"
  | "open"
  | "reconnecting"
  | "closed";

export interface ClientOptions {
  /** Default request timeout (ms). Default 15 000. */
  requestTimeoutMs?: number;
  /** Timeout for the auth handshake (ms). Default 10 000. */
  authTimeoutMs?: number;
  /** Reconnect policy; `false` disables reconnecting. */
  reconnect?: BackoffOptions | false;
  /** Fill event gaps after reconnect via `events.replay` (default true). */
  replayOnReconnect?: boolean;
  /** Client identification sent in the auth frame. */
  clientInfo?: ClientInfo;
  /** WebSocket factory (defaults to the global `WebSocket`). */
  webSocket?: WebSocketFactory;
  /** Diagnostic logger. */
  logger?: (level: "debug" | "warn" | "error", message: string, data?: unknown) => void;
}

export interface CallOptions {
  timeoutMs?: number;
  signal?: AbortSignal;
}

export type EventHandler = (event: Event) => void;
export type StateListener = (state: ConnectionState, error?: OrkestrError) => void;
export type Unsubscribe = () => void;

interface Pending {
  resolve: (value: unknown) => void;
  reject: (err: OrkestrError) => void;
  timer: ReturnType<typeof setTimeout>;
  cleanup: () => void;
}

interface Subscription {
  topics: string[];
  handler: EventHandler;
}

const OPEN = 1;
const AUTH_ID = "auth";

function defaultFactory(): WebSocketFactory {
  const WS = (globalThis as { WebSocket?: new (url: string) => WebSocketLike }).WebSocket;
  if (!WS) {
    throw new OrkestrError("protocol", "no global WebSocket available; pass options.webSocket");
  }
  return (url) => new WS(url);
}

/**
 * Typed client for the Orkestr core RPC protocol.
 *
 * ```ts
 * const client = await OrkestrClient.connect(url, token);
 * const { projects } = await client.call("projects.list", {});
 * const off = client.subscribe(["global"], (ev) => console.log(ev.kind));
 * ```
 *
 * Reconnects automatically with exponential backoff, re-subscribes, and
 * (by default) replays missed persisted events so subscribers see a gap-free
 * stream. An invalid token is fatal (state `closed`, no reconnect).
 */
export class OrkestrClient {
  readonly url: string;
  private readonly token: string;
  private readonly opts: Required<Omit<ClientOptions, "clientInfo" | "webSocket" | "logger" | "reconnect">> & {
    reconnect: BackoffOptions | false;
    clientInfo: ClientInfo | undefined;
    webSocket: WebSocketFactory | undefined;
    logger: ClientOptions["logger"];
  };

  private ws: WebSocketLike | null = null;
  private stateValue: ConnectionState = "idle";
  private readonly stateListeners = new Set<StateListener>();
  private readonly pending = new Map<string, Pending>();
  private readonly subscriptions = new Set<Subscription>();
  private readonly topicRefs = new Map<string, number>();
  private nextId = 1;
  private attempt = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private authTimer: ReturnType<typeof setTimeout> | null = null;
  private openWaiters: Array<{ resolve: () => void; reject: (e: OrkestrError) => void }> = [];
  private closedByUser = false;
  private hasConnectedOnce = false;
  /** Highest persisted `seq` delivered to subscribers. */
  private lastSeq = 0;
  /** Events buffered while a replay is in flight. */
  private replayBuffer: Event[] | null = null;
  private serverInfo: AuthResult | null = null;

  constructor(url: string, token: string, options: ClientOptions = {}) {
    this.url = url;
    this.token = token;
    this.opts = {
      requestTimeoutMs: options.requestTimeoutMs ?? 15_000,
      authTimeoutMs: options.authTimeoutMs ?? 10_000,
      replayOnReconnect: options.replayOnReconnect ?? true,
      reconnect: options.reconnect ?? {},
      clientInfo: options.clientInfo,
      webSocket: options.webSocket,
      logger: options.logger,
    };
  }

  /** Creates a client and waits until it is authenticated. */
  static async connect(url: string, token: string, options: ClientOptions = {}): Promise<OrkestrClient> {
    const client = new OrkestrClient(url, token, options);
    await client.connect();
    return client;
  }

  /** Current connection state. */
  get state(): ConnectionState {
    return this.stateValue;
  }

  /** Server info from the last successful auth. */
  get server(): AuthResult | null {
    return this.serverInfo;
  }

  /** Observes state changes. The listener is called immediately with the current state. */
  onStateChange(listener: StateListener): Unsubscribe {
    this.stateListeners.add(listener);
    listener(this.stateValue);
    return () => this.stateListeners.delete(listener);
  }

  /** Opens the connection (idempotent). Resolves once authenticated. */
  connect(): Promise<void> {
    if (this.stateValue === "open") return Promise.resolve();
    if (this.stateValue === "closed" && this.closedByUser) {
      return Promise.reject(new OrkestrError("closed", "client was closed"));
    }
    const p = this.waitOpen();
    if (this.stateValue === "idle") this.open();
    return p;
  }

  /** Closes the connection for good. Pending calls reject with `closed`. */
  close(): void {
    this.closedByUser = true;
    this.clearTimers();
    const ws = this.ws;
    this.ws = null;
    if (ws) {
      this.detach(ws);
      try {
        ws.close(1000, "client closed");
      } catch {
        /* ignore */
      }
    }
    this.failAll(new OrkestrError("closed", "client was closed"));
    this.setState("closed");
  }

  /** Calls a method. Waits for the connection (within the timeout) if it is not open yet. */
  async call<M extends MethodName>(method: M, params: ParamsOf<M>, options: CallOptions = {}): Promise<ResultOf<M>> {
    const timeoutMs = options.timeoutMs ?? this.opts.requestTimeoutMs;
    const deadline = Date.now() + timeoutMs;
    if (options.signal?.aborted) throw new OrkestrError("aborted", "request aborted");
    if (this.stateValue !== "open") {
      await this.withTimeout(this.connect(), timeoutMs, `connection not ready for ${method}`);
    }
    const remaining = Math.max(1, deadline - Date.now());
    return (await this.request(
      { type: "req", id: this.allocId("r"), method, params: params as unknown as JsonValue },
      remaining,
      options.signal,
      method,
    )) as ResultOf<M>;
  }

  /**
   * Subscribes a handler to topics (`global`, `project:<id>`, `*`, …).
   * Topics are reference counted across handlers; the server subscription is
   * kept in sync and restored after reconnects.
   */
  subscribe(topics: string[], handler: EventHandler): Unsubscribe {
    const bad = topics.find((t) => !isValidTopic(t));
    if (bad !== undefined) throw new OrkestrError("bad_request", `invalid topic "${bad}"`);
    const sub: Subscription = { topics: [...new Set(topics)], handler };
    this.subscriptions.add(sub);
    const added: string[] = [];
    for (const t of sub.topics) {
      const n = this.topicRefs.get(t) ?? 0;
      this.topicRefs.set(t, n + 1);
      if (n === 0) added.push(t);
    }
    if (added.length > 0 && this.stateValue === "open") {
      this.sendSub("sub", added).catch((e: unknown) => this.log("warn", "subscribe failed", e));
    }
    let active = true;
    return () => {
      if (!active) return;
      active = false;
      this.subscriptions.delete(sub);
      const removed: string[] = [];
      for (const t of sub.topics) {
        const n = (this.topicRefs.get(t) ?? 1) - 1;
        if (n <= 0) {
          this.topicRefs.delete(t);
          removed.push(t);
        } else {
          this.topicRefs.set(t, n);
        }
      }
      if (removed.length > 0 && this.stateValue === "open") {
        this.sendSub("unsub", removed).catch((e: unknown) => this.log("warn", "unsubscribe failed", e));
      }
    };
  }

  // ---------------------------------------------------------------------------
  // internals
  // ---------------------------------------------------------------------------

  private log(level: "debug" | "warn" | "error", message: string, data?: unknown): void {
    this.opts.logger?.(level, message, data);
  }

  private setState(state: ConnectionState, error?: OrkestrError): void {
    if (this.stateValue === state && !error) return;
    this.stateValue = state;
    for (const l of [...this.stateListeners]) {
      try {
        l(state, error);
      } catch (e) {
        this.log("error", "state listener threw", e);
      }
    }
  }

  private allocId(prefix: string): string {
    return `${prefix}${this.nextId++}`;
  }

  private waitOpen(): Promise<void> {
    return new Promise((resolve, reject) => this.openWaiters.push({ resolve, reject }));
  }

  private settleOpenWaiters(error?: OrkestrError): void {
    const waiters = this.openWaiters;
    this.openWaiters = [];
    for (const w of waiters) {
      if (error) w.reject(error);
      else w.resolve();
    }
  }

  private withTimeout<T>(p: Promise<T>, ms: number, what: string): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => reject(new OrkestrError("timeout", `timed out: ${what}`)), ms);
      p.then(
        (v) => {
          clearTimeout(timer);
          resolve(v);
        },
        (e: unknown) => {
          clearTimeout(timer);
          reject(e);
        },
      );
    });
  }

  private open(): void {
    this.clearTimers();
    this.setState(this.hasConnectedOnce ? "reconnecting" : "connecting");
    let ws: WebSocketLike;
    try {
      ws = (this.opts.webSocket ?? defaultFactory())(this.url);
    } catch (e) {
      this.log("error", "failed to create WebSocket", e);
      this.scheduleReconnect();
      return;
    }
    this.ws = ws;
    ws.onopen = () => {
      if (this.ws !== ws) return;
      this.setState("authenticating");
      const frame: ClientFrame = { type: "auth", token: this.token, id: AUTH_ID };
      if (this.opts.clientInfo) frame.client = this.opts.clientInfo;
      this.authTimer = setTimeout(() => {
        this.log("warn", "auth handshake timed out");
        ws.close(4000, "auth timeout");
      }, this.opts.authTimeoutMs);
      this.sendRaw(frame);
    };
    ws.onmessage = (ev) => {
      if (this.ws !== ws) return;
      this.onMessage(ev.data);
    };
    ws.onerror = (ev) => {
      if (this.ws !== ws) return;
      this.log("warn", "websocket error", ev);
    };
    ws.onclose = (ev) => {
      if (this.ws !== ws) return;
      this.detach(ws);
      this.ws = null;
      this.onDisconnected(ev.code, ev.reason);
    };
  }

  private detach(ws: WebSocketLike): void {
    ws.onopen = null;
    ws.onmessage = null;
    ws.onerror = null;
    ws.onclose = null;
  }

  private clearTimers(): void {
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    if (this.authTimer) clearTimeout(this.authTimer);
    this.reconnectTimer = null;
    this.authTimer = null;
  }

  private onDisconnected(code: number, reason: string): void {
    this.clearTimers();
    this.replayBuffer = null;
    this.failAll(new OrkestrError("disconnected", `connection lost (${code}${reason ? `: ${reason}` : ""})`));
    if (this.closedByUser) return;
    if (code === 4401) {
      const err = new OrkestrError("unauthorized", "the core rejected the token");
      this.closedByUser = true;
      this.settleOpenWaiters(err);
      this.setState("closed", err);
      return;
    }
    this.scheduleReconnect();
  }

  private scheduleReconnect(): void {
    if (this.opts.reconnect === false) {
      const err = new OrkestrError("disconnected", "connection lost and reconnect is disabled");
      this.settleOpenWaiters(err);
      this.setState("closed", err);
      return;
    }
    const delay = backoffDelay(this.attempt++, this.opts.reconnect);
    this.setState("reconnecting");
    this.log("debug", `reconnecting in ${delay}ms`);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      if (!this.closedByUser) this.open();
    }, delay);
  }

  private failAll(err: OrkestrError): void {
    const pending = [...this.pending.values()];
    this.pending.clear();
    for (const p of pending) {
      clearTimeout(p.timer);
      p.cleanup();
      p.reject(err);
    }
  }

  private sendRaw(frame: ClientFrame): boolean {
    const ws = this.ws;
    if (!ws || ws.readyState !== OPEN) return false;
    ws.send(JSON.stringify(frame));
    return true;
  }

  private request(frame: ClientFrame & { id: string }, timeoutMs: number, signal?: AbortSignal, what?: string): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const id = frame.id;
      const onAbort = () => {
        const p = this.pending.get(id);
        if (!p) return;
        this.pending.delete(id);
        clearTimeout(p.timer);
        reject(new OrkestrError("aborted", "request aborted"));
      };
      const cleanup = () => signal?.removeEventListener("abort", onAbort);
      const timer = setTimeout(() => {
        if (!this.pending.delete(id)) return;
        cleanup();
        reject(new OrkestrError("timeout", `request ${what ?? id} timed out after ${timeoutMs}ms`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer, cleanup });
      signal?.addEventListener("abort", onAbort, { once: true });
      if (!this.sendRaw(frame)) {
        this.pending.delete(id);
        clearTimeout(timer);
        cleanup();
        reject(new OrkestrError("disconnected", "not connected"));
      }
    });
  }

  private async sendSub(type: "sub" | "unsub", topics: string[]): Promise<SubResult> {
    const result = await this.request({ type, id: this.allocId("s"), topics }, this.opts.requestTimeoutMs, undefined, type);
    return result as SubResult;
  }

  private onMessage(data: unknown): void {
    if (typeof data !== "string") {
      this.log("warn", "ignoring non-text frame");
      return;
    }
    let frame: ServerFrame;
    try {
      frame = JSON.parse(data) as ServerFrame;
    } catch (e) {
      this.log("error", "invalid JSON from server", e);
      return;
    }
    if (frame.type === "event") {
      this.onEvent(frame.event);
      return;
    }
    if (frame.type === "res") {
      this.onRes(frame);
      return;
    }
    this.log("warn", "unknown frame", frame);
  }

  private onRes(res: ResFrame): void {
    if (res.id === AUTH_ID && this.stateValue === "authenticating") {
      if (this.authTimer) clearTimeout(this.authTimer);
      this.authTimer = null;
      if (res.ok) {
        this.serverInfo = (res.result ?? null) as AuthResult | null;
        this.onAuthenticated();
      } else {
        const err = res.error ? OrkestrError.fromRpc(res.error) : new OrkestrError("unauthorized", "auth failed");
        this.closedByUser = true; // fatal: do not retry with a bad token
        this.settleOpenWaiters(err);
        this.setState("closed", err);
        const ws = this.ws;
        this.ws = null;
        if (ws) {
          this.detach(ws);
          ws.close(1000, "auth failed");
        }
        this.failAll(err);
      }
      return;
    }
    const p = this.pending.get(res.id);
    if (!p) {
      this.log("debug", `response for unknown id ${res.id}`);
      return;
    }
    this.pending.delete(res.id);
    clearTimeout(p.timer);
    p.cleanup();
    if (res.ok) p.resolve(res.result ?? null);
    else p.reject(res.error ? OrkestrError.fromRpc(res.error) : new OrkestrError("protocol", "error without payload"));
  }

  private onAuthenticated(): void {
    const reconnected = this.hasConnectedOnce;
    this.hasConnectedOnce = true;
    this.attempt = 0;
    let replayAllowed = reconnected && this.opts.replayOnReconnect && this.lastSeq > 0;
    const serverSeq = this.serverInfo?.lastSeq ?? 0;
    if (!reconnected) {
      // Only events published after this point are new to us.
      this.lastSeq = Math.max(this.lastSeq, serverSeq);
    } else if (serverSeq < this.lastSeq) {
      // The server's sequence went backwards (restarted core whose last events
      // were ephemeral, or a different data dir): accept its numbering as-is.
      this.log("warn", `server sequence reset (${this.lastSeq} -> ${serverSeq}); skipping replay`);
      this.lastSeq = serverSeq;
      replayAllowed = false;
    }
    this.setState("open");
    this.settleOpenWaiters();
    const topics = [...this.topicRefs.keys()];
    if (topics.length === 0) return;
    const replay = replayAllowed;
    if (replay) this.replayBuffer = [];
    this.sendSub("sub", topics)
      .then(async () => {
        if (replay) await this.replayMissed(topics);
      })
      .catch((e: unknown) => this.log("warn", "resubscribe/replay failed", e))
      .finally(() => this.flushReplayBuffer());
  }

  private async replayMissed(topics: string[]): Promise<void> {
    let fromSeq = this.lastSeq + 1;
    for (let page = 0; page < 20; page++) {
      const res = await this.call("events.replay", { fromSeq, topics, limit: 1000 });
      for (const ev of res.events) this.deliver(ev);
      if (!res.hasMore || res.events.length === 0) return;
      const last = res.events[res.events.length - 1];
      if (!last) return;
      fromSeq = last.seq + 1;
    }
  }

  private flushReplayBuffer(): void {
    const buffered = this.replayBuffer;
    this.replayBuffer = null;
    if (!buffered) return;
    for (const ev of buffered) this.deliver(ev);
  }

  private onEvent(event: Event): void {
    if (this.replayBuffer) {
      this.replayBuffer.push(event);
      return;
    }
    this.deliver(event);
  }

  private deliver(event: Event): void {
    if (event.seq > 0) {
      if (event.seq <= this.lastSeq) return; // duplicate (replay overlap)
      this.lastSeq = event.seq;
    }
    for (const sub of [...this.subscriptions]) {
      if (!eventMatches(sub.topics, event)) continue;
      try {
        sub.handler(event);
      } catch (e) {
        this.log("error", "event handler threw", e);
      }
    }
  }
}
