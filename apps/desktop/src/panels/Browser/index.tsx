import { Globe } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import { PLANNED_PHASE } from "../plannedPhases";
import type { PanelProps } from "../types";

/** Browser panel — placeholder until its planned phase (texts: `placeholders:browser`). */
export default function BrowserPanel(_props: PanelProps) {
  return <PhasePlaceholder icon={Globe} panel="browser" phase={PLANNED_PHASE.browser} />;
}
