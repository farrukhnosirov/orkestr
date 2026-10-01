import { Bot } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "Spawn agents from templates",
  "Live state: thinking, coding, testing, reviewing",
  "Budgets, turns and models per agent",
  "Emergency stop per agent",
] as const;

/** Agents panel — placeholder until Phase 1. */
export default function AgentsPanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={Bot}
      title="Agents"
      phase={1}
      summary="Agent templates, live instances and teams (§6)."
      features={FEATURES}
    />
  );
}
