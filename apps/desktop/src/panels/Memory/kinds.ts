import type { DecisionStatus, IndexStatus, MemoryKind } from "@orkestr/protocol";
import type { Tone } from "@orkestr/ui";

export interface KindInfo {
  kind: MemoryKind;
  label: string;
  tone: Tone;
}

/** Every memory kind in display order (exhaustive: checked by the compiler). */
export const KIND_INFO: Record<MemoryKind, Omit<KindInfo, "kind">> = {
  Requirement: { label: "Requirement", tone: "accent" },
  Architecture: { label: "Architecture", tone: "violet" },
  BusinessLogic: { label: "Business logic", tone: "info" },
  Convention: { label: "Convention", tone: "accent" },
  Schema: { label: "Schema", tone: "info" },
  Api: { label: "API", tone: "info" },
  Decision: { label: "Decision", tone: "violet" },
  Mistake: { label: "Mistake", tone: "danger" },
  Todo: { label: "TODO", tone: "warning" },
  KnownIssue: { label: "Known issue", tone: "warning" },
  AgentSummary: { label: "Agent summary", tone: "neutral" },
  Preference: { label: "Preference", tone: "violet" },
  QualityStandard: { label: "Quality standard", tone: "success" },
  ProviderPreference: { label: "Provider preference", tone: "neutral" },
  Infrastructure: { label: "Infrastructure", tone: "neutral" },
  Note: { label: "Note", tone: "neutral" },
};

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
    const t = raw.trim().replace(/^#/, "");
    if (t && !out.some((x) => x.toLowerCase() === t.toLowerCase())) out.push(t);
  }
  return out;
}

/** Short relative time ("3m ago", "2d ago") or a date. */
export function relativeTime(iso: string, now: number = Date.now()): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const s = Math.max(0, Math.round((now - t) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)}d ago`;
  return new Date(t).toLocaleDateString();
}
