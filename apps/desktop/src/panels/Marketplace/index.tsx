/**
 * Marketplace panel (Browse / Installed / Sources / Keys, ADR-0040). **Owner: WP2.** Step-0 skeleton: honest "being built" state; the
 * RPC methods behind it answer `unimplementedFeature` until WP2 lands.
 */
import { EmptyState } from "@orkestr/ui";
import { Store } from "lucide-react";

import { useT } from "../../i18n";
import type { PanelProps } from "../types";

export default function MarketplacePanel(_props: PanelProps) {
  const { t } = useT("marketplace");
  return (
    <EmptyState
      icon={<Store size={28} strokeWidth={1.6} />}
      title={t("stub.title")}
      description={t("stub.description")}
    />
  );
}
