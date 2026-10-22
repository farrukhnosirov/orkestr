/** Gateway HTTP API (account auth, VAPID key). URLs live in `@orkestr/client` `GATEWAY_PATHS`. */
import { GATEWAY_PATHS, credentialToJson, decodeCreationOptions, decodeRequestOptions, httpUrl } from "@orkestr/client";

export interface AuthState {
  hasAccount: boolean;
  authenticated: boolean;
  accountId?: string;
  rpId?: string;
  totpEnabled?: boolean;
}

export interface LoginResult {
  token?: string;
  accountId?: string;
}

export class GatewayError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = "GatewayError";
  }
}

type Json = Record<string, unknown>;

export interface GatewayApi {
  readonly url: string;
  state(): Promise<AuthState>;
  registerPasskey(name: string): Promise<LoginResult>;
  loginPasskey(): Promise<LoginResult>;
  totpLogin(code: string): Promise<LoginResult>;
  logout(): Promise<void>;
  vapidKey(): Promise<string | null>;
  /** One-time token that connects a computer (core) to this account. */
  enrollmentToken(): Promise<{ token: string; expiresAt: string }>;
}

/** Default gateway: the origin that served the app (or `VITE_GATEWAY_URL` in dev). */
export function defaultGatewayUrl(): string {
  const env = (import.meta.env as Record<string, string | undefined>).VITE_GATEWAY_URL;
  return env ?? (typeof location === "undefined" ? "http://127.0.0.1:8787" : location.origin);
}

export class HttpGateway implements GatewayApi {
  constructor(
    readonly url: string = defaultGatewayUrl(),
    private readonly f: typeof fetch = globalThis.fetch.bind(globalThis),
    private readonly creds: () => CredentialsContainer = () => navigator.credentials,
  ) {}

  private async req<T>(path: string, body?: unknown): Promise<T> {
    const res = await this.f(httpUrl(this.url, path), {
      method: body === undefined ? "GET" : "POST",
      credentials: "include",
      headers: body === undefined ? {} : { "content-type": "application/json" },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    if (!res.ok) throw new GatewayError(res.status, (await res.text().catch(() => "")) || res.statusText);
    const text = await res.text();
    return (text ? JSON.parse(text) : {}) as T;
  }

  state() {
    return this.req<AuthState>(GATEWAY_PATHS.authState);
  }

  async registerPasskey(name: string) {
    const begin = await this.req<{ ceremonyId: string; publicKey: Json }>(GATEWAY_PATHS.registerBegin, { name });
    const cred = (await this.creds().create({ publicKey: decodeCreationOptions(begin.publicKey) })) as PublicKeyCredential | null;
    if (!cred) throw new GatewayError(0, "cancelled");
    return this.req<LoginResult>(GATEWAY_PATHS.registerFinish, { ceremonyId: begin.ceremonyId, credential: credentialToJson(cred) });
  }

  async loginPasskey() {
    const begin = await this.req<{ ceremonyId: string; publicKey: Json }>(GATEWAY_PATHS.loginBegin, {});
    const cred = (await this.creds().get({ publicKey: decodeRequestOptions(begin.publicKey) })) as PublicKeyCredential | null;
    if (!cred) throw new GatewayError(0, "cancelled");
    return this.req<LoginResult>(GATEWAY_PATHS.loginFinish, { ceremonyId: begin.ceremonyId, credential: credentialToJson(cred) });
  }

  totpLogin(code: string) {
    return this.req<LoginResult>(GATEWAY_PATHS.totpLogin, { code });
  }

  async logout() {
    await this.req(GATEWAY_PATHS.logout, {});
  }

  enrollmentToken() {
    return this.req<{ token: string; expiresAt: string }>(GATEWAY_PATHS.enrollmentToken, {});
  }

  async vapidKey() {
    try {
      return (await this.req<{ publicKey: string }>(GATEWAY_PATHS.pushVapid)).publicKey;
    } catch {
      return null;
    }
  }
}
