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
import { type StepUpProvider, callWithStepUp } from "./approvals";
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

/** Outcome of `MobileCore.unpair`: whether the core revoked this device too. */
export type UnpairResult =
  | { revoked: true }
  | { revoked: false; reason: "skipped" | "not_admin" | "failed"; error?: string };

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

  /**
   * Unpairs a core. With `revoke`, an admin phone first revokes its own
   * device on the core (`devices.revoke`, biometric step-up); other roles
   * cannot (admin-only method), so the result says the computer must revoke
   * it. The local keys are always forgotten.
   */
  async unpair(coreId: string, opts: { revoke?: boolean } = {}): Promise<UnpairResult> {
    let result: UnpairResult = { revoked: false, reason: "skipped" };
    const rec = await this.store.get(coreId);
    if (opts.revoke && rec) {
      if (rec.role !== "admin") {
        result = { revoked: false, reason: "not_admin" };
      } else {
        try {
          const client = await this.connections.get(coreId);
          await callWithStepUp(client, "devices.revoke", { deviceId: rec.deviceId }, this.stepUp(coreId));
          result = { revoked: true };
        } catch (e) {
          const code = (e as { code?: unknown }).code;
          // Revoking itself closes this very session, usually before the
          // response is flushed: confirm by reconnecting (refused if revoked).
          if (code === "disconnected" && (await this.refused(coreId))) {
            result = { revoked: true };
          } else {
            result = { revoked: false, reason: "failed", error: typeof code === "string" ? code : e instanceof Error ? e.message : String(e) };
          }
        }
      }
    }
    await this.connections.disconnect(coreId);
    await this.stepUpKeys.remove(coreId);
    await this.store.delete(coreId);
    return result;
  }

  /** True when a fresh connection to `coreId` is refused by the core. */
  private async refused(coreId: string): Promise<boolean> {
    await this.connections.disconnect(coreId);
    const attempt = this.connections.get(coreId).then((c) => c.call("remote.status", {}));
    const timeout = new Promise<"timeout">((r) => setTimeout(() => r("timeout"), 8000));
    try {
      await Promise.race([attempt, timeout]); // answered (or hung): not refused
      return false;
    } catch (e) {
      const code = (e as { code?: unknown }).code;
      return code === "unauthorized" || code === "forbidden" || code === "revoked";
    } finally {
      void attempt.catch(() => undefined);
      await this.connections.disconnect(coreId);
    }
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
