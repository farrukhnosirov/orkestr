import { Rocket } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

/** Deployments panel — placeholder until Phase 2 (texts: `placeholders:deployments`). */
export default function DeploymentsPanel(_props: PanelProps) {
  return <PhasePlaceholder icon={Rocket} panel="deployments" phase={2} />;
}
