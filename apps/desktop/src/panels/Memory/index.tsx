import { BrainCircuit } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "Requirements, architecture, conventions",
  "Mistakes and known issues",
  "Knowledge sources (Markdown, PDF, docs)",
  "Full-text search",
] as const;

/** Memory panel — placeholder until Phase 1. */
export default function MemoryPanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={BrainCircuit}
      title="Memory"
      phase={1}
      summary="Project and global memory, knowledge base and conventions (§20)."
      features={FEATURES}
    />
  );
}
