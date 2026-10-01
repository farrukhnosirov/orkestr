import { Badge, EmptyState } from "@orkestr/ui";
import type { ReactNode } from "react";

import { useActiveProject } from "../store/app";
import type { PanelIcon } from "./types";

export interface PhasePlaceholderProps {
  icon: PanelIcon;
  title: string;
  phase: number;
  summary: ReactNode;
  features: readonly string[];
}

/** Honest "not built yet" state for panels that arrive in a later phase. */
export function PhasePlaceholder({ icon: Icon, title, phase, summary, features }: PhasePlaceholderProps) {
  const project = useActiveProject();
  return (
    <div className="phase-placeholder">
      <EmptyState
        icon={<Icon size={28} strokeWidth={1.6} />}
        badge={
          <Badge tone="violet" className="phase-placeholder__badge">
            Arrives in Phase {phase}
          </Badge>
        }
        title={title}
        description={
          <>
            {summary}
            {project && (
              <span className="phase-placeholder__project">
                Will operate on <strong>{project.name}</strong>.
              </span>
            )}
          </>
        }
      />
      <ul className="phase-placeholder__features" aria-label={`${title} planned features`}>
        {features.map((f) => (
          <li key={f}>{f}</li>
        ))}
      </ul>
    </div>
  );
}
