/**
 * Pure-JS `RemoteCrypto` provider for React Native/Hermes (ADR-0032):
 * @noble/curves P-256 (ECDH, ECDSA P1363 over SHA-256), @noble/hashes
 * (SHA-256, HMAC, HKDF) and @noble/ciphers AES-GCM. Never touches
 * `crypto.subtle`; randomness comes from `crypto.getRandomValues` (polyfilled
 * from expo-crypto on mobile).
 */
import { gcm } from "@noble/ciphers/aes.js";
import { p256 } from "@noble/curves/nist.js";
import { hkdf } from "@noble/hashes/hkdf.js";
import { hmac } from "@noble/hashes/hmac.js";
import { sha256 } from "@noble/hashes/sha2.js";

import { b64uDecode, b64uEncode } from "./bytes";
import type { Bytes, DeviceSigner, EcdhKeyPair, RemoteCrypto } from "./crypto";

const b = (u: Uint8Array): Bytes => u as Bytes;

function ecdhPair(secretKey: Uint8Array): EcdhKeyPair {
  return {
    publicKey: b(p256.getPublicKey(secretKey, false)),
    // Shared secret = affine x coordinate (same as WebCrypto deriveBits(256)).
    deriveBits: async (peer) => b(p256.getSharedSecret(secretKey, peer, true).slice(1)),
  };
}

function signer(secretKey: Uint8Array, publicKey: Uint8Array): DeviceSigner {
  return {
    publicKey: b(publicKey),
    sign: async (data) => b(p256.sign(data, secretKey, { prehash: true, format: "compact" })),
  };
}

export function nobleCrypto(): RemoteCrypto {
  return {
    name: "noble",
    randomBytes: (n) => globalThis.crypto.getRandomValues(new Uint8Array(n)),
    sha256: async (d) => b(sha256(d)),
    hmacSha256: async (k, d) => b(hmac(sha256, k, d)),
    hkdfSha256: async (ikm, salt, info, len) => b(hkdf(sha256, ikm, salt, info, len)),
    async verifyP256(pub, sig, data) {
      try {
        // lowS: false — WebCrypto and RustCrypto may emit high-S signatures.
        return p256.verify(sig, data, pub, { prehash: true, format: "compact", lowS: false });
      } catch {
        return false;
      }
    },
    generateEcdh: async () => ecdhPair(p256.utils.randomSecretKey()),
    ecdhFromPrivate: async (d) => ecdhPair(d),
    async aesGcm(key) {
      if (key.length !== 32 && key.length !== 16) throw new Error("aesGcm: bad key length");
      return {
        seal: async (iv, aad, pt) => b(gcm(key, iv, aad).encrypt(pt)),
        open: async (iv, aad, ct) => b(gcm(key, iv, aad).decrypt(ct)),
      };
    },
    async generateSigner() {
      const d = p256.utils.randomSecretKey();
      const pub = p256.getPublicKey(d, false);
      return { signer: signer(d, pub), raw: { d: b64uEncode(d), pub: b64uEncode(pub) } };
    },
    signerFromRaw(raw) {
      const d = b64uDecode(raw.d);
      const pub = p256.getPublicKey(d, false);
      if (b64uEncode(pub) !== raw.pub) throw new Error("signerFromRaw: public key does not match the private scalar");
      return signer(d, pub);
    },
  };
}
