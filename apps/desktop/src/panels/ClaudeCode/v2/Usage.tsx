/**
 * Plan usage of the Claude Code subscription: SESSION (5-hour window) and
 * WEEKLY bars with reset times (`claudeCode.usage`, the CLI's `/usage`),
 * refreshed after every turn and every few minutes; «Нет данных» when the
 * CLI reports no plan limits. `UsageMeter` is the compact header version
 * (opens the full `UsageCard`).
 */
import type { ClaudeLimitWindow } from "@orkestr/protocol";
import { IconButton, Skeleton } from "@orkestr/ui";
import clsx from "clsx";
import { Gauge, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";

import { chatTotals, msUntil, usageTone } from "../../../features/claudeCode/chatTools";
import type { ChatState } from "../../../features/claudeCode/model";
import { useClaudeStore } from "../../../features/claudeCode/store";
import { useFormat, useT } from "../../../i18n";
import { Popover } from "./Popover";

/** Background refresh interval of the plan usage. */
export const USAGE_POLL_MS = 3 * 60_000;

/** Loads the usage now and every few minutes while mounted. */
export function useUsagePolling(sessionId: string | null): void {
  const loadUsage = useClaudeStore((s) => s.loadUsage);
  useEffect(() => {
    void loadUsage(sessionId);
    const id = setInterval(() => void loadUsage(sessionId, true), USAGE_POLL_MS);
    return () => clearInterval(id);
  }, [sessionId, loadUsage]);
}

function useNow(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}

function Bar({ w, compact }: { w: ClaudeLimitWindow | null; compact?: boolean }) {
  const pct = w ? Math.min(100, Math.max(0, w.percent)) : 0;
  return (
    <span className={clsx("cc2-bar", compact && "cc2-bar--compact", `cc2-bar--${usageTone(w)}`, !w && "cc2-bar--empty")} aria-hidden="true">
      <span className="cc2-bar__fill" style={{ width: `${pct}%` }} />
    </span>
  );
}

function WindowRow({ title, hint, w, weekly, now }: { title: string; hint: string; w: ClaudeLimitWindow | null; weekly: boolean; now: number }) {
  const { t } = useT("claudeCodePanel");
  const fmt = useFormat();
  const left = msUntil(w?.resetsAt, now);
  const reset =
    w?.resetsAt == null
      ? null
      : weekly
        ? t("usage.resetsAt", { date: fmt.date(w.resetsAt, "dateTime") })
        : left !== null
          ? t("usage.resetsIn", { time: fmt.duration(Math.max(60_000, Math.round(left / 60_000) * 60_000)) })
          : null;
  return (
    <div className="cc2-usage__row" role="group" aria-label={title}>
      <div className="cc2-usage__head">
        <span className="cc2-usage__name">{title}</span>
        <span className={clsx("cc2-usage__pct", `cc2-tone--${usageTone(w)}`)}>{w ? fmt.percent(w.percent / 100) : t("usage.noData")}</span>
      </div>
      <Bar w={w} />
      <div className="cc2-usage__meta">
        <span>{hint}</span>
        {reset && <span>{reset}</span>}
      </div>
    </div>
  );
}

export function ChatStats({ chat }: { chat: ChatState }) {
  const { t } = useT("claudeCodePanel");
  const fmt = useFormat();
  const tot = chatTotals(chat);
  if (tot.turns === 0) return null;
  return (
    <div className="cc2-usage__chat" role="group" aria-label={t("chatStats.label")}>
      <span className="cc2-usage__name">{t("chatStats.label")}</span>
      <span>{t("chatStats.turns", { count: tot.turns })}</span>
      <span>{t("chatStats.tokens", { input: fmt.integer(tot.inputTokens), output: fmt.integer(tot.outputTokens) })}</span>
      {tot.durationMs > 0 && <span>{t("chatStats.duration", { time: fmt.duration(tot.durationMs) })}</span>}
      {tot.costUsd > 0 && <span title={t("chatStats.costHint")}>{t("chatStats.cost", { cost: fmt.currency(tot.costUsd) })}</span>}
    </div>
  );
}

/** Full usage card (sidebar footer / header popover). */
export function UsageCard({ sessionId, chat }: { sessionId: string | null; chat?: ChatState | null }) {
  const { t } = useT("claudeCodePanel");
  const fmt = useFormat();
  const usage = useClaudeStore((s) => s.usage);
  const loading = useClaudeStore((s) => s.usageLoading);
  const loadUsage = useClaudeStore((s) => s.loadUsage);
  const now = useNow();
  return (
    <section className="cc2-usage" aria-label={t("usage.label")}>
      <header className="cc2-usage__title">
        <Gauge size={14} aria-hidden />
        <span>{t("usage.title")}</span>
        {usage?.subscriptionType && <span className="cc2-usage__plan">{t("usage.plan", { plan: usage.subscriptionType })}</span>}
        <IconButton
          size="sm"
          label={t("usage.refresh")}
          icon={<RefreshCw size={12} className={clsx(loading && "cc2-spin")} />}
          onClick={() => void loadUsage(sessionId, true)}
          tooltip="left"
        />
      </header>
      {!usage && loading ? (
        <div className="cc2-usage__skeleton">
          <Skeleton height={8} />
          <Skeleton height={8} />
        </div>
      ) : usage && usage.available ? (
        <>
          <WindowRow title={t("usage.session")} hint={t("usage.sessionHint")} w={usage.session} weekly={false} now={now} />
          <WindowRow title={t("usage.weekly")} hint={t("usage.weeklyHint")} w={usage.weekly} weekly now={now} />
          {usage.weeklyModels
            .filter((w) => w.percent > 0)
            .map((w) => (
              <WindowRow key={w.label} title={t("usage.weeklyModel", { model: w.label ?? "" })} hint={t("usage.weeklyHint")} w={w} weekly now={now} />
            ))}
          {usage.extraUsageEnabled && <div className="cc2-usage__meta">{t("usage.extraOn")}</div>}
          <div className="cc2-usage__meta cc2-usage__updated">{t("usage.updated", { time: fmt.relative(usage.fetchedAt, now) })}</div>
        </>
      ) : (
        <div className="cc2-usage__nodata">
          <strong>{t("usage.noData")}</strong>
          <span>{t("usage.noDataHint")}</span>
        </div>
      )}
      {chat && <ChatStats chat={chat} />}
    </section>
  );
}

/** Compact SESSION | WEEKLY meter for the header; click → the full card. */
export function UsageMeter({ sessionId, chat }: { sessionId: string | null; chat?: ChatState | null }) {
  const { t } = useT("claudeCodePanel");
  const fmt = useFormat();
  const usage = useClaudeStore((s) => s.usage);
  const [open, setOpen] = useState(false);
  const ok = usage?.available === true;
  const pct = (w: ClaudeLimitWindow | null | undefined) => (w ? fmt.percent(w.percent / 100) : "—");
  return (
    <div className="cc2-anchor">
      <button
        type="button"
        className="cc2-meter"
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={t("usage.label")}
        title={t("usage.label")}
        onClick={() => setOpen((v) => !v)}
      >
        <Gauge size={13} aria-hidden />
        {ok ? (
          <>
            <span className="cc2-meter__item">
              <span className="cc2-meter__label">{t("usage.session")}</span>
              <Bar w={usage.session} compact />
              <span className={clsx("cc2-meter__pct", `cc2-tone--${usageTone(usage.session)}`)}>{pct(usage.session)}</span>
            </span>
            <span className="cc2-meter__sep" aria-hidden="true" />
            <span className="cc2-meter__item">
              <span className="cc2-meter__label">{t("usage.weekly")}</span>
              <Bar w={usage.weekly} compact />
              <span className={clsx("cc2-meter__pct", `cc2-tone--${usageTone(usage.weekly)}`)}>{pct(usage.weekly)}</span>
            </span>
          </>
        ) : (
          <span className="cc2-meter__label">{usage ? t("usage.noData") : t("usage.title")}</span>
        )}
      </button>
      <Popover open={open} onClose={() => setOpen(false)} label={t("usage.label")} align="end" className="cc2-pop--usage">
        <UsageCard sessionId={sessionId} chat={chat ?? null} />
      </Popover>
    </div>
  );
}
