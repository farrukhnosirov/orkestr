/**
 * Pure-JS `RemoteCrypto` provider for React Native/Hermes (@noble/curves,
 * @noble/hashes, @noble/ciphers). Step 0 skeleton: every method throws
 * "not implemented" until WP-B lands.
 */
import type { RemoteCrypto } from "./crypto";

const notImplemented =
  (what: string) =>
  (): never => {
    throw new Error(`nobleCrypto.${what}: not implemented`);
  };

export function nobleCrypto(): RemoteCrypto {
  return {
    name: "noble",
    randomBytes: notImplemented("randomBytes"),
    sha256: notImplemented("sha256"),
    hmacSha256: notImplemented("hmacSha256"),
    hkdfSha256: notImplemented("hkdfSha256"),
    verifyP256: notImplemented("verifyP256"),
    generateEcdh: notImplemented("generateEcdh"),
    ecdhFromPrivate: notImplemented("ecdhFromPrivate"),
    aesGcm: notImplemented("aesGcm"),
    generateSigner: notImplemented("generateSigner"),
    signerFromRaw: notImplemented("signerFromRaw"),
  };
}
