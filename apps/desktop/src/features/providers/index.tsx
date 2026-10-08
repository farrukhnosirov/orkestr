/**
 * Provider hub UI (§55): "AI Providers" and "AI Manager" settings sections,
 * the reusable <ProviderSetup/> (onboarding embeds it) and palette commands.
 */
import { Plug, Sparkles } from "lucide-react";

import { t, useT } from "../../i18n";
import type { AppCommand } from "../commands/registry";
import type { SettingsSection, SettingsSectionProps } from "../settings/sections";
import { useSettingsStore } from "../settings/settingsStore";
import { ClaudeEngineCard } from "../claudeEngine/ClaudeEngineCard";
import { ManagerSettings } from "./ManagerSettings";
import { ProviderSetup } from "./ProviderSetup";
import { checkHealth, loadProviders, useProvidersStore } from "./store";

export { ManagerSettings } from "./ManagerSettings";
export { ModelPicker } from "./ModelPicker";
export { ProviderSetup, type ProviderSetupProps } from "./ProviderSetup";
export { useProvidersLive, useProvidersStore } from "./store";
export { instanceName } from "./logic";

function ProvidersSection(_props: SettingsSectionProps) {
  const { t: tr } = useT("providers");
  return (
    <div className="pv">
      <p className="pv__intro">{tr("intro")}</p>
      <ClaudeEngineCard onApplied={() => void loadProviders()} />
      <ProviderSetup />
    </div>
  );
}

function ManagerSection(_props: SettingsSectionProps) {
  return <ManagerSettings />;
}

export const providersSettingsSection: SettingsSection = {
  id: "providers",
  get title() {
    return t("providers:sections.providers");
  },
  icon: Plug,
  order: 20,
  component: ProvidersSection,
};

export const managerSettingsSection: SettingsSection = {
  id: "manager",
  get title() {
    return t("providers:sections.manager");
  },
  icon: Sparkles,
  order: 21,
  component: ManagerSection,
};

function openSettings(section: string): void {
  useSettingsStore.getState().openSettings(section);
}

export const providerCommands: AppCommand[] = [
  {
    id: "providers.open",
    get title() {
      return t("providers:commands.open");
    },
    get group() {
      return t("providers:commands.group");
    },
    icon: Plug,
    keywords: ["provider", "api key", "openai", "anthropic", "claude", "codex", "gemini", "ollama", "deepseek", "провайдер", "ключ", "подключить"],
    run: () => openSettings("providers"),
  },
  {
    id: "providers.manager",
    get title() {
      return t("providers:commands.manager");
    },
    get group() {
      return t("providers:commands.group");
    },
    icon: Sparkles,
    keywords: ["manager", "model", "routing", "fallback", "менеджер", "модель", "маршрутизация", "резервная"],
    run: () => openSettings("manager"),
  },
  {
    id: "providers.checkAll",
    get title() {
      return t("providers:commands.checkAll");
    },
    get group() {
      return t("providers:commands.group");
    },
    icon: Plug,
    keywords: ["health", "status", "test", "проверить", "доступность", "провайдеры"],
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
