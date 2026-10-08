/**
 * "Claude Code (subscription)" card: whether the official CLI is installed
 * and signed in, whether the AI Manager / agent roles run on it, and the
 * one-click «Use Claude Code for everything» (Manager, every agent role,
 * review, consensus, PM — previous models stay as fallbacks).
 * Shown in Settings → AI providers, Settings → AI Manager and onboarding.
 */
import type { ClaudeEngineStatus } from "@orkestr/protocol";
import { Badge, Button, IconButton } from "@orkestr/ui";
import { CheckCircle2, RefreshCw, Sparkles, Zap } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";
import { CLAUDE_MODELS, engineStatus, applyClaudeForEverything } from "./api";
import "./claudeEngine.css";

export interface ClaudeEngineCardProps {
  /** Called after the routing changed (e.g. to reload the Manager form). */
  onApplied?: () => void;
}

export function ClaudeEngineCard({ onApplied }: ClaudeEngineCardProps) {
  const { t } = useT(["providers", "common"]);
  const connected = useAppStore((s) => s.connection.state === "open");
  const [status, setStatus] = useState<ClaudeEngineStatus | null>(null);
  const [model, setModel] = useState<string>("sonnet");
  const [busy, setBusy] = useState(false);
  const [checking, setChecking] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);

  const load = useCallback(async (refresh: boolean) => {
    setChecking(true);
    try {
      setStatus(await engineStatus(refresh));
    } catch (e) {
      setMsg({ ok: false, text: describeError(e) });
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    if (connected) void load(false);
  }, [connected, load]);

  const apply = async () => {
    setBusy(true);
    setMsg(null);
    try {
      const r = await applyClaudeForEverything(model);
      setStatus(r.status);
      setMsg({ ok: true, text: t("claudeEngine.applied", { count: r.status.rolesOnClaude }) });
      onApplied?.();
    } catch (e) {
      setMsg({ ok: false, text: describeError(e) });
    } finally {
      setBusy(false);
    }
  };

  const ready = status?.installed === true && (status.signedIn || status.providerId != null);
  let state: string;
  if (!status) state = t("claudeEngine.checking");
  else if (!status.installed) state = t("claudeEngine.notInstalled");
  else if (!status.signedIn) state = t("claudeEngine.notSignedIn");
  else state = t("claudeEngine.ready");

  return (
    <section className="ce-card" aria-label={t("claudeEngine.title")}>
      <div className="ce-card__glow" aria-hidden="true" />
      <header className="ce-card__head">
        <span className="ce-card__icon" aria-hidden="true">
          <Sparkles size={18} />
        </span>
        <div className="ce-card__titles">
          <h3 className="ce-card__title">{t("claudeEngine.title")}</h3>
          <p className="ce-card__subtitle">{t("claudeEngine.subtitle")}</p>
        </div>
        <IconButton
          label={t("claudeEngine.refresh")}
          icon={<RefreshCw size={14} className={checking ? "ce-spin" : undefined} />}
          onClick={() => void load(true)}
          disabled={checking}
        />
      </header>
      <div className="ce-card__chips">
        <Badge tone={ready ? "success" : "warning"} icon={ready ? <CheckCircle2 size={12} /> : undefined}>
          {state}
        </Badge>
        {status?.subscription && <Badge tone="violet">{t("claudeEngine.plan", { plan: status.subscription })}</Badge>}
        {status?.providerId && <Badge tone="info">{t("claudeEngine.providerReady")}</Badge>}
        {status?.managerOnClaude && <Badge tone="accent">{t("claudeEngine.managerOn")}</Badge>}
        {status != null && status.rolesOnClaude > 0 && (
          <Badge tone="accent">{t("claudeEngine.rolesOn", { count: status.rolesOnClaude })}</Badge>
        )}
      </div>
      <div className="ce-card__actions">
        <label className="ce-card__model">
          <span>{t("claudeEngine.model")}</span>
          <select className="pv-select" value={model} onChange={(e) => setModel(e.target.value)} disabled={busy}>
            {CLAUDE_MODELS.map((m) => (
              <option key={m} value={m}>
                {t(`claudeEngine.models.${m}`)}
              </option>
            ))}
          </select>
        </label>
        <Button variant="primary" leftIcon={<Zap size={14} />} loading={busy} disabled={!ready} onClick={() => void apply()}>
          {t("claudeEngine.useForEverything")}
        </Button>
      </div>
      <p className="ce-card__hint">{t("claudeEngine.hint")}</p>
      {msg && (
        <p className={msg.ok ? "ce-card__msg ce-card__msg--ok" : "ce-card__msg ce-card__msg--err"} role="status">
          {msg.text}
        </p>
      )}
    </section>
  );
}
