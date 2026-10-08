/**
 * Pure helpers for the Extensions panel (unit tested).
 */
import type { ContributionSummary } from "@orkestr/protocol";

import { compareVersions } from "../vscode/logic";

/** One "works in Orkestr" line: a translation key + count + detail. */
export interface SupportedItem {
  key: "themes" | "iconThemes" | "grammars" | "languages" | "snippets" | "languageServers";
  count: number;
  detail: string;
}

/** Declarative contributions the Orkestr editor applies. */
export function supportedItems(c: ContributionSummary): SupportedItem[] {
  const uniq = (xs: readonly (string | null)[]) => [...new Set(xs.filter((x): x is string => Boolean(x)))];
  const items: SupportedItem[] = [
    { key: "themes", count: c.themes.length, detail: c.themes.map((t) => t.label).join(", ") },
    { key: "iconThemes", count: c.iconThemes.length, detail: c.iconThemes.map((t) => t.label).join(", ") },
    { key: "grammars", count: c.grammars.length, detail: uniq(c.grammars.map((g) => g.language)).join(", ") },
    { key: "languages", count: c.languages.length, detail: c.languages.map((l) => l.aliases[0] ?? l.id).join(", ") },
    { key: "snippets", count: c.snippets.length, detail: uniq(c.snippets.map((s) => s.language)).join(", ") },
    { key: "languageServers", count: c.languageServers.length, detail: c.languageServers.join(", ") },
  ];
  return items.filter((i) => i.count > 0);
}

/** Contribution points that need the full VS Code extension host (excluding the code itself). */
export function unsupportedKinds(c: ContributionSummary): string[] {
  return c.unsupported.filter((u) => u.kind !== "activation").map((u) => u.kind);
}

/** Overall verdict for a badge. */
export type Support = "full" | "partial" | "none";

export function supportLevel(c: ContributionSummary): Support {
  const ok = supportedItems(c).length > 0;
  const missing = unsupportedKinds(c).length > 0 || c.hasCode;
  if (ok && !missing) return "full";
  if (ok) return "partial";
  return "none";
}


/** Whether `latest` is a newer release than `installed` (numeric, segment-wise). */
export function isNewerVersion(latest: string | null | undefined, installed: string | null | undefined): boolean {
  return Boolean(latest && installed && compareVersions(latest, installed) > 0);
}
