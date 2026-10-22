/**
 * RemoteTransport: a `WebSocketFactory` whose sockets run the e2e-v1
 * handshake over the gateway relay and then encrypt/decrypt every frame, so
 * `OrkestrClient` works unchanged (its `auth` frame travels encrypted and is
 * ignored by the remote session — the principal comes from the handshake).
 */
import type { WebSocketFactory, WebSocketLike } from "../client";
import { fromUtf8, utf8 } from "./bytes";
import { connectUrl } from "./endpoints";
import {
  DIR_C2D,
  DIR_D2C,
  DeviceHandshake,
  type HandshakeInit,
  type ServerHello,
  type SessionKeys,
  openRecord,
  sealRecord,
} from "./e2e";

/** Underlying (untrusted) socket to the gateway relay. */
export interface RawSocket {
  readonly readyState: number;
  binaryType?: string;
  send(data: string | ArrayBuffer | Uint8Array<ArrayBuffer>): void;
  close(code?: number, reason?: string): void;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: { code: number; reason: string }) => void) | null;
  onerror: ((ev: unknown) => void) | null;
}
export type RawSocketFactory = (url: string) => RawSocket;

export interface RemoteSocketOptions {
  /** Gateway origin, e.g. `https://gw.example` (or `http://127.0.0.1:8787`). */
  gatewayUrl: string;
  coreId: string;
  deviceId: string;
  /** Device key DK (non-extractable private key). */
  deviceKey: CryptoKeyPair;
  /** Core identity key, raw SEC1 base64url (pinned at pairing). */
  corePubKey: string;
  /** Gateway session token appended as `&token=` (default: rely on the session cookie). */
  sessionToken?: string;
  /** Raw socket factory (default: global `WebSocket`). */
  rawSocket?: RawSocketFactory;
  /** Handshake timeout (ms). Default 10 000. */
  handshakeTimeoutMs?: number;
  /** Called with the core's error code (`revoked`, `unknown_device`, …) or a local reason. */
  onHandshakeError?: (code: string) => void;
  /** Test hooks: fixed ephemeral key / nonce / clock. */
  handshake?: Pick<HandshakeInit, "ephemeral" | "nonce" | "now">;
}

/** Close code when the core refuses the device or the crypto fails (fatal: no reconnect). */
export const E2E_CLOSE_CODE = 4401;
/** Close code for a transient E2E failure (timeout): the client reconnects. */
export const E2E_TRANSIENT_CLOSE_CODE = 4408;

const CONNECTING = 0;
const OPEN = 1;
const CLOSING = 2;
const CLOSED = 3;

async function toBytes(data: unknown): Promise<Uint8Array | null> {
  if (data instanceof Uint8Array) return data;
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  if (typeof Blob !== "undefined" && data instanceof Blob) return new Uint8Array(await data.arrayBuffer());
  return null;
}

class RemoteSocket implements WebSocketLike {
  readyState = CONNECTING;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: { code: number; reason: string }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;

