/** Remote core connection (E2E through the gateway) and step-up retry. */
import {
  type DeviceRecord,
  deviceSigner,
  OrkestrClient,
  type StepUp,
  createRemoteSocketFactory,
  remoteClientUrl,
  stepUpChallenge,
} from "@orkestr/client";
import type { MethodName, ParamsOf, ResultOf } from "@orkestr/protocol";

/** The subset of `OrkestrClient` the web UI uses (fakes in tests). */
export type RemoteClient = Pick<OrkestrClient, "call" | "subscribe" | "close">;

export async function connectRemote(rec: DeviceRecord, sessionToken?: string): Promise<OrkestrClient> {
  const opts = {
    gatewayUrl: rec.gatewayUrl,
    coreId: rec.coreId,
    deviceId: rec.deviceId,
    deviceKey: rec.keyPair ?? (await deviceSigner(rec)),
    corePubKey: rec.corePubKey,
    ...(sessionToken ? { sessionToken } : {}),
  };
  const client = new OrkestrClient(remoteClientUrl(opts), "remote", {
    webSocket: createRemoteSocketFactory(opts),
    clientInfo: { kind: "web", version: "0.1.0" },
  });
  await client.connect();
  return client;
}

/**
 * Calls `method`; when the core answers `forbidden{stepUpRequired, challenge}`
 * obtains a WebAuthn assertion for that challenge and retries once with
 * `params.stepUp`.
 */
export async function callWithStepUp<M extends MethodName>(
  client: RemoteClient,
  method: M,
  params: ParamsOf<M>,
  stepUp: (challenge: string) => Promise<StepUp>,
): Promise<ResultOf<M>> {
  try {
    return await client.call(method, params);
  } catch (e) {
    const challenge = stepUpChallenge(e);
    if (!challenge) throw e;
    const assertion = await stepUp(challenge);
    return client.call(method, { ...(params as object), stepUp: assertion } as ParamsOf<M>);
  }
}

/** Error code of an RPC/gateway failure, for `t("errors.<code>")`. */
export function errorCode(e: unknown): string {
  const c = (e as { code?: unknown } | null)?.code;
  return typeof c === "string" ? c : "internal";
}
