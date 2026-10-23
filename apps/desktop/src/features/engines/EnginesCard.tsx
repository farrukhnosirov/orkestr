/**
 * «Parallel engines» card: both subscriptions side by side — Claude Code
 * (Claude subscription) and Codex (ChatGPT subscription) — with their
 * installed version, sign-in, plan, limit and running agents
 * (`engines.status`), and the routing configuration (`EnginesConfig`):
 * mode, the Manager's engine, the primary engine, roles on Codex, each
 * engine's model and the cross fallback. «Save» → `engines.setConfig`.
 * Shown in Settings → AI providers, next to the subscription cards.
 */
import type { EngineId, EnginesConfig, EnginesMode, EngineStatus } from "@orkestr/protocol";
import { Badge, Button, IconButton, Skeleton } from "@orkestr/ui";
import clsx from "clsx";
import { RefreshCw, Save, Split } from "lucide-react";
import { useEffect, useState } from "react";

import { useCodexStore } from "../codex/store";
import { Switch } from "../providers/controls";
import { useFormat, useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { CLAUDE_ENGINE_MODELS, ENGINE_IDS, ENGINE_MODES, ENGINE_ROLES, configChanged, engineReasonText, engineTone, hasRole, modeUses, roleKey, toggleRole } from "./logic";
import { useEnginesStore } from "./store";
import "./engines.css";

function EngineColumn({ status, name }: { status: EngineStatus | null; name: string }) {
  const { t } = useT("engines");
  const fmt = useFormat();
  const tone = engineTone(status);
  return (
    <div className={clsx("eng-col", `eng-col--${tone}`)} data-testid={`engines-status-${status?.engine ?? "unknown"}`}>
      <div className="eng-col__head">
        <span className={clsx("eng-dot", `eng-dot--${tone}`)} aria-hidden="true" />
        <strong>{name}</strong>
        <span className="eng-col__state">{t(`tone.${tone}`)}</span>
      </div>
      {status ? (
        <dl className="eng-facts">
          <div>
            <dt>{t("facts.installed")}</dt>
            <dd>{status.installed ? (status.version ? t("facts.version", { version: status.version }) : t("facts.yes")) : t("facts.no")}</dd>
          </div>
          <div>
            <dt>{t("facts.signedIn")}</dt>
            <dd>{status.signedIn ? t("facts.yes") : t("facts.no")}</dd>
          </div>
          <div>
            <dt>{t("facts.plan")}</dt>
            <dd>{status.plan ?? "—"}</dd>
          </div>
          <div>
            <dt>{t("facts.limit")}</dt>
            <dd>
              {status.limitPercent != null ? fmt.percent(status.limitPercent / 100) : t("facts.unknown")}
              {status.limitResetsAt && <span className="eng-muted"> · {t("facts.resets", { date: fmt.date(status.limitResetsAt, "dateTime") })}</span>}
            </dd>
          </div>
          <div>
            <dt>{t("facts.running")}</dt>
            <dd>{t("facts.agents", { count: status.runningAgents })}</dd>
          </div>
        </dl>
      ) : (
        <Skeleton height={60} />
      )}
      {status?.reason && <p className="eng-muted">{engineReasonText(status.reason, t)}</p>}
    </div>
  );
}

function EngineSelect({ value, onChange, label, disabled, testId }: { value: EngineId; onChange: (v: EngineId) => void; label: string; disabled?: boolean; testId: string }) {
  const { t } = useT("engines");
  return (
    <label className="eng-field">
      <span>{label}</span>
      <select className="pv-select" value={value} disabled={disabled} onChange={(e) => onChange(e.target.value as EngineId)} data-testid={testId}>
        {ENGINE_IDS.map((id) => (
          <option key={id} value={id}>
            {t(`engine.${id}`)}
          </option>
        ))}
      </select>
    </label>
  );
}

export function EnginesCard() {
  const { t } = useT(["engines", "agents"]);
  const connected = useAppStore((s) => s.connection.state === "open");
  const status = useEnginesStore((s) => s.status);
  const loading = useEnginesStore((s) => s.loading);
  const saving = useEnginesStore((s) => s.saving);
  const error = useEnginesStore((s) => s.error);
  const load = useEnginesStore((s) => s.load);
  const save = useEnginesStore((s) => s.save);
  const codexModels = useCodexStore((s) => s.models);
  const loadModels = useCodexStore((s) => s.loadModels);
  const [draft, setDraft] = useState<EnginesConfig | null>(null);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (!connected) return;
    void load();
    if (useCodexStore.getState().models.length === 0) void loadModels();
  }, [connected, load, loadModels]);

  // The form follows the saved configuration until the user edits it.
  const base = status?.config ?? null;
  const cfg = draft ?? base;
  const changed = cfg !== null && base !== null && configChanged(cfg, base);
  const set = (patch: Partial<EnginesConfig>) => {
    if (!cfg) return;
    setSaved(false);
    setDraft({ ...cfg, ...patch });
  };
  const uses = modeUses(cfg?.mode ?? "off");

  const onSave = async () => {
    if (!cfg) return;
    if (await save(cfg)) {
      setDraft(null);
      setSaved(true);
    }
  };

  return (
    <section className="ce-card eng-card" aria-label={t("card.title")} data-testid="engines-card">
      <div className="ce-card__glow" aria-hidden="true" />
      <header className="ce-card__head">
        <span className="ce-card__icon" aria-hidden="true">
          <Split size={18} />
        </span>
        <div className="ce-card__titles">
          <h3 className="ce-card__title">{t("card.title")}</h3>
          <p className="ce-card__subtitle">{t("card.subtitle")}</p>
        </div>
        <IconButton label={t("card.refresh")} icon={<RefreshCw size={14} className={loading ? "ce-spin" : undefined} />} onClick={() => void load(true)} disabled={loading} />
      </header>

      <div className="eng-cols">
        <EngineColumn status={status?.claude ?? null} name={t("engine.claudeCode")} />
        <EngineColumn status={status?.codex ?? null} name={t("engine.codex")} />
      </div>

      {cfg && (
        <>
          <fieldset className="eng-modes">
            <legend>{t("form.mode")}</legend>
            {ENGINE_MODES.map((m: EnginesMode) => (
              <label key={m} className={clsx("eng-mode", cfg.mode === m && "eng-mode--on")} data-testid={`engines-mode-${m}`}>
                <input type="radio" name="engines-mode" value={m} checked={cfg.mode === m} onChange={() => set({ mode: m })} />
                <span className="eng-mode__name">{t(`modes.${m}.title`)}</span>
                <span className="eng-mode__desc">{t(`modes.${m}.description`)}</span>
              </label>
            ))}
          </fieldset>

          <div className="eng-grid">
            <EngineSelect label={t("form.manager")} value={cfg.manager} onChange={(v) => set({ manager: v })} disabled={cfg.mode === "off"} testId="engines-manager" />
            <EngineSelect label={t("form.primary")} value={cfg.primary} onChange={(v) => set({ primary: v })} disabled={!uses.primary} testId="engines-primary" />
            <label className="eng-field">
              <span>{t("form.claudeModel")}</span>
              <select className="pv-select" value={cfg.claudeModel} onChange={(e) => set({ claudeModel: e.target.value })} data-testid="engines-claude-model">
                {CLAUDE_ENGINE_MODELS.map((m) => (
                  <option key={m} value={m}>
                    {t(`claudeModels.${m}`)}
                  </option>
                ))}
                {!(CLAUDE_ENGINE_MODELS as readonly string[]).includes(cfg.claudeModel) && <option value={cfg.claudeModel}>{cfg.claudeModel}</option>}
              </select>
            </label>
            <label className="eng-field">
              <span>{t("form.codexModel")}</span>
              <select className="pv-select" value={cfg.codexModel} onChange={(e) => set({ codexModel: e.target.value })} data-testid="engines-codex-model">
                <option value="default">{t("form.codexDefault")}</option>
                {codexModels.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.displayName}
                  </option>
                ))}
                {cfg.codexModel !== "default" && !codexModels.some((m) => m.id === cfg.codexModel) && <option value={cfg.codexModel}>{cfg.codexModel}</option>}
              </select>
            </label>
          </div>

          <fieldset className={clsx("eng-roles", !uses.roles && "eng-roles--dim")} data-testid="engines-roles">
            <legend>{t("form.codexRoles")}</legend>
            <p className="eng-muted">{uses.roles ? t("form.codexRolesHint") : t("form.codexRolesSplitOnly")}</p>
            <div className="eng-roles__list">
              {ENGINE_ROLES.map((r) => (
                <label key={r} className={clsx("eng-role", hasRole(cfg.codexRoles, r) && "eng-role--on")}>
                  <input type="checkbox" checked={hasRole(cfg.codexRoles, r)} onChange={() => set({ codexRoles: toggleRole(cfg.codexRoles, r) })} />
                  {t(`agents:roles.${roleKey(r) as "Backend"}`)}
                </label>
              ))}
            </div>
          </fieldset>

          <Switch checked={cfg.crossFallback} onChange={(v) => set({ crossFallback: v })} label={t("form.crossFallback")} />
          <p className="eng-muted">{t("form.crossFallbackHint")}</p>

          <div className="ce-card__actions">
            <Button variant="primary" leftIcon={<Save size={14} />} loading={saving} disabled={!changed} onClick={() => void onSave()} data-testid="engines-save">
              {t("form.save")}
            </Button>
            {changed && (
              <Button variant="ghost" onClick={() => setDraft(null)}>
                {t("form.reset")}
              </Button>
            )}
            {saved && !changed && (
              <Badge tone="success" role="status">
                {t("form.saved")}
              </Badge>
            )}
          </div>
        </>
      )}
      {!cfg && !error && <Skeleton height={120} />}
      {error && <p className="ce-card__msg ce-card__msg--err">{error}</p>}
    </section>
  );
}
