/**
 * Settings → Cursor mode (ADR-0050): the model source switch
 * («Мои подписки (Claude Code / ChatGPT)» ↔ «API-ключи»), the priority of
 * the backends inside each source (a failing one hands over to the next),
 * per-backend model and privacy consent, limits and safety switches.
 */
import type { CursorBackendInfo, CursorSettings, CursorSource } from "@orkestr/protocol";
import { Badge, IconButton, Tabs } from "@orkestr/ui";
import { ArrowDown, ArrowUp } from "lucide-react";
import { useEffect } from "react";

import { useT } from "../../i18n";
import { refreshCursor, saveCursorSettings, setConsent, useCursorStore, watchCursor } from "./store";
import "./cursor.css";

function orderOf(s: CursorSettings, source: CursorSource): string[] {
  return source === "subscriptions" ? s.subscriptionOrder : s.apiOrder;
}

/** Moves `id` one place up (-1) / down (+1) in the source's order (exported for tests). */
export function reorder(s: CursorSettings, source: CursorSource, all: string[], id: string, dir: -1 | 1): CursorSettings {
  const order = [...orderOf(s, source)];
  for (const a of all) if (!order.includes(a)) order.push(a);
  const i = order.indexOf(id);
  const j = i + dir;
  if (i < 0 || j < 0 || j >= order.length) return s;
  [order[i], order[j]] = [order[j]!, order[i]!];
  return source === "subscriptions" ? { ...s, subscriptionOrder: order } : { ...s, apiOrder: order };
}

function BackendRow({
  b,
  index,
  count,
  settings,
  ids,
  active,
}: {
  b: CursorBackendInfo;
  index: number;
  count: number;
  settings: CursorSettings;
  ids: string[];
  active: boolean;
}) {
  const { t } = useT("cursor");
  return (
    <li className="cu-backend" data-testid={`cursor-backend-${b.id}`}>
      <span className="cu-backend__rank">{index + 1}</span>
      <div className="cu-backend__main">
        <div className="cu-backend__name">
          {b.label} {active && <Badge tone="success">{t("settings.activeBadge")}</Badge>}
          {!b.available && <Badge tone="warning">{t("settings.unavailable")}</Badge>}
        </div>
        <div className="cu-backend__detail">{b.detail}</div>
        <label className="cu-backend__model">
          {t("settings.model")}
          <input
            className="cu-input"
            defaultValue={settings.models[b.id] ?? ""}
            placeholder={b.model || t("settings.modelDefault")}
            onBlur={(e) => {
              const models = { ...settings.models };
              if (e.target.value.trim()) models[b.id] = e.target.value.trim();
              else delete models[b.id];
              void saveCursorSettings({ ...settings, models });
            }}
          />
        </label>
        <label className="cu-check">
          <input type="checkbox" checked={b.consented} onChange={(e) => void setConsent(b.id, e.target.checked)} />{" "}
          {t("settings.consent", { provider: b.label })}
        </label>
      </div>
      <div className="cu-backend__move">
        <IconButton
          label={t("settings.up")}
          icon={<ArrowUp size={14} />}
          disabled={index === 0}
          onClick={() => void saveCursorSettings(reorder(settings, b.source, ids, b.id, -1))}
        />
        <IconButton
          label={t("settings.down")}
          icon={<ArrowDown size={14} />}
          disabled={index === count - 1}
          onClick={() => void saveCursorSettings(reorder(settings, b.source, ids, b.id, 1))}
        />
      </div>
    </li>
  );
}

export function CursorSettingsSection() {
  const { t } = useT("cursor");
  const status = useCursorStore((s) => s.status);
  const error = useCursorStore((s) => s.error);
  useEffect(() => {
    watchCursor();
    void refreshCursor();
  }, []);
  if (!status) return <div className="cu-settings">{error ?? t("settings.loading")}</div>;
  const s = status.settings;
  const list = status.backends.filter((b) => b.source === s.source && b.id !== "mock");
  const ids = list.map((b) => b.id);
  const num = (key: "maxSteps" | "maxSeconds" | "maxImageSide", v: string) => {
    const n = Number(v);
    if (Number.isFinite(n) && n > 0) void saveCursorSettings({ ...s, [key]: Math.round(n) });
  };
  return (
    <div className="cu-settings" data-testid="cursor-settings">
      <p className="cu-note">{t("settings.intro")}</p>
      <div className="cu-field">
        <span className="cu-field__label">{t("settings.source")}</span>
        <Tabs<CursorSource>
          aria-label={t("settings.source")}
          items={[
            { id: "subscriptions", label: t("settings.sourceSubscriptions") },
            { id: "api", label: t("settings.sourceApi") },
          ]}
          value={s.source}
          onChange={(source) => void saveCursorSettings({ ...s, source })}
        />
        <span className="cu-note">{s.source === "subscriptions" ? t("settings.sourceSubscriptionsHint") : t("settings.sourceApiHint")}</span>
      </div>
      <div className="cu-field">
        <span className="cu-field__label">{t("settings.priority")}</span>
        <ol className="cu-backends">
          {list.map((b, i) => (
            <BackendRow key={b.id} b={b} index={i} count={list.length} settings={s} ids={ids} active={status.activeBackend === b.id} />
          ))}
        </ol>
        <span className="cu-note">{t("settings.failover")}</span>
      </div>
      <div className="cu-grid">
        <label className="cu-field">
          <span className="cu-field__label">{t("settings.maxSteps")}</span>
          <input className="cu-input" type="number" min={1} max={100} defaultValue={s.maxSteps} onBlur={(e) => num("maxSteps", e.target.value)} />
        </label>
        <label className="cu-field">
          <span className="cu-field__label">{t("settings.maxMinutes")}</span>
          <input
            className="cu-input"
            type="number"
            min={1}
            max={30}
            defaultValue={Math.round(s.maxSeconds / 60)}
            onBlur={(e) => num("maxSeconds", String(Number(e.target.value) * 60))}
          />
        </label>
        <label className="cu-field">
          <span className="cu-field__label">{t("settings.imageSide")}</span>
          <input className="cu-input" type="number" min={640} max={1600} step={160} defaultValue={s.maxImageSide} onBlur={(e) => num("maxImageSide", e.target.value)} />
        </label>
      </div>
      <label className="cu-check">
        <input type="checkbox" checked={s.abortOnMouse} onChange={(e) => void saveCursorSettings({ ...s, abortOnMouse: e.target.checked })} />{" "}
        {t("settings.abortOnMouse")}
      </label>
      <label className="cu-check">
        <input type="checkbox" checked={s.minimizeOrkestr} onChange={(e) => void saveCursorSettings({ ...s, minimizeOrkestr: e.target.checked })} />{" "}
        {t("settings.minimize")}
      </label>
      <p className="cu-note">{t("privacy")}</p>
      {error && <div className="cu-error">{error}</div>}
    </div>
  );
}
