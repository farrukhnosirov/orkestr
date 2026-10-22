/**
 * WebAuthn helpers: JSON (base64url) ⇄ WebAuthn option/credential objects,
 * step-up assertions (remote-access.md §6.3) and step-up registration.
 */
import { b64uDecode, b64uEncode, randomBytes } from "./bytes";

/** Flat step-up assertion sent as `params.stepUp` (core `AssertionJson`). */
export interface StepUp {
  credentialId: string;
  authenticatorData: string;
  clientDataJson: string;
  /** DER ECDSA signature, base64url. */
  signature: string;
}

type Json = Record<string, unknown>;

const buf = (s: unknown): ArrayBuffer => b64uDecode(String(s)).buffer;

function mapCreds(list: unknown): PublicKeyCredentialDescriptor[] | undefined {
  if (!Array.isArray(list)) return undefined;
  return list.map((c: Json) => ({ ...(c as object), type: "public-key", id: buf(c.id) }) as PublicKeyCredentialDescriptor);
}

/** Server JSON → `PublicKeyCredentialCreationOptions`. */
export function decodeCreationOptions(o: Json): PublicKeyCredentialCreationOptions {
  const user = o.user as Json;
  const excl = mapCreds(o.excludeCredentials);
  return {
    ...(o as object),
    challenge: buf(o.challenge),
    user: { ...(user as object), id: buf(user.id) } as PublicKeyCredentialUserEntity,
    ...(excl ? { excludeCredentials: excl } : {}),
  } as PublicKeyCredentialCreationOptions;
}

/** Server JSON → `PublicKeyCredentialRequestOptions`. */
export function decodeRequestOptions(o: Json): PublicKeyCredentialRequestOptions {
  const allow = mapCreds(o.allowCredentials);
  return {
    ...(o as object),
    challenge: buf(o.challenge),
    ...(allow ? { allowCredentials: allow } : {}),
  } as PublicKeyCredentialRequestOptions;
}

/** `PublicKeyCredential` → JSON with base64url fields (WebAuthn L3 `toJSON` shape). */
export function credentialToJson(c: PublicKeyCredential): Json {
  const r = c.response as AuthenticatorAttestationResponse & AuthenticatorAssertionResponse;
  const response: Json = { clientDataJSON: b64uEncode(r.clientDataJSON) };
  if ("attestationObject" in r && r.attestationObject) response.attestationObject = b64uEncode(r.attestationObject);
  if ("authenticatorData" in r && r.authenticatorData) response.authenticatorData = b64uEncode(r.authenticatorData);
  if ("signature" in r && r.signature) response.signature = b64uEncode(r.signature);
  if ("userHandle" in r && r.userHandle) response.userHandle = b64uEncode(r.userHandle);
  return { id: c.id, rawId: b64uEncode(c.rawId), type: c.type, response };
}

function creds(): CredentialsContainer {
  if (typeof navigator === "undefined" || !navigator.credentials) throw new Error("WebAuthn is not available");
  return navigator.credentials;
}

/**
 * Produces a step-up assertion (user verification required) for the
 * core-issued `challenge` (base64url) from a `forbidden{stepUpRequired}` error.
 */
export async function stepUpAssertion(
  challenge: string,
  opts: { rpId?: string; credentialIds?: string[]; timeoutMs?: number } = {},
): Promise<StepUp> {
  const pk: PublicKeyCredentialRequestOptions = {
    challenge: buf(challenge),
    userVerification: "required",
    timeout: opts.timeoutMs ?? 60_000,
    ...(opts.rpId ? { rpId: opts.rpId } : {}),
    ...(opts.credentialIds
      ? { allowCredentials: opts.credentialIds.map((id) => ({ type: "public-key" as const, id: buf(id) })) }
      : {}),
  };
  const c = (await creds().get({ publicKey: pk })) as PublicKeyCredential | null;
  if (!c) throw new Error("no credential returned");
  const r = c.response as AuthenticatorAssertionResponse;
  return {
    credentialId: b64uEncode(c.rawId),
    authenticatorData: b64uEncode(r.authenticatorData),
    clientDataJson: b64uEncode(r.clientDataJSON),
    signature: b64uEncode(r.signature),
  };
}

/**
 * Registers the core-held step-up credential (sent via `devices.registerStepUp`
 * inside the E2E channel). The challenge is local: the core verifies the
 * attestation and origin, not a server challenge.
 */
export async function registerStepUpCredential(o: { rpId?: string; userName: string; deviceId: string }) {
  const c = (await creds().create({
    publicKey: {
      challenge: randomBytes(32),
      rp: { name: "Orkestr", ...(o.rpId ? { id: o.rpId } : {}) },
      user: { id: new TextEncoder().encode(o.deviceId), name: o.userName, displayName: o.userName },
      pubKeyCredParams: [{ type: "public-key", alg: -7 }],
      authenticatorSelection: { userVerification: "required" },
      attestation: "none",
    },
  })) as PublicKeyCredential | null;
  if (!c) throw new Error("no credential returned");
  return credentialToJson(c);
}

/** Extracts `{challenge}` from an RPC `forbidden` error carrying `stepUpRequired`. */
export function stepUpChallenge(err: unknown): string | null {
  const e = err as { code?: unknown; data?: unknown } | null;
  if (!e || e.code !== "forbidden" || typeof e.data !== "object" || e.data === null) return null;
  const d = e.data as { stepUpRequired?: unknown; challenge?: unknown };
  return d.stepUpRequired === true && typeof d.challenge === "string" ? d.challenge : null;
}
