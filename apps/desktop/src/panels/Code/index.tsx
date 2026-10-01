import { FileCode } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "Tabs, syntax highlighting, search",
  "LSP / IntelliSense and diagnostics",
  "Inline diff of agent changes",
  "Open in external IDE",
] as const;

/** Code panel — placeholder until Phase 1. */
export default function CodePanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={FileCode}
      title="Code"
      phase={1}
      summary="Built-in Monaco editor with diagnostics, diff and agent change markers (§15)."
      features={FEATURES}
    />
  );
}
