/**
 * Approvals with biometric step-up (remote-access.md §6.3, ADR-0038): call;
 * on `forbidden{stepUpRequired, challenge}` obtain ONE native assertion for
 * that challenge and retry once with `params.stepUp`. A denied biometric
 * prompt rejects without a retry — the approval stays pending.
 */
import { type StepUp, stepUpChallenge } from "@orkestr/client";
import type { Approval, MethodName, ParamsOf, ResolveApprovalParams, ResultOf } from "@orkestr/protocol";

import type { RemoteClient } from "./connection";

/** Produces a step-up assertion for a core challenge (e.g. `StepUpKeys.assert`). */
export type StepUpProvider = (challenge: string) => Promise<StepUp>;

/** Generic step-up retry for any dangerous method (approvals.resolve, devices.revoke, …). */
export async function callWithStepUp<M extends MethodName>(
  client: RemoteClient,
  method: M,
  params: ParamsOf<M>,
  stepUp: StepUpProvider,
): Promise<ResultOf<M>> {
  try {
    return await client.call(method, params);
  } catch (e) {
    const challenge = stepUpChallenge(e);
    if (!challenge) throw e;
    const assertion = await stepUp(challenge); // BiometricDeniedError propagates: no retry
    return client.call(method, { ...(params as object), stepUp: assertion } as ParamsOf<M>);
  }
}

export function resolveWithStepUp(client: RemoteClient, params: ResolveApprovalParams, stepUp: StepUpProvider): Promise<Approval> {
  return callWithStepUp(client, "approvals.resolve", params, stepUp).then((r) => r.approval);
}

/** Pending approvals, newest first. */
export async function listPendingApprovals(client: RemoteClient): Promise<Approval[]> {
  return (await client.call("approvals.list", { status: "Pending" })).approvals;
}

/** True for approvals the core will ask a step-up for (dangerous / high risk). */
export function isDangerous(a: Approval): boolean {
  return a.danger !== null || a.risk === "High" || a.risk === "Critical";
}
