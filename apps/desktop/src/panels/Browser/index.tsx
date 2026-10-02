import { Globe } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

/** Browser panel — placeholder until Phase 2 (texts: `placeholders:browser`). */
export default function BrowserPanel(_props: PanelProps) {
  return <PhasePlaceholder icon={Globe} panel="browser" phase={2} />;
}
