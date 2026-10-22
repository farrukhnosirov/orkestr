/** Pure helpers of the plugin management UI (no RPC, no React). */
import type { PluginInfo } from "./api";

export type PluginStateName = PluginInfo["plugin"]["state"];

export function stateTone(
  state: PluginStateName,
): "success" | "neutral" | "warning" | "danger" | "info" {
  switch (state) {
    case "Enabled":
      return "success";
    case "Installed":
      return "info";
    case "Disabled":
      return "neutral";
    case "Killed":
      return "danger";
    default:
      return "warning";
  }
}

/** Which primary toggle a plugin row offers. */
export function toggleAction(p: PluginInfo): "enable" | "disable" | "decide" {
  if (p.plugin.state === "Enabled") return "disable";
  if (p.pendingGrants.length > 0) return "decide";
  return "enable";
}

/** Key of a grant / request for list rendering and matching. */
export function grantKey(g: { capability: string; scope?: string }): string {
  return `${g.capability}:${g.scope ?? ""}`;
}

/** Number of plugins whose permissions are still undecided. */
export function pendingCount(plugins: PluginInfo[]): number {
  return plugins.filter((p) => p.pendingGrants.length > 0).length;
}

/** Sorts by name; plugins awaiting a decision first. */
export function sortPlugins(plugins: PluginInfo[]): PluginInfo[] {
  return [...plugins].sort((a, b) => {
    const pa = a.pendingGrants.length > 0 ? 0 : 1;
    const pb = b.pendingGrants.length > 0 ? 0 : 1;
    return pa - pb || a.plugin.name.localeCompare(b.plugin.name);
  });
}

/** Locale key of the state badge (`Installed` without open requests = ready to enable). */
export function stateKey(p: PluginInfo): PluginStateName | "Ready" {
  return p.plugin.state === "Installed" && p.pendingGrants.length === 0
    ? "Ready"
    : p.plugin.state;
}

/** Groups a bare hex fingerprint into 4-character blocks (already grouped ones pass through). */
export function groupFingerprint(fp: string): string {
  const s = fp.trim();
  if (/\s/.test(s)) return s;
  return (s.match(/.{1,4}/g) ?? []).join(" ");
}
