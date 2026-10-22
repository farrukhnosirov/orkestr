/**
 * Crypto provider seam for the remote transport (ADR-0032).
 *
 * Browsers/desktop use WebCrypto (`webCrypto()`, default); React Native
 * (Hermes has no `crypto.subtle`) installs the pure-JS `nobleCrypto()` at
 * startup via `setRemoteCrypto`. All keys are raw SEC1 P-256, signatures
 * P1363 `r‖s` over SHA-256.
 */
import { webCrypto } from "./webcrypto";

export type Bytes = Uint8Array<ArrayBuffer>;

export interface EcdhKeyPair {
  readonly publicKey: Bytes;
  deriveBits(peer: Bytes): Promise<Bytes>;
}

/** Device signing key: P1363 signatures over SHA-256. */
export interface DeviceSigner {
  readonly publicKey: Bytes;
  sign(data: Bytes): Promise<Bytes>;
}

export interface AeadKey {
  seal(iv: Bytes, aad: Bytes, pt: Bytes): Promise<Bytes>;
  open(iv: Bytes, aad: Bytes, ct: Bytes): Promise<Bytes>;
}

/** Exportable P-256 key: base64url private scalar + raw SEC1 public key. */
export interface RawKey {
  d: string;
  pub: string;
}

export interface RemoteCrypto {
  readonly name: "webcrypto" | "noble";
  randomBytes(n: number): Bytes;
  sha256(d: Bytes): Promise<Bytes>;
  hmacSha256(k: Bytes, d: Bytes): Promise<Bytes>;
  hkdfSha256(ikm: Bytes, salt: Bytes, info: Bytes, len: number): Promise<Bytes>;
  verifyP256(pub: Bytes, sig: Bytes, data: Bytes): Promise<boolean>;
  generateEcdh(): Promise<EcdhKeyPair>;
  /** Deterministic ECDH key from a private scalar (test vectors). */
  ecdhFromPrivate(d: Bytes): Promise<EcdhKeyPair>;
  aesGcm(key: Bytes): Promise<AeadKey>;
  /** WebCrypto: non-extractable key, no `raw`; noble: `raw` for SecureStore. */
  generateSigner(): Promise<{ signer: DeviceSigner; raw?: RawKey }>;
  signerFromRaw(raw: RawKey): DeviceSigner;
}

let current: RemoteCrypto | null = null;

export function setRemoteCrypto(c: RemoteCrypto): void {
  current = c;
}

/** The installed provider (default: WebCrypto). */
export function remoteCrypto(): RemoteCrypto {
  current ??= webCrypto();
  return current;
}

export function isDeviceSigner(k: unknown): k is DeviceSigner {
  return (
    typeof k === "object" &&
    k !== null &&
    typeof (k as DeviceSigner).sign === "function" &&
    (k as DeviceSigner).publicKey instanceof Uint8Array
  );
}
