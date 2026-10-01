import type { Event, Project } from "@orkestr/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { backoffDelay } from "./backoff";
import { type ConnectionState, OrkestrClient, type WebSocketLike } from "./client";
import { OrkestrError } from "./errors";

const TOKEN = "t".repeat(64);
const PID = "0190f0b0-0000-7000-8000-000000000001";

type Frame = Record<string, unknown> & { type: string; id?: string };

/** In-memory fake of the core's WebSocket endpoint. */
class FakeServer {
  sockets: FakeSocket[] = [];
  received: Frame[] = [];
  topics = new Set<string>();
  seq = 0;
  log: Event[] = [];
  acceptToken = TOKEN;
  /** Custom request handler; return undefined to answer `{}`. */
  onReq: (method: string, params: unknown) => unknown = () => ({});

  factory = (url: string): WebSocketLike => {
    const s = new FakeSocket(this, url);
    this.sockets.push(s);
    queueMicrotask(() => s.serverOpen());
    return s;
  };

  get current(): FakeSocket {
    const s = this.sockets[this.sockets.length - 1];
    if (!s) throw new Error("no socket");
    return s;
  }

  handle(sock: FakeSocket, frame: Frame): void {
    this.received.push(frame);
    switch (frame.type) {
      case "auth":
        if (frame["token"] === this.acceptToken) {
          sock.push({ type: "res", id: frame.id, ok: true, result: { sessionId: "s", serverVersion: "0.1.0", protocolVersion: 1, lastSeq: this.seq } });
        } else {
          sock.push({ type: "res", id: frame.id, ok: false, error: { code: "unauthorized", message: "bad token", data: null } });
          sock.serverClose(4401, "unauthorized");
        }
        return;
      case "sub":
        for (const t of frame["topics"] as string[]) this.topics.add(t);
        sock.push({ type: "res", id: frame.id, ok: true, result: { topics: [...this.topics] } });
        return;
      case "unsub":
        for (const t of frame["topics"] as string[]) this.topics.delete(t);
        sock.push({ type: "res", id: frame.id, ok: true, result: { topics: [...this.topics] } });
        return;
      case "req": {
        const method = frame["method"] as string;
        if (method === "events.replay") {
          const from = (frame["params"] as { fromSeq: number }).fromSeq;
          sock.push({ type: "res", id: frame.id, ok: true, result: { events: this.log.filter((e) => e.seq >= from), lastSeq: this.seq, hasMore: false } });
          return;
        }
        const result = this.onReq(method, frame["params"]);
        if (result instanceof OrkestrError) {
          sock.push({ type: "res", id: frame.id, ok: false, error: { code: result.code, message: result.message, data: null } });
        } else if (result !== "__noreply__") {
          sock.push({ type: "res", id: frame.id, ok: true, result: result ?? {} });
        }
        return;
      }
    }
  }

  /** Publishes a global Log event (persisted in the fake log). */
  publish(message: string, deliver = true): Event {
    const ev: Event = {
      seq: ++this.seq,
      id: `e${this.seq}`,
      ts: new Date().toISOString(),
      projectId: null,
      taskId: null,
      agentId: null,
      nodeId: null,
      correlationId: null,
      kind: "Log",
      payload: { level: "info", target: "test", message, fields: null },
    };
    this.log.push(ev);
    if (deliver && this.topics.size > 0) this.current.push({ type: "event", seq: ev.seq, event: ev });
    return ev;
  }
}

class FakeSocket implements WebSocketLike {
  readyState = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: { code: number; reason: string }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;

  constructor(
    private readonly server: FakeServer,
    readonly url: string,
  ) {}

  serverOpen(): void {
    if (this.readyState !== 0) return;
    this.readyState = 1;
    this.onopen?.({});
  }

  send(data: string): void {
    if (this.readyState !== 1) throw new Error("not open");
    const frame = JSON.parse(data) as Frame;
    queueMicrotask(() => this.server.handle(this, frame));
  }

  push(frame: unknown): void {
    if (this.readyState !== 1) return;
    const data = JSON.stringify(frame);
    queueMicrotask(() => this.onmessage?.({ data }));
  }

  serverClose(code = 1006, reason = ""): void {
    if (this.readyState === 3) return;
    this.readyState = 3;
    queueMicrotask(() => this.onclose?.({ code, reason }));
  }

  close(code = 1000, reason = ""): void {
    this.serverClose(code, reason);
  }
}

async function flush(times = 10): Promise<void> {
  for (let i = 0; i < times; i++) await Promise.resolve();
}

