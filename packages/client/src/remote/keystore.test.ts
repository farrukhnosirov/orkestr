import { p256 } from "@noble/curves/nist.js";
import { describe, expect, it } from "vitest";

import { b64uDecode, b64uEncode, utf8 } from "./bytes";
import { remoteCrypto } from "./crypto";
import { MemoryKeyStore, deviceSigner, generateDeviceKey } from "./keystore";
import { nobleCrypto } from "./noble";
import { PairingError, completionMessage, fingerprint, pairDevice, parsePairingQr, sas } from "./pairing";
import { useProvider } from "./providers.testutil";
import v from "./testdata/e2e-vector.json";

const ECDSA = { name: "ECDSA", hash: "SHA-256" } as const;

async function cik() {
  const p = b64uDecode(v.cikPublic);
  const jwk = { kty: "EC", crv: "P-256", d: v.cikPrivate, x: b64uEncode(p.slice(1, 33)), y: b64uEncode(p.slice(33)) };
  return crypto.subtle.importKey("jwk", jwk, { name: "ECDSA", namedCurve: "P-256" }, false, ["sign"]);
}

describe("device key", () => {
  it("is non-extractable and survives a structured-clone store round trip", async () => {
    const kp = await generateDeviceKey();
    expect(kp.privateKey.extractable).toBe(false);
    await expect(crypto.subtle.exportKey("pkcs8", kp.privateKey)).rejects.toThrow();
    await expect(crypto.subtle.exportKey("jwk", kp.privateKey)).rejects.toThrow();

    const store = new MemoryKeyStore();
    // IndexedDB persists CryptoKey objects by structured clone — emulate that.
    const cloned = structuredClone(kp);
    await store.put({
      id: "core-1", gatewayUrl: "http://gw", coreId: "core-1", deviceId: "d1", corePubKey: v.cikPublic,
      deviceName: "test", keyPair: cloned, pairedAt: 1,
    });
    const back = await store.get("core-1");
    expect(back?.keyPair?.privateKey.extractable).toBe(false);
    const msg = utf8("hello");
    const sig = await crypto.subtle.sign(ECDSA, back!.keyPair!.privateKey, msg);
    expect(await crypto.subtle.verify(ECDSA, kp.publicKey, sig, msg)).toBe(true);
    expect(await store.list()).toHaveLength(1);
    await store.delete("core-1");
    expect(await store.get("core-1")).toBeUndefined();
  });
});

describe("pairDevice", () => {
  it("claims with codeMac, shows SAS, verifies the CIK-signed approval and stores the record", async () => {
    const store = new MemoryKeyStore();
    const calls: { url: string; body?: Record<string, string> }[] = [];
    let devicePub = new Uint8Array();
    const cikKey = await cik();
    let polls = 0;
    const fakeFetch = (async (url: string, init?: RequestInit) => {
      const body = init?.body ? (JSON.parse(String(init.body)) as Record<string, string>) : undefined;
      calls.push({ url, ...(body ? { body } : {}) });
      if (url.endsWith("/claim")) {
        devicePub = b64uDecode(body!.devicePubKey!);
        expect(body!.coreId).toBe("core-1");
        return new Response(JSON.stringify({ status: "pending", deviceId: "dev-9", coreId: "core-1", corePublicKey: v.cikPublic }));
      }
      polls++;
      if (polls < 2) return new Response(JSON.stringify({ status: "pending" }));
      const sig = await crypto.subtle.sign(ECDSA, cikKey, await completionMessage("pair-1", "dev-9", devicePub, "operator"));
      return new Response(JSON.stringify({ status: "approved", coreId: "core-1", corePublicKey: v.cikPublic, result: { status: "approved", deviceId: "dev-9", role: "operator", sig: b64uEncode(sig) } }));
    }) as typeof fetch;

    const session = await pairDevice({
      gatewayUrl: "http://gw/", coreId: "core-1", pairingId: "pair-1", code: "abcd1234",
      coreKeyFingerprint: await fingerprint(b64uDecode(v.cikPublic)),
      deviceName: "Laptop", platform: "web", store, fetch: fakeFetch,
    });
    expect(calls[0]!.url).toBe("http://gw/v1/pairings/pair-1/claim");
    expect(calls[0]!.body!.codeMac).toHaveLength(43);
    expect(JSON.stringify(calls[0]!.body)).not.toContain("ABCD1234");
    expect(session.sas).toBe(await sas(b64uDecode(v.cikPublic), devicePub, "pair-1"));
    const rec = await session.waitForApproval({ intervalMs: 1 });
    expect(rec.deviceId).toBe("dev-9");
    expect(rec.keyPair?.privateKey.extractable).toBe(false);
    expect((await store.get("core-1"))?.role).toBe("operator");
  });

  it("refuses a core key that does not match the offer fingerprint", async () => {
    const fakeFetch = (async () => new Response(JSON.stringify({ coreId: "c", corePublicKey: v.devicePublic }))) as unknown as typeof fetch;
    await expect(
      pairDevice({
        gatewayUrl: "http://gw", coreId: "c", pairingId: "p", code: "X", coreKeyFingerprint: await fingerprint(b64uDecode(v.cikPublic)),
        deviceName: "d", platform: "web", store: new MemoryKeyStore(), fetch: fakeFetch,
      }),
    ).rejects.toBeInstanceOf(PairingError);
  });

  it("parses the QR payload", () => {
    expect(parsePairingQr('orkestr-pair:{"gatewayUrl":"https://g","coreId":"c","pairingId":"p","code":"C","coreKeyFingerprint":"f"}')).toEqual({
      gatewayUrl: "https://g", coreId: "c", pairingId: "p", code: "C", coreKeyFingerprint: "f",
    });
    expect(parsePairingQr("nope")).toBeNull();
  });
});

describe("pairDevice under the noble provider (mobile, crypto.subtle poisoned)", () => {
  useProvider(nobleCrypto);

  it("generates a raw provider key, pairs and stores it; deviceSigner(rec) signs with it", async () => {
    const store = new MemoryKeyStore();
    let devicePub = new Uint8Array();
    const fakeFetch = (async (url: string, init?: RequestInit) => {
      if (url.endsWith("/claim")) {
        devicePub = b64uDecode((JSON.parse(String(init!.body)) as { devicePubKey: string }).devicePubKey);
        return new Response(JSON.stringify({ coreId: "core-1", corePublicKey: v.cikPublic }));
      }
      const msg = await completionMessage("pair-1", "dev-9", devicePub, "admin");
      const sig = p256.sign(msg, b64uDecode(v.cikPrivate), { format: "compact" });
      return new Response(JSON.stringify({ status: "approved", result: { status: "approved", deviceId: "dev-9", role: "admin", sig: b64uEncode(sig) } }));
    }) as typeof fetch;
    const session = await pairDevice({
      gatewayUrl: "http://gw", coreId: "core-1", pairingId: "pair-1", code: "abcd1234",
      coreKeyFingerprint: await fingerprint(b64uDecode(v.cikPublic)),
      deviceName: "Phone", platform: "android", store, fetch: fakeFetch,
    });
    expect(session.sas).toMatch(/^\d{6}$/);
    const rec = await session.waitForApproval({ intervalMs: 1 });
    expect(rec.keyPair).toBeUndefined();
    expect(rec.rawKey?.pub).toBe(b64uEncode(devicePub));
    const signer = await deviceSigner(rec);
    const msg = utf8("hello");
    expect(await remoteCrypto().verifyP256(devicePub, await signer.sign(msg), msg)).toBe(true);
  });
});
