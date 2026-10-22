import { describe, expect, it } from "vitest";

import { isDeviceSigner, remoteCrypto, setRemoteCrypto } from "./crypto";
import { nobleCrypto } from "./noble";
import { webCrypto } from "./webcrypto";

describe("RemoteCrypto seam", () => {
  it("defaults to WebCrypto and can be swapped", () => {
    expect(remoteCrypto().name).toBe("webcrypto");
    setRemoteCrypto(nobleCrypto());
    expect(remoteCrypto().name).toBe("noble");
    setRemoteCrypto(webCrypto());
  });

  it("webcrypto signer signs P1363 signatures that verify", async () => {
    const c = webCrypto();
    const { signer, raw } = await c.generateSigner();
    expect(raw).toBeUndefined();
    expect(isDeviceSigner(signer)).toBe(true);
    const data = c.randomBytes(32);
    const sig = await signer.sign(data);
    expect(sig.length).toBe(64);
    expect(await c.verifyP256(signer.publicKey, sig, data)).toBe(true);
  });

  it("webcrypto ECDH, HKDF and AES-GCM round-trip", async () => {
    const c = webCrypto();
    const [a, b] = [await c.generateEcdh(), await c.generateEcdh()];
    const s1 = await a.deriveBits(b.publicKey);
    expect(s1).toEqual(await b.deriveBits(a.publicKey));
    const key = await c.aesGcm(await c.hkdfSha256(s1, c.randomBytes(16), c.randomBytes(4), 32));
    const iv = c.randomBytes(12);
    const aad = c.randomBytes(8);
    const pt = c.randomBytes(40);
    expect(await key.open(iv, aad, await key.seal(iv, aad, pt))).toEqual(pt);
  });
});