describe("OrkestrClient", () => {
  let server: FakeServer;

  beforeEach(() => {
    server = new FakeServer();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("authenticates and makes typed calls", async () => {
    const project = { id: PID, name: "Demo" } as unknown as Project;
    server.onReq = (method) => (method === "projects.list" ? { projects: [project] } : {});
    const states: ConnectionState[] = [];
    const client = new OrkestrClient("ws://x/rpc", TOKEN, { webSocket: server.factory });
    client.onStateChange((s) => states.push(s));
    await client.connect();
    expect(client.state).toBe("open");
    expect(server.received[0]).toMatchObject({ type: "auth", token: TOKEN, id: "auth" });
    const res = await client.call("projects.list", {});
    expect(res.projects[0]?.name).toBe("Demo");
    expect(states).toEqual(["idle", "connecting", "authenticating", "open"]);
    client.close();
    expect(client.state).toBe("closed");
  });

  it("maps server errors to OrkestrError", async () => {
    server.onReq = () => new OrkestrError("not_found", "nope");
    const client = await OrkestrClient.connect("ws://x/rpc", TOKEN, { webSocket: server.factory });
    await expect(client.call("projects.get", { projectId: PID })).rejects.toMatchObject({ code: "not_found", message: "nope" });
    client.close();
  });

  it("rejects a bad token without reconnecting", async () => {
    server.acceptToken = "other";
    const client = new OrkestrClient("ws://x/rpc", TOKEN, { webSocket: server.factory });
    await expect(client.connect()).rejects.toMatchObject({ code: "unauthorized" });
    await flush();
    expect(client.state).toBe("closed");
    expect(server.sockets).toHaveLength(1);
  });

  it("times out requests", async () => {
    server.onReq = () => "__noreply__";
    const client = await OrkestrClient.connect("ws://x/rpc", TOKEN, { webSocket: server.factory });
    await expect(client.call("system.health", {}, { timeoutMs: 20 })).rejects.toMatchObject({ code: "timeout" });
    client.close();
  });

  it("supports abort signals", async () => {
    server.onReq = () => "__noreply__";
    const client = await OrkestrClient.connect("ws://x/rpc", TOKEN, { webSocket: server.factory });
    const ctrl = new AbortController();
    const p = client.call("system.health", {}, { signal: ctrl.signal });
    ctrl.abort();
    await expect(p).rejects.toMatchObject({ code: "aborted" });
    client.close();
  });

  it("subscribes with reference counting and delivers events", async () => {
    const client = await OrkestrClient.connect("ws://x/rpc", TOKEN, { webSocket: server.factory });
    const a: string[] = [];
    const b: string[] = [];
    const offA = client.subscribe(["global"], (e) => a.push(e.kind));
    const offB = client.subscribe(["global"], (e) => b.push(e.kind));
    await flush();
    expect(server.received.filter((f) => f.type === "sub")).toHaveLength(1);
    server.publish("hello");
    await flush();
    expect(a).toEqual(["Log"]);
    expect(b).toEqual(["Log"]);
    offA();
    await flush();
    expect(server.received.filter((f) => f.type === "unsub")).toHaveLength(0);
    offB();
    await flush();
    expect(server.received.filter((f) => f.type === "unsub")).toHaveLength(1);
    expect(() => client.subscribe(["bogus"], () => {})).toThrow(OrkestrError);
    client.close();
  });

  it("reconnects, resubscribes and replays missed events without duplicates", async () => {
    const client = await OrkestrClient.connect("ws://x/rpc", TOKEN, {
      webSocket: server.factory,
      reconnect: { initialDelayMs: 1, maxDelayMs: 2, jitter: 0 },
    });
    const seen: number[] = [];
    client.subscribe(["global"], (e) => seen.push(e.seq));
    await flush();
    server.publish("one");
    await flush();
    expect(seen).toEqual([1]);

    // Connection drops; two events are published while offline.
    server.current.serverClose(1006, "network");
    server.topics.clear();
    await flush();
    expect(client.state).toBe("reconnecting");
    server.publish("two", false);
    server.publish("three", false);

    await vi.waitFor(() => expect(client.state).toBe("open"), { timeout: 1000 });
    await vi.waitFor(() => expect(seen).toEqual([1, 2, 3]), { timeout: 1000 });
    expect(server.topics.has("global")).toBe(true);

    server.publish("four");
    await vi.waitFor(() => expect(seen).toEqual([1, 2, 3, 4]), { timeout: 1000 });
    client.close();
  });

  it("fails pending calls on disconnect", async () => {
    server.onReq = () => "__noreply__";
    const client = await OrkestrClient.connect("ws://x/rpc", TOKEN, {
      webSocket: server.factory,
      reconnect: { initialDelayMs: 50 },
    });
    const p = client.call("system.health", {});
    await flush();
    server.current.serverClose(1006);
    await expect(p).rejects.toMatchObject({ code: "disconnected" });
    client.close();
  });

  it("waits for the connection before sending calls", async () => {
    server.onReq = () => ({ status: "ok", version: "0.1.0", uptimeMs: 1 });
    const client = new OrkestrClient("ws://x/rpc", TOKEN, { webSocket: server.factory });
    void client.connect();
    const res = await client.call("system.health", {});
    expect(res.status).toBe("ok");
    client.close();
  });
});

describe("backoffDelay", () => {
  it("grows exponentially and caps", () => {
    const fixed = () => 0.5;
    expect(backoffDelay(0, { initialDelayMs: 100, jitter: 0 }, fixed)).toBe(100);
    expect(backoffDelay(3, { initialDelayMs: 100, jitter: 0 }, fixed)).toBe(800);
    expect(backoffDelay(30, { initialDelayMs: 100, maxDelayMs: 5000, jitter: 0 }, fixed)).toBe(5000);
    const d = backoffDelay(1, { initialDelayMs: 100, jitter: 0.5 }, () => 0);
    expect(d).toBe(100);
  });
});
