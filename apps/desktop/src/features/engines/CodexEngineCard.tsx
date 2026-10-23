/**
 * «Codex (ChatGPT subscription)» card, the twin of the Claude Code
 * subscription card: whether the official Codex CLI is installed (install /
 * update with npm), signed in with ChatGPT (browser, device code or
 * terminal), the account's plan and limit, and a shortcut to the Codex panel.
 * Shown in Settings → AI providers.
 */
import { Badge, Button, IconButton } from "@orkestr/ui";
import { Bot, CheckCircle2, MessageSquare, RefreshCw } from "lucide-react";
import { useEffect } from "react";

import { openCodexPanel } from "../codex/commands";
import { CodexAccountLine, CodexInstall, CodexSignIn } from "../codex/CodexSetup";
import { maxLimitPercent } from "../codex/model";
import { useCodexStore } from "../codex/store";
import { useFormat, useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { useSettingsStore } from "../settings/settingsStore";
import "../claudeEngine/claudeEngine.css";

export function CodexEngineCard() {
  const { t } = useT("engines");
  const fmt = useFormat();
  const connected = useAppStore((s) => s.connection.state === "open");
  const projectId = useAppStore((s) => s.activeProjectId);
  const status = useCodexStore((s) => s.status);
  const loading = useCodexStore((s) => s.statusLoading);
  const statusError = useCodexStore((s) => s.statusError);
  const limits = useCodexStore((s) => s.limits);
  const loadStatus = useCodexStore((s) => s.loadStatus);
  const loadLimits = useCodexStore((s) => s.loadLimits);

  useEffect(() => {
    if (!connected) return;
    void loadStatus();
  }, [connected, loadStatus]);
  useEffect(() => {
    if (connected && status?.loggedIn) void loadLimits();
  }, [connected, status?.loggedIn, loadLimits]);

  const installed = status?.binary != null;
  const ready = installed && status?.loggedIn === true;
  const state = !status ? t("codexCard.checking") : !installed ? t("codexCard.notInstalled") : !status.loggedIn ? t("codexCard.notSignedIn") : t("codexCard.ready");
  const pct = maxLimitPercent(limits);

  return (
    <section className="ce-card" aria-label={t("codexCard.title")} data-testid="codex-engine-card">
      <div className="ce-card__glow" aria-hidden="true" />
      <header className="ce-card__head">
        <span className="ce-card__icon" aria-hidden="true">
          <Bot size={18} />
        </span>
        <div className="ce-card__titles">
          <h3 className="ce-card__title">{t("codexCard.title")}</h3>
          <p className="ce-card__subtitle">{t("codexCard.subtitle")}</p>
        </div>
        <IconButton
          label={t("codexCard.refresh")}
          icon={<RefreshCw size={14} className={loading ? "ce-spin" : undefined} />}
          onClick={() => void loadStatus(true)}
          disabled={loading}
        />
      </header>
      <div className="ce-card__chips">
        <Badge tone={ready ? "success" : "warning"} icon={ready ? <CheckCircle2 size={12} /> : undefined}>
          {state}
        </Badge>
        {status?.binary?.version && <Badge tone="neutral">{t("codexCard.version", { version: status.binary.version })}</Badge>}
        {status?.account?.plan && <Badge tone="violet">{t("codexCard.plan", { plan: status.account.plan })}</Badge>}
        {pct !== null && <Badge tone={pct > 80 ? "warning" : "info"}>{t("codexCard.limit", { percent: fmt.percent(pct / 100) })}</Badge>}
        {status?.providerId && <Badge tone="info">{t("codexCard.providerReady")}</Badge>}
      </div>
      {status && !installed && <CodexInstall compact />}
      {status && installed && !status.loggedIn && <CodexSignIn projectId={projectId} compact />}
      {ready && (
        <>
          <CodexAccountLine />
          <CodexInstall compact />
          <div className="ce-card__actions">
            <Button
              variant="primary"
              leftIcon={<MessageSquare size={14} />}
              onClick={() => {
                useSettingsStore.getState().closeSettings();
                openCodexPanel();
              }}
            >
              {t("codexCard.openPanel")}
            </Button>
          </div>
        </>
      )}
      <p className="ce-card__hint">{t("codexCard.hint")}</p>
      {statusError && <p className="ce-card__msg ce-card__msg--err">{statusError}</p>}
    </section>
  );
}
