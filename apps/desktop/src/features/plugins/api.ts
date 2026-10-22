/**
 * Plugin host RPC (`plugins.*`, ADR-0034) used by the plugin management UI.
 * Wire types come from `@orkestr/protocol`.
 */
import type {
  GrantPluginParams,
  InstallPluginParams,
  InstallPluginResult,
  KillAllPluginsResult,
  ListPluginsResult,
  PluginInfo,
  PluginLogLine,
} from "@orkestr/protocol";

import { getClient } from "../../core/client";

export type { InstallPluginResult, PluginInfo, PluginLogLine };

type LooseCall = (method: string, params: unknown) => Promise<unknown>;

function call<T>(method: string, params: unknown): Promise<T> {
  const client = getClient() as unknown as { call: LooseCall };
  return client.call(method, params) as Promise<T>;
}

export const pluginsApi = {
  list: () => call<ListPluginsResult>("plugins.list", {}),
  get: (pluginId: string) =>
    call<{ plugin: PluginInfo }>("plugins.get", { pluginId }).then(
      (r) => r.plugin,
    ),
  install: (p: InstallPluginParams) =>
    call<InstallPluginResult>("plugins.install", p),
  grant: (p: GrantPluginParams) =>
    call<{ plugin: PluginInfo }>("plugins.grant", p).then((r) => r.plugin),
  enable: (pluginId: string) =>
    call<{ plugin: PluginInfo }>("plugins.enable", {
      pluginId,
      reason: "user",
    }).then((r) => r.plugin),
  disable: (pluginId: string) =>
    call<{ plugin: PluginInfo }>("plugins.disable", {
      pluginId,
      reason: "user",
    }).then((r) => r.plugin),
  uninstall: (pluginId: string) =>
    call<object>("plugins.uninstall", { pluginId }),
  killAll: () =>
    call<KillAllPluginsResult>("plugins.killAll", {
      reason: "kill switch (plugins UI)",
    }),
  setSafeMode: (enabled: boolean) =>
    call<{ safeMode: boolean }>("plugins.setSafeMode", { enabled }).then(
      (r) => r.safeMode,
    ),
  logs: (pluginId: string, limit = 200) =>
    call<{ lines: PluginLogLine[] }>("plugins.logs", { pluginId, limit }).then(
      (r) => r.lines,
    ),
};
