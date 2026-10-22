/// <reference types="vite/client" />
// Native step-up assertion + native push decryption (ADR-0037/0038), under
// both providers. TODO(integration): add checks against the Rust vectors
// crates/core/src/remote/testdata/native-{push,stepup}-vector.json (WP-A).
import { p256 } from "@noble/curves/nist.js";
import type { PushPayload } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { b64uDecode, b64uEncode, concat, fromUtf8, utf8 } from "./bytes";
import { remoteCrypto } from "./crypto";
import { NATIVE_PUSH_AAD, decryptNativePush, encryptNativePush } from "./nativePush";
import {
  NATIVE_STEPUP_RP_ID,
  nativeStepUpAssertion,
  nativeStepUpOrigin,
  nativeStepUpRegistration,
  p1363ToDer,
} from "./nativeStepUp";
import { PROVIDERS, useProvider } from "./providers.testutil";

const B = b64uDecode;

describe("p1363ToDer", () => {
  it("matches noble's DER encoding, including leading-zero and high-bit cases", () => {
    const cases = [
      new Uint8Array(64).fill(0x11),
      concat(new Uint8Array(32).fill(0x80), new Uint8Array(32).fill(0x7f)),
      concat(new Uint8Array([0, 0, 5, ...new Uint8Array(29).fill(9)]), new Uint8Array([0x81, ...new Uint8Array(31).fill(1)])),
    ];
    for (const sig of cases) {
      expect(b64uEncode(p1363ToDer(sig))).toBe(b64uEncode(p256.Signature.fromBytes(sig, "compact").toBytes("der")));
    }
  });
});

describe.each(PROVIDERS)("native step-up [%s]", (_name, provider) => {
  useProvider(provider);

  /** Core-side check (mirrors webauthn::verify_assertion for the native RP). */
  async function verifyLikeCore(a: Awaited<ReturnType<typeof nativeStepUpAssertion>>, pub: Uint8Array, deviceId: string, challenge: string) {
    const c = remoteCrypto();
    const auth = B(a.authenticatorData);
    const cd = JSON.parse(fromUtf8(B(a.clientDataJSON))) as Record<string, unknown>;
    const rpIdHashOk = b64uEncode(auth.slice(0, 32)) === b64uEncode(await c.sha256(utf8(NATIVE_STEPUP_RP_ID)));
    const flags = auth[32] as number;
    const counter = new DataView(auth.buffer, 33, 4).getUint32(0);
    const msg = concat(auth, await c.sha256(B(a.clientDataJSON)));
    const sigOk = p256.verify(B(a.signature), msg, pub, { format: "der", lowS: false });
    return {
      ok: rpIdHashOk && (flags & 0x05) === 0x05 && cd.type === "webauthn.get" && cd.challenge === challenge &&
        cd.origin === nativeStepUpOrigin(deviceId) && cd.crossOrigin === false && sigOk,
      counter,
      authLen: auth.length,
    };
  }

  it("builds a WebAuthn-shaped assertion the core can verify", async () => {
    const { signer } = await remoteCrypto().generateSigner();
    const reg = nativeStepUpRegistration(signer, "software");
    expect(reg.type).toBe("orkestr-native-v1");
    expect(B(reg.publicKey)).toEqual(signer.publicKey);
    expect(B(reg.credentialId)).toHaveLength(16);

    const challenge = b64uEncode(remoteCrypto().randomBytes(32));
    const a = await nativeStepUpAssertion({ challenge, deviceId: "dev-1", credentialId: reg.credentialId, counter: 7, signer });
    expect(a.credentialId).toBe(reg.credentialId);
    expect(fromUtf8(B(a.clientDataJSON))).toBe(
      `{"type":"webauthn.get","challenge":"${challenge}","origin":"orkestr-native:dev-1","crossOrigin":false}`,
    );
    const r = await verifyLikeCore(a, signer.publicKey, "dev-1", challenge);
    expect(r).toEqual({ ok: true, counter: 7, authLen: 37 });
    expect(B(a.authenticatorData)[32]).toBe(0x05);
    // Another device id / challenge does not verify.
    expect((await verifyLikeCore(a, signer.publicKey, "dev-2", challenge)).ok).toBe(false);
    expect((await verifyLikeCore(a, signer.publicKey, "dev-1", "other")).ok).toBe(false);
  });

  it("rejects counters outside u32", async () => {
    const { signer } = await remoteCrypto().generateSigner();
    for (const counter of [-1, 1.5, 2 ** 32]) {
      await expect(nativeStepUpAssertion({ challenge: "c", deviceId: "d", credentialId: "x", counter, signer })).rejects.toThrow(/u32/);
    }
  });
});

