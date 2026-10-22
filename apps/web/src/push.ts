/** Web Push subscription toggle (service worker `/sw.js`, keys from the core / gateway). */
import { b64uDecode, b64uEncode } from "@orkestr/client";

import type { GatewayApi } from "./gateway";
import type { RemoteClient } from "./remote";

export function pushSupported(): boolean {
  return typeof navigator !== "undefined" && "serviceWorker" in navigator && typeof PushManager !== "undefined";
}

async function registration(): Promise<ServiceWorkerRegistration> {
  await navigator.serviceWorker.register("/sw.js");
  return navigator.serviceWorker.ready;
}

export async function currentSubscription(): Promise<PushSubscription | null> {
  if (!pushSupported()) return null;
  const reg = await navigator.serviceWorker.getRegistration();
  return (await reg?.pushManager.getSubscription()) ?? null;
}

/** Subscribes this browser and registers the subscription with the core. */
export async function enablePush(client: RemoteClient, gateway: GatewayApi): Promise<void> {
  if (!pushSupported()) throw new Error("unsupported");
  if ((await Notification.requestPermission()) !== "granted") throw new Error("permission");
  let key: string | null = null;
  try {
    key = (await client.call("push.vapidKey", {})).publicKey;
  } catch {
    key = await gateway.vapidKey();
  }
  if (!key) throw new Error("vapid");
  const reg = await registration();
  const sub = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: b64uDecode(key) });
  const p256dh = sub.getKey("p256dh");
  const auth = sub.getKey("auth");
  if (!p256dh || !auth) throw new Error("keys");
  await client.call("push.subscribe", { endpoint: sub.endpoint, p256dh: b64uEncode(p256dh), auth: b64uEncode(auth) });
}

export async function disablePush(client: RemoteClient): Promise<void> {
  const sub = await currentSubscription();
  if (!sub) return;
  await client.call("push.unsubscribe", { endpoint: sub.endpoint }).catch(() => undefined);
  await sub.unsubscribe();
}
