import { Globe } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "Open URLs and record flows",
  "Screenshots and responsive checks",
  "Console and network errors",
  "Agent-driven E2E testing",
] as const;

/** Browser panel — placeholder until Phase 2. */
export default function BrowserPanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={Globe}
      title="Browser"
      phase={2}
      summary="Browser agent for UI checks and user-flow tests (§18)."
      features={FEATURES}
    />
  );
}
