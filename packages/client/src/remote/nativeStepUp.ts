/**
 * Native biometric step-up credential (ADR-0038): a WebAuthn-shaped assertion
 * made with a biometric-gated device key and verified by the core with
 * `rpId = NATIVE_STEPUP_RP_ID` and origin `nativeStepUpOrigin(deviceId)`.
 */
import { b64uEncode, be32, concat, utf8 } from "./bytes";
import { type Bytes, type DeviceSigner, remoteCrypto } from "./crypto";
import type { StepUp } from "./webauthn";

export const NATIVE_STEPUP_RP_ID = "orkestr-native-stepup";
export const nativeStepUpOrigin = (deviceId: string) => `orkestr-native:${deviceId}`;

/** `credential` sent to `devices.registerStepUp` for a native key. */
export interface NativeStepUpRegistration {
  type: "orkestr-native-v1";
  credentialId: string;
  /** Raw SEC1 P-256 public key, base64url. */
  publicKey: string;
  keyStorage: "android-keystore-wrapped" | "ios-keychain-wrapped" | "software";
}

/** `credential` for `devices.registerStepUp` from a (biometric-gated) step-up key. */
export function nativeStepUpRegistration(
  signer: DeviceSigner,
  keyStorage: NativeStepUpRegistration["keyStorage"],
  credentialId: string = b64uEncode(remoteCrypto().randomBytes(16)),
): NativeStepUpRegistration {
  return { type: "orkestr-native-v1", credentialId, publicKey: b64uEncode(signer.publicKey), keyStorage };
}

/** DER length prefix (short or one/two-byte long form). */
function derLen(n: number): number[] {
  return n < 0x80 ? [n] : n < 0x100 ? [0x81, n] : [0x82, n >> 8, n & 0xff];
}

/** Minimal DER INTEGER for an unsigned big-endian value. */
function derInt(v: Uint8Array): number[] {
  let i = 0;
  while (i < v.length - 1 && v[i] === 0) i++;
  const body = [...v.subarray(i)];
  if ((body[0] as number) & 0x80) body.unshift(0);
  return [0x02, ...derLen(body.length), ...body];
}

/** P1363 `r‖s` → DER `SEQUENCE { INTEGER r, INTEGER s }` (WebAuthn signature encoding). */
export function p1363ToDer(sig: Uint8Array): Bytes {
  if (sig.length === 0 || sig.length % 2 !== 0) throw new Error("p1363ToDer: bad signature length");
  const h = sig.length / 2;
  const body = [...derInt(sig.subarray(0, h)), ...derInt(sig.subarray(h))];
  return new Uint8Array([0x30, ...derLen(body.length), ...body]);
}

/** WebAuthn flags: UP (0x01) | UV (0x04). */
const FLAGS_UP_UV = 0x05;

/**
 * Builds a WebAuthn-shaped assertion with the native step-up key:
 * `authenticatorData = SHA-256(rpId) ‖ flags(UP|UV) ‖ counter_be32`,
 * `clientDataJSON = {"type":"webauthn.get","challenge","origin":"orkestr-native:<deviceId>","crossOrigin":false}`,
 * `signature = DER(ECDSA-SHA256(authenticatorData ‖ SHA-256(clientDataJSON)))`.
 * The caller gates `signer` behind biometrics and persists an increasing `counter`.
 */
export async function nativeStepUpAssertion(o: {
  challenge: string;
  deviceId: string;
  credentialId: string;
  counter: number;
  signer: DeviceSigner;
}): Promise<StepUp> {
  if (!Number.isInteger(o.counter) || o.counter < 0 || o.counter > 0xffffffff) {
    throw new Error("nativeStepUpAssertion: counter must be a u32");
  }
  const c = remoteCrypto();
  const authData = concat(await c.sha256(utf8(NATIVE_STEPUP_RP_ID)), new Uint8Array([FLAGS_UP_UV]), be32(o.counter));
  const clientData = utf8(
    JSON.stringify({
      type: "webauthn.get",
      challenge: o.challenge,
      origin: nativeStepUpOrigin(o.deviceId),
      crossOrigin: false,
    }),
  );
  const sig = await o.signer.sign(concat(authData, await c.sha256(clientData)));
  return {
    credentialId: o.credentialId,
    authenticatorData: b64uEncode(authData),
    clientDataJSON: b64uEncode(clientData),
    signature: b64uEncode(p1363ToDer(sig)),
  };
}