  private readonly raw: RawSocket;
  private readonly hs: DeviceHandshake;
  private keys: SessionKeys | null = null;
  private sendCtr = 0;
  private recvCtr = 0;
  private sendChain: Promise<void> = Promise.resolve();
  private recvChain: Promise<void> = Promise.resolve();
  private phase: "hello" | "serverHello" | "open" | "done" = "hello";
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    url: string,
    private readonly o: RemoteSocketOptions,
  ) {
    this.hs = new DeviceHandshake({
      coreId: o.coreId,
      deviceId: o.deviceId,
      deviceKey: o.deviceKey.privateKey,
      corePubKey: o.corePubKey,
      ...o.handshake,
    });
    const make = o.rawSocket ?? ((u: string) => new WebSocket(u) as unknown as RawSocket);
    this.raw = make(url);
    this.raw.binaryType = "arraybuffer";
    this.timer = setTimeout(() => this.fail("handshake_timeout", E2E_TRANSIENT_CLOSE_CODE), o.handshakeTimeoutMs ?? 10_000);
    this.raw.onopen = () => {
      this.hs
        .hello()
        .then((h) => {
          if (this.phase !== "hello") return;
          this.phase = "serverHello";
          this.raw.send(JSON.stringify(h));
        })
        .catch((e: unknown) => this.fail(`handshake: ${String(e)}`));
    };
    this.raw.onmessage = (ev) => {
      this.recvChain = this.recvChain.then(() => this.onRaw(ev.data)).catch((e: unknown) => this.fail(String(e)));
    };
    this.raw.onerror = (ev) => this.onerror?.(ev);
    this.raw.onclose = (ev) => this.finish(ev.code, ev.reason);
  }

  private async onRaw(data: unknown): Promise<void> {
    if (this.phase === "done") return;
    // Handshake frames are JSON; the gateway relays them as text or binary.
    let text: string | null = typeof data === "string" ? data : null;
    if (text === null && this.phase !== "open") {
      const b = await toBytes(data);
      text = b ? fromUtf8(b) : null;
    }
    if (text !== null) {
      let frame: { t?: string; code?: string };
      try {
        frame = JSON.parse(text) as { t?: string; code?: string };
      } catch {
        throw new Error("malformed handshake frame");
      }
      if (frame.t === "error") {
        this.fail(frame.code ?? "error");
        return;
      }
      if (frame.t !== "serverHello" || this.phase !== "serverHello") throw new Error("unexpected handshake frame");
      const { keys, finish } = await this.hs.finish(frame as ServerHello);
      this.keys = keys;
      this.raw.send(JSON.stringify(finish));
      this.phase = "open";
      this.readyState = OPEN;
      if (this.timer) clearTimeout(this.timer);
      this.timer = null;
      this.onopen?.({});
      return;
    }
    const bytes = await toBytes(data);
    if (!bytes || !this.keys || this.phase !== "open") throw new Error("unexpected binary frame");
    const pt = await openRecord(this.keys.c2d, DIR_C2D, this.recvCtr, bytes);
    this.recvCtr += 1;
    this.onmessage?.({ data: fromUtf8(pt) });
  }

  send(data: string): void {
    if (this.readyState !== OPEN || !this.keys) throw new Error("remote socket is not open");
    const keys = this.keys;
    const ctr = this.sendCtr++;
    const pt = utf8(data);
    this.sendChain = this.sendChain
      .then(async () => {
        const rec = await sealRecord(keys.d2c, DIR_D2C, ctr, pt);
        if (this.raw.readyState === OPEN) this.raw.send(rec);
      })
      .catch((e: unknown) => this.fail(String(e)));
  }

  close(code = 1000, reason = ""): void {
    if (this.readyState >= CLOSING) return;
    this.readyState = CLOSING;
    this.raw.close(code, reason);
    // Some environments never deliver `onclose` for a socket closed before open.
    if (this.raw.readyState === CLOSED) this.finish(code, reason);
  }

  private fail(reason: string, code = E2E_CLOSE_CODE): void {
    if (this.phase === "done") return;
    this.o.onHandshakeError?.(reason);
    this.raw.onclose = null;
    try {
      this.raw.close(1000, "e2e failure");
    } catch {
      /* ignore */
    }
    this.finish(code, `e2e: ${reason}`);
  }

  private finish(code: number, reason: string): void {
    if (this.phase === "done") return;
    this.phase = "done";
    this.readyState = CLOSED;
    this.keys = null;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.raw.onopen = this.raw.onmessage = this.raw.onerror = this.raw.onclose = null;
    this.onclose?.({ code, reason });
  }
}

/**
 * Creates a `WebSocketFactory` for `OrkestrClient` that reaches the core
 * through the gateway relay with end-to-end encryption. An empty `url`
 * argument means "the relay URL for `coreId`".
 */
export function createRemoteSocketFactory(opts: RemoteSocketOptions): WebSocketFactory {
  return (url: string) => new RemoteSocket(url || connectUrl(opts.gatewayUrl, opts.coreId, opts.sessionToken), opts);
}

/** URL to pass to `new OrkestrClient(url, token, { webSocket })` for a remote core. */
export function remoteClientUrl(opts: Pick<RemoteSocketOptions, "gatewayUrl" | "coreId" | "sessionToken">): string {
  return connectUrl(opts.gatewayUrl, opts.coreId, opts.sessionToken);
}
