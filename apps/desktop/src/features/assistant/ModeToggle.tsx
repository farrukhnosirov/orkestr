/** Title-bar switch between Orchestra mode and Assistant mode (Ctrl+Alt+A). */
import { Bot, LayoutDashboard } from "lucide-react";

import { useT } from "../../i18n";
import { useAssistantStore } from "./store";
import "./assistant.css";

export function ModeToggle() {
  const { t } = useT("assistant");
  const mode = useAssistantStore((s) => s.mode);
  const setMode = useAssistantStore((s) => s.setMode);
  return (
    <div className="as-mode" role="group" aria-label={t("mode.label")} data-testid="mode-toggle">
      <button
        type="button"
        className="as-mode__btn"
        aria-pressed={mode === "orchestra"}
        title={t("mode.orchestraHint", { keys: "Ctrl+Alt+A" })}
        onClick={() => setMode("orchestra")}
      >
        <LayoutDashboard size={13} />
        {mode === "orchestra" && <span>{t("mode.orchestra")}</span>}
      </button>
      <button
        type="button"
        className="as-mode__btn"
        aria-pressed={mode === "assistant"}
        title={t("mode.assistantHint", { keys: "Ctrl+Alt+A" })}
        onClick={() => setMode("assistant")}
      >
        <Bot size={13} />
        {mode === "assistant" && <span>{t("mode.assistant")}</span>}
      </button>
    </div>
  );
}
