import { Activity } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "CPU, RAM, GPU, disk per node",
  "Provider health and limits",
  "Builds, deployments and alerts",
  "Bug Detective",
] as const;

/** Monitoring panel — placeholder until Phase 3. */
export default function MonitoringPanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={Activity}
      title="Monitoring"
      phase={3}
      summary="Mission Control and infrastructure monitor (§27, §32)."
      features={FEATURES}
    />
  );
}
