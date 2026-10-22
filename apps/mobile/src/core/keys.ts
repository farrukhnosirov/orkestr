/**
 * Device identity on mobile (ADR-0032 / ADR-0038):
 * - device key DK: raw P-256 key (installed provider, noble on Hermes) kept in
 *   SecureKV without user authentication — needed for every E2E handshake;
 * - step-up key: a separate P-256 key in the biometric-gated store, registered
 *   with `devices.registerStepUp` as `orkestr-native-v1`; its signature
 *   counter lives in SecureKV and strictly increases.
 */
import {
  type DeviceKeyStore,
  type DeviceRecord,
  type DeviceSigner,
  type NativeStepUpRegistration,
  type RawKey,
  type StepUp,
  nativeStepUpAssertion,
  nativeStepUpRegistration,
  remoteCrypto,
} from "@orkestr/client";

import type { BiometricGate, SecureKV } from "./adapters";

/** SecureStore keys only allow `[A-Za-z0-9._-]`. */
export const kvKey = (...parts: string[]) => parts.map((p) => p.replace(/[^A-Za-z0-9._-]/g, "_")).join(".");

const INDEX = "devices.index";

/** `DeviceKeyStore` over SecureKV: one item per paired core + an index. */
export class SecureKVKeyStore implements DeviceKeyStore {
  constructor(private readonly kv: SecureKV) {}

  private async ids(): Promise<string[]> {
    const raw = await this.kv.get(INDEX);
    try {
      const v = raw ? (JSON.parse(raw) as unknown) : [];
      return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
    } catch {
      return [];
    }
  }

  async get(id: string): Promise<DeviceRecord | undefined> {
    const raw = await this.kv.get(kvKey("device", id));
    return raw ? (JSON.parse(raw) as DeviceRecord) : undefined;
  }

  async list(): Promise<DeviceRecord[]> {
    const out: DeviceRecord[] = [];
    for (const id of await this.ids()) {
      const r = await this.get(id);
      if (r) out.push(r);
    }
    return out;
  }

  async put(rec: DeviceRecord): Promise<void> {
    // Only the raw key is persistable here (CryptoKeyPair is not serialisable).
    const { keyPair: _kp, ...plain } = rec;
    await this.kv.set(kvKey("device", rec.id), JSON.stringify(plain));
    const ids = await this.ids();
    if (!ids.includes(rec.id)) await this.kv.set(INDEX, JSON.stringify([...ids, rec.id]));
  }

  async delete(id: string): Promise<void> {
    await this.kv.delete(kvKey("device", id));
    await this.kv.set(INDEX, JSON.stringify((await this.ids()).filter((x) => x !== id)));
  }
}

/** Fresh exportable device key from the installed provider (noble on device). */
export async function generateRawDeviceKey(): Promise<{ signer: DeviceSigner; rawKey: RawKey }> {
  const { signer, raw } = await remoteCrypto().generateSigner();
  if (!raw) throw new Error("the installed crypto provider cannot export device keys (install nobleCrypto())");
  return { signer, rawKey: raw };
}

interface StepUpMeta {
  credentialId: string;
  publicKey: string;
  counter: number;
}

export type KeyStorage = NativeStepUpRegistration["keyStorage"];

/** Biometric step-up credential for one paired core. */
export class StepUpKeys {
  constructor(
    private readonly kv: SecureKV,
    private readonly gate: BiometricGate,
    private readonly keyStorage: KeyStorage = "software",
  ) {}

  private metaKey = (coreId: string) => kvKey("stepup", coreId, "meta");
  private secretKey = (coreId: string) => kvKey("stepup", coreId, "key");

  async meta(coreId: string): Promise<StepUpMeta | null> {
    const raw = await this.kv.get(this.metaKey(coreId));
    return raw ? (JSON.parse(raw) as StepUpMeta) : null;
  }

  /** Creates the gated key and returns the `devices.registerStepUp` credential. */
  async create(coreId: string): Promise<NativeStepUpRegistration> {
    const { signer, raw } = await remoteCrypto().generateSigner();
    if (!raw) throw new Error("the installed crypto provider cannot export step-up keys");
    const reg = nativeStepUpRegistration(signer, this.keyStorage);
    await this.gate.setSecret(this.secretKey(coreId), JSON.stringify(raw));
    const meta: StepUpMeta = { credentialId: reg.credentialId, publicKey: reg.publicKey, counter: 0 };
    await this.kv.set(this.metaKey(coreId), JSON.stringify(meta));
    return reg;
  }

  /**
   * Signs a core challenge behind the biometric prompt. The counter is
   * bumped and persisted BEFORE signing so a crash never reuses a value.
   * Rejects with `BiometricDeniedError` when the user declines.
   */
  async assert(coreId: string, deviceId: string, challenge: string, prompt: string): Promise<StepUp> {
    const meta = await this.meta(coreId);
    if (!meta) throw new Error("no step-up key registered on this device");
    const raw = await this.gate.getSecret(this.secretKey(coreId), prompt);
    if (!raw) throw new Error("step-up key is missing from the secure store");
    const signer = remoteCrypto().signerFromRaw(JSON.parse(raw) as RawKey);
    const counter = meta.counter + 1;
    await this.kv.set(this.metaKey(coreId), JSON.stringify({ ...meta, counter }));
    return nativeStepUpAssertion({ challenge, deviceId, credentialId: meta.credentialId, counter, signer });
  }

  async remove(coreId: string): Promise<void> {
    await this.kv.delete(this.metaKey(coreId));
    await this.gate.deleteSecret(this.secretKey(coreId));
  }
}
