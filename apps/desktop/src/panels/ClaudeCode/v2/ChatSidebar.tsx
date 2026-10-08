/**
 * «Чаты» sidebar of the Claude Code panel: search, «Новый чат», chats
 * grouped Закреплённые / Сегодня / Вчера / Эта неделя / Ранее (open tabs and
 * past conversations together), a running indicator, and per-chat actions —
 * pin, rename, remove from the list (labels are stored in Orkestr, never in
 * Claude's files), close an open chat.
 */
import { IconButton, Skeleton, Spinner } from "@orkestr/ui";
import clsx from "clsx";
import { Eye, EyeOff, MessageSquarePlus, MoreHorizontal, PanelLeftClose, Pencil, Pin, PinOff, Search, Trash2, Undo2, X } from "lucide-react";
import { type FormEvent, useMemo, useState } from "react";

import { buildChatList, folderName, groupChats, searchChats, type ChatEntry } from "../../../features/claudeCode/chatList";
import { useClaudeStore } from "../../../features/claudeCode/store";
import { useFormat, useT } from "../../../i18n";
import { UsageCard } from "./Usage";

const EMPTY: string[] = [];

export interface ChatSidebarProps {
  projectId: string;
  activeSessionId: string | null;
  onNew: () => void;
  onCollapse: () => void;
}

function RenameForm({ entry, onDone }: { entry: ChatEntry; onDone: () => void }) {
  const { t } = useT("claudeCodePanel");
  const setChatMeta = useClaudeStore((s) => s.setChatMeta);
  const [value, setValue] = useState(entry.title ?? "");
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (entry.claudeSessionId) await setChatMeta(entry.claudeSessionId, { title: value });
    onDone();
  };
  return (
    <form className="cc2-rename" onSubmit={(e) => void submit(e)} title={t("sidebar.renameHint")}>
      <input
        className="cc2-input"
        value={value}
        autoFocus
        maxLength={200}
        aria-label={t("sidebar.renameLabel")}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            onDone();
          }
        }}
      />
      <IconButton type="submit" size="sm" label={t("sidebar.renameSave")} icon={<Pencil size={12} />} />
      <IconButton size="sm" label={t("sidebar.renameCancel")} icon={<X size={12} />} onClick={onDone} />
    </form>
  );
}

function ChatRow({
  entry,
  active,
  projectId,
  now,
}: {
  entry: ChatEntry;
  active: boolean;
  projectId: string;
  now: number;
}) {
  const { t } = useT("claudeCodePanel");
  const fmt = useFormat();
  const openChat = useClaudeStore((s) => s.openChat);
  const setChatMeta = useClaudeStore((s) => s.setChatMeta);
  const close = useClaudeStore((s) => s.close);
  const [menu, setMenu] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const title = entry.title ?? t("sidebar.untitled");
  const folder = folderName(entry.cwd);
  const cid = entry.claudeSessionId;

  if (renaming) return <RenameForm entry={entry} onDone={() => setRenaming(false)} />;

  return (
    <li className={clsx("cc2-chat", active && "cc2-chat--active", entry.hidden && "cc2-chat--hidden")}>
      <button
        type="button"
        className="cc2-chat__main"
        aria-current={active ? "true" : undefined}
        title={title}
        onClick={() => {
          setMenu(false);
          void openChat(projectId, { sessionId: entry.sessionId, claudeSessionId: entry.claudeSessionId });
        }}
      >
        <span className="cc2-chat__title">
          {entry.running ? (
            <Spinner size={10} label={t("sidebar.running")} />
          ) : entry.live ? (
            <span className="cc2-dot" aria-label={t("sidebar.live")} role="img" />
          ) : null}
          {entry.pinned && <Pin size={11} className="cc2-chat__pin" aria-hidden />}
          <span className="cc2-chat__text">{title}</span>
        </span>
        <span className="cc2-chat__meta">
          <span>{fmt.relative(entry.updatedAt, now)}</span>
          {entry.messages !== null && <span>{t("sidebar.messages", { count: entry.messages })}</span>}
          {folder && <span className="cc2-chat__folder">{folder}</span>}
          {entry.hidden && <span className="cc2-chat__badge">{t("sidebar.removedBadge")}</span>}
        </span>
      </button>
      <div className="cc2-chat__actions">
        <IconButton
          size="sm"
          label={t("sidebar.menu")}
          icon={<MoreHorizontal size={14} />}
          aria-haspopup="menu"
          aria-expanded={menu}
          onClick={() => setMenu((v) => !v)}
        />
      </div>
      {menu && (
        <div className="cc2-menu" role="menu" aria-label={t("sidebar.menu")} onMouseLeave={() => setMenu(false)}>
          {cid && (
            <button type="button" role="menuitem" onClick={() => { setMenu(false); void setChatMeta(cid, { pinned: !entry.pinned }); }}>
              {entry.pinned ? <PinOff size={13} aria-hidden /> : <Pin size={13} aria-hidden />}
              {entry.pinned ? t("sidebar.unpin") : t("sidebar.pin")}
            </button>
          )}
          {cid && (
            <button type="button" role="menuitem" onClick={() => { setMenu(false); setRenaming(true); }}>
              <Pencil size={13} aria-hidden />
              {t("sidebar.rename")}
            </button>
          )}
          {entry.sessionId && (
            <button type="button" role="menuitem" onClick={() => { setMenu(false); void close(projectId, entry.sessionId ?? ""); }}>
              <X size={13} aria-hidden />
              {t("sidebar.close")}
            </button>
          )}
          {cid && !entry.live && (
            <button
              type="button"
              role="menuitem"
              className={clsx(!entry.hidden && "cc2-menu__danger")}
              onClick={() => { setMenu(false); void setChatMeta(cid, { hidden: !entry.hidden }); }}
            >
              {entry.hidden ? <Undo2 size={13} aria-hidden /> : <Trash2 size={13} aria-hidden />}
              {entry.hidden ? t("sidebar.restore") : t("sidebar.remove")}
            </button>
          )}
        </div>
      )}
    </li>
  );
}

