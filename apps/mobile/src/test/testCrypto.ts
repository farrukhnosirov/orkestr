/**
 * Test crypto provider: WebCrypto (Node) with exportable signer keys, standing
 * in for `nobleCrypto()` (WP-B) so the raw-key paths run under jest.
 */
import { type RemoteCrypto, b64uEncode, setRemoteCrypto, webCrypto } from "@orkestr/client";

const P256 = { name: "ECDSA", namedCurve: "P-256" } as const;

export function testCrypto(): RemoteCrypto {
  const base = webCrypto();
  return {
    ...base,
    async generateSigner() {
      const pair = (await crypto.subtle.generateKey(P256, true, ["sign", "verify"])) as CryptoKeyPair;
      const jwk = await crypto.subtle.exportKey("jwk", pair.privateKey);
      const pub = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
      const raw = { d: jwk.d as string, pub: b64uEncode(pub) };
      return { signer: base.signerFromRaw(raw), raw };
    },
  };
}

export function installTestCrypto(): void {
  setRemoteCrypto(testCrypto());
}
