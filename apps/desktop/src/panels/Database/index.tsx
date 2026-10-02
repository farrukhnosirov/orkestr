import { Database } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

/** Database panel — placeholder until Phase 2 (texts: `placeholders:database`). */
export default function DatabasePanel(_props: PanelProps) {
  return <PhasePlaceholder icon={Database} panel="database" phase={2} />;
}
