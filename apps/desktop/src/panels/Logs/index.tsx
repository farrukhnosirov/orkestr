import { Badge, EmptyState, IconButton, Input, Spinner, Tabs } from "@orkestr/ui";
import type { Event } from "@orkestr/protocol";
import { TOPIC_ALL, topic } from "@orkestr/protocol";
import { clsx } from "clsx";
import { ArrowDownToLine, Pause, Play, ScrollText, Search, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { maybeClient } from "../../core/client";
import { useAppStore } from "../../store/app";
import type { PanelProps } from "../types";
import { type EventCategory, categoryOf, describeEvent, formatTime, severityOf } from "./describe";

const MAX_EVENTS = 1000;
const HISTORY = 200;

type Filter = "all" | "project" | "logs" | "problems";

const FILTERS: { id: Filter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "project", label: "This project" },
  { id: "logs", label: "Core logs" },
  { id: "problems", label: "Problems" },
];

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

/** Live event stream from the core (all topics), with history, filters and pause. */
export default function LogsPanel({ projectId }: PanelProps) {
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
  const queueRef = useRef<Event[]>([]);
  const listRef = useRef<HTMLDivElement>(null);
  pausedRef.current = paused;

  const append = useCallback((incoming: Event[]) => {
    if (incoming.length === 0) return;
    setEvents((prev) => {
      const seen = new Set(prev.map((e) => e.id));
      const merged = [...prev, ...incoming.filter((e) => !seen.has(e.id))];
      return merged.length > MAX_EVENTS ? merged.slice(merged.length - MAX_EVENTS) : merged;
    });
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
      // High-volume ephemeral streams (2 s node heartbeats, terminal bytes)
      // have their own views (System settings, Terminal panel).
      if (ev.kind === "NodeStatus" || ev.kind === "TerminalOutput") return;
      if (pausedRef.current) {
        queueRef.current.push(ev);
        setQueued(queueRef.current.length);
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
    if (!paused && queueRef.current.length > 0) {
      append(queueRef.current);
      queueRef.current = [];
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
  }, [events, filter, query, projectId]);

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
        <Tabs aria-label="Event filter" items={FILTERS} value={filter} onChange={setFilter} />
        <Input
          className="logs-search"
          placeholder="Filter events…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          leftIcon={<Search size={14} />}
          aria-label="Filter events"
        />
        <div className="logs-toolbar__spacer" />
        <Badge tone={paused ? "warning" : "success"}>{paused ? `Paused · ${queued} queued` : "Live"}</Badge>
        <IconButton
          label={paused ? "Resume stream" : "Pause stream"}
          icon={paused ? <Play size={15} /> : <Pause size={15} />}
          onClick={() => setPaused((p) => !p)}
          tooltip="bottom"
        />
        <IconButton
          label="Jump to latest"
          icon={<ArrowDownToLine size={15} />}
          active={follow}
          onClick={() => setFollow(true)}
          tooltip="bottom"
        />
        <IconButton
          label="Clear view"
          icon={<Trash2 size={15} />}
          onClick={() => {
            setEvents([]);
            queueRef.current = [];
            setQueued(0);
          }}
          tooltip="bottom"
        />
      </div>

      <div className="logs-list" ref={listRef} onScroll={onScroll} role="log" aria-live="polite" aria-relevant="additions">
        {loading && events.length === 0 ? (
          <div className="logs-loading">
            <Spinner size={18} /> Loading event history…
          </div>
        ) : visible.length === 0 ? (
          <EmptyState
            icon={<ScrollText size={26} strokeWidth={1.6} />}
            title={events.length === 0 ? "No events yet" : "No matching events"}
            description={
              events.length === 0
                ? "Everything the core does — projects, tasks, agents, tools, builds — streams here live. Create a project to see the first events."
                : "Try another filter or search term."
            }
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
                  <span className="logs-row__seq">{ev.seq > 0 ? `#${ev.seq}` : "local"}</span>
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
