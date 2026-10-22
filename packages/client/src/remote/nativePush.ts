/**
 * Native (FCM/APNs) push payload decryption (ADR-0037): AES-256-GCM with the
 * per-registration key, random 12-byte nonce `n`, AAD "orkestr-push-v1",
 * ciphertext `c` (both base64url). Step 0 skeleton: implemented in WP-B.
 */
import type { PushPayload } from "@orkestr/protocol";
import type { Bytes } from "./crypto";

export const NATIVE_PUSH_AAD = "orkestr-push-v1";

export function decryptNativePush(key: Bytes, data: { n: string; c: string }): Promise<PushPayload> {
  void key;
  void data;
  return Promise.reject(new Error("decryptNativePush: not implemented"));
}
