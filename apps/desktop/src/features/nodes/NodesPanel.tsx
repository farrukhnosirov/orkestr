/**
 * Nodes panel (Phase 6, multi-node — ADR-0033 / ADR-0039). **Owner: WP-E.**
 *
 * Step 0 skeleton: an empty state. WP-E adds enrollment (SAS compare + trust
 * level), drain / revoke, jobs with placement explanations and the model store.
 */
import { EmptyState } from "@orkestr/ui";
import { Server } from "lucide-react";

import { useT } from "../../i18n";

export function NodesPanel() {
  const { t } = useT("nodes");
  return (
    <div className="nodes-panel">
      <EmptyState icon={<Server size={24} />} title={t("empty.title")} description={t("empty.description")} />
    </div>
  );
}
