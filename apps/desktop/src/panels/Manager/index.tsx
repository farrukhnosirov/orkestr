import { Sparkles } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "Chat with the Manager in natural language",
  "Plans with tasks, dependencies and acceptance criteria",
  "Model, budget and approval policy per run",
  "Live streaming of Manager messages",
] as const;

/** Manager panel — placeholder until Phase 1. */
export default function ManagerPanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={Sparkles}
      title="Manager"
      phase={1}
      summary="The AI Manager plans, decomposes and delegates your goals to a team of agents (§1, §5)."
      features={FEATURES}
    />
  );
}
