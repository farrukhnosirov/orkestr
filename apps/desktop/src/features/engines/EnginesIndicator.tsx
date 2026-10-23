/**
 * Title-bar indicator of the two subscriptions: tiny «Claude» and «Codex»
 * chips with a status dot — green signed in, amber limit above 80 %, grey
 * not installed, red not signed in — and a tooltip with the plan and limit.
 * «Claude» opens Settings → AI providers (subscriptions and parallel
 * engines), «Codex» opens the Codex panel. Polls `engines.status`.
 */
import type { EngineStatus } from "@orkestr/protocol";
import clsx from "clsx";
import { useEffect } from "react";

import { openCodexPanel } from "../codex/commands";
import { useCodexStore } from "../codex/store";
import { useSettingsStore } from "../settings/settingsStore";
import { useFormat, useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { engineTone } from "./logic";
import { useEnginesStore } from "./store";
import "./engines.css";

/** Background refresh of the indicator. */
export const ENGINES_POLL_MS = 2 * 60_000;

function useTooltip() {
  const { t } = useT("engines");
  const fmt = useFormat();
  return (name: string, s: EngineStatus | null): string => {
    const tone = engineTone(s);
    const lines = [t("indicator.state", { name, state: t(`tone.${tone}`) })];
    if (s?.plan) lines.push(t("indicator.plan", { plan: s.plan }));
    if (s?.limitPercent != null) lines.push(t("indicator.limit", { percent: fmt.percent(s.limitPercent / 100) }));
    if (s && s.runningAgents > 0) lines.push(t("facts.agents", { count: s.runningAgents }));
    if (s?.reason && tone !== "ok") lines.push(s.reason);
    return lines.join("\n");
  };
}

export function EnginesIndicator() {
  const { t } = useT("engines");
  const connected = useAppStore((s) => s.connection.state === "open");
  const status = useEnginesStore((s) => s.status);
  const load = useEnginesStore((s) => s.load);
  const codexLoggedIn = useCodexStore((s) => s.status?.loggedIn);
  const tooltip = useTooltip();

  useEffect(() => {
    if (!connected) return undefined;
    void load();
    const id = setInterval(() => void load(), ENGINES_POLL_MS);
    return () => clearInterval(id);
  }, [connected, load]);

  // A sign-in / sign-out in the Codex panel changes the Codex chip at once.
  useEffect(() => {
    if (connected && codexLoggedIn !== undefined) void load();
  }, [connected, codexLoggedIn, load]);

  if (!connected) return null;
  const claude = status?.claude ?? null;
  const codex = status?.codex ?? null;
  const claudeName = t("indicator.claude");
  const codexName = t("indicator.codex");
  const claudeTip = tooltip(claudeName, claude);
  const codexTip = tooltip(codexName, codex);

  return (
    <div className="eng-ind" role="group" aria-label={t("indicator.label")} data-testid="engines-indicator">
      <button
        type="button"
        className="eng-chip"
        title={claudeTip}
        aria-label={claudeTip}
        onClick={() => useSettingsStore.getState().openSettings("providers")}
        data-testid="engines-indicator-claude"
      >
        <span className={clsx("eng-dot", `eng-dot--${engineTone(claude)}`)} aria-hidden="true" />
        {claudeName}
      </button>
      <button type="button" className="eng-chip" title={codexTip} aria-label={codexTip} onClick={() => openCodexPanel()} data-testid="engines-indicator-codex">
        <span className={clsx("eng-dot", `eng-dot--${engineTone(codex)}`)} aria-hidden="true" />
        {codexName}
      </button>
    </div>
  );
}
