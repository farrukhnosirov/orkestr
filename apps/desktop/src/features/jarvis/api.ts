/**
 * `jarvis.*` calls (ADR-0049): settings, template answers, the streamed AI
 * conversation, memory. Keeps the Jarvis store and the persona (`{{addr}}`)
 * in sync.
 */
import type { Event, JarvisConfig, JarvisTemplateReply } from "@orkestr/protocol";
import { TOPIC_GLOBAL } from "@orkestr/protocol";

import { getClient, maybeClient } from "../../core/client";
import { currentLanguage } from "../../i18n";
import { applyPersona } from "./persona";
import { useJarvisStore } from "./store";

/** Loads the settings (and the persona). Best effort: older cores keep the defaults. */
export async function loadJarvisConfig(): Promise<JarvisConfig | null> {
  const client = maybeClient();
  if (!client) return null;
  try {
    const r = await client.call("jarvis.getConfig", {});
    useJarvisStore.getState().setConfig(r.config, r.bank);
    applyPersona(r.config);
    return r.config;
  } catch {
    applyPersona();
    return null;
  }
}

/** Saves the settings (and applies the persona at once). */
export async function saveJarvisConfig(config: JarvisConfig): Promise<JarvisConfig> {
  const r = await getClient().call("jarvis.setConfig", { config });
  useJarvisStore.getState().setConfig(r.config, r.bank);
  applyPersona(r.config);
  return r.config;
}

/** Changes the form of address only (voice: «называй меня босс»). */
export async function setAddress(address: JarvisConfig["address"]): Promise<void> {
  const c = useJarvisStore.getState().config;
  if (!c) {
    applyPersona({ persona: true, address, userName: "" });
    return;
  }
  await saveJarvisConfig({ ...c, address, persona: address === "None" ? c.persona : true });
}

/** Template answer for `text`, or null (not small talk, templates off, older core). */
export async function matchTemplate(text: string): Promise<JarvisTemplateReply | null> {
  const client = maybeClient();
  if (!client) return null;
  try {
    const lastReply = useJarvisStore.getState().lastReply;
    const r = await client.call("jarvis.match", { text, lang: currentLanguage(), ...(lastReply ? { lastReply } : {}) });
    return r.matched ?? null;
  } catch {
    return null;
  }
}

/** Adds a template / command exchange to the conversation memory (best effort). */
export async function recordExchange(user: string, reply: string, source: "template" | "command"): Promise<void> {
  const client = maybeClient();
  if (!client || !user.trim() || !reply.trim()) return;
  try {
    await client.call("jarvis.record", { user: user.slice(0, 4000), reply: reply.slice(0, 4000), source });
  } catch {
    /* memory is best effort */
  }
}

/** Loads the persisted conversation into the store. */
export async function loadHistory(): Promise<void> {
  const client = maybeClient();
  if (!client) return;
  try {
    const r = await client.call("jarvis.history", { limit: 60 });
    useJarvisStore.getState().setHistory(r.messages);
  } catch {
    /* older core */
  }
}

/** «Забудь разговор»: clears the memory and the view. */
export async function forgetConversation(): Promise<void> {
  useJarvisStore.getState().clear();
  const client = maybeClient();
  if (!client) return;
  await client.call("jarvis.forget", {});
}

export interface ChatReply {
  messageId: string;
  text: string;
  model: string;
  /** Computer command handed off by the model. */
  command: string | null;
}

let seq = 0;
function newMessageId(): string {
  seq += 1;
  return `jv-${Date.now().toString(36)}-${seq}`;
}

/**
 * One AI turn with streaming: `onDelta(delta, reset)` gets the text as it
 * arrives (`JarvisMessage` events); resolves with the whole reply.
 */
export async function chatTurn(
  text: string,
  o: { spoken: boolean; onDelta?: (delta: string, reset: boolean) => void; messageId?: string },
): Promise<ChatReply> {
  const client = getClient();
  const messageId = o.messageId ?? newMessageId();
  const off = o.onDelta
    ? client.subscribe([TOPIC_GLOBAL], (ev: Event) => {
        if (ev.kind !== "JarvisMessage" || ev.payload.messageId !== messageId) return;
        if (ev.payload.reset || ev.payload.delta) o.onDelta?.(ev.payload.delta, ev.payload.reset);
      })
    : () => undefined;
  useJarvisStore.getState().patch({ streamingId: messageId });
  try {
    const r = await client.call("jarvis.chat", { text, lang: currentLanguage(), messageId, spoken: o.spoken }, { timeoutMs: 95_000 });
    return { messageId: r.messageId, text: r.text, model: r.model, command: r.command ?? null };
  } finally {
    off();
    if (useJarvisStore.getState().streamingId === messageId) useJarvisStore.getState().patch({ streamingId: null });
  }
}

/** Stops the reply being streamed. */
export async function cancelChat(): Promise<void> {
  const id = useJarvisStore.getState().streamingId;
  const client = maybeClient();
  if (!id || !client) return;
  try {
    await client.call("jarvis.cancel", { messageId: id });
  } catch {
    /* already finished */
  }
}
