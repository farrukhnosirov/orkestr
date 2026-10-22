/**
 * Pure-TS mobile core (no react-native imports): bundleable by Node for
 * scripts/smoke/phase5-acceptance.mjs. Install a crypto provider first
 * (`setRemoteCrypto(nobleCrypto())`), then `createMemoryCore()`.
 */
export * from "./adapters";
export * from "./app";
export * from "./approvals";
export * from "./connection";
export * from "./gatewaySession";
export * from "./keys";
export * from "./monitoring";
export * from "./pairing";
export * from "./push";
export * from "./voice/executor";

// Re-exported so a single bundle of this module shares the crypto provider
// singleton with the client code it embeds (acceptance script, tests).
export {
  OrkestrClient,
  b64uDecode,
  b64uEncode,
  nativeStepUpAssertion,
  nobleCrypto,
  remoteCrypto,
  setRemoteCrypto,
  webCrypto,
} from "@orkestr/client";
export type { DeviceRecord, PairingOffer, PairingSession, RawSocketFactory, StepUp } from "@orkestr/client";
