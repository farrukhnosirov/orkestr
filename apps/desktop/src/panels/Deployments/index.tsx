/**
 * Deployments panel: environments, adapters, deploy / rollback (ADR-0041). **Owner: WP4.** Step-0 skeleton: honest "being built" state; the
 * RPC methods behind it answer `unimplementedFeature` until WP4 lands.
 */
import { EmptyState } from "@orkestr/ui";
import { Rocket } from "lucide-react";

import { useT } from "../../i18n";
import type { PanelProps } from "../types";

export default function DeploymentsPanel(_props: PanelProps) {
  const { t } = useT("deploy");
  return (
    <EmptyState
      icon={<Rocket size={28} strokeWidth={1.6} />}
      title={t("stub.title")}
      description={t("stub.description")}
    />
  );
}
