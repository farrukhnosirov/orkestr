import { Badge, EmptyState, IconButton, Input, Spinner, Tabs } from "@orkestr/ui";
import type { Event } from "@orkestr/protocol";
import { TOPIC_ALL, topic } from "@orkestr/protocol";
import { clsx } from "clsx";
import { ArrowDownToLine, History, Pause, Play, ScrollText, Search, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { maybeClient } from "../../core/client";
import { useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import type { PanelProps } from "../types";
import { mergeEvents } from "./merge";
import { PauseQueue } from "./pauseQueue";
import { ReplayView } from "./replay/ReplayView";
import { type EventCategory, categoryOf, describeEvent, formatTime, severityOf } from "./describe";

const MAX_EVENTS = 1000;
const HISTORY = 200;

type Filter = "all" | "project" | "logs" | "problems";

const FILTERS: Filter[] = ["all", "project", "logs", "problems"];

const CATEGORY_TONE: Record<EventCategory, "accent" | "violet" | "success" | "warning" | "danger" | "info" | "neutral"> = {
  project: "accent",
  task: "violet",
  agent: "info",
  tool: "neutral",
  build: "success",
  approval: "warning",
  provider: "violet",
  system: "neutral",
  log: "neutral",
};

type LogsView = "events" | "replay";

/** Logs panel: the live event stream and the Execution Replay (§21) tab. */
export default function LogsPanel({ projectId }: PanelProps) {
  const { t } = useT("logs");
  const [view, setView] = useState<LogsView>("events");
  return (
    <div className="logs-shell">
      <Tabs
        className="logs-shell__tabs"
        aria-label={t("view.label")}
        items={[
          { id: "events", label: t("view.events"), icon: <ScrollText size={13} /> },
          { id: "replay", label: t("view.replay"), icon: <History size={13} /> },
        ]}
        value={view}
        onChange={setView}
      />
      {view === "events" ? <EventsView projectId={projectId} /> : <ReplayView projectId={projectId} />}
    </div>
  );
}

/** Live event stream from the core (all topics), with history, filters and pause. */
function EventsView({ projectId }: PanelProps) {
  const { t, i18n } = useT("logs");
  const connectionState = useAppStore((s) => s.connection.state);
  const [events, setEvents] = useState<Event[]>([]);
  const [loading, setLoading] = useState(true);
  const [paused, setPaused] = useState(false);
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);
  const [follow, setFollow] = useState(true);
  const [queued, setQueued] = useState(0);
  const pausedRef = useRef(paused);
  const queueRef = useRef(new PauseQueue<Event>(MAX_EVENTS));
  const listRef = useRef<HTMLDivElement>(null);
  pausedRef.current = paused;

  const append = useCallback((incoming: Event[]) => {
    if (incoming.length === 0) return;
    // Live events may arrive before the history page that precedes them, so
    // merge in seq order instead of appending in arrival order.
    setEvents((prev) => mergeEvents(prev, incoming, MAX_EVENTS));
  }, []);

  // History + live subscription (re-established after reconnects by the client).
  useEffect(() => {
    if (connectionState !== "open") return;
    const client = maybeClient();
    if (!client) return;
    let cancelled = false;
    setLoading(true);
    client
      .call("events.replay", { limit: HISTORY, newestFirst: true })
      .then((res) => {
        if (!cancelled) append(res.events);
      })
      .catch((e: unknown) => console.warn("events.replay failed", e))
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    const off = client.subscribe([TOPIC_ALL], (ev) => {
      // High-volume ephemeral streams (2 s node heartbeats, terminal bytes,
      // language-server JSON-RPC traffic and install progress) have their own
      // views (System settings, Terminal panel, Code panel, Extensions panel).
      if (
        ev.kind === "NodeStatus" ||
        ev.kind === "TerminalOutput" ||
        ev.kind === "LspMessage" ||
        ev.kind === "ExtensionInstallProgress"
      )
        return;
      if (pausedRef.current) {
        queueRef.current.push(ev);
        setQueued(queueRef.current.count);
      } else {
        append([ev]);
      }
    });
    return () => {
      cancelled = true;
      off();
    };
  }, [connectionState, append]);

  // Flush buffered events on resume.
  useEffect(() => {
    if (!paused && queueRef.current.count > 0) {
      append(queueRef.current.drain());
      setQueued(0);
    }
  }, [paused, append]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    const projectTopic = projectId ? topic.project(projectId) : null;
    return events.filter((ev) => {
      if (filter === "project" && (!projectTopic || ev.projectId !== projectId)) return false;
      if (filter === "logs" && ev.kind !== "Log") return false;
      if (filter === "problems" && !["error", "warning"].includes(severityOf(ev))) return false;
      if (!q) return true;
      return ev.kind.toLowerCase().includes(q) || describeEvent(ev).toLowerCase().includes(q);
    });
    // describeEvent() output depends on the UI language.
  }, [events, filter, query, projectId, i18n.language]);

  // Auto-scroll to the newest entry while following.
  useEffect(() => {
    const el = listRef.current;
    if (el && follow) el.scrollTop = el.scrollHeight;
  }, [visible, follow]);

  const onScroll = () => {
    const el = listRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
    if (atBottom !== follow) setFollow(atBottom);
  };

  return (
    <div className="logs-panel">
      <div className="logs-toolbar">
        <Tabs aria-label={t("filter.label")} items={FILTERS.map((id) => ({ id, label: t(`filter.${id}`) }))} value={filter} onChange={setFilter} />
        <Input
          className="logs-search"
          placeholder={t("search.placeholder")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          leftIcon={<Search size={14} />}
          aria-label={t("search.label")}
        />
        <div className="logs-toolbar__spacer" />
        <Badge tone={paused ? "warning" : "success"}>{paused ? t("paused", { queued }) : t("live")}</Badge>
        <IconButton
          label={paused ? t("resume") : t("pause")}
          icon={paused ? <Play size={15} /> : <Pause size={15} />}
          onClick={() => setPaused((p) => !p)}
          tooltip="bottom"
        />
        <IconButton
          label={t("jumpToLatest")}
          icon={<ArrowDownToLine size={15} />}
          active={follow}
          onClick={() => setFollow(true)}
          tooltip="bottom"
        />
        <IconButton
          label={t("clear")}
          icon={<Trash2 size={15} />}
          onClick={() => {
            setEvents([]);
            queueRef.current.drain();
            setQueued(0);
          }}
          tooltip="bottom"
        />
      </div>

      <div className="logs-list" ref={listRef} onScroll={onScroll} role="log" aria-live="polite" aria-relevant="additions">
        {loading && events.length === 0 ? (
          <div className="logs-loading">
            <Spinner size={18} /> {t("loading")}
          </div>
        ) : visible.length === 0 ? (
          <EmptyState
            icon={<ScrollText size={26} strokeWidth={1.6} />}
            title={events.length === 0 ? t("empty.title") : t("empty.noMatchTitle")}
            description={events.length === 0 ? t("empty.description") : t("empty.noMatchDescription")}
          />
        ) : (
          visible.map((ev) => {
            const sev = severityOf(ev);
            const cat = categoryOf(ev);
            const open = expanded === ev.id;
            return (
              <div key={ev.id} className={clsx("logs-row", `logs-row--${sev}`, open && "logs-row--open")}>
                <button type="button" className="logs-row__main" onClick={() => setExpanded(open ? null : ev.id)} aria-expanded={open}>
                  <span className="logs-row__time">{formatTime(ev.ts)}</span>
                  <span className="logs-row__seq">{ev.seq > 0 ? t("seq", { seq: ev.seq }) : t("local")}</span>
                  <Badge tone={sev === "error" ? "danger" : sev === "warning" ? "warning" : CATEGORY_TONE[cat]} className="logs-row__kind">
                    {ev.kind}
                  </Badge>
                  <span className="logs-row__text">{describeEvent(ev)}</span>
                </button>
                {open && <pre className="logs-row__detail">{JSON.stringify(ev, null, 2)}</pre>}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
