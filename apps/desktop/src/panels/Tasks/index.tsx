import { ListTodo } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "Board by status with priorities",
  "Dependency graph",
  "Execution history, cost and artifacts",
  "Retry, cancel and rollback",
] as const;

/** Tasks panel — placeholder until Phase 1. */
export default function TasksPanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={ListTodo}
      title="Tasks"
      phase={1}
      summary="Task board and DAG with the 12-state task machine (§10–11)."
      features={FEATURES}
    />
  );
}
