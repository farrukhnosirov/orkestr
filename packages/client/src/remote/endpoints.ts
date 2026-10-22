/**
 * Gateway endpoint URLs used by the web client — kept in ONE place so they
 * can be reconciled with the gateway implementation at integration time.
 *
 * Assumed shapes (JSON, cookie session `credentials: "include"`):
 * - `POST /v1/auth/passkey/register/begin {username}` → `{options}` (PublicKeyCredentialCreationOptions, base64url binary fields)
 * - `POST /v1/auth/passkey/register/finish {credential}` → `{accountId}`
 * - `POST /v1/auth/passkey/login/begin {username?}` → `{options}` (PublicKeyCredentialRequestOptions)
 * - `POST /v1/auth/passkey/login/finish {credential}` → `{accountId}`
 * - `POST /v1/auth/totp {username, code}` → `{accountId}`
 * - `POST /v1/auth/logout`, `GET /v1/auth/session` → `{accountId, username}`
 * - `GET /v1/cores` → `{cores:[{coreId, name?, cikPublic, online}]}`
 * - `POST /v1/pairings/{pairingId}/claim {devicePubKey, deviceName, platform, codeMac}` → `{coreId, corePubKey}`
 * - `GET /v1/pairings/{pairingId}` → `{status:"pending"|"approved"|"rejected"|"expired", deviceId?, role?, sig?}`
 * - `GET  ws(s)://…/v1/connect?coreId=…` → opaque relay of the e2e-v1 stream to the core
 */
export const GATEWAY_PATHS = {
  passkeyRegisterBegin: "/v1/auth/passkey/register/begin",
  passkeyRegisterFinish: "/v1/auth/passkey/register/finish",
  passkeyLoginBegin: "/v1/auth/passkey/login/begin",
  passkeyLoginFinish: "/v1/auth/passkey/login/finish",
  totp: "/v1/auth/totp",
  logout: "/v1/auth/logout",
  session: "/v1/auth/session",
  cores: "/v1/cores",
  pairingClaim: (pairingId: string) => `/v1/pairings/${encodeURIComponent(pairingId)}/claim`,
  pairing: (pairingId: string) => `/v1/pairings/${encodeURIComponent(pairingId)}`,
  connect: (coreId: string) => `/v1/connect?coreId=${encodeURIComponent(coreId)}`,
} as const;

const trim = (u: string) => u.replace(/\/+$/, "");

/** `https://gw` + path. */
export function httpUrl(gatewayUrl: string, path: string): string {
  return trim(gatewayUrl) + path;
}

/** WebSocket URL of the relay for `coreId` (http→ws, https→wss). */
export function connectUrl(gatewayUrl: string, coreId: string): string {
  return trim(gatewayUrl).replace(/^http/i, "ws") + GATEWAY_PATHS.connect(coreId);
}
