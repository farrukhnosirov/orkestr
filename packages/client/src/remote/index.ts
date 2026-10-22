/** Remote access (Phase 4): e2e-v1 RemoteTransport, pairing, step-up. */
export { b64uDecode, b64uEncode } from "./bytes";
export { DeviceHandshake, E2E_PROTOCOL } from "./e2e";
export type { ClientFinish, ClientHello, HandshakeError, ServerHello } from "./e2e";
export { GATEWAY_PATHS, connectUrl, httpUrl } from "./endpoints";
export { IndexedDbKeyStore, MemoryKeyStore, defaultKeyStore, deviceSigner, generateDeviceKey } from "./keystore";
export { isDeviceSigner, remoteCrypto, setRemoteCrypto } from "./crypto";
export type { AeadKey, Bytes, DeviceSigner, EcdhKeyPair, RawKey, RemoteCrypto } from "./crypto";
export { cryptoKeySigner, webCrypto } from "./webcrypto";
export { nobleCrypto } from "./noble";
export { NATIVE_STEPUP_RP_ID, nativeStepUpAssertion, nativeStepUpOrigin } from "./nativeStepUp";
export type { NativeStepUpRegistration } from "./nativeStepUp";
export { NATIVE_PUSH_AAD, decryptNativePush } from "./nativePush";
export type { DeviceKeyStore, DeviceRecord } from "./keystore";
export { PAIRING_QR_PREFIX, PairingError, codeMac, fingerprint, pairDevice, parsePairingQr, sas } from "./pairing";
export type { PairDeviceOptions, PairingOffer, PairingSession, PairingStatus } from "./pairing";
export { E2E_CLOSE_CODE, E2E_TRANSIENT_CLOSE_CODE, createRemoteSocketFactory, remoteClientUrl } from "./transport";
export type { RawSocket, RawSocketFactory, RemoteSocketOptions } from "./transport";
export {
  credentialToJson,
  decodeCreationOptions,
  decodeRequestOptions,
  registerStepUpCredential,
  stepUpAssertion,
  stepUpChallenge,
} from "./webauthn";
export type { StepUp } from "./webauthn";
