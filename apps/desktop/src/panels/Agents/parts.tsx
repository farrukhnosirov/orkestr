import type { AgentRole, AgentState } from "@orkestr/protocol";
import { Badge } from "@orkestr/ui";
import { clsx } from "clsx";
import { X } from "lucide-react";

import { useT } from "../../i18n";
import { STATE_TONE, isBusy, roleInitials, roleLabel } from "./model";
import { useAgentsUi } from "./store";

/** Agent state chip; busy states pulse, Waiting blinks (reduced-motion aware via CSS). */
export function StateChip({ state }: { state: AgentState }) {
  const { t } = useT("common");
  return (
    <Badge tone={STATE_TONE[state]} className={clsx("agents-state", `agents-state--${state.toLowerCase()}`)}>
      <span className={clsx("agents-state__dot", isBusy(state) && "agents-state__dot--busy")} aria-hidden="true" />
      {t(`status.agent.${state}`)}
    </Badge>
  );
}

/** Role monogram ("robot" placeholder until the Robot Designer, §30). */
export function RoleAvatar({ role, state, size = 32 }: { role: AgentRole; state?: AgentState; size?: number }) {
  const { t } = useT("agents");
  return (
    <span
      className={clsx("agents-avatar", state && isBusy(state) && "agents-avatar--busy", state === "Waiting" && "agents-avatar--waiting")}
      style={{ width: size, height: size }}
      title={roleLabel(role)}
      aria-label={t("avatar", { role: roleLabel(role) })}
      role="img"
    >
      {roleInitials(role)}
    </span>
  );
}

export function Toasts() {
  const { t } = useT("common");
  const toasts = useAgentsUi((s) => s.toasts);
  const dismiss = useAgentsUi((s) => s.dismissToast);
  if (toasts.length === 0) return null;
  return (
    <div className="agents-toasts" role="status" aria-live="polite">
      {toasts.map((toast) => (
        <div key={toast.id} className={clsx("agents-toast", `agents-toast--${toast.tone}`)}>
          <span>{toast.message}</span>
          <button type="button" aria-label={t("actions.close")} onClick={() => dismiss(toast.id)}>
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
