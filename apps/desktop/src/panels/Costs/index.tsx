import { Coins } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "Spend by provider, model, agent, task",
  "Daily/monthly budgets and alerts",
  "Quota and rate-limit state",
  "Model benchmarks (Phase 2)",
] as const;

/** Costs panel — placeholder until Phase 1. */
export default function CostsPanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={Coins}
      title="Costs"
      phase={1}
      summary="Cost & Limit Center: usage, budgets and limits (§22)."
      features={FEATURES}
    />
  );
}
