import { Rocket } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import { PLANNED_PHASE } from "../plannedPhases";
import type { PanelProps } from "../types";

/** Deployments panel — placeholder until its planned phase (texts: `placeholders:deployments`). */
export default function DeploymentsPanel(_props: PanelProps) {
  return <PhasePlaceholder icon={Rocket} panel="deployments" phase={PLANNED_PHASE.deployments} />;
}
