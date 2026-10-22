/**
 * Native biometric step-up credential (ADR-0038): a WebAuthn-shaped assertion
 * made with a biometric-gated device key and verified by the core with
 * `rpId = NATIVE_STEPUP_RP_ID` and origin `nativeStepUpOrigin(deviceId)`.
 * Step 0 skeleton: implemented in WP-B.
 */
import type { DeviceSigner } from "./crypto";
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

export function nativeStepUpAssertion(o: {
  challenge: string;
  deviceId: string;
  credentialId: string;
  counter: number;
  signer: DeviceSigner;
}): Promise<StepUp> {
  void o;
  return Promise.reject(new Error("nativeStepUpAssertion: not implemented"));
}
