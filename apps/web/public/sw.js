/* Orkestr web client service worker: Web Push (ADR-0036).
 * The browser decrypts the RFC 8291 payload; `data` is the core's PushPayload
 * {v, eventId, category, projectId?, ts, title, body}. */
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("push", (event) => {
  let p = {};
  try {
    p = event.data ? event.data.json() : {};
  } catch {
    p = {};
  }
  const title = typeof p.title === "string" && p.title ? p.title : "Orkestr";
  const body = typeof p.body === "string" ? p.body : "";
  event.waitUntil(
    self.registration.showNotification(title, {
      body,
      tag: typeof p.eventId === "string" && p.eventId ? `${p.category}:${p.eventId}` : undefined,
      data: { category: p.category, projectId: p.projectId },
      requireInteraction: p.category === "approval",
    }),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = new URL("/", self.location.origin).href;
  event.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((list) => {
      for (const c of list) {
        if (c.url.startsWith(self.location.origin) && "focus" in c) return c.focus();
      }
      return self.clients.openWindow(url);
    }),
  );
});
