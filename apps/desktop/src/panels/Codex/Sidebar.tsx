/**
 * «Chats» sidebar of the Codex panel: «New chat», search, open sessions and
 * the conversations Codex saved for this project (grouped by day), with
 * per-chat actions — continue (resume), fork into a new chat, open in the
 * terminal, close an open chat — and the plan limits card at the bottom.
 */
import { IconButton, Skeleton, Spinner } from "@orkestr/ui";
import clsx from "clsx";
import { GitFork, MessageSquarePlus, MoreHorizontal, PanelLeftClose, Play, RefreshCw, Search, SquareTerminal, X } from "lucide-react";
import { useMemo, useState } from "react";

import { workspaceActions } from "../../components/workspaceActions";
import { folderName } from "../../features/claudeCode/chatList";
import { buildCodexChatList, groupCodexChats, searchCodexChats, type CodexChatEntry } from "../../features/codex/chatList";
import { isRunning, useCodexStore } from "../../features/codex/store";
import { useFormat, useT } from "../../i18n";
import { useTerminalUi } from "../Terminal/store";
import { LimitsCard } from "./Limits";

const EMPTY: string[] = [];

function ChatRow({ entry, active, projectId, now }: { entry: CodexChatEntry; active: boolean; projectId: string; now: number }) {
  const { t } = useT("codex");
  const fmt = useFormat();
  const openChat = useCodexStore((s) => s.openChat);
  const start = useCodexStore((s) => s.start);
  const close = useCodexStore((s) => s.close);
  const openInTerminal = useCodexStore((s) => s.openInTerminal);
  const [menu, setMenu] = useState(false);
  const title = entry.title ?? t("sidebar.untitled");
  const folder = folderName(entry.cwd);

  const terminal = async () => {
    const id = await openInTerminal({ projectId, threadId: entry.threadId });
    if (id) {
      useTerminalUi.getState().setActive(projectId, id);
      workspaceActions.openPanel("terminal");
    }
  };

  return (
    <li className={clsx("cc2-chat", active && "cc2-chat--active")} data-testid="codex-chat-row">
      <button
        type="button"
        className="cc2-chat__main"
        aria-current={active ? "true" : undefined}
        title={entry.live ? title : t("sidebar.resumeHint", { title })}
        onClick={() => {
          setMenu(false);
          void openChat(projectId, { sessionId: entry.sessionId, threadId: entry.threadId });
        }}
      >
        <span className="cc2-chat__title">
          {entry.running ? (
            <Spinner size={10} label={t("sidebar.running")} />
          ) : entry.live ? (
            <span className="cc2-dot" aria-label={t("sidebar.live")} role="img" />
          ) : null}
          {entry.forkedFrom && <GitFork size={11} className="cc2-chat__pin" aria-label={t("sidebar.forked")} />}
          <span className="cc2-chat__text">{title}</span>
        </span>
        <span className="cc2-chat__meta">
          <span>{fmt.relative(entry.updatedAt, now)}</span>
          {entry.model && <span>{entry.model}</span>}
          {folder && <span className="cc2-chat__folder">{folder}</span>}
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
          {entry.threadId && !entry.live && (
            <button type="button" role="menuitem" onClick={() => { setMenu(false); void openChat(projectId, { sessionId: null, threadId: entry.threadId }); }}>
              <Play size={13} aria-hidden />
              {t("sidebar.resume")}
            </button>
          )}
          {entry.threadId && (
            <button
              type="button"
              role="menuitem"
              data-testid="codex-fork"
              onClick={() => { setMenu(false); void start(projectId, { threadId: entry.threadId ?? undefined, fork: true }); }}
            >
              <GitFork size={13} aria-hidden />
              {t("sidebar.fork")}
            </button>
          )}
          {entry.threadId && (
            <button type="button" role="menuitem" onClick={() => { setMenu(false); void terminal(); }}>
              <SquareTerminal size={13} aria-hidden />
              {t("sidebar.terminal")}
            </button>
          )}
          {entry.sessionId && (
            <button type="button" role="menuitem" onClick={() => { setMenu(false); void close(projectId, entry.sessionId ?? ""); }}>
              <X size={13} aria-hidden />
              {t("sidebar.close")}
            </button>
          )}
        </div>
      )}
    </li>
  );
}

export interface CodexSidebarProps {
  projectId: string;
  activeSessionId: string | null;
  onNew: () => void;
  onCollapse: () => void;
}

export function CodexSidebar({ projectId, activeSessionId, onNew, onCollapse }: CodexSidebarProps) {
  const { t } = useT("codex");
  const order = useCodexStore((s) => s.order[projectId] ?? EMPTY);
  const sessions = useCodexStore((s) => s.sessions);
  const threads = useCodexStore((s) => s.threads[projectId]);
  const loading = useCodexStore((s) => s.threadsLoading);
  const threadsError = useCodexStore((s) => s.threadsError);
  const loadThreads = useCodexStore((s) => s.loadThreads);
  const busy = useCodexStore((s) => s.busy);
  const [query, setQuery] = useState("");
  const now = Date.now();

  const entries = useMemo(() => {
    const live = order.flatMap((id) => {
      const v = sessions[id];
      return v ? [{ info: v.info, threadId: v.chat.threadId, running: isRunning(v), updatedAt: v.updatedAt }] : [];
    });
    return buildCodexChatList(live, threads ?? []);
  }, [order, sessions, threads]);
  const sections = groupCodexChats(searchCodexChats(entries, query), new Date(now));
  const active = activeSessionId ? sessions[activeSessionId] : undefined;

  return (
    <aside className="cc2-side" aria-label={t("sidebar.label")} data-testid="codex-sidebar">
      <div className="cc2-side__head">
        <h2 className="cc2-side__title">{t("sidebar.title")}</h2>
        <IconButton label={t("sidebar.reload")} icon={<RefreshCw size={14} />} onClick={() => void loadThreads(projectId)} tooltip="bottom" />
        <IconButton label={t("sidebar.toggle")} icon={<PanelLeftClose size={15} />} onClick={onCollapse} tooltip="bottom" />
      </div>
      <button type="button" className="cc2-new" onClick={onNew} disabled={busy} data-testid="codex-new-chat">
        {busy ? <Spinner size={13} /> : <MessageSquarePlus size={15} aria-hidden />}
        <span>{t("sidebar.newChat")}</span>
      </button>
      <label className="cc2-search">
        <Search size={13} aria-hidden />
        <input type="search" value={query} placeholder={t("sidebar.searchPlaceholder")} aria-label={t("sidebar.search")} onChange={(e) => setQuery(e.target.value)} />
      </label>
      <nav className="cc2-side__list">
        {threads === undefined && loading ? (
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
                  <ChatRow key={e.key} entry={e} projectId={projectId} now={now} active={e.sessionId !== null && e.sessionId === activeSessionId} />
                ))}
              </ul>
            </section>
          ))
        )}
        {threadsError && <p className="cc2-side__empty">{t("sidebar.threadsError", { error: threadsError })}</p>}
      </nav>
      <div className="cc2-side__foot">
        <LimitsCard chat={active?.chat ?? null} />
      </div>
    </aside>
  );
}
