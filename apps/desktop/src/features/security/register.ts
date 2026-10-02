/**
 * Registers the security feature in the shared registries (settings
 * sections + commands). Imported once for its side effects.
 */
import { Inbox, KeyRound, Lock, ShieldCheck } from "lucide-react";

import { maybeClient } from "../../core/client";
import { commands } from "../commands/registry";
import { settingsSections } from "../settings/sections";
import { PermissionsSettings } from "./PermissionsSettings";
import { SecretsSettings } from "./SecretsSettings";
import { refreshFlags, useSecurityStore } from "./store";

let registered = false;

export function registerSecurityFeature(): void {
  if (registered) return;
  registered = true;
  settingsSections.push(
    { id: "security", title: "Security & Permissions", icon: ShieldCheck, order: 40, component: PermissionsSettings },
    { id: "secrets", title: "Secrets Vault", icon: KeyRound, order: 41, component: SecretsSettings },
  );
  commands.push(
    {
      id: "security.openApprovals",
      title: "Open Approvals",
      group: "Safety",
      icon: Inbox,
      keywords: ["inbox", "approve", "pending", "review"],
      run: () => useSecurityStore.getState().setInboxOpen(true),
    },
    {
      id: "security.openSettings",
      title: "Security & Permissions…",
      group: "Safety",
      icon: ShieldCheck,
      keywords: ["approval mode", "rules", "full auto", "semi auto"],
      run: () => useSecurityStore.getState().openCenter("permissions"),
    },
    {
      id: "security.openSecrets",
      title: "Secrets Vault…",
      group: "Safety",
      icon: KeyRound,
      keywords: ["api key", "token", "password", "credentials"],
      run: () => useSecurityStore.getState().openCenter("secrets"),
    },
    {
      id: "security.lockVault",
      title: "Lock Vault",
      group: "Safety",
      icon: Lock,
      keywords: ["secrets", "emergency"],
      enabled: () => maybeClient() !== null,
      run: async () => {
        const c = maybeClient();
        if (!c) return;
        await c.call("secrets.lock", {});
        await refreshFlags();
        useSecurityStore.getState().pushToast({ title: "Vault locked", body: "Secret access is denied until you unlock it.", tone: "info" });
      },
    },
  );
}