describe.each(PROVIDERS)("native push [%s]", (_name, provider) => {
  useProvider(provider);
  const payload: PushPayload = {
    v: 1,
    eventId: "evt-1",
    category: "approval",
    projectId: "p1",
    ts: "2026-10-22T10:00:00Z",
    title: "Approval needed",
    body: "Действие требует подтверждения",
  };

  it("round-trips and binds the AAD", async () => {
    const key = remoteCrypto().randomBytes(32);
    const body = await encryptNativePush(key, payload);
    expect(B(body.n)).toHaveLength(12);
    expect(await decryptNativePush(key, body)).toEqual(payload);

    // Same plaintext sealed with a different AAD must not open.
    const aead = await remoteCrypto().aesGcm(key);
    const n = remoteCrypto().randomBytes(12);
    const wrongAad = await aead.seal(n, utf8(`${NATIVE_PUSH_AAD}x`), utf8(JSON.stringify(payload)));
    await expect(decryptNativePush(key, { n: b64uEncode(n), c: b64uEncode(wrongAad) })).rejects.toThrow();
  });

  it("rejects a wrong key, tampering and malformed input", async () => {
    const key = remoteCrypto().randomBytes(32);
    const body = await encryptNativePush(key, payload);
    await expect(decryptNativePush(remoteCrypto().randomBytes(32), body)).rejects.toThrow();
    const c = B(body.c);
    c[0]! ^= 1;
    await expect(decryptNativePush(key, { n: body.n, c: b64uEncode(c) })).rejects.toThrow();
    await expect(decryptNativePush(key.slice(0, 16), body)).rejects.toThrow(/32 bytes/);
    await expect(decryptNativePush(key, { n: "AAAA", c: body.c })).rejects.toThrow(/12 bytes/);
  });
});

describe("providers interoperate", () => {
  it("noble and webcrypto agree on signatures, ECDH and AES-GCM", async () => {
    const { nobleCrypto } = await import("./noble");
    const { webCrypto } = await import("./webcrypto");
    const [n, w] = [nobleCrypto(), webCrypto()];
    const data = n.randomBytes(48);

    const ns = await n.generateSigner();
    expect(await w.verifyP256(ns.signer.publicKey, await ns.signer.sign(data), data)).toBe(true);
    expect(w.signerFromRaw(ns.raw!).publicKey).toEqual(ns.signer.publicKey);
    expect(await n.verifyP256(ns.signer.publicKey, await w.signerFromRaw(ns.raw!).sign(data), data)).toBe(true);
    const ws = await w.generateSigner();
    for (let i = 0; i < 8; i++) {
      // WebCrypto may emit high-S signatures; noble must accept them.
      expect(await n.verifyP256(ws.signer.publicKey, await ws.signer.sign(data), data)).toBe(true);
    }
    expect(await n.verifyP256(ws.signer.publicKey, new Uint8Array(64), data)).toBe(false);

    const [a, b] = [await n.generateEcdh(), await w.generateEcdh()];
    expect(await a.deriveBits(b.publicKey)).toEqual(await b.deriveBits(a.publicKey));
    expect(await n.hkdfSha256(data, data.slice(0, 8), data.slice(8, 12), 64)).toEqual(
      await w.hkdfSha256(data, data.slice(0, 8), data.slice(8, 12), 64),
    );
    expect(await n.hmacSha256(data.slice(0, 5), data)).toEqual(await w.hmacSha256(data.slice(0, 5), data));

    const key = n.randomBytes(32);
    const iv = n.randomBytes(12);
    const sealed = await (await n.aesGcm(key)).seal(iv, data.slice(0, 3), data);
    expect(sealed).toEqual(await (await w.aesGcm(key)).seal(iv, data.slice(0, 3), data));
    expect(await (await w.aesGcm(key)).open(iv, data.slice(0, 3), sealed)).toEqual(data);
  });

  it("noble signerFromRaw rejects a mismatched public key", async () => {
    const { nobleCrypto } = await import("./noble");
    const n = nobleCrypto();
    const a = await n.generateSigner();
    const b = await n.generateSigner();
    expect(() => n.signerFromRaw({ d: a.raw!.d, pub: b.raw!.pub })).toThrow(/does not match/);
  });
});

