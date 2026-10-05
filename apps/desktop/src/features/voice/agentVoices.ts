/**
 * Voice per agent (§30, §33): an agent speaks with its template's
 * `VoiceProfile` (engine, voice, language, rate, pitch, style). Templates
 * and agent → template links are cached from `templates.list` /
 * `agents.get`; `TemplateUpdated`-style changes refresh via `invalidate`.
 * Public API for robots/notifications: `speakAsAgent`.
 */
import type { AgentTemplate, VoiceProfile } from "@orkestr/protocol";

import { maybeClient } from "../../core/client";
import { currentLanguage } from "../../i18n";
import { useVoiceStore } from "./store";
import { optionsForProfile, optionsFromConfig, speak, type SpeakOptions } from "./tts/speaker";

let templates: Map<string, AgentTemplate> | null = null;
let loading: Promise<Map<string, AgentTemplate>> | null = null;
const agentTemplate = new Map<string, string | null>();

export async function loadTemplates(force = false): Promise<Map<string, AgentTemplate>> {
  if (templates && !force) return templates;
  if (loading && !force) return loading;
  const client = maybeClient();
  if (!client) return new Map();
  loading = client.call("templates.list", {}).then(
    (r) => {
      templates = new Map(r.templates.map((tpl) => [tpl.id, tpl]));
      loading = null;
      return templates;
    },
    (e: unknown) => {
      loading = null;
      console.warn("templates.list failed", e);
      return templates ?? new Map<string, AgentTemplate>();
    },
  );
  return loading;
}

export function invalidateAgentVoices(): void {
  templates = null;
  agentTemplate.clear();
}

/** The voice profile of an agent instance (via its template), if any. */
export async function voiceProfileForAgent(agentId: string): Promise<VoiceProfile | null> {
  const client = maybeClient();
  if (!client) return null;
  let tplId = agentTemplate.get(agentId);
  if (tplId === undefined) {
    try {
      const r = await client.call("agents.get", { agentId });
      tplId = r.agent.templateId;
    } catch {
      tplId = null;
    }
    agentTemplate.set(agentId, tplId);
  }
  if (!tplId) return null;
  return voiceProfileForTemplate(tplId);
}

/** A template's effective voice: the user's override (voice config) or the template's own. */
export async function voiceProfileForTemplate(templateId: string): Promise<VoiceProfile | null> {
  const override = useVoiceStore.getState().config?.agentVoices[templateId];
  if (override) return override;
  return (await loadTemplates()).get(templateId)?.voice ?? null;
}

/** Assistant speak options from the current config (null when voice is off). */
export function assistantOptions(): SpeakOptions | null {
  const c = useVoiceStore.getState().config;
  if (!c || !c.enabled) return null;
  return optionsFromConfig(c, currentLanguage());
}

/** Speaks `text` with an agent's voice (falls back to the assistant voice). */
export async function speakAsAgent(agentId: string | null, text: string): Promise<void> {
  const base = assistantOptions();
  if (!base) return;
  const profile = agentId ? await voiceProfileForAgent(agentId) : null;
  await speak(text, optionsForProfile(profile, base));
}
