import { Rocket } from "lucide-react";

import { PhasePlaceholder } from "../PhasePlaceholder";
import type { PanelProps } from "../types";

const FEATURES = [
  "Local, Dev, Test, Staging, Production",
  "Docker, Compose, SSH, Vercel, Cloudflare, clouds",
  "Production always requires approval",
  "Rollback history",
] as const;

/** Deployments panel — placeholder until Phase 2. */
export default function DeploymentsPanel(_props: PanelProps) {
  return (
    <PhasePlaceholder
      icon={Rocket}
      title="Deployments"
      phase={2}
      summary="Environments and deployments with an audit trail (§26)."
      features={FEATURES}
    />
  );
}
