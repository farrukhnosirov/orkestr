/**
 * Roadmap phase in which each placeholder panel becomes functional — the
 * plan of record is docs/roadmap/feature-traceability.md. Single source for
 * the registry (`PanelDefinition.phase`) and the "Arrives in Phase N" badge,
 * so the two can never disagree again.
 */
export const PLANNED_PHASE = {
  // #17 Visual Browser Agent: no browser tool exists yet; deferred from P3.
  browser: 4,
  // §21: Database is Phase 4. (Deployments shipped in Phase 7, ADR-0041.)
  database: 4,
} as const;

export type PlannedPanel = keyof typeof PLANNED_PHASE;
