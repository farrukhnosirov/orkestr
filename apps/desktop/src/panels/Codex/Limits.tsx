/**
 * ChatGPT plan limits of the Codex subscription (`codex.limits`, refreshed
 * after every turn and pushed by the CLI as `rateLimits`): the primary
 * (usually 5-hour) and secondary (usually weekly) windows with the used
 * share and reset time, «unknown» when the CLI reports none, plus the
 * chat's token usage. `LimitsMeter` is the compact header strip; it opens
 * the full `LimitsCard`.
 */
import type { CodexLimitWindow } from "@orkestr/protocol";
import { IconButton, Skeleton } from "@orkestr/ui";
import clsx from "clsx";
import { Gauge, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";

import { msUntil } from "../../features/claudeCode/chatTools";
import { contextShare, limitTone, totalTokens, windowKind, type CodexChat } from "../../features/codex/model";
import { useCodexStore } from "../../features/codex/store";
import { useFormat, useT } from "../../i18n";
import { Popover } from "../ClaudeCode/v2/Popover";

/** Background refresh of the limits. */
export const LIMITS_POLL_MS = 3 * 60_000;

export function useLimitsPolling(): void {
  const loadLimits = useCodexStore((s) => s.loadLimits);
  useEffect(() => {
    void loadLimits();
    const id = setInterval(() => void loadLimits(true), LIMITS_POLL_MS);
    return () => clearInterval(id);
  }, [loadLimits]);
}

function useNow(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}

function Bar({ w, compact }: { w: CodexLimitWindow | null | undefined; compact?: boolean }) {
  const pct = w ? Math.min(100, Math.max(0, w.percent)) : 0;
  return (
    <span className={clsx("cc2-bar", compact && "cc2-bar--compact", `cc2-bar--${limitTone(w?.percent)}`, !w && "cc2-bar--empty")} aria-hidden="true">
      <span className="cc2-bar__fill" style={{ width: `${pct}%` }} />
    </span>
  );
}

/** Localized name of a plan window. */
function useWindowName() {
  const { t } = useT("codex");
  const fmt = useFormat();
  return (w: CodexLimitWindow | null | undefined, which: "primary" | "secondary"): string => {
    switch (windowKind(w, which)) {
      case "week":
        return t("limits.weekly");
      case "hours":
        return t("limits.hours", { count: Math.round((w?.windowMinutes ?? 0) / 60) });
      case "minutes":
        return t("limits.minutes", { time: fmt.duration((w?.windowMinutes ?? 0) * 60_000) });
      case "primary":
        return t("limits.primary");
      case "secondary":
        return t("limits.secondary");
    }
  };
}

function WindowRow({ w, which, now }: { w: CodexLimitWindow | null; which: "primary" | "secondary"; now: number }) {
  const { t } = useT("codex");
  const fmt = useFormat();
  const name = useWindowName()(w, which);
  const left = msUntil(w?.resetsAt, now);
  const long = (w?.windowMinutes ?? 0) >= 1440;
  const reset =
    w?.resetsAt == null
      ? null
      : long
        ? t("limits.resetsAt", { date: fmt.date(w.resetsAt, "dateTime") })
        : left !== null
          ? t("limits.resetsIn", { time: fmt.duration(Math.max(60_000, Math.round(left / 60_000) * 60_000)) })
          : null;
  return (
    <div className="cc2-usage__row" role="group" aria-label={name} data-testid={`codex-limit-${which}`}>
      <div className="cc2-usage__head">
        <span className="cc2-usage__name">{name}</span>
        <span className={clsx("cc2-usage__pct", `cc2-tone--${limitTone(w?.percent)}`)}>{w ? fmt.percent(w.percent / 100) : t("limits.unknown")}</span>
      </div>
      <Bar w={w} />
      {reset && (
        <div className="cc2-usage__meta">
          <span>{reset}</span>
        </div>
      )}
    </div>
  );
}

function TokenStats({ chat }: { chat: CodexChat }) {
  const { t } = useT("codex");
  const fmt = useFormat();
  const tk = chat.tokens;
  if (!tk) return null;
  const share = contextShare(tk);
  return (
    <div className="cc2-usage__chat" role="group" aria-label={t("tokens.label")} data-testid="codex-tokens">
      <span className="cc2-usage__name">{t("tokens.label")}</span>
      <span>{t("tokens.lastTurn", { input: fmt.integer(tk.last.inputTokens), output: fmt.integer(tk.last.outputTokens) })}</span>
      <span>{t("tokens.total", { total: fmt.integer(totalTokens(tk.total)) })}</span>
      {tk.total.cachedInputTokens > 0 && <span>{t("tokens.cached", { cached: fmt.integer(tk.total.cachedInputTokens) })}</span>}
      {share !== null && tk.contextWindow && (
        <span>{t("tokens.context", { share: fmt.percent(share), window: fmt.integer(tk.contextWindow) })}</span>
      )}
    </div>
  );
}

export function LimitsCard({ chat }: { chat?: CodexChat | null }) {
  const { t } = useT("codex");
  const fmt = useFormat();
  const limits = useCodexStore((s) => s.limits);
  const loading = useCodexStore((s) => s.limitsLoading);
  const loadLimits = useCodexStore((s) => s.loadLimits);
  const now = useNow();
  return (
    <section className="cc2-usage" aria-label={t("limits.label")} data-testid="codex-limits">
      <header className="cc2-usage__title">
        <Gauge size={14} aria-hidden />
        <span>{t("limits.title")}</span>
        {limits?.plan && <span className="cc2-usage__plan">{t("limits.plan", { plan: limits.plan })}</span>}
        <IconButton
          size="sm"
          label={t("limits.refresh")}
          icon={<RefreshCw size={12} className={clsx(loading && "cc2-spin")} />}
          onClick={() => void loadLimits(true)}
          tooltip="left"
        />
      </header>
      {!limits && loading ? (
        <div className="cc2-usage__skeleton">
          <Skeleton height={8} />
          <Skeleton height={8} />
        </div>
      ) : limits && limits.available ? (
        <>
          <WindowRow w={limits.primary} which="primary" now={now} />
          <WindowRow w={limits.secondary} which="secondary" now={now} />
          {limits.credits && <div className="cc2-usage__meta">{t("limits.credits", { credits: limits.credits })}</div>}
          {limits.limitReached && <div className="cc2-usage__meta cc2-tone--danger">{t("limits.reached", { limit: limits.limitReached })}</div>}
          <div className="cc2-usage__meta cc2-usage__updated">{t("limits.updated", { time: fmt.relative(limits.fetchedAt, now) })}</div>
        </>
      ) : (
        <div className="cc2-usage__nodata">
          <strong>{t("limits.unknown")}</strong>
          <span>{limits?.error ?? t("limits.unknownHint")}</span>
        </div>
      )}
      {chat && <TokenStats chat={chat} />}
    </section>
  );
}

/** Compact header strip: primary | secondary | context; click → the full card. */
export function LimitsMeter({ chat }: { chat?: CodexChat | null }) {
  const { t } = useT("codex");
  const fmt = useFormat();
  const limits = useCodexStore((s) => s.limits);
  const windowName = useWindowName();
  const [open, setOpen] = useState(false);
  const ok = limits?.available === true;
  const pct = (w: CodexLimitWindow | null | undefined) => (w ? fmt.percent(w.percent / 100) : "—");
  const share = contextShare(chat?.tokens ?? null);
  return (
    <div className="cc2-anchor">
      <button
        type="button"
        className="cc2-meter"
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={t("limits.label")}
        title={t("limits.label")}
        onClick={() => setOpen((v) => !v)}
        data-testid="codex-limits-meter"
      >
        <Gauge size={13} aria-hidden />
        {ok ? (
          <>
            <span className="cc2-meter__item">
              <span className="cc2-meter__label">{windowName(limits.primary, "primary")}</span>
              <Bar w={limits.primary} compact />
              <span className={clsx("cc2-meter__pct", `cc2-tone--${limitTone(limits.primary?.percent)}`)}>{pct(limits.primary)}</span>
            </span>
            <span className="cc2-meter__sep" aria-hidden="true" />
            <span className="cc2-meter__item">
              <span className="cc2-meter__label">{windowName(limits.secondary, "secondary")}</span>
              <Bar w={limits.secondary} compact />
              <span className={clsx("cc2-meter__pct", `cc2-tone--${limitTone(limits.secondary?.percent)}`)}>{pct(limits.secondary)}</span>
            </span>
          </>
        ) : (
          <span className="cc2-meter__label">{limits ? t("limits.unknown") : t("limits.title")}</span>
        )}
        {share !== null && (
          <>
            <span className="cc2-meter__sep" aria-hidden="true" />
            <span className="cc2-meter__item">
              <span className="cc2-meter__label">{t("tokens.contextShort")}</span>
              <span className="cc2-meter__pct">{fmt.percent(share)}</span>
            </span>
          </>
        )}
      </button>
      <Popover open={open} onClose={() => setOpen(false)} label={t("limits.label")} align="end" className="cc2-pop--usage">
        <LimitsCard chat={chat ?? null} />
      </Popover>
    </div>
  );
}
