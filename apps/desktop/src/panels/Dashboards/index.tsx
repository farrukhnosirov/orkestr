/**
 * Dashboard Builder panel (ADR-0034); logic in `features/dashboards`. **Owner: WP3.** Step-0 skeleton: honest "being built" state; the
 * RPC methods behind it answer `unimplementedFeature` until WP3 lands.
 */
import { EmptyState } from "@orkestr/ui";
import { LayoutGrid } from "lucide-react";

import { useT } from "../../i18n";
import type { PanelProps } from "../types";

export default function DashboardsPanel(_props: PanelProps) {
  const { t } = useT("dashboards");
  return (
    <EmptyState
      icon={<LayoutGrid size={28} strokeWidth={1.6} />}
      title={t("stub.title")}
      description={t("stub.description")}
    />
  );
}
