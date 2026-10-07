/**
 * Registers the security feature in the shared registries (settings
 * sections + commands). Imported once for its side effects.
 */
import { Inbox, KeyRound, Lock, ShieldCheck } from "lucide-react";

import { maybeClient } from "../../core/client";
import { t } from "../../i18n";
import { commands } from "../commands/registry";
import { settingsSections } from "../settings/sections";
import { useSettingsStore } from "../settings/settingsStore";
import { PermissionsSettings } from "./PermissionsSettings";
import { PrivacySettings } from "./PrivacySettings";
import { SecretsSettings } from "./SecretsSettings";
import { refreshFlags, useSecurityStore } from "./store";

let registered = false;

export function registerSecurityFeature(): void {
  if (registered) return;
  registered = true;
  settingsSections.push(
    {
      id: "security",
      get title() {
        return t("security:sections.permissions");
      },
      icon: ShieldCheck,
      order: 40,
      component: PermissionsSettings,
    },
    {
      id: "secrets",
      get title() {
        return t("security:sections.secrets");
      },
      icon: KeyRound,
      order: 41,
      component: SecretsSettings,
    },
    {
      id: "privacy",
      get title() {
        return t("security:sections.privacy");
      },
      icon: Lock,
      order: 42,
      component: PrivacySettings,
    },
  );
  commands.push(
    {
      id: "security.openApprovals",
      get title() {
        return t("security:commands.openApprovals");
      },
      get group() {
        return t("shell:commandGroups.safety");
      },
      icon: Inbox,
      keywords: ["inbox", "approve", "pending", "review", "подтверждение", "запрос", "одобрить", "входящие"],
      run: () => useSecurityStore.getState().setInboxOpen(true),
    },
    {
      id: "security.openSettings",
      get title() {
        return t("security:commands.openSettings");
      },
      get group() {
        return t("shell:commandGroups.safety");
      },
      icon: ShieldCheck,
      keywords: ["approval mode", "rules", "full auto", "semi auto", "безопасность", "разрешения", "правила", "режим подтверждений", "полный автомат", "полуавтомат"],
      run: () => useSecurityStore.getState().openCenter("permissions"),
    },
    {
      id: "security.openPrivacy",
      get title() {
        return t("security:commands.openPrivacy");
      },
      get group() {
        return t("shell:commandGroups.safety");
      },
      icon: Lock,
      keywords: ["privacy", "local models", "cloud", "classification", "confidential", "конфиденциальность", "локальные модели", "облако", "классификация", "секретные"],
      run: () => useSettingsStore.getState().openSettings("privacy"),
    },
    {
      id: "security.openSecrets",
      get title() {
        return t("security:commands.openSecrets");
      },
      get group() {
        return t("shell:commandGroups.safety");
      },
      icon: KeyRound,
      keywords: ["api key", "token", "password", "credentials", "секреты", "хранилище", "ключ", "токен", "пароль"],
      run: () => useSecurityStore.getState().openCenter("secrets"),
    },
    {
      id: "security.lockVault",
      get title() {
        return t("security:commands.lockVault");
      },
      get group() {
        return t("shell:commandGroups.safety");
      },
      icon: Lock,
      keywords: ["secrets", "emergency", "lock", "заблокировать", "хранилище", "секреты"],
      enabled: () => maybeClient() !== null,
      run: async () => {
        const c = maybeClient();
        if (!c) return;
        await c.call("secrets.lock", {});
        await refreshFlags();
        useSecurityStore
          .getState()
          .pushToast({ title: t("security:toast.vaultLockedTitle"), body: t("security:toast.vaultLockedBody"), tone: "info" });
      },
    },
  );
}