// Rust vectors (WP-A): copy crates/core/src/remote/testdata/native-{push,stepup}-vector.json
// verbatim into ./testdata; these suites activate automatically once the files exist
// (and are non-empty).
const rustVectors = import.meta.glob<Record<string, unknown>>("./testdata/native-*-vector.json", {
  eager: true,
  import: "default",
});
const pushVector = rustVectors["./testdata/native-push-vector.json"] as
  | { key: string; nonce: string; aad: string; plaintext: string; payload: PushPayload; body: { n: string; c: string } }
  | undefined;
const stepUpVector = rustVectors["./testdata/native-stepup-vector.json"] as
  | {
      privateKey: string; publicKey: string; deviceId: string; credentialId: string; rpId: string; origin: string;
      challenge: string; counter: number; registration: Record<string, string>; clientDataJSONText: string;
      authenticatorData: string; clientDataJSON: string; signature: string; signatureRaw: string;
    }
  | undefined;

describe.each(PROVIDERS)("Rust native vectors [%s]", (name, provider) => {
  useProvider(provider);

  it.skipIf(!pushVector?.key)("decrypts native-push-vector.json and re-encrypts it byte-exactly", async () => {
    const v = pushVector!;
    expect(v.aad).toBe(NATIVE_PUSH_AAD);
    expect(await decryptNativePush(B(v.key), v.body)).toEqual(v.payload);
    expect(JSON.stringify(v.payload)).toBe(v.plaintext);
    expect(await encryptNativePush(B(v.key), v.payload, B(v.nonce))).toEqual(v.body);
  });

  it.skipIf(!stepUpVector?.privateKey)("reproduces native-stepup-vector.json", async () => {
    const v = stepUpVector!;
    expect(NATIVE_STEPUP_RP_ID).toBe(v.rpId);
    expect(nativeStepUpOrigin(v.deviceId)).toBe(v.origin);
    const signer = remoteCrypto().signerFromRaw({ d: v.privateKey, pub: v.publicKey });
    expect(nativeStepUpRegistration(signer, "software", v.credentialId)).toEqual(v.registration);
    const a = await nativeStepUpAssertion({
      challenge: v.challenge, deviceId: v.deviceId, credentialId: v.credentialId, counter: v.counter, signer,
    });
    expect(a.authenticatorData).toBe(v.authenticatorData);
    expect(a.clientDataJSON).toBe(v.clientDataJSON);
    expect(fromUtf8(B(a.clientDataJSON))).toBe(v.clientDataJSONText);
    expect(b64uEncode(p1363ToDer(B(v.signatureRaw)))).toBe(v.signature);
    // RFC 6979 + low-S: noble reproduces the Rust signature; WebCrypto signs randomly.
    if (name === "noble") expect(a.signature).toBe(v.signature);
    const msg = concat(B(a.authenticatorData), await remoteCrypto().sha256(B(a.clientDataJSON)));
    expect(p256.verify(B(a.signature), msg, B(v.publicKey), { format: "der", lowS: false })).toBe(true);
  });
});
