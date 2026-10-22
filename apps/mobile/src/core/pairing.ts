/**
 * Mobile pairing flow (remote-access.md §4.2) on top of `pairDevice`:
 * parse a QR / deep link `orkestr://pair?offer=…` / pasted text, claim with a
 * fresh raw device key (bearer-authenticated fetch), show the SAS, wait for
 * the desktop's approval, then register the biometric step-up key.
 */
import {
  type DeviceKeyStore,
  type DeviceRecord,
  type PairingOffer,
  type PairingSession,
  b64uDecode,
  pairDevice,
  parsePairingQr,
} from "@orkestr/client";

import type { RemoteClient } from "./connection";
import type { StepUpKeys } from "./keys";
import { generateRawDeviceKey } from "./keys";
import type { FetchLike } from "./gatewaySession";

export const DEEP_LINK_SCHEME = "orkestr";

function tryOffer(s: string): PairingOffer | null {
  const direct = parsePairingQr(s);
  if (direct) return direct;
  try {
    // base64url-encoded JSON / QR payload
    return parsePairingQr(new TextDecoder().decode(b64uDecode(s.trim())));
  } catch {
    return null;
  }
}

/**
 * Parses any pairing input: `orkestr-pair:{…}`, bare JSON, a deep link
 * `orkestr://pair?offer=<urlencoded or base64url payload>` or an
 * `https://…/pair?offer=…` link. Returns `null` when it is not an offer.
 */
export function parsePairingInput(input: string): PairingOffer | null {
  const t = input.trim();
  if (!t) return null;
  const m = /[?&]offer=([^&#]+)/.exec(t);
  if (m && /^(orkestr:\/\/|https?:\/\/)/i.test(t)) {
    let v = m[1] as string;
    try {
      v = decodeURIComponent(v.replace(/\+/g, "%20"));
    } catch {
      /* keep raw */
    }
    return tryOffer(v);
  }
  return tryOffer(t);
}

/** Deep link for an offer (used by tests / the Android lane). */
export function pairingDeepLink(offer: PairingOffer): string {
  return `${DEEP_LINK_SCHEME}://pair?offer=${encodeURIComponent(JSON.stringify(offer))}`;
}

export interface StartPairingOptions {
  offer: PairingOffer;
  deviceName: string;
  platform: string;
  store: DeviceKeyStore;
  /** Bearer-authenticated fetch (`GatewaySession.fetch()`). */
  fetch: FetchLike;
}

/** Claims the offer with a fresh raw device key; the session carries the SAS. */
export async function startPairing(o: StartPairingOptions): Promise<PairingSession> {
  const key = await generateRawDeviceKey();
  return pairDevice({
    ...o.offer,
    deviceName: o.deviceName,
    platform: o.platform,
    store: o.store,
    fetch: o.fetch,
    key,
  });
}

/** True when the 6-digit codes match (whitespace-insensitive). */
export function sasMatches(shown: string, typedOrSeen: string): boolean {
  const n = (s: string) => s.replace(/\s+/g, "");
  return n(shown).length === 6 && n(shown) === n(typedOrSeen);
}

/**
 * After approval: registers the biometric step-up key with the core. Failure
 * is non-fatal for pairing (the device works, dangerous actions stay locked);
 * the result says what happened.
 */
export async function registerStepUpAfterPairing(
  client: RemoteClient,
  rec: DeviceRecord,
  stepUp: StepUpKeys,
): Promise<{ registered: boolean; error?: string }> {
  try {
    const credential = await stepUp.create(rec.coreId);
    await client.call("devices.registerStepUp", { credential: { ...credential } });
    return { registered: true };
  } catch (e) {
    return { registered: false, error: (e as { code?: string; message?: string }).code ?? (e as Error).message };
  }
}
