/**
 * Project AI privacy (§56): the per-project provider policy and the data
 * classification that tightens it. Mirrors `DataClassification::tighten` in
 * `orkestr-domain` — the core enforces it; the UI only explains it.
 */
import type { AiPolicy, DataClassification } from "@orkestr/protocol";

export type AiMode = AiPolicy["mode"];

export const AI_MODES: readonly AiMode[] = ["CloudAllowed", "SelectedProvidersOnly", "LocalModelsOnly"];
export const CLASSIFICATIONS: readonly DataClassification[] = ["Public", "Internal", "Confidential", "Secret"];

/** The AI policy in force after the classification floor. */
export function effectiveAiPolicy(classification: DataClassification, ai: AiPolicy): AiPolicy {
  if (classification === "Secret") return { mode: "LocalModelsOnly" };
  if (classification === "Confidential" && ai.mode === "CloudAllowed") return { mode: "LocalModelsOnly" };
  return ai;
}

/** The AI policy for a newly picked mode, keeping the provider list when one exists. */
export function policyForMode(mode: AiMode, current: AiPolicy): AiPolicy {
  if (mode === "SelectedProvidersOnly") {
    return { mode, providers: current.mode === "SelectedProvidersOnly" ? current.providers : [] };
  }
  return { mode };
}

/** Toggles one provider in a `SelectedProvidersOnly` list. */
export function toggleProvider(ai: AiPolicy, providerId: string, on: boolean): AiPolicy {
  const list = ai.mode === "SelectedProvidersOnly" ? ai.providers : [];
  const next = on ? [...list.filter((p) => p !== providerId), providerId] : list.filter((p) => p !== providerId);
  return { mode: "SelectedProvidersOnly", providers: next };
}
