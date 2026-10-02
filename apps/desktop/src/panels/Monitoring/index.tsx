import { Activity } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

/** Monitoring panel — placeholder until Phase 3 (texts: `placeholders:monitoring`). */
export default function MonitoringPanel(_props: PanelProps) {
  return <PhasePlaceholder icon={Activity} panel="monitoring" phase={3} />;
}
