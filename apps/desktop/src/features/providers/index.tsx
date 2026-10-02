/**
 * Provider hub UI (§55): "AI Providers" and "AI Manager" settings sections,
 * the reusable <ProviderSetup/> (onboarding embeds it) and palette commands.
 */
import { Plug, Sparkles } from "lucide-react";

import type { AppCommand } from "../commands/registry";
import type { SettingsSection, SettingsSectionProps } from "../settings/sections";
import { ManagerSettings } from "./ManagerSettings";
import { ProviderSetup } from "./ProviderSetup";
import { checkHealth, loadProviders, useProvidersStore } from "./store";

export { ManagerSettings } from "./ManagerSettings";
export { ModelPicker } from "./ModelPicker";
export { ProviderSetup, type ProviderSetupProps } from "./ProviderSetup";
export { useProvidersLive, useProvidersStore } from "./store";

function ProvidersSection(_props: SettingsSectionProps) {
  return (
    <div className="pv">
      <p className="pv__intro">
        Connect AI services with official mechanisms only: API keys (kept in the OS secret vault), the official Claude Code / Codex CLIs with your own
        login, or local endpoints like Ollama. Prices shown are estimates.
      </p>
      <ProviderSetup />
    </div>
  );
}

function ManagerSection(_props: SettingsSectionProps) {
  return <ManagerSettings />;
}

export const providersSettingsSection: SettingsSection = {
  id: "providers",
  title: "AI Providers",
  icon: Plug,
  order: 20,
  component: ProvidersSection,
};

export const managerSettingsSection: SettingsSection = {
  id: "manager",
  title: "AI Manager",
  icon: Sparkles,
  order: 21,
  component: ManagerSection,
};

/** Event the settings view listens to (`detail.section` = section id). */
export const OPEN_SETTINGS_EVENT = "orkestr:open-settings";

function openSettings(section: string): void {
  window.dispatchEvent(new CustomEvent(OPEN_SETTINGS_EVENT, { detail: { section } }));
}

export const providerCommands: AppCommand[] = [
  {
    id: "providers.open",
    title: "Open AI Providers settings",
    group: "AI Providers",
    icon: Plug,
    keywords: ["provider", "api key", "openai", "anthropic", "claude", "codex", "gemini", "ollama", "deepseek", "провайдер"],
    run: () => openSettings("providers"),
  },
  {
    id: "providers.manager",
    title: "Open AI Manager settings",
    group: "AI Providers",
    icon: Sparkles,
    keywords: ["manager", "model", "routing", "fallback", "менеджер"],
    run: () => openSettings("manager"),
  },
  {
    id: "providers.checkAll",
    title: "Check health of all AI providers",
    group: "AI Providers",
    icon: Plug,
    keywords: ["health", "status", "test"],
    run: async () => {
      if (!useProvidersStore.getState().loaded) await loadProviders();
      await Promise.allSettled(
        useProvidersStore
          .getState()
          .providers.filter((p) => p.provider.enabled)
          .map((p) => checkHealth(p.provider.id)),
      );
    },
  },
];
