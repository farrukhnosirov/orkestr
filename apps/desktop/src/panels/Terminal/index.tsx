import "./terminal.css";

import type { OrkestrClient } from "@orkestr/client";
import type { OpenTerminalParams, ShellDescriptor, TerminalSession } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Spinner, StatusDot } from "@orkestr/ui";
import { clsx } from "clsx";
import { Bot, ChevronDown, Plus, RotateCcw, SquareTerminal, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";

import { maybeClient } from "../../core/client";
import { useAppStore } from "../../store/app";
import type { PanelProps } from "../types";
import {
  errorMessage,
  isAgentSession,
  isReadOnly,
  nextActiveAfterClose,
  normalizeTitle,
  orderShells,
  sessionSubtitle,
  sessionsForProject,
} from "./logic";
import { scopeKey, useTerminalUi } from "./store";
import { TerminalView } from "./TerminalView";

const POLL_MS = 4000;

/** Terminal panel: PTY sessions from the core, one xterm.js view per tab. */
export default function TerminalPanel({ projectId }: PanelProps) {
  const connectionState = useAppStore((s) => s.connection.state);
  const project = useAppStore((s) => s.projects.find((p) => p.id === projectId) ?? null);
  const client = connectionState === "open" ? maybeClient() : null;

  const [sessions, setSessions] = useState<TerminalSession[] | null>(null);
  const [shells, setShells] = useState<ShellDescriptor[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [opening, setOpening] = useState(false);

  const activeMap = useTerminalUi((s) => s.active);
  const setActive = useTerminalUi((s) => s.setActive);
  const openRequest = useTerminalUi((s) => s.openRequest);

  const refresh = useCallback(async (c: OrkestrClient) => {
    const { terminals } = await c.call("terminal.list", {});
    setSessions(terminals);
  }, []);

  // Initial load + polling (agent command views are created by the core).
  useEffect(() => {
    if (!client) return;
    let cancelled = false;
    setLoadError(null);
    Promise.all([client.call("terminal.shells", {}), refresh(client)])
      .then(([res]) => {
        if (!cancelled) setShells(orderShells(res.shells));
      })
      .catch((e: unknown) => {
        if (!cancelled) setLoadError(errorMessage(e));
      });
    const timer = setInterval(() => {
      refresh(client).catch(() => undefined);
    }, POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [client, refresh]);

  const visible = useMemo(() => (sessions ? sessionsForProject(sessions, projectId) : []), [sessions, projectId]);
  const storedActive = activeMap[scopeKey(projectId)];
  const activeId: string | null =
    storedActive !== undefined && visible.some((s) => s.terminalId === storedActive)
      ? storedActive
      : (visible[0]?.terminalId ?? null);

  const openTerminal = useCallback(
    async (shellId?: string, title?: string) => {
      if (!client) return;
      setOpening(true);
      setActionError(null);
      const params: OpenTerminalParams = {};
      if (project?.rootPath && projectId) params.projectId = projectId;
      if (shellId) params.shellId = shellId;
      if (title) params.title = title;
      try {
        const { terminal } = await client.call("terminal.open", params);
        setSessions((prev) => [...(prev ?? []).filter((s) => s.terminalId !== terminal.terminalId), terminal]);
        setActive(projectId, terminal.terminalId);
      } catch (e) {
        setActionError(errorMessage(e));
      } finally {
        setOpening(false);
      }
    },
    [client, project?.rootPath, projectId, setActive],
  );

  const closeTerminal = useCallback(
    async (terminalId: string) => {
      if (!client) return;
      setActionError(null);
      const ids = visible.map((s) => s.terminalId);
      try {
        await client.call("terminal.close", { terminalId });
      } catch (e) {
        // Already gone on the core side is fine; anything else is shown.
        const msg = errorMessage(e);
        if (!/not found/i.test(msg)) setActionError(msg);
      }
      setSessions((prev) => (prev ?? []).filter((s) => s.terminalId !== terminalId));
      if (activeId === terminalId) setActive(projectId, nextActiveAfterClose(ids, terminalId));
    },
    [client, visible, activeId, projectId, setActive],
  );

  const renameTerminal = useCallback(
    async (terminalId: string, title: string) => {
      if (!client) return;
      try {
        const { terminal } = await client.call("terminal.rename", { terminalId, title });
        setSessions((prev) => (prev ?? []).map((s) => (s.terminalId === terminalId ? terminal : s)));
      } catch (e) {
        setActionError(errorMessage(e));
      }
    },
    [client],
  );

  const onExited = useCallback((terminalId: string, exitCode: number | null) => {
    setSessions((prev) =>
      (prev ?? []).map((s) => (s.terminalId === terminalId ? { ...s, status: "exited", exitCode } : s)),
    );
  }, []);

  // "Open Terminal" command: open one when none is running, otherwise just show the panel.
  const handledRequest = useRef(openRequest);
  useEffect(() => {
    if (openRequest === handledRequest.current || !client || sessions === null) return;
    handledRequest.current = openRequest;
    if (!visible.some((s) => s.status === "running" && !isReadOnly(s))) void openTerminal();
  }, [openRequest, client, sessions, visible, openTerminal]);

  if (!client) {
    return (
      <div className="term-panel term-panel--center">
        <EmptyState
          icon={<SquareTerminal size={26} strokeWidth={1.6} />}
          title="Waiting for the core"
          description="Terminals run inside the Orkestr core. They appear here as soon as the connection is up."
          action={<Spinner size={18} />}
        />
      </div>
    );
  }

  if (loadError && sessions === null) {
    return (
      <div className="term-panel term-panel--center">
        <EmptyState
          icon={<SquareTerminal size={26} strokeWidth={1.6} />}
          title="Terminals unavailable"
          description={loadError}
          action={
            <Button
              variant="secondary"
              leftIcon={<RotateCcw size={14} />}
              onClick={() => {
                setLoadError(null);
                refresh(client).catch((e: unknown) => setLoadError(errorMessage(e)));
              }}
            >
              Retry
            </Button>
          }
        />
      </div>
    );
  }

  if (sessions === null) {
    return (
      <div className="term-panel term-panel--center">
        <div className="term-loading">
          <Spinner size={18} /> Loading terminals…
        </div>
      </div>
    );
  }

  const active = visible.find((s) => s.terminalId === activeId) ?? null;
  const noRoot = projectId !== null && !project?.rootPath;

  return (
    <div className="term-panel">
      <div className="term-bar">
        <TabStrip
          sessions={visible}
          activeId={activeId}
          onSelect={(id) => setActive(projectId, id)}
          onClose={(id) => void closeTerminal(id)}
          onRename={(id, title) => void renameTerminal(id, title)}
        />
        <NewTerminalButton shells={shells} busy={opening} onOpen={(id) => void openTerminal(id)} />
      </div>

      {actionError && (
        <div className="term-alert" role="alert">
          <span>{actionError}</span>
          <IconButton label="Dismiss" size="sm" icon={<X size={13} />} onClick={() => setActionError(null)} />
        </div>
      )}

      {active && isAgentSession(active) && (
        <div className="term-banner">
          <Bot size={14} />
          <span>
            {active.kind === "commandView" ? "Agent command output" : "Agent terminal"} · read-only — commands are
            checked by the security gate before they run.
          </span>
        </div>
      )}

      <div className="term-body">
        {visible.length === 0 ? (
          <EmptyState
            icon={<SquareTerminal size={26} strokeWidth={1.6} />}
            title="No terminals yet"
            description={
              noRoot
                ? "This project has no root folder, so the terminal opens in your home folder."
                : projectId
                  ? "Open a shell in the project folder. Sessions keep running when you switch panels or reload."
                  : "Open a shell in your home folder. Sessions keep running when you switch panels or reload."
            }
            action={
              <Button variant="primary" leftIcon={<Plus size={14} />} loading={opening} onClick={() => void openTerminal()}>
                New terminal
              </Button>
            }
          />
        ) : (
          visible.map((s) => (
            <TerminalView key={s.terminalId} client={client} session={s} active={s.terminalId === activeId} onExited={onExited} />
          ))
        )}
        {active && active.status === "exited" && active.kind === "pty" && (
          <div className="term-exit" role="status">
            <span>Process {sessionSubtitle(active)}.</span>
            {!isAgentSession(active) && (
              <Button
                size="sm"
                variant="secondary"
                leftIcon={<RotateCcw size={13} />}
                onClick={() => {
                  void openTerminal(active.shellId === "custom" ? undefined : active.shellId, active.title).then(() =>
                    closeTerminal(active.terminalId),
                  );
                }}
              >
                Restart
              </Button>
            )}
            <Button size="sm" variant="ghost" onClick={() => void closeTerminal(active.terminalId)}>
              Close
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}

// ── Tabs ──────────────────────────────────────────────────

interface TabStripProps {
  sessions: TerminalSession[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
  onRename: (id: string, title: string) => void;
}

function TabStrip({ sessions, activeId, onSelect, onClose, onRename }: TabStripProps) {
  const [editing, setEditing] = useState<string | null>(null);
  const tabRefs = useRef(new Map<string, HTMLDivElement>());

  const focusTab = (id: string | undefined) => {
    if (!id) return;
    onSelect(id);
    tabRefs.current.get(id)?.focus();
  };

  const onKeyDown = (e: ReactKeyboardEvent, s: TerminalSession, idx: number) => {
    if (editing) return;
    switch (e.key) {
      case "ArrowRight":
        e.preventDefault();
        focusTab(sessions[(idx + 1) % sessions.length]?.terminalId);
        break;
      case "ArrowLeft":
        e.preventDefault();
        focusTab(sessions[(idx - 1 + sessions.length) % sessions.length]?.terminalId);
        break;
      case "Home":
        e.preventDefault();
        focusTab(sessions[0]?.terminalId);
        break;
      case "End":
        e.preventDefault();
        focusTab(sessions[sessions.length - 1]?.terminalId);
        break;
      case "F2":
        e.preventDefault();
        setEditing(s.terminalId);
        break;
      case "Delete":
        e.preventDefault();
        onClose(s.terminalId);
        break;
      case "Enter":
      case " ":
        e.preventDefault();
        onSelect(s.terminalId);
        break;
      default:
        break;
    }
  };

  return (
    <div className="term-tabs" role="tablist" aria-label="Terminal sessions">
      {sessions.map((s, idx) => {
        const selected = s.terminalId === activeId;
        const agent = isAgentSession(s);
        return (
          <div
            key={s.terminalId}
            ref={(el) => {
              if (el) tabRefs.current.set(s.terminalId, el);
              else tabRefs.current.delete(s.terminalId);
            }}
            role="tab"
            aria-selected={selected}
            tabIndex={selected ? 0 : -1}
            title={`${s.title} — ${sessionSubtitle(s)}${editing ? "" : " (F2 to rename)"}`}
            className={clsx("term-tab", selected && "term-tab--active", s.status === "exited" && "term-tab--exited")}
            onClick={() => onSelect(s.terminalId)}
            onDoubleClick={() => !agent && setEditing(s.terminalId)}
            onKeyDown={(e) => onKeyDown(e, s, idx)}
          >
            <StatusDot
              status={s.status === "running" ? "online" : s.exitCode === 0 || s.exitCode === null ? "offline" : "error"}
              pulse={s.status === "running" && agent}
            />
            {agent && (
              <Badge tone="violet" icon={<Bot size={11} />} className="term-tab__badge">
                agent
              </Badge>
            )}
            {editing === s.terminalId ? (
              <RenameInput
                initial={s.title}
                onDone={(title) => {
                  setEditing(null);
                  if (title && title !== s.title) onRename(s.terminalId, title);
                  tabRefs.current.get(s.terminalId)?.focus();
                }}
              />
            ) : (
              <span className="term-tab__title">{s.title}</span>
            )}
            <button
              type="button"
              className="term-tab__close"
              aria-label={`Close ${s.title}`}
              tabIndex={-1}
              onClick={(e) => {
                e.stopPropagation();
                onClose(s.terminalId);
              }}
            >
              <X size={12} />
            </button>
          </div>
        );
      })}
    </div>
  );
}

function RenameInput({ initial, onDone }: { initial: string; onDone: (title: string | null) => void }) {
  const [value, setValue] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  const finish = (title: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(title);
  };
  const valid = normalizeTitle(value) !== null;
  return (
    <input
      ref={ref}
      className={clsx("term-tab__rename", !valid && "term-tab__rename--invalid")}
      value={value}
      maxLength={80}
      aria-label="Terminal name"
      aria-invalid={!valid}
      onChange={(e) => setValue(e.target.value)}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(normalizeTitle(value));
        if (e.key === "Escape") finish(null);
      }}
      onBlur={() => finish(normalizeTitle(value))}
    />
  );
}

// ── New terminal (split button + shell menu) ─────────────

interface NewTerminalButtonProps {
  shells: ShellDescriptor[];
  busy: boolean;
  onOpen: (shellId?: string) => void;
}

function NewTerminalButton({ shells, busy, onOpen }: NewTerminalButtonProps) {
  const [open, setOpen] = useState(false);
  const [index, setIndex] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const toggleRef = useRef<HTMLButtonElement>(null);
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const defaultShell = shells.find((s) => s.isDefault) ?? shells[0];

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);

  useEffect(() => {
    if (open) itemRefs.current[index]?.focus();
  }, [open, index]);

  const choose = (id: string) => {
    setOpen(false);
    toggleRef.current?.focus();
    onOpen(id);
  };

  const onMenuKey = (e: ReactKeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setIndex((i) => (i + 1) % Math.max(1, shells.length));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setIndex((i) => (i - 1 + shells.length) % Math.max(1, shells.length));
    } else if (e.key === "Home") {
      e.preventDefault();
      setIndex(0);
    } else if (e.key === "End") {
      e.preventDefault();
      setIndex(Math.max(0, shells.length - 1));
    } else if (e.key === "Escape" || e.key === "Tab") {
      setOpen(false);
      toggleRef.current?.focus();
    }
  };

  return (
    <div className="term-new" ref={rootRef}>
      <IconButton
        label={defaultShell ? `New terminal (${defaultShell.name})` : "New terminal"}
        icon={busy ? <Spinner size={13} /> : <Plus size={15} />}
        size="sm"
        disabled={busy}
        onClick={() => onOpen(defaultShell?.id)}
        tooltip="bottom"
      />
      <IconButton
        ref={toggleRef}
        label="Choose shell"
        icon={<ChevronDown size={14} />}
        size="sm"
        disabled={busy || shells.length === 0}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => {
          setIndex(0);
          setOpen((o) => !o);
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" && !open) {
            e.preventDefault();
            setIndex(0);
            setOpen(true);
          }
        }}
        tooltip="bottom"
      />
      {open && (
        <div className="term-menu" role="menu" aria-label="Shells" onKeyDown={onMenuKey}>
          {shells.map((s, i) => (
            <button
              key={s.id}
              ref={(el) => {
                itemRefs.current[i] = el;
              }}
              type="button"
              role="menuitem"
              tabIndex={i === index ? 0 : -1}
              className={clsx("term-menu__item", i === index && "term-menu__item--focus")}
              onMouseEnter={() => setIndex(i)}
              onClick={() => choose(s.id)}
            >
              <SquareTerminal size={14} />
              <span className="term-menu__name">{s.name}</span>
              {s.isDefault && <Badge tone="accent">default</Badge>}
              <span className="term-menu__path" title={s.path}>
                {s.path}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
