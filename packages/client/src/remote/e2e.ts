/**
 * `e2e-v1` device side (remote-access.md §6, frozen wire spec in the Phase 4
 * plan; reference vector `testdata/e2e-vector.json` from the core).
 *
 * - keys: raw SEC1 uncompressed P-256, base64url; signatures P1363 r‖s;
 * - transcript = SHA-256("orkestr-e2e-v1" ‖ coreId ‖ deviceId ‖ ephD ‖ nonceD ‖ ts_be64 ‖ ephC ‖ nonceC);
 *   both sides sign it with ECDSA/SHA-256;
 * - HKDF-SHA256(ikm = ECDH x, salt = nonceD ‖ nonceC, info = "orkestr-e2e-v1" ‖ coreId ‖ deviceId) → kD2C ‖ kC2D;
 * - record = ctr_be64 ‖ AES-256-GCM(k, iv = dir_be32 ‖ ctr_be64, aad = ctr_be64).
 */
import { b64uDecode, b64uEncode, be32, be64, concat, readBe64, utf8 } from "./bytes";
import { type AeadKey, type Bytes, type DeviceSigner, type EcdhKeyPair, isDeviceSigner, remoteCrypto } from "./crypto";
import { signWithCryptoKey } from "./webcrypto";

export const E2E_PROTOCOL = "orkestr-e2e-v1";
export const DIR_D2C = 1;
export const DIR_C2D = 2;

export interface ClientHello {
  t: "clientHello";
  v: "e2e-v1";
  coreId: string;
  deviceId: string;
  ephPub: string;
  nonce: string;
  ts: number;
}
export interface ServerHello {
  t: "serverHello";
  ephPub: string;
  nonce: string;
  sig: string;
}
export interface ClientFinish {
  t: "clientFinish";
  sig: string;
}
export interface HandshakeError {
  t: "error";
  code: string;
}

export function transcriptHash(
  coreId: string,
  deviceId: string,
  ephD: Uint8Array,
  nonceD: Uint8Array,
  ts: number,
  ephC: Uint8Array,
  nonceC: Uint8Array,
): Promise<Bytes> {
  return remoteCrypto().sha256(
    concat(utf8(E2E_PROTOCOL), utf8(coreId), utf8(deviceId), ephD, nonceD, be64(ts), ephC, nonceC),
  );
}

/** Verifies a P1363 ECDSA/SHA-256 signature with a raw SEC1 P-256 public key. */
export async function verifySig(pub: Uint8Array, sig: Uint8Array, data: Uint8Array): Promise<boolean> {
  try {
    return await remoteCrypto().verifyP256(new Uint8Array(pub), new Uint8Array(sig), new Uint8Array(data));
  } catch {
    return false;
  }
}

/** Session keys derived from the handshake. */
export interface SessionKeys {
  d2c: AeadKey;
  c2d: AeadKey;
}

export async function deriveSessionKeys(
  ephD: EcdhKeyPair,
  ephCRaw: Bytes,
  nonceD: Uint8Array,
  nonceC: Uint8Array,
  coreId: string,
  deviceId: string,
): Promise<{ keys: SessionKeys; raw: Bytes }> {
  const c = remoteCrypto();
  const shared = await ephD.deriveBits(ephCRaw);
  const okm = await c.hkdfSha256(
    shared,
    concat(nonceD, nonceC),
    concat(utf8(E2E_PROTOCOL), utf8(coreId), utf8(deviceId)),
    64,
  );
  return { keys: { d2c: await c.aesGcm(okm.slice(0, 32)), c2d: await c.aesGcm(okm.slice(32, 64)) }, raw: okm };
}

const iv = (dir: number, ctr: number) => concat(be32(dir), be64(ctr));

export async function sealRecord(key: AeadKey, dir: number, ctr: number, plaintext: Bytes): Promise<Bytes> {
  const aad = be64(ctr);
  return concat(aad, await key.seal(iv(dir, ctr), aad, plaintext));
}

/** Opens a record; throws on tamper or when the counter is not `expected`. */
export async function openRecord(key: AeadKey, dir: number, expected: number, record: Uint8Array): Promise<Bytes> {
  if (record.length < 8 + 16) throw new Error("record too short");
  const ctr = readBe64(record.subarray(0, 8));
  if (ctr !== expected) throw new Error(`unexpected record counter ${ctr} (want ${expected})`);
  return key.open(iv(dir, ctr), record.slice(0, 8), record.slice(8));
}

/** Device-side handshake inputs (ephemeral key / nonce / clock injectable for tests). */
export interface HandshakeInit {
  coreId: string;
  deviceId: string;
  /** Device key DK (private part signs `clientFinish`): WebCrypto key or provider signer. */
  deviceKey: CryptoKey | DeviceSigner;
  /** Core identity key (CIK), raw SEC1 base64url. */
  corePubKey: string;
  /** Fixed ephemeral key (tests; see `RemoteCrypto.ecdhFromPrivate`). */
  ephemeral?: EcdhKeyPair;
  nonce?: Uint8Array<ArrayBuffer>;
  now?: () => number;
}

/** Device handshake state machine: `hello()` → `finish(serverHello)`. */
export class DeviceHandshake {
  private eph: EcdhKeyPair | null = null;
  private hello_: ClientHello | null = null;
  private readonly nonce: Uint8Array<ArrayBuffer>;

  constructor(private readonly init: HandshakeInit) {
    this.nonce = init.nonce ?? remoteCrypto().randomBytes(32);
  }

  async hello(): Promise<ClientHello> {
    this.eph = this.init.ephemeral ?? (await remoteCrypto().generateEcdh());
    this.hello_ = {
      t: "clientHello",
      v: "e2e-v1",
      coreId: this.init.coreId,
      deviceId: this.init.deviceId,
      ephPub: b64uEncode(this.eph.publicKey),
      nonce: b64uEncode(this.nonce),
      ts: (this.init.now ?? Date.now)(),
    };
    return this.hello_;
  }

  /** Verifies `serverHello`, derives keys and builds `clientFinish`. Throws when the CIK signature is bad. */
  async finish(sh: ServerHello): Promise<{ keys: SessionKeys; finish: ClientFinish }> {
    const h = this.hello_;
    if (!h || !this.eph) throw new Error("hello() not called");
    const ephC = b64uDecode(sh.ephPub);
    const nonceC = b64uDecode(sh.nonce);
    if (ephC.length !== 65 || nonceC.length !== 32) throw new Error("malformed serverHello");
    const th = await transcriptHash(h.coreId, h.deviceId, this.eph.publicKey, this.nonce, h.ts, ephC, nonceC);
    if (!(await verifySig(b64uDecode(this.init.corePubKey), b64uDecode(sh.sig), th))) {
      throw new Error("bad core signature");
    }
    const { keys } = await deriveSessionKeys(this.eph, ephC, this.nonce, nonceC, h.coreId, h.deviceId);
    const dk = this.init.deviceKey;
    const sig = isDeviceSigner(dk) ? await dk.sign(th) : await signWithCryptoKey(dk, th);
    return { keys, finish: { t: "clientFinish", sig: b64uEncode(sig) } };
  }
}
