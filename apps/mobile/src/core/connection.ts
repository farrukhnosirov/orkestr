/**
 * One E2E `OrkestrClient` per paired core, through the gateway relay
 * (`createRemoteSocketFactory` with the raw device key as `DeviceSigner`;
 * the transport sets `binaryType = "arraybuffer"` on the raw socket).
 */
import {
  type ConnectionState,
  type DeviceKeyStore,
  type DeviceRecord,
  OrkestrClient,
  type RawSocketFactory,
  createRemoteSocketFactory,
  deviceSigner,
  remoteClientUrl,
} from "@orkestr/client";

/** The subset of `OrkestrClient` the app uses (fakes in tests). */
export type RemoteClient = Pick<OrkestrClient, "call" | "subscribe">;

export interface ConnectionOptions {
  store: DeviceKeyStore;
  /** Current gateway bearer (appended as `&token=`). */
  token: () => string | null;
  rawSocket?: RawSocketFactory;
  clientVersion?: string;
  /** Called on each core's connection state change. */
  onState?: (coreId: string, state: ConnectionState, error?: { code: string; message: string }) => void;
  /** Called with the core's handshake refusal (`revoked`, `unknown_device`, …). */
  onHandshakeError?: (coreId: string, code: string) => void;
}

/** Builds and connects a client for a device record. */
export async function connectCore(rec: DeviceRecord, o: Omit<ConnectionOptions, "store">): Promise<OrkestrClient> {
  const token = o.token();
  const opts = {
    gatewayUrl: rec.gatewayUrl,
    coreId: rec.coreId,
    deviceId: rec.deviceId,
    deviceKey: await deviceSigner(rec),
    corePubKey: rec.corePubKey,
    ...(token ? { sessionToken: token } : {}),
    ...(o.rawSocket ? { rawSocket: o.rawSocket } : {}),
    onHandshakeError: (code: string) => o.onHandshakeError?.(rec.coreId, code),
  };
  const client = new OrkestrClient(remoteClientUrl(opts), "remote", {
    webSocket: createRemoteSocketFactory(opts),
    clientInfo: { kind: "mobile", version: o.clientVersion ?? "0.1.0" },
  });
  client.onStateChange((s, e) => o.onState?.(rec.coreId, s, e ? { code: e.code, message: e.message } : undefined));
  await client.connect();
  return client;
}

export class ConnectionManager {
  private readonly clients = new Map<string, Promise<OrkestrClient>>();

  constructor(private readonly o: ConnectionOptions) {}

  /** The (cached) connected client for `coreId`. */
  get(coreId: string): Promise<OrkestrClient> {
    let p = this.clients.get(coreId);
    if (!p) {
      p = (async () => {
        const rec = await this.o.store.get(coreId);
        if (!rec) throw Object.assign(new Error(`core ${coreId} is not paired on this device`), { code: "not_found" });
        return connectCore(rec, this.o);
      })();
      p.catch(() => this.clients.delete(coreId));
      this.clients.set(coreId, p);
    }
    return p;
  }

  has(coreId: string): boolean {
    return this.clients.has(coreId);
  }

  async disconnect(coreId: string): Promise<void> {
    const p = this.clients.get(coreId);
    this.clients.delete(coreId);
    if (p) (await p.catch(() => null))?.close();
  }

  async closeAll(): Promise<void> {
    await Promise.all([...this.clients.keys()].map((id) => this.disconnect(id)));
  }
}
