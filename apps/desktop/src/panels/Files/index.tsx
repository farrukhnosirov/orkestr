import { FolderTree } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "Tree with git status decorations",
  "Create, rename, move, delete with approvals",
  "Quick open and search",
  "Agent change markers",
] as const;

/** Files panel — placeholder until Phase 1. */
export default function FilesPanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={FolderTree}
      title="Files"
      phase={1}
      summary="Project file explorer confined to the project root (§14)."
      features={FEATURES}
    />
  );
}
