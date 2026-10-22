/**
 * Mobile notifications (ADR-0037):
 * - native push: register the FCM/APNs token with a fresh 32-byte AES-256-GCM
 *   key (kept in SecureKV) via `push.registerNative`; incoming payloads are
 *   decrypted with `decryptNativePush`;
 * - while connected: `ApprovalRequested` events arriving over E2E raise a
 *   local notification (works without Firebase, e.g. on the emulator).
 * Tapping a notification routes to the approval (`routeForNotification`).
 */
import { type Bytes, b64uDecode, b64uEncode, decryptNativePush, remoteCrypto } from "@orkestr/client";
import type { Event, PushLockScreen, PushPayload } from "@orkestr/protocol";

import type { LocalNotification, Notifier, SecureKV } from "./adapters";
import type { RemoteClient } from "./connection";
import { kvKey } from "./keys";

export const PUSH_CATEGORIES = ["approval", "taskCompleted", "taskBlocked", "buildFailed", "testFailed", "budget"] as const;
export type PushCategory = (typeof PUSH_CATEGORIES)[number];

export interface PushPrefs {
  categories: PushCategory[];
  lockScreen: PushLockScreen;
}

export const DEFAULT_PUSH_PREFS: PushPrefs = { categories: [...PUSH_CATEGORIES], lockScreen: "generic" };

export type RegisterResult =
  | { status: "registered"; id: string; deliverable: boolean; transport: "fcm" | "apns" }
  | { status: "unavailable"; reason: "no_token" | "permission" };

/** Where a tapped notification leads. */
export type NotificationRoute =
  | { screen: "ApprovalDetail"; coreId: string; approvalId: string }
  | { screen: "Approvals"; coreId: string }
  | { screen: "Home"; coreId?: string };

const keyName = (coreId: string) => kvKey("push", coreId, "key");
const PREFS = "push.prefs";

export class PushManager {
  constructor(
    private readonly kv: SecureKV,
    private readonly notifier: Notifier,
  ) {}

  async prefs(): Promise<PushPrefs> {
    const raw = await this.kv.get(PREFS);
    return raw ? { ...DEFAULT_PUSH_PREFS, ...(JSON.parse(raw) as Partial<PushPrefs>) } : DEFAULT_PUSH_PREFS;
  }

  async setPrefs(p: PushPrefs): Promise<void> {
    await this.kv.set(PREFS, JSON.stringify(p));
  }

  /**
   * Registers this device's native token with the core. Without push
   * credentials in the build (no google-services.json) nothing is registered
   * and the UI shows "push unavailable on this build".
   */
  async registerNative(client: RemoteClient, coreId: string): Promise<RegisterResult> {
    if (!(await this.notifier.requestPermission())) return { status: "unavailable", reason: "permission" };
    const tok = await this.notifier.nativeToken();
    if (!tok) return { status: "unavailable", reason: "no_token" };
    const key = remoteCrypto().randomBytes(32);
    await this.kv.set(keyName(coreId), b64uEncode(key));
    const prefs = await this.prefs();
    const r = await client.call("push.registerNative", {
      transport: tok.transport,
      token: tok.token,
      key: b64uEncode(key),
      categories: prefs.categories,
      lockScreen: prefs.lockScreen,
    });
    return { status: "registered", id: r.id, deliverable: r.deliverable, transport: tok.transport };
  }

  async payloadKey(coreId: string): Promise<Bytes | null> {
    const k = await this.kv.get(keyName(coreId));
    return k ? b64uDecode(k) : null;
  }

  /**
   * Decrypts a native push `data` map: `{p: JSON "{n,c}"}` (FCM data /
   * APNs custom key) or `{n, c}` directly. `coreId` picks the key.
   */
  async decrypt(coreId: string, data: Record<string, unknown>): Promise<PushPayload> {
    const key = await this.payloadKey(coreId);
    if (!key) throw new Error("no push key for this core");
    return decryptNativePush(key, extractSealed(data));
  }

  /** Raises a local notification for an approval request received over E2E. */
  async onEvent(coreId: string, event: Event): Promise<LocalNotification | null> {
    if (event.kind !== "ApprovalRequested") return null;
    const prefs = await this.prefs();
    if (!prefs.categories.includes("approval")) return null;
    const a = event.payload.approval;
    const n: LocalNotification = {
      id: `approval-${a.id}`,
      title: "Orkestr",
      // Lock-screen privacy: generic text unless the user opted into details.
      body: prefs.lockScreen === "detailed" ? a.actionSummary : "Action needed",
      data: { kind: "approval", coreId, approvalId: a.id },
    };
    await this.notifier.show(n);
    return n;
  }

  /** Subscribes to a connected core's events for local notifications. */
  attach(client: RemoteClient, coreId: string): () => void {
    return client.subscribe(["*"], (ev) => {
      void this.onEvent(coreId, ev).catch(() => undefined);
    });
  }
}

export function extractSealed(data: Record<string, unknown>): { n: string; c: string } {
  let src: unknown = data;
  if (typeof data.p === "string") {
    try {
      src = JSON.parse(data.p);
    } catch {
      throw new Error("malformed push payload");
    }
  }
  const s = src as { n?: unknown; c?: unknown };
  if (typeof s.n !== "string" || typeof s.c !== "string") throw new Error("malformed push payload");
  return { n: s.n, c: s.c };
}

/** Maps notification data (local or decrypted push) to a screen. */
export function routeForNotification(data: Record<string, unknown> | undefined, fallbackCoreId?: string): NotificationRoute {
  const coreId = typeof data?.coreId === "string" ? data.coreId : fallbackCoreId;
  const kind = data?.kind ?? data?.category ?? data?.k;
  if ((kind === "approval" || typeof data?.approvalId === "string") && coreId) {
    return typeof data?.approvalId === "string"
      ? { screen: "ApprovalDetail", coreId, approvalId: data.approvalId }
      : { screen: "Approvals", coreId };
  }
  return coreId ? { screen: "Home", coreId } : { screen: "Home" };
}
