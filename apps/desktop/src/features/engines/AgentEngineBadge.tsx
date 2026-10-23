/**
 * Engine badge of an agent — «Claude Code», «Codex» or «API» — from the kind
 * of the provider its model runs on. Nothing while the providers are not
 * loaded or the agent has no model.
 */
import clsx from "clsx";

import { useProvidersStore } from "../providers/store";
import { useT } from "../../i18n";
import { providerEngine } from "./logic";
import "./engines.css";

export function AgentEngineBadge({ providerId }: { providerId: string | null | undefined }) {
  const { t } = useT("engines");
  const kind = useProvidersStore((s) => (providerId ? s.providers.find((p) => p.provider.id === providerId)?.provider.kind : undefined));
  const engine = providerEngine(kind);
  if (!engine) return null;
  return (
    <span className={clsx("eng-badge", `eng-badge--${engine}`)} title={t(`agentBadge.hint.${engine}`)} data-testid="agent-engine-badge">
      {t(`agentBadge.${engine}`)}
    </span>
  );
}
