import { GitBranch } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "Status, staging and commits (never automatic)",
  "Branches and worktrees per agent",
  "Semantic diff and conflict detection",
  "AI Merge Manager proposals",
] as const;

/** Git panel — placeholder until Phase 1. */
export default function GitPanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={GitBranch}
      title="Git"
      phase={1}
      summary="Git status, diffs, branches and per-agent worktrees (§16)."
      features={FEATURES}
    />
  );
}
