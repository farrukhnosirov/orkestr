/**
 * Calls to RPC methods that other Phase 1 areas add (e.g. `emergency.stopAll`,
 * `permissions.setMode`). They are not in the generated `Methods` map of
 * every build, so they are called untyped and "unknown method" is reported
 * as `{ available: false }` instead of an error.
 */
import { OrkestrError } from "@orkestr/client";

import { getClient } from "../../core/client";

export type OptionalCallResult = { available: true; result: unknown } | { available: false };

/** True when the core answered "unknown method" (the area is not in this build). */
export function isUnknownMethod(e: unknown): boolean {
  return e instanceof OrkestrError && e.code === "bad_request" && /unknown method/i.test(e.message);
}

/** Calls `method` if the core implements it. Other errors are rethrown. */
export async function callOptional(method: string, params: Record<string, unknown>): Promise<OptionalCallResult> {
  // The typed `call` only accepts generated method names; this wrapper exists
  // precisely for names that may not be generated yet, so we widen it here.
  const untyped = getClient().call.bind(getClient()) as unknown as (m: string, p: unknown) => Promise<unknown>;
  try {
    return { available: true, result: await untyped(method, params) };
  } catch (e) {
    if (isUnknownMethod(e)) return { available: false };
    throw e;
  }
}
