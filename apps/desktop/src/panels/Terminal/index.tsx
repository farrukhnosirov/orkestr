import { SquareTerminal } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "User- and agent-owned terminals",
  "Streaming over the event bus",
  "History and search",
  "Approval interception for dangerous commands",
] as const;

/** Terminal panel — placeholder until Phase 1. */
export default function TerminalPanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={SquareTerminal}
      title="Terminal"
      phase={1}
      summary="Multiple PTY sessions — PowerShell, cmd, Git Bash, WSL, bash, zsh, fish (§15)."
      features={FEATURES}
    />
  );
}
