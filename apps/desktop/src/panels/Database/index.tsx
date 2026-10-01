import { Database } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "Connections per environment",
  "Schema browser and queries",
  "DROP/TRUNCATE always require approval",
  "Read-only by default",
] as const;

/** Database panel — placeholder until Phase 2. */
export default function DatabasePanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={Database}
      title="Database"
      phase={2}
      summary="Database explorer with approval-gated writes (§23)."
      features={FEATURES}
    />
  );
}
