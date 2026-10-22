/**
 * Persistence of the paired device identity. The device key DK is a
 * non-extractable WebCrypto key: IndexedDB stores the `CryptoKey` object via
 * structured clone, the private scalar never leaves the browser's key store.
 */

/** A paired core as seen by this device. */
import { type DeviceSigner, type RawKey, remoteCrypto } from "./crypto";
import { cryptoKeySigner } from "./webcrypto";

export interface DeviceRecord {
  /** Record id (`coreId`). */
  id: string;
  gatewayUrl: string;
  coreId: string;
  deviceId: string;
  /** CIK, raw SEC1 base64url (pinned at pairing; fingerprint checked). */
  corePubKey: string;
  role?: string;
  deviceName: string;
  /** Device key DK (non-extractable WebCrypto pair; browsers/desktop). */
  keyPair?: CryptoKeyPair;
  /** Device key DK as an exportable raw key (mobile, kept in SecureStore). */
  rawKey?: RawKey;
  pairedAt: number;
}

export interface DeviceKeyStore {
  get(id: string): Promise<DeviceRecord | undefined>;
  list(): Promise<DeviceRecord[]>;
  put(rec: DeviceRecord): Promise<void>;
  delete(id: string): Promise<void>;
}

/** In-memory store (tests, or browsers without IndexedDB). */
export class MemoryKeyStore implements DeviceKeyStore {
  private readonly m = new Map<string, DeviceRecord>();
  async get(id: string) {
    return this.m.get(id);
  }
  async list() {
    return [...this.m.values()];
  }
  async put(rec: DeviceRecord) {
    this.m.set(rec.id, rec);
  }
  async delete(id: string) {
    this.m.delete(id);
  }
}

const DB = "orkestr-remote";
const STORE = "devices";

function req<T>(r: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error ?? new Error("IndexedDB request failed"));
  });
}

/** IndexedDB-backed store (`orkestr-remote/devices`). */
export class IndexedDbKeyStore implements DeviceKeyStore {
  private dbp: Promise<IDBDatabase> | null = null;

  constructor(private readonly idb: IDBFactory = globalThis.indexedDB) {}

  private db(): Promise<IDBDatabase> {
    this.dbp ??= new Promise((resolve, reject) => {
      const open = this.idb.open(DB, 1);
      open.onupgradeneeded = () => {
        if (!open.result.objectStoreNames.contains(STORE)) open.result.createObjectStore(STORE, { keyPath: "id" });
      };
      open.onsuccess = () => resolve(open.result);
      open.onerror = () => reject(open.error ?? new Error("IndexedDB open failed"));
    });
    return this.dbp;
  }

  private async tx(mode: IDBTransactionMode): Promise<IDBObjectStore> {
    return (await this.db()).transaction(STORE, mode).objectStore(STORE);
  }

  async get(id: string) {
    return req((await this.tx("readonly")).get(id) as IDBRequest<DeviceRecord | undefined>);
  }
  async list() {
    return req((await this.tx("readonly")).getAll() as IDBRequest<DeviceRecord[]>);
  }
  async put(rec: DeviceRecord) {
    await req((await this.tx("readwrite")).put(rec));
  }
  async delete(id: string) {
    await req((await this.tx("readwrite")).delete(id));
  }
}

/** Default store for the current environment. */
export function defaultKeyStore(): DeviceKeyStore {
  return typeof indexedDB === "undefined" ? new MemoryKeyStore() : new IndexedDbKeyStore();
}

/** Generates the device key DK: P-256 ECDSA, private part NOT extractable. */
export async function generateDeviceKey(): Promise<CryptoKeyPair> {
  return (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, [
    "sign",
    "verify",
  ])) as CryptoKeyPair;
}

/** The record's device key as a `DeviceSigner` (raw key via the installed provider, else the WebCrypto pair). */
export async function deviceSigner(rec: DeviceRecord): Promise<DeviceSigner> {
  if (rec.rawKey) return remoteCrypto().signerFromRaw(rec.rawKey);
  if (rec.keyPair) return cryptoKeySigner(rec.keyPair);
  throw new Error(`device record ${rec.id} has no device key`);
}
