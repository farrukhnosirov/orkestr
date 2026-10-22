/** WebCrypto `RemoteCrypto` provider (browsers, desktop webview, Node >= 20). */
import { b64uDecode, b64uEncode, concat } from "./bytes";
import type { AeadKey, Bytes, DeviceSigner, EcdhKeyPair, RawKey, RemoteCrypto } from "./crypto";

const P256 = { name: "ECDSA", namedCurve: "P-256" } as const;
const ECDH = { name: "ECDH", namedCurve: "P-256" } as const;
const ECDSA = { name: "ECDSA", hash: "SHA-256" } as const;
const u8 = (b: ArrayBuffer): Bytes => new Uint8Array(b);
const subtle = () => globalThis.crypto.subtle;

/** DER PrivateKeyInfo{ecPublicKey, prime256v1, ECPrivateKey{v1, d}} header; append the 32-byte scalar. */
const PKCS8_P256_PREFIX = new Uint8Array([
  0x30, 0x41, 0x02, 0x01, 0x00, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
  0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x04, 0x27, 0x30, 0x25, 0x02, 0x01, 0x01, 0x04, 0x20,
]);

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

/** P1363 ECDSA/SHA-256 signature with a WebCrypto private key. */
export async function signWithCryptoKey(key: CryptoKey, data: Bytes): Promise<Bytes> {
  return u8(await subtle().sign(ECDSA, key, data));
}

/** Wraps a WebCrypto ECDSA key pair (e.g. a non-extractable device key) as a `DeviceSigner`. */
export async function cryptoKeySigner(pair: CryptoKeyPair): Promise<DeviceSigner> {
  const publicKey = u8(await subtle().exportKey("raw", pair.publicKey));
  return { publicKey, sign: (data) => signWithCryptoKey(pair.privateKey, data) };
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
    async ecdhFromPrivate(d) {
      // PKCS#8 without the optional public key: the implementation derives it.
      if (d.length !== 32) throw new Error("ecdhFromPrivate: scalar must be 32 bytes");
      const privateKey = await subtle().importKey("pkcs8", concat(PKCS8_P256_PREFIX, d), ECDH, true, ["deriveBits"]);
      const jwk = await subtle().exportKey("jwk", privateKey);
      const pub = concat(new Uint8Array([4]), b64uDecode(jwk.x ?? ""), b64uDecode(jwk.y ?? ""));
      const publicKey = await subtle().importKey("raw", pub, ECDH, true, []);
      return ecdhPair({ privateKey, publicKey });
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
