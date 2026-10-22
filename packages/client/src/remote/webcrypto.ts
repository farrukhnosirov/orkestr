/** WebCrypto `RemoteCrypto` provider (browsers, desktop webview, Node >= 20). */
import { b64uDecode, b64uEncode } from "./bytes";
import type { AeadKey, Bytes, DeviceSigner, EcdhKeyPair, RawKey, RemoteCrypto } from "./crypto";

const P256 = { name: "ECDSA", namedCurve: "P-256" } as const;
const ECDH = { name: "ECDH", namedCurve: "P-256" } as const;
const ECDSA = { name: "ECDSA", hash: "SHA-256" } as const;
const u8 = (b: ArrayBuffer): Bytes => new Uint8Array(b);
const subtle = () => globalThis.crypto.subtle;

function privateJwk(raw: RawKey): JsonWebKey {
  const pub = b64uDecode(raw.pub);
  return {
    kty: "EC",
    crv: "P-256",
    x: b64uEncode(pub.slice(1, 33)),
    y: b64uEncode(pub.slice(33, 65)),
    d: raw.d,
  };
}

async function ecdhPair(pair: CryptoKeyPair): Promise<EcdhKeyPair> {
  const publicKey = u8(await subtle().exportKey("raw", pair.publicKey));
  return {
    publicKey,
    async deriveBits(peer) {
      const pk = await subtle().importKey("raw", peer, ECDH, false, []);
      return u8(await subtle().deriveBits({ name: "ECDH", public: pk }, pair.privateKey, 256));
    },
  };
}

/** Wraps a WebCrypto ECDSA key pair (e.g. a non-extractable device key) as a `DeviceSigner`. */
export async function cryptoKeySigner(pair: CryptoKeyPair): Promise<DeviceSigner> {
  const publicKey = u8(await subtle().exportKey("raw", pair.publicKey));
  return { publicKey, sign: async (data) => u8(await subtle().sign(ECDSA, pair.privateKey, data)) };
}

export function webCrypto(): RemoteCrypto {
  return {
    name: "webcrypto",
    randomBytes: (n) => globalThis.crypto.getRandomValues(new Uint8Array(n)),
    sha256: async (d) => u8(await subtle().digest("SHA-256", d)),
    async hmacSha256(k, d) {
      const key = await subtle().importKey("raw", k, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
      return u8(await subtle().sign("HMAC", key, d));
    },
    async hkdfSha256(ikm, salt, info, len) {
      const key = await subtle().importKey("raw", ikm, "HKDF", false, ["deriveBits"]);
      return u8(await subtle().deriveBits({ name: "HKDF", hash: "SHA-256", salt, info }, key, len * 8));
    },
    async verifyP256(pub, sig, data) {
      const key = await subtle().importKey("raw", pub, P256, false, ["verify"]);
      return subtle().verify(ECDSA, key, sig, data);
    },
    async generateEcdh() {
      return ecdhPair((await subtle().generateKey(ECDH, false, ["deriveBits"])) as CryptoKeyPair);
    },
    ecdhFromPrivate() {
      // WebCrypto cannot derive the public point from a bare scalar; vector
      // tests inject a CryptoKeyPair (or use the noble provider).
      return Promise.reject(new Error("webcrypto: ecdhFromPrivate is not supported"));
    },
    async aesGcm(raw): Promise<AeadKey> {
      const key = await subtle().importKey("raw", raw, { name: "AES-GCM" }, false, ["encrypt", "decrypt"]);
      return {
        seal: async (iv, aad, pt) => u8(await subtle().encrypt({ name: "AES-GCM", iv, additionalData: aad }, key, pt)),
        open: async (iv, aad, ct) => u8(await subtle().decrypt({ name: "AES-GCM", iv, additionalData: aad }, key, ct)),
      };
    },
    async generateSigner() {
      const pair = (await subtle().generateKey(P256, false, ["sign", "verify"])) as CryptoKeyPair;
      return { signer: await cryptoKeySigner(pair) };
    },
    signerFromRaw(raw) {
      let key: Promise<CryptoKey> | null = null;
      return {
        publicKey: b64uDecode(raw.pub),
        async sign(data) {
          key ??= subtle().importKey("jwk", privateJwk(raw), P256, false, ["sign"]);
          return u8(await subtle().sign(ECDSA, await key, data));
        },
      };
    },
  };
}
