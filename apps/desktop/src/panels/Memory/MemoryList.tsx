import type { MemoryItem, MemoryKind, MemoryScope } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { BrainCircuit, Pencil, Pin, PinOff, Plus, Search, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { errorMessage, requireClient, useRpcQuery } from "../../lib/rpcQuery";
import { KIND_INFO, kindLabel } from "./kinds";
import { Highlighted, Markdown } from "./markdown";
import { MemoryEditor } from "./MemoryEditor";

export interface MemoryListProps {
  scope: MemoryScope;
  /** Kinds offered as filter chips and in the editor. */
  kinds: MemoryKind[];
}

interface Row {
  item: MemoryItem;
  snippet: string | null;
}

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

/** Memory items of one scope: FTS search, kind chips, cards, pin, edit, delete. */
export function MemoryList({ scope, kinds }: MemoryListProps) {
  const { t } = useT(["memory", "common"]);
  const fmt = useFormat();
  const [query, setQuery] = useState("");
  const [kindFilter, setKindFilter] = useState<MemoryKind | null>(null);
  const [editing, setEditing] = useState<MemoryItem | "new" | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const q = useDebounced(query.trim(), 200);
  const projectId = scope.scope === "Project" ? scope.projectId : null;
  const scopeKey = projectId ?? scope.scope;

  const { data, error, loading, reload } = useRpcQuery<Row[]>(
    `memory:${scopeKey}:${kindFilter ?? ""}:${q}`,
    async (client) => {
      const kindsParam = kindFilter ? [kindFilter] : undefined;
      if (q) {
        const res = await client.call("memory.search", {
          query: q,
          ...(projectId ? { projectId, includeGlobal: false } : {}),
          ...(kindsParam ? { kinds: kindsParam } : {}),
        });
        return res.hits.map((h) => ({ item: h.item, snippet: h.snippet }));
      }
      const res = await client.call("memory.list", { scope, ...(kindsParam ? { kinds: kindsParam } : {}) });
      return res.items.map((item) => ({ item, snippet: null }));
    },
    { topics: [projectId ? topic.project(projectId) : "global"], reloadOn: ["MemoryUpdated"] },
  );

  const rows = data ?? [];
  const counts = useMemo(() => {
    const m = new Map<MemoryKind, number>();
    for (const r of rows) m.set(r.item.kind, (m.get(r.item.kind) ?? 0) + 1);
    return m;
  }, [rows]);

  const act = async (fn: () => Promise<unknown>) => {
    setActionError(null);
    try {
      await fn();
      reload();
    } catch (e) {
      setActionError(errorMessage(e));
    }
  };

  const togglePin = (item: MemoryItem) =>
    act(() => requireClient().call("memory.update", { id: item.id, patch: { pinned: !item.pinned } }));
  const remove = (item: MemoryItem) => {
    if (!window.confirm(t("list.confirmDelete", { title: item.title }))) return;
    void act(() => requireClient().call("memory.delete", { id: item.id }));
  };

  return (
    <div className="mem-view">
      <div className="mem-toolbar">
        <Input
          className="mem-search"
          placeholder={scope.scope === "Global" ? t("list.searchGlobal") : t("list.searchProject")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          leftIcon={<Search size={14} />}
          aria-label={t("list.searchLabel")}
        />
        <div className="mem-toolbar__spacer" />
        {loading && <Spinner size={14} />}
        <Button variant="primary" size="sm" leftIcon={<Plus size={14} />} onClick={() => setEditing("new")}>
          {t("list.new")}
        </Button>
      </div>
      <div className="mem-chips" role="group" aria-label={t("list.filterByKind")}>
        <button type="button" className={clsx("mem-chip", kindFilter === null && "mem-chip--active")} aria-pressed={kindFilter === null} onClick={() => setKindFilter(null)}>
          {t("list.all")}
        </button>
        {kinds.map((k) => (
          <button
            key={k}
            type="button"
            className={clsx("mem-chip", kindFilter === k && "mem-chip--active")}
            aria-pressed={kindFilter === k}
            onClick={() => setKindFilter(kindFilter === k ? null : k)}
          >
            {kindLabel(k)}
            {kindFilter === null && counts.get(k) ? <span className="mem-chip__count">{counts.get(k)}</span> : null}
          </button>
        ))}
      </div>
      {(error || actionError) && (
        <div className="mem-error" role="alert">
          {actionError ?? error}
          {error && (
            <Button size="sm" variant="ghost" onClick={reload}>
              {t("common:actions.retry")}
            </Button>
          )}
        </div>
      )}
      <div className="mem-scroll">
        {loading && data === null ? (
          <div className="mem-loading">
            <Spinner size={18} /> {t("list.loading")}
          </div>
        ) : rows.length === 0 && !error ? (
          <EmptyState
            icon={<BrainCircuit size={26} strokeWidth={1.6} />}
            title={q || kindFilter ? t("list.emptyMatching") : scope.scope === "Global" ? t("list.emptyGlobal") : t("list.emptyProject")}
            description={q || kindFilter ? t("list.emptyMatchingHint") : t("list.emptyHint")}
            action={
              !q && !kindFilter ? (
                <Button variant="primary" leftIcon={<Plus size={14} />} onClick={() => setEditing("new")}>
                  {t("list.add")}
                </Button>
              ) : undefined
            }
          />
        ) : (
          <ul className="mem-cards" aria-label={t("list.items")}>
            {rows.map(({ item, snippet }) => {
              const info = KIND_INFO[item.kind];
              const open = expanded === item.id;
              return (
                <li key={item.id} className={clsx("mem-card", item.pinned && "mem-card--pinned")}>
                  <div className="mem-card__head">
                    <Badge tone={info.tone}>{kindLabel(item.kind)}</Badge>
                    <button type="button" className="mem-card__title" onClick={() => setExpanded(open ? null : item.id)} aria-expanded={open}>
                      {item.title}
                    </button>
                    <div className="mem-card__actions">
                      <IconButton
                        size="sm"
                        label={item.pinned ? t("list.unpin") : t("list.pin")}
                        icon={item.pinned ? <PinOff size={14} /> : <Pin size={14} />}
                        active={item.pinned}
                        onClick={() => void togglePin(item)}
                      />
                      <IconButton size="sm" label={t("common:actions.edit")} icon={<Pencil size={14} />} onClick={() => setEditing(item)} />
                      <IconButton size="sm" label={t("common:actions.delete")} variant="danger" icon={<Trash2 size={14} />} onClick={() => remove(item)} />
                    </div>
                  </div>
                  {snippet && !open ? (
                    <p className="mem-card__snippet">
                      <Highlighted text={snippet} />
                    </p>
                  ) : item.content.trim() ? (
                    <div className={clsx("mem-card__body", !open && "mem-card__body--clamped")}>
                      <Markdown source={item.content} />
                    </div>
                  ) : null}
                  <div className="mem-card__meta">
                    {item.pinned && <span className="mem-pin-label">{t("list.pinned")}</span>}
                    {item.key && <span title={t("list.ruleKey")}>{t("list.keyValue", { key: item.key })}</span>}
                    {item.tags.map((tag) => (
                      <span key={tag} className="mem-tag">
                        #{tag}
                      </span>
                    ))}
                    <span className="mem-card__time" title={fmt.date(item.updatedAt)}>
                      {item.source ? `${item.source} · ` : ""}
                      {fmt.relative(item.updatedAt)}
                    </span>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>
      {editing && (
        <MemoryEditor
          item={editing === "new" ? null : editing}
          scope={scope}
          kinds={kinds}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            reload();
          }}
        />
      )}
    </div>
  );
}
