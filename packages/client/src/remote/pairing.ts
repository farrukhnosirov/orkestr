/**
 * Device-side pairing (remote-access.md §4.2), mirroring the core's
 * `remote::pairing`:
 * - `codeMac = HMAC-SHA256(key = code, devicePub ‖ pairingId)` — the code never leaves the device;
 * - `SAS = u32_be(SHA-256(corePub ‖ devicePub ‖ pairingId)[0..4]) mod 10^6` (6 digits);
 * - `fingerprint = base64url(SHA-256(corePub))`;
 * - approval proof: `Sign_CIK(SHA-256("orkestr-pairing-v1" ‖ pairingId ‖ deviceId ‖ devicePub ‖ role))`.
 */
import { b64uDecode, b64uEncode, concat, utf8 } from "./bytes";
import { importVerifyKey, verifySig } from "./e2e";
import { GATEWAY_PATHS, httpUrl } from "./endpoints";
import { type DeviceKeyStore, type DeviceRecord, defaultKeyStore, generateDeviceKey } from "./keystore";

export async function codeMac(code: string, devicePub: Uint8Array, pairingId: string): Promise<Uint8Array> {
  const key = await crypto.subtle.importKey("raw", utf8(code), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return new Uint8Array(await crypto.subtle.sign("HMAC", key, concat(devicePub, utf8(pairingId))));
}

export async function sas(corePub: Uint8Array, devicePub: Uint8Array, pairingId: string): Promise<string> {
  const d = new DataView(await crypto.subtle.digest("SHA-256", concat(corePub, devicePub, utf8(pairingId))));
  return String(d.getUint32(0) % 1_000_000).padStart(6, "0");
}

export async function fingerprint(corePub: Uint8Array): Promise<string> {
  return b64uEncode(await crypto.subtle.digest("SHA-256", concat(corePub)));
}

export function completionMessage(pairingId: string, deviceId: string, devicePub: Uint8Array, role: string) {
  return crypto.subtle.digest(
    "SHA-256",
    concat(utf8("orkestr-pairing-v1"), utf8(pairingId), utf8(deviceId), devicePub, utf8(role)),
  );
}

/** Pairing offer (from the desktop QR / manual entry). */
export interface PairingOffer {
  gatewayUrl: string;
  pairingId: string;
  /** 8-character code. */
  code: string;
  /** `base64url(SHA-256(CIK))` shown by the desktop; checked against the gateway's answer. */
  coreKeyFingerprint?: string;
}

/** Parses the QR payload `{gatewayUrl, coreId, pairingId, code, coreKeyFingerprint}` (JSON). */
export function parsePairingQr(text: string): PairingOffer | null {
  try {
    const v = JSON.parse(text) as Partial<PairingOffer>;
    if (typeof v.gatewayUrl === "string" && typeof v.pairingId === "string" && typeof v.code === "string") {
      return {
        gatewayUrl: v.gatewayUrl,
        pairingId: v.pairingId,
        code: v.code,
        ...(typeof v.coreKeyFingerprint === "string" ? { coreKeyFingerprint: v.coreKeyFingerprint } : {}),
      };
    }
  } catch {
    /* not JSON */
  }
  return null;
}

export type PairingStatus = "pending" | "approved" | "rejected" | "expired";

export interface PairingSession {
  coreId: string;
  /** 6-digit SAS to compare with the desktop. */
  sas: string;
  /** Polls the gateway until the desktop decides; on approval the record is stored. */
  waitForApproval(opts?: { intervalMs?: number; signal?: AbortSignal }): Promise<DeviceRecord>;
}

export class PairingError extends Error {
  constructor(
    readonly code: "fingerprint_mismatch" | "claim_failed" | "rejected" | "expired" | "bad_signature" | "aborted",
    message: string,
  ) {
    super(message);
    this.name = "PairingError";
  }
}

export interface PairDeviceOptions extends PairingOffer {
  deviceName: string;
  platform: string;
  store?: DeviceKeyStore;
  fetch?: typeof fetch;
  /** Injected device key (tests). */
  keyPair?: CryptoKeyPair;
}

/** Claims a pairing offer with a fresh non-extractable device key. */
export async function pairDevice(o: PairDeviceOptions): Promise<PairingSession> {
  const f = o.fetch ?? globalThis.fetch.bind(globalThis);
  const store = o.store ?? defaultKeyStore();
  const keyPair = o.keyPair ?? (await generateDeviceKey());
  const devicePub = new Uint8Array(await crypto.subtle.exportKey("raw", keyPair.publicKey));
  const mac = await codeMac(o.code.trim().toUpperCase(), devicePub, o.pairingId);
  const res = await f(httpUrl(o.gatewayUrl, GATEWAY_PATHS.pairingClaim(o.pairingId)), {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      devicePubKey: b64uEncode(devicePub),
      deviceName: o.deviceName,
      platform: o.platform,
      codeMac: b64uEncode(mac),
    }),
  });
  if (!res.ok) throw new PairingError("claim_failed", `pairing claim failed (${res.status})`);
  const claim = (await res.json()) as { coreId: string; corePubKey: string };
  const corePub = b64uDecode(claim.corePubKey);
  if (o.coreKeyFingerprint && (await fingerprint(corePub)) !== o.coreKeyFingerprint) {
    throw new PairingError("fingerprint_mismatch", "core key fingerprint does not match the pairing offer");
  }
  const code = await sas(corePub, devicePub, o.pairingId);

  return {
    coreId: claim.coreId,
    sas: code,
    async waitForApproval({ intervalMs = 1500, signal } = {}) {
      for (;;) {
        if (signal?.aborted) throw new PairingError("aborted", "pairing aborted");
        const r = await f(httpUrl(o.gatewayUrl, GATEWAY_PATHS.pairing(o.pairingId)), { credentials: "include" });
        if (r.ok) {
          const s = (await r.json()) as { status: PairingStatus; deviceId?: string; role?: string; sig?: string };
          if (s.status === "rejected" || s.status === "expired") throw new PairingError(s.status, `pairing ${s.status}`);
          if (s.status === "approved" && s.deviceId && s.role && s.sig) {
            const msg = await completionMessage(o.pairingId, s.deviceId, devicePub, s.role);
            if (!(await verifySig(await importVerifyKey(corePub), b64uDecode(s.sig), msg))) {
              throw new PairingError("bad_signature", "pairing approval is not signed by the core key");
            }
            const rec: DeviceRecord = {
              id: claim.coreId,
              gatewayUrl: o.gatewayUrl,
              coreId: claim.coreId,
              deviceId: s.deviceId,
              corePubKey: claim.corePubKey,
              role: s.role,
              deviceName: o.deviceName,
              keyPair,
              pairedAt: Date.now(),
            };
            await store.put(rec);
            return rec;
          }
        }
        await new Promise((r2) => setTimeout(r2, intervalMs));
      }
    },
  };
}
