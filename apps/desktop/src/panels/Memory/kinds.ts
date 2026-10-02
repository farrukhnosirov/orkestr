import type { DecisionStatus, IndexStatus, MemoryKind } from "@orkestr/protocol";
import type { Tone } from "@orkestr/ui";

import { formatRelativeTime, t } from "../../i18n";

export interface KindInfo {
  tone: Tone;
}

/** Every memory kind in display order (exhaustive: checked by the compiler). Labels: `kindLabel()`. */
export const KIND_INFO: Record<MemoryKind, KindInfo> = {
  Requirement: { tone: "accent" },
  Architecture: { tone: "violet" },
  BusinessLogic: { tone: "info" },
  Convention: { tone: "accent" },
  Schema: { tone: "info" },
  Api: { tone: "info" },
  Decision: { tone: "violet" },
  Mistake: { tone: "danger" },
  Todo: { tone: "warning" },
  KnownIssue: { tone: "warning" },
  AgentSummary: { tone: "neutral" },
  Preference: { tone: "violet" },
  QualityStandard: { tone: "success" },
  ProviderPreference: { tone: "neutral" },
  Infrastructure: { tone: "neutral" },
  Note: { tone: "neutral" },
};

/** Localized label of a memory kind (`memory:kinds.*`). */
export function kindLabel(kind: MemoryKind): string {
  return t(`memory:kinds.${kind}`);
}

export const ALL_KINDS = Object.keys(KIND_INFO) as MemoryKind[];

/** Kinds suggested for project memory (§20). */
export const PROJECT_KINDS: MemoryKind[] = [
  "Requirement",
  "Architecture",
  "BusinessLogic",
  "Convention",
  "Schema",
  "Api",
  "Decision",
  "Mistake",
  "Todo",
  "KnownIssue",
  "AgentSummary",
  "Note",
];

/** Kinds suggested for global memory (§20). */
export const GLOBAL_KINDS: MemoryKind[] = [
  "Convention",
  "Architecture",
  "Preference",
  "QualityStandard",
  "ProviderPreference",
  "Infrastructure",
  "Mistake",
  "Note",
];

export const DECISION_STATUS_TONE: Record<DecisionStatus, Tone> = {
  Proposed: "info",
  Accepted: "success",
  Rejected: "danger",
  Superseded: "neutral",
};

export const INDEX_STATUS_TONE: Record<IndexStatus, Tone> = {
  Pending: "neutral",
  Indexing: "info",
  Ready: "success",
  Failed: "danger",
};

/** Parses a comma/space separated tag list. */
export function parseTags(input: string): string[] {
  const out: string[] = [];
  for (const raw of input.split(/[,\n]/)) {
    const tag = raw.trim().replace(/^#/, "");
    if (tag && !out.some((x) => x.toLowerCase() === tag.toLowerCase())) out.push(tag);
  }
  return out;
}

/** Localized relative time ("5 minutes ago" / «5 минут назад»); empty for an invalid timestamp. */
export function relativeTime(iso: string, now: number = Date.now(), lng?: string): string {
  if (Number.isNaN(Date.parse(iso))) return "";
  return formatRelativeTime(iso, now, lng);
}
