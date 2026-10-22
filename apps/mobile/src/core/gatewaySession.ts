/**
 * Gateway account session for the mobile app: TOTP login → bearer token kept
 * in SecureKV, `/v1/auth/state`, `/v1/cores` (with `online`). The gateway
 * accepts `Authorization: Bearer` instead of the browser cookie.
 */
import { GATEWAY_PATHS, httpUrl } from "@orkestr/client";

import type { SecureKV } from "./adapters";

export interface AuthState {
  hasAccount: boolean;
  authenticated: boolean;
  accountId?: string;
  rpId?: string;
  totpEnabled?: boolean;
}

export interface GatewayDevice {
  deviceId: string;
  name: string;
  role: string;
  status: string;
}

export interface GatewayCore {
  coreId: string;
  name: string;
  online: boolean;
  lastSeen?: string | null;
  cikPublic?: string;
  devices?: GatewayDevice[];
}

export class GatewayError extends Error {
  readonly code: string;
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = "GatewayError";
    this.code = status === 401 ? "unauthenticated" : status === 0 ? "unavailable" : status === 403 ? "forbidden" : "internal";
  }
}

const KV_URL = "gw.url";
const KV_TOKEN = "gw.token";

export type FetchLike = typeof fetch;

/** `fetch` that adds the bearer token (used for pairing claims, which need the account session). */
export function bearerFetch(token: string | null, f: FetchLike = globalThis.fetch.bind(globalThis)): FetchLike {
  return ((input: Parameters<FetchLike>[0], init: RequestInit = {}) => {
    const headers = new Headers(init.headers);
    if (token) headers.set("authorization", `Bearer ${token}`);
    return f(input, { ...init, headers });
  }) as FetchLike;
}

export class GatewaySession {
  private tokenValue: string | null;

  constructor(
    readonly url: string,
    private readonly opts: { kv?: SecureKV; fetch?: FetchLike; token?: string | null } = {},
  ) {
    this.tokenValue = opts.token ?? null;
  }

  /** Restores the last session (URL + bearer) from SecureKV, or `null`. */
  static async restore(kv: SecureKV, f?: FetchLike): Promise<GatewaySession | null> {
    const url = await kv.get(KV_URL);
    if (!url) return null;
    const token = await kv.get(KV_TOKEN);
    return new GatewaySession(url, { kv, token, ...(f ? { fetch: f } : {}) });
  }

  get token(): string | null {
    return this.tokenValue;
  }

  get authenticated(): boolean {
    return this.tokenValue !== null;
  }

  /** `fetch` carrying this session's bearer. */
  fetch(): FetchLike {
    return bearerFetch(this.tokenValue, this.opts.fetch);
  }

  private async req<T>(path: string, body?: unknown): Promise<T> {
    let res: Response;
    try {
      res = await this.fetch()(httpUrl(this.url, path), {
        method: body === undefined ? "GET" : "POST",
        headers: body === undefined ? {} : { "content-type": "application/json" },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      });
    } catch (e) {
      throw new GatewayError(0, e instanceof Error ? e.message : String(e));
    }
    if (!res.ok) throw new GatewayError(res.status, (await res.text().catch(() => "")) || res.statusText);
    const text = await res.text();
    return (text ? JSON.parse(text) : {}) as T;
  }

  state(): Promise<AuthState> {
    return this.req<AuthState>(GATEWAY_PATHS.authState);
  }

  /** TOTP login; stores the bearer token. */
  async totpLogin(code: string): Promise<{ token: string; accountId?: string }> {
    const r = await this.req<{ token?: string; accountId?: string }>(GATEWAY_PATHS.totpLogin, { code: code.trim() });
    if (!r.token) throw new GatewayError(500, "gateway returned no session token");
    this.tokenValue = r.token;
    if (this.opts.kv) {
      await this.opts.kv.set(KV_URL, this.url);
      await this.opts.kv.set(KV_TOKEN, r.token);
    }
    return { token: r.token, ...(r.accountId ? { accountId: r.accountId } : {}) };
  }

  async cores(): Promise<GatewayCore[]> {
    return (await this.req<{ cores?: GatewayCore[] }>(GATEWAY_PATHS.cores)).cores ?? [];
  }

  async logout(): Promise<void> {
    await this.req(GATEWAY_PATHS.logout, {}).catch(() => undefined);
    this.tokenValue = null;
    await this.opts.kv?.delete(KV_TOKEN);
  }
}
