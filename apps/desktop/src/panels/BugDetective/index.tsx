/**
 * Bug Detective panel (ADR-0042). **Owner: WP6.** Step-0 skeleton: honest "being built" state; the
 * RPC methods behind it answer `unimplementedFeature` until WP6 lands.
 */
import { EmptyState } from "@orkestr/ui";
import { Bug } from "lucide-react";

import { useT } from "../../i18n";
import type { PanelProps } from "../types";

export default function BugDetectivePanel(_props: PanelProps) {
  const { t } = useT("detective");
  return (
    <EmptyState
      icon={<Bug size={28} strokeWidth={1.6} />}
      title={t("stub.title")}
      description={t("stub.description")}
    />
  );
}
