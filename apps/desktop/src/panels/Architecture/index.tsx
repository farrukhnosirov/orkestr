import { Network } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "Modules, services, databases, APIs",
  "ADR-style decision log",
  "Execution replay",
  "Links to tasks and files",
] as const;

/** Architecture panel — placeholder until Phase 2. */
export default function ArchitecturePanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={Network}
      title="Architecture"
      phase={2}
      summary="Architecture map and decision log (§21)."
      features={FEATURES}
    />
  );
}
