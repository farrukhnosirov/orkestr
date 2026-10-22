/**
 * Gateway endpoint URLs used by the web client — kept in ONE place so they
 * can be reconciled with the gateway (crates/gateway, package A) at integration.
 *
 * JSON, binary values base64url, cookie session `orkestr_session` (`credentials: "include"`):
 * - `GET  /v1/auth/state` → `{hasAccount, authenticated, accountId?, rpId, origins, totpEnabled}`
 * - `POST /v1/auth/register/begin {name?}` / `login/begin` → `{ceremonyId, publicKey}`
 * - `POST /v1/auth/register/finish|login/finish {ceremonyId, credential}` → `{token, accountId}`
 * - `POST /v1/auth/totp/login {code}` → `{token, accountId}`; `POST /v1/auth/logout`
 * - `GET  /v1/cores` (session)
 * - `POST /v1/pairings/{id}/claim {coreId, devicePubKey, deviceName, platform, codeMac}`
 *   → `{status:"pending", deviceId, coreId, corePublicKey}`
 * - `GET  /v1/pairings/{id}` → `{status, coreId, corePublicKey, result:{status, deviceId, role, sig}}`
 * - `WS   /v1/connect?coreId=…[&token=…]` → opaque relay of the e2e-v1 stream
 * - `GET  /v1/push/vapid` → `{publicKey}` (gateway push module; fallback when the core does not know the key)
 */
export const GATEWAY_PATHS = {
  authState: "/v1/auth/state",
  registerBegin: "/v1/auth/register/begin",
  registerFinish: "/v1/auth/register/finish",
  loginBegin: "/v1/auth/login/begin",
  loginFinish: "/v1/auth/login/finish",
  totpLogin: "/v1/auth/totp/login",
  logout: "/v1/auth/logout",
  cores: "/v1/cores",
  pushVapid: "/v1/push/vapid",
  pairingClaim: (pairingId: string) => `/v1/pairings/${encodeURIComponent(pairingId)}/claim`,
  pairing: (pairingId: string) => `/v1/pairings/${encodeURIComponent(pairingId)}`,
  connect: (coreId: string, token?: string) =>
    `/v1/connect?coreId=${encodeURIComponent(coreId)}${token ? `&token=${encodeURIComponent(token)}` : ""}`,
} as const;

const trim = (u: string) => u.replace(/\/+$/, "");

/** `https://gw` + path. */
export function httpUrl(gatewayUrl: string, path: string): string {
  return trim(gatewayUrl) + path;
}

/** WebSocket URL of the relay for `coreId` (http→ws, https→wss). */
export function connectUrl(gatewayUrl: string, coreId: string, token?: string): string {
  return trim(gatewayUrl).replace(/^http/i, "ws") + GATEWAY_PATHS.connect(coreId, token);
}
