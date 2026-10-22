/** Pure marketplace helpers (no RPC, no React). */
import type {
  EntryDetails,
  GrantDecision,
  MarketEntry,
  PackageKind,
  PermissionRequest,
  PublisherKey,
} from "./api";

export const PACKAGE_KINDS: readonly PackageKind[] = [
  "AgentTemplate",
  "Plugin",
  "Widget",
];

/** Semver-ish comparison (numeric parts, then lexical pre-release). */
export function compareVersions(a: string, b: string): number {
  const split = (v: string) => {
    const [core = "", pre = ""] = v.split("-", 2);
    return {
      nums: core.split(".").map((n) => Number.parseInt(n, 10) || 0),
      pre,
    };
  };
  const x = split(a);
  const y = split(b);
  for (let i = 0; i < Math.max(x.nums.length, y.nums.length); i += 1) {
    const d = (x.nums[i] ?? 0) - (y.nums[i] ?? 0);
    if (d !== 0) return Math.sign(d);
  }
  if (x.pre === y.pre) return 0;
  if (!x.pre) return 1;
  if (!y.pre) return -1;
  return x.pre < y.pre ? -1 : 1;
}

export type EntryAction = "install" | "update" | "installed" | "blocked";

/** What the primary button of an entry does. */
export function entryAction(entry: MarketEntry): EntryAction {
  if (entry.revoked) return "blocked";
  if (!entry.installedVersion) return "install";
  return compareVersions(entry.version, entry.installedVersion) > 0
    ? "update"
    : "installed";
}

/** Why an install cannot proceed without user action. */
export type InstallGate = "revoked" | "keyRevoked" | "untrusted" | null;

export function installGate(d: EntryDetails): InstallGate {
  if (d.entry.revoked) return "revoked";
  if (d.key.trust === "Revoked") return "keyRevoked";
  if (d.key.trust === "Untrusted") return "untrusted";
  return null;
}

/** Splits a fingerprint (`ab12 cd34 …`) into two rows for display. */
export function fingerprintRows(fp: string): string[] {
  const groups = fp.trim().split(/\s+/).filter(Boolean);
  const half = Math.ceil(groups.length / 2);
  return [groups.slice(0, half).join(" "), groups.slice(half).join(" ")].filter(
    Boolean,
  );
}

/** Initial decisions of the permission prompt: nothing allowed until the user opts in. */
export function initialDecisions(
  pending: PermissionRequest[],
): GrantDecision[] {
  return pending.map((p) => ({
    capability: p.capability,
    scope: p.scope,
    allow: false,
  }));
}

export function trustTone(
  trust: PublisherKey["trust"],
): "success" | "info" | "warning" | "danger" {
  switch (trust) {
    case "Official":
      return "info";
    case "Trusted":
      return "success";
    case "Revoked":
      return "danger";
    default:
      return "warning";
  }
}

/** Distinct tags of a result set (facet chips), sorted. */
export function collectTags(entries: MarketEntry[]): string[] {
  return [...new Set(entries.flatMap((e) => e.tags ?? []))].sort();
}
