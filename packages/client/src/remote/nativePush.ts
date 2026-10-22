/**
 * Native (FCM/APNs) push payload decryption (ADR-0037): AES-256-GCM with the
 * per-registration key, random 12-byte nonce `n`, AAD "orkestr-push-v1",
 * ciphertext `c` (both base64url).
 */
import type { PushPayload } from "@orkestr/protocol";
import { b64uDecode, b64uEncode, fromUtf8, utf8 } from "./bytes";
import { type Bytes, remoteCrypto } from "./crypto";

export const NATIVE_PUSH_AAD = "orkestr-push-v1";

/** Decrypts a native push body `{n, c}` with the registration key; throws on tamper or a wrong key. */
export async function decryptNativePush(key: Bytes, data: { n: string; c: string }): Promise<PushPayload> {
  if (key.length !== 32) throw new Error("decryptNativePush: key must be 32 bytes (AES-256)");
  const nonce = b64uDecode(data.n);
  if (nonce.length !== 12) throw new Error("decryptNativePush: nonce must be 12 bytes");
  const aead = await remoteCrypto().aesGcm(key);
  const pt = await aead.open(nonce, utf8(NATIVE_PUSH_AAD), b64uDecode(data.c));
  return JSON.parse(fromUtf8(pt)) as PushPayload;
}

/** Encrypts a push payload like the core does (tests / tooling). */
export async function encryptNativePush(
  key: Bytes,
  payload: PushPayload,
  nonce: Bytes = remoteCrypto().randomBytes(12),
): Promise<{ n: string; c: string }> {
  const aead = await remoteCrypto().aesGcm(key);
  const c = await aead.seal(nonce, utf8(NATIVE_PUSH_AAD), utf8(JSON.stringify(payload)));
  return { n: b64uEncode(nonce), c: b64uEncode(c) };
}
