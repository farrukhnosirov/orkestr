import { Database } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import { PLANNED_PHASE } from "../plannedPhases";
import type { PanelProps } from "../types";

/** Database panel — placeholder until its planned phase (texts: `placeholders:database`). */
export default function DatabasePanel(_props: PanelProps) {
  return <PhasePlaceholder icon={Database} panel="database" phase={PLANNED_PHASE.database} />;
}