export function ChatSidebar({ projectId, activeSessionId, onNew, onCollapse }: ChatSidebarProps) {
  const { t } = useT("claudeCodePanel");
  const tabs = useClaudeStore((s) => s.tabs[projectId] ?? EMPTY);
  const sessions = useClaudeStore((s) => s.sessions);
  const history = useClaudeStore((s) => s.history[projectId]);
  const historyLoading = useClaudeStore((s) => s.historyLoading);
  const meta = useClaudeStore((s) => s.meta);
  const busy = useClaudeStore((s) => s.busy);
  const [query, setQuery] = useState("");
  const [showHidden, setShowHidden] = useState(false);
  const now = Date.now();

  const entries = useMemo(() => {
    const live = tabs.flatMap((id) => {
      const v = sessions[id];
      return v ? [{ info: v.info, claudeSessionId: v.chat.claudeSessionId, running: v.chat.running, updatedAt: v.updatedAt }] : [];
    });
    return buildChatList(live, history ?? [], meta);
  }, [tabs, sessions, history, meta]);
  const sections = groupChats(searchChats(entries, query), new Date(now), showHidden);
  const hiddenCount = entries.filter((e) => e.hidden).length;
  const active = activeSessionId ? sessions[activeSessionId] : undefined;

  return (
    <aside className="cc2-side" aria-label={t("sidebar.label")}>
      <div className="cc2-side__head">
        <h2 className="cc2-side__title">{t("sidebar.title")}</h2>
        <IconButton label={t("sidebar.toggle")} icon={<PanelLeftClose size={15} />} onClick={onCollapse} tooltip="bottom" />
      </div>
      <button type="button" className="cc2-new" onClick={onNew} disabled={busy}>
        {busy ? <Spinner size={13} /> : <MessageSquarePlus size={15} aria-hidden />}
        <span>{t("sidebar.newChat")}</span>
      </button>
      <label className="cc2-search">
        <Search size={13} aria-hidden />
        <input
          type="search"
          value={query}
          placeholder={t("sidebar.searchPlaceholder")}
          aria-label={t("sidebar.search")}
          onChange={(e) => setQuery(e.target.value)}
        />
      </label>
      <nav className="cc2-side__list">
        {history === undefined && historyLoading ? (
          <div className="cc2-side__skeleton" aria-label={t("sidebar.loading")}>
            <Skeleton height={28} />
            <Skeleton height={28} />
            <Skeleton height={28} />
          </div>
        ) : sections.length === 0 ? (
          <p className="cc2-side__empty">{query.trim() ? t("sidebar.noMatches", { query: query.trim() }) : t("sidebar.empty")}</p>
        ) : (
          sections.map((sec) => (
            <section key={sec.group} className="cc2-group" aria-label={t(`sidebar.group.${sec.group}`)}>
              <h3 className="cc2-group__title">{t(`sidebar.group.${sec.group}`)}</h3>
              <ul className="cc2-group__list">
                {sec.entries.map((e) => (
                  <ChatRow
                    key={e.key}
                    entry={e}
                    projectId={projectId}
                    now={now}
                    active={e.sessionId !== null && e.sessionId === activeSessionId}
                  />
                ))}
              </ul>
            </section>
          ))
        )}
        {hiddenCount > 0 && (
          <button type="button" className="cc2-side__hidden" onClick={() => setShowHidden((v) => !v)}>
            {showHidden ? <EyeOff size={12} aria-hidden /> : <Eye size={12} aria-hidden />}
            {showHidden ? t("sidebar.hideHidden") : t("sidebar.showHidden")}
          </button>
        )}
      </nav>
      <div className="cc2-side__foot">
        <UsageCard sessionId={activeSessionId} chat={active?.chat ?? null} />
      </div>
    </aside>
  );
}
