/**
 * `MobileCore`: wires session, key store, connections, step-up and push for
 * one app instance. The RN app builds it with Expo adapters; tests and the
 * acceptance script with the in-memory ones (`createMemoryCore`).
 */
import type { DeviceRecord, PairingOffer, PairingSession, RawSocketFactory, StepUp } from "@orkestr/client";

import {
  type BiometricGate,
  MemoryBiometricGate,
  MemoryNotifier,
  MemorySecureKV,
  type Notifier,
  type SecureKV,
} from "./adapters";
import { type StepUpProvider } from "./approvals";
import { ConnectionManager } from "./connection";
import { type FetchLike, GatewaySession } from "./gatewaySession";
import { type KeyStorage, SecureKVKeyStore, StepUpKeys } from "./keys";
import { registerStepUpAfterPairing, startPairing } from "./pairing";
import { PushManager } from "./push";

export interface MobileCoreOptions {
  kv: SecureKV;
  gate: BiometricGate;
  notifier: Notifier;
  fetch?: FetchLike;
  rawSocket?: RawSocketFactory;
  platform?: string;
  deviceName?: string;
  keyStorage?: KeyStorage;
  /** Text of the biometric prompt. */
  stepUpPrompt?: string;
}

export class MobileCore {
  readonly store: SecureKVKeyStore;
  readonly stepUpKeys: StepUpKeys;
  readonly push: PushManager;
  readonly connections: ConnectionManager;
  session: GatewaySession | null = null;

  constructor(readonly o: MobileCoreOptions) {
    this.store = new SecureKVKeyStore(o.kv);
    this.stepUpKeys = new StepUpKeys(o.kv, o.gate, o.keyStorage ?? "software");
    this.push = new PushManager(o.kv, o.notifier);
    this.connections = new ConnectionManager({
      store: this.store,
      token: () => this.session?.token ?? null,
      ...(o.rawSocket ? { rawSocket: o.rawSocket } : {}),
    });
  }

  /** Restores the stored gateway session (if any). */
  async restore(): Promise<GatewaySession | null> {
    this.session = await GatewaySession.restore(this.o.kv, this.o.fetch);
    return this.session;
  }

  async login(gatewayUrl: string, totp: string): Promise<GatewaySession> {
    const s = new GatewaySession(gatewayUrl.trim().replace(/\/+$/, ""), { kv: this.o.kv, ...(this.o.fetch ? { fetch: this.o.fetch } : {}) });
    await s.totpLogin(totp);
    this.session = s;
    return s;
  }

  async logout(): Promise<void> {
    await this.connections.closeAll();
    await this.session?.logout();
    this.session = null;
  }

  /** Claims a pairing offer; show `session.sas`, then `completePairing`. */
  pair(offer: PairingOffer): Promise<PairingSession> {
    const fetch = this.session?.fetch() ?? this.o.fetch ?? globalThis.fetch.bind(globalThis);
    return startPairing({
      offer,
      deviceName: this.o.deviceName ?? "Orkestr Mobile",
      platform: this.o.platform ?? "android",
      store: this.store,
      fetch,
    });
  }

  /** Waits for the desktop's approval, connects and registers the step-up key. */
  async completePairing(
    session: PairingSession,
    opts: { intervalMs?: number; signal?: AbortSignal } = {},
  ): Promise<{ record: DeviceRecord; stepUpRegistered: boolean; stepUpError?: string }> {
    const record = await session.waitForApproval(opts);
    const client = await this.connections.get(record.coreId);
    const r = await registerStepUpAfterPairing(client, record, this.stepUpKeys);
    return { record, stepUpRegistered: r.registered, ...(r.error ? { stepUpError: r.error } : {}) };
  }

  /** Step-up provider for a core (biometric prompt → native assertion). */
  stepUp(coreId: string): StepUpProvider {
    return async (challenge: string): Promise<StepUp> => {
      const rec = await this.store.get(coreId);
      if (!rec) throw new Error(`core ${coreId} is not paired`);
      return this.stepUpKeys.assert(coreId, rec.deviceId, challenge, this.o.stepUpPrompt ?? "Confirm with biometrics");
    };
  }

  /** Forgets a core locally (keys, push key, connection). */
  async unpair(coreId: string): Promise<void> {
    await this.connections.disconnect(coreId);
    await this.stepUpKeys.remove(coreId);
    await this.store.delete(coreId);
  }
}

/** In-memory core for tests / the Node acceptance script. */
export function createMemoryCore(o: Partial<MobileCoreOptions> = {}): MobileCore & {
  o: MobileCoreOptions & { kv: MemorySecureKV; gate: MemoryBiometricGate; notifier: MemoryNotifier };
} {
  const opts = {
    kv: new MemorySecureKV(),
    gate: new MemoryBiometricGate(),
    notifier: new MemoryNotifier(),
    ...o,
  } as MobileCoreOptions & { kv: MemorySecureKV; gate: MemoryBiometricGate; notifier: MemoryNotifier };
  return new MobileCore(opts) as MobileCore & { o: typeof opts };
}
