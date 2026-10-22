/** Small shared pieces of the Deployments panel. */
import type { AdapterInfo, DeploymentStatus } from "@orkestr/protocol";
import { Badge } from "@orkestr/ui";
import { useCallback } from "react";

import { useT } from "../../i18n";
import { availabilityTone, isKnownAdapter, statusTone } from "./logic";

/** Localized adapter name (plugin providers keep their id). */
export function useAdapterLabel(): (id: string) => string {
  const { t } = useT("deploy");
  return useCallback((id: string) => (isKnownAdapter(id) ? t(`adapters.names.${id}`) : id), [t]);
}

export function StatusBadge({ status, testId }: { status: DeploymentStatus; testId?: string }) {
  const { t } = useT("deploy");
  return (
    <Badge tone={statusTone(status)} data-testid={testId}>
      {t(`status.${status}`)}
    </Badge>
  );
}

export function AvailabilityBadge({ adapter }: { adapter: AdapterInfo }) {
  const { t } = useT("deploy");
  const a = adapter.availability;
  const text =
    a.state === "Ready" ? t("adapters.ready") : a.state === "NeedsCredential" ? t("adapters.needsCredential", { secret: a.secret }) : t("adapters.unavailable", { reason: a.reason });
  return (
    <Badge tone={availabilityTone(a)} data-testid={`adapter-availability-${adapter.id}`}>
      {text}
    </Badge>
  );
}
