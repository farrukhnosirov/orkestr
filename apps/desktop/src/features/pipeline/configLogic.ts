/**
 * Quality pipeline settings (`pipeline.getConfig` / `pipeline.setConfig`):
 * pure helpers mirroring the core's validation (orchestrator pipeline.rs
 * `set_pipeline_config`). Unit-tested.
 */
import type { HealingLimits, PipelineKindOverride, PipelineStageKind } from "@orkestr/protocol";

/** Stages in pipeline order. Author always runs; Merge follows the merge mode. */
export const STAGE_ORDER: readonly PipelineStageKind[] = ["Author", "Review", "Build", "Test", "Security", "Merge"];
/** Stages the user switches on and off. */
export const TOGGLE_STAGES: readonly PipelineStageKind[] = ["Review", "Build", "Test", "Security"];

/** Core limits (`MAX_HEALING_ATTEMPTS`, `MAX_PIPELINE_MINS`, override rules). */
export const MAX_ATTEMPTS = 10;
export const MAX_WALL_MINS = 1440;
export const MAX_OVERRIDES = 32;
export const MAX_KIND_CHARS = 60;

/** `stages` with `kind` switched on/off, in pipeline order (Author kept). */
export function toggleStage(stages: readonly PipelineStageKind[], kind: PipelineStageKind, on: boolean): PipelineStageKind[] {
  const set = new Set(stages);
  if (on) set.add(kind);
  else set.delete(kind);
  set.add("Author");
  return STAGE_ORDER.filter((s) => set.has(s));
}

export interface HealingForm {
  maxAttempts: string;
  maxWallMins: string;
  /** Empty = no spend limit. */
  maxBudgetUsd: string;
  stopOnRepeat: boolean;
  escalateModel: boolean;
}

export type HealingError = "attempts" | "wallMins" | "budget";

export function healingForm(h: HealingLimits): HealingForm {
  return {
    maxAttempts: String(h.maxAttempts),
    maxWallMins: String(h.maxWallMins),
    maxBudgetUsd: h.maxBudgetUsd === undefined ? "" : String(h.maxBudgetUsd),
    stopOnRepeat: h.stopOnRepeat,
    escalateModel: h.escalateModel,
  };
}

function wholeNumber(s: string): number | null {
  const v = s.trim();
  return /^\d+$/.test(v) ? Number(v) : null;
}

/** The limits to save, or the first invalid field. */
export function parseHealing(f: HealingForm): { limits: HealingLimits } | { error: HealingError } {
  const attempts = wholeNumber(f.maxAttempts);
  if (attempts === null || attempts > MAX_ATTEMPTS) return { error: "attempts" };
  const mins = wholeNumber(f.maxWallMins);
  if (mins === null || mins < 1 || mins > MAX_WALL_MINS) return { error: "wallMins" };
  const budgetText = f.maxBudgetUsd.trim().replace(",", ".");
  let budget: number | undefined;
  if (budgetText) {
    if (!/^\d+(\.\d+)?$/.test(budgetText)) return { error: "budget" };
    budget = Number(budgetText);
    if (!Number.isFinite(budget) || budget < 0) return { error: "budget" };
  }
  const limits: HealingLimits = { maxAttempts: attempts, maxWallMins: mins, stopOnRepeat: f.stopOnRepeat, escalateModel: f.escalateModel };
  if (budget !== undefined) limits.maxBudgetUsd = budget;
  return { limits };
}

export type OverrideError = "kindRequired" | "kindTooLong" | "duplicate" | "tooMany";

/** Adds an override for `kind` (project stages as the starting set), or says why not. */
export function addOverride(
  overrides: readonly PipelineKindOverride[],
  kind: string,
  stages: readonly PipelineStageKind[],
): { overrides: PipelineKindOverride[] } | { error: OverrideError } {
  const k = kind.trim();
  if (!k) return { error: "kindRequired" };
  if ([...k].length > MAX_KIND_CHARS) return { error: "kindTooLong" };
  if (overrides.some((o) => o.kind.toLowerCase() === k.toLowerCase())) return { error: "duplicate" };
  if (overrides.length >= MAX_OVERRIDES) return { error: "tooMany" };
  return { overrides: [...overrides, { kind: k, stages: toggleStage(stages, "Author", true) }] };
}
