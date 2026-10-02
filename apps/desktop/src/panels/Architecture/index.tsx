import { Network } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

/** Architecture panel — placeholder until Phase 2 (texts: `placeholders:architecture`). */
export default function ArchitecturePanel(_props: PanelProps) {
  return <PhasePlaceholder icon={Network} panel="architecture" phase={2} />;
}
