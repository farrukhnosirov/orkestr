import type { NotificationLevel } from "@orkestr/protocol";
import { Button, EmptyState } from "@orkestr/ui";
import { clsx } from "clsx";
import { AlertOctagon, AlertTriangle, Bell, BellOff, CheckCircle2, Info, X, XCircle } from "lucide-react";
import { type ReactNode, useEffect, useRef } from "react";

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

export function relativeTime(iso: string, now = Date.now()): string {
  const diff = Math.max(0, now - new Date(iso).getTime());
  const s = Math.floor(diff / 1000);
  if (s < 45) return "just now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${Math.max(1, m)} min ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} h ago`;
  return new Date(iso).toLocaleDateString();
}

function Row({ n }: { n: AppNotification }) {
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
        aria-label={unread > 0 ? `Notifications (${unread} unread)` : "Notifications"}
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
          aria-label="Notifications"
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              markAllRead();
              setOpen(false);
            }
          }}
        >
          <header className="ntf-popover__head">
            <span className="ntf-popover__title">Notifications</span>
            <label className="ntf-popover__dnd">
              <span>Do not disturb</span>
              <Switch
                label="Do not disturb"
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
              title="All caught up"
              description="Approvals, failures, budget alerts and agent notices appear here."
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
                Mark all read
              </Button>
              <Button size="sm" variant="ghost" onClick={clear}>
                Clear
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
  useEffect(() => {
    const ttl = TOAST_TTL[n.level];
    if (ttl == null) return;
    const t = setTimeout(() => dismiss(n.id), ttl);
    return () => clearTimeout(t);
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
      <button type="button" className="ntf-toast__close" aria-label="Dismiss notification" onClick={() => dismiss(n.id)}>
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
      {toasts.map((t) => (
        <Toast key={t.id} n={t} />
      ))}
    </div>
  );
}
