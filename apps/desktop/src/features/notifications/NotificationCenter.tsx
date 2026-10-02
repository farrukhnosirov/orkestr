import type { NotificationLevel } from "@orkestr/protocol";
import { Button, EmptyState } from "@orkestr/ui";
import { clsx } from "clsx";
import { AlertOctagon, AlertTriangle, Bell, BellOff, CheckCircle2, Info, X, XCircle } from "lucide-react";
import { type ReactNode, useEffect, useRef } from "react";

import { formatDate, formatRelativeTime, t as translate, useLanguage, useT } from "../../i18n";
import { useSettingsStore } from "../settings/settingsStore";
import { Switch } from "../settings/controls";
import type { AppNotification } from "./fromEvent";
import "./notifications.css";
import { TOAST_TTL, unreadCount, useNotificationsStore } from "./notificationsStore";

const ICON: Record<NotificationLevel, (size: number) => ReactNode> = {
  Info: (s) => <Info size={s} />,
  Success: (s) => <CheckCircle2 size={s} />,
  Warning: (s) => <AlertTriangle size={s} />,
  Error: (s) => <XCircle size={s} />,
  Critical: (s) => <AlertOctagon size={s} />,
};

/** "just now" / "10 minutes ago" / "3 hours ago", then the date (current language). */
export function relativeTime(iso: string, now = Date.now()): string {
  const diff = Math.max(0, now - new Date(iso).getTime());
  if (diff < 45_000) return translate("common:time.justNow");
  if (diff < 24 * 3600_000) return formatRelativeTime(iso, now);
  return formatDate(iso, "date");
}

function Row({ n }: { n: AppNotification }) {
  useLanguage(); // re-render times on language change
  return (
    <li className={clsx("ntf-row", `ntf-row--${n.level.toLowerCase()}`, !n.read && "ntf-row--unread")}>
      <span className="ntf-row__icon">{ICON[n.level](15)}</span>
      <div className="ntf-row__text">
        <div className="ntf-row__title">{n.title}</div>
        {n.body && <div className="ntf-row__body">{n.body}</div>}
      </div>
      <time className="ntf-row__time" dateTime={n.ts}>
        {relativeTime(n.ts)}
      </time>
    </li>
  );
}

/** Title-bar bell with the notification history popover. */
export function NotificationBell() {
  const items = useNotificationsStore((s) => s.items);
  const open = useNotificationsStore((s) => s.centerOpen);
  const setOpen = useNotificationsStore((s) => s.setCenterOpen);
  const markAllRead = useNotificationsStore((s) => s.markAllRead);
  const clear = useNotificationsStore((s) => s.clear);
  const dnd = useNotificationsStore((s) => s.doNotDisturb);
  const updateSettings = useSettingsStore((s) => s.update);
  const settingsLoaded = useSettingsStore((s) => s.settings !== null);
  const rootRef = useRef<HTMLDivElement>(null);
  const unread = unreadCount(items);
  const { t } = useT("notifications");

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open, setOpen]);

  const toggle = () => {
    if (open) markAllRead();
    setOpen(!open);
  };

  return (
    <div className="ntf-bell" ref={rootRef}>
      <button
        type="button"
        className={clsx("titlebar__ctl ntf-bell__button", open && "ntf-bell__button--open")}
        aria-label={unread > 0 ? t("center.unread", { count: unread }) : t("center.title")}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={toggle}
      >
        {dnd ? <BellOff size={15} /> : <Bell size={15} />}
        {unread > 0 && <span className="ntf-bell__badge">{unread > 99 ? "99+" : unread}</span>}
      </button>
      {open && (
        <div
          className="ntf-popover"
          role="dialog"
          aria-label={t("center.title")}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              markAllRead();
              setOpen(false);
            }
          }}
        >
          <header className="ntf-popover__head">
            <span className="ntf-popover__title">{t("center.title")}</span>
            <label className="ntf-popover__dnd">
              <span>{t("center.dnd")}</span>
              <Switch
                label={t("center.dnd")}
                checked={dnd}
                disabled={!settingsLoaded}
                onChange={(v) => void updateSettings({ doNotDisturb: v })}
              />
            </label>
          </header>
          {items.length === 0 ? (
            <EmptyState
              className="ntf-popover__empty"
              icon={<Bell size={20} />}
              title={t("center.emptyTitle")}
              description={t("center.emptyDescription")}
            />
          ) : (
            <ul className="ntf-popover__list">
              {items.map((n) => (
                <Row key={n.id} n={n} />
              ))}
            </ul>
          )}
          {items.length > 0 && (
            <footer className="ntf-popover__foot">
              <Button size="sm" variant="ghost" onClick={markAllRead}>
                {t("center.markAllRead")}
              </Button>
              <Button size="sm" variant="ghost" onClick={clear}>
                {t("center.clear")}
              </Button>
            </footer>
          )}
        </div>
      )}
    </div>
  );
}

function Toast({ n }: { n: AppNotification }) {
  const dismiss = useNotificationsStore((s) => s.dismissToast);
  const { t } = useT("notifications");
  useEffect(() => {
    const ttl = TOAST_TTL[n.level];
    if (ttl == null) return;
    const timer = setTimeout(() => dismiss(n.id), ttl);
    return () => clearTimeout(timer);
  }, [n.id, n.level, dismiss]);
  return (
    <div
      className={clsx("ntf-toast", `ntf-toast--${n.level.toLowerCase()}`)}
      role={n.level === "Error" || n.level === "Critical" ? "alert" : "status"}
    >
      <span className="ntf-toast__icon">{ICON[n.level](16)}</span>
      <div className="ntf-toast__text">
        <div className="ntf-toast__title">{n.title}</div>
        {n.body && <div className="ntf-toast__body">{n.body}</div>}
      </div>
      <button type="button" className="ntf-toast__close" aria-label={t("center.dismiss")} onClick={() => dismiss(n.id)}>
        <X size={14} />
      </button>
    </div>
  );
}

/** Toast stack (bottom-right, above the status bar). */
export function Toasts() {
  const toasts = useNotificationsStore((s) => s.toasts);
  if (toasts.length === 0) return null;
  return (
    <div className="ntf-toasts" aria-live="polite">
      {toasts.map((n) => (
        <Toast key={n.id} n={n} />
      ))}
    </div>
  );
}
