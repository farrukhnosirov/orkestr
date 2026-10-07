import type { KnowledgeHit, KnowledgeSource } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { BookOpen, FolderPlus, RefreshCw, Search, Trash2 } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { errorMessage, requireClient, useRpcQuery } from "../../lib/rpcQuery";
import { useMemoryFocus } from "./focus";
import { INDEX_STATUS_TONE } from "./kinds";
import { Highlighted } from "./markdown";

/** Knowledge sources (folders/files ingested into FTS) and search over their chunks. */
export function KnowledgeBase({ projectId }: { projectId: string | null }) {
  const { t } = useT(["memory", "common"]);
  const fmt = useFormat();
  const [location, setLocation] = useState("");
  const [adding, setAdding] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [debounced, setDebounced] = useState("");
  const scopeKey = projectId ?? "global";

  useEffect(() => {
    const timer = setTimeout(() => setDebounced(query.trim()), 250);
    return () => clearTimeout(timer);
  }, [query]);

  // A global-search hit: run the same search here.
  const focus = useMemoryFocus((s) => s.focus);
  useEffect(() => {
    if (focus?.tab !== "knowledge") return;
    setQuery(focus.query);
    setDebounced(focus.query.trim());
    useMemoryFocus.getState().clear();
  }, [focus]);

  const sources = useRpcQuery<KnowledgeSource[]>(
    `knowledge:${scopeKey}`,
    async (client) =>
      (await client.call("knowledge.list", projectId ? { projectId, includeGlobal: true } : {})).sources,
  );
  const hits = useRpcQuery<KnowledgeHit[]>(
    `knowledge-search:${scopeKey}:${debounced}`,
    async (client) =>
      (await client.call("knowledge.search", { query: debounced, ...(projectId ? { projectId } : {}), limit: 30 })).hits,
    { enabled: debounced.length > 0 },
  );

  const add = async (e: FormEvent) => {
    e.preventDefault();
    const loc = location.trim();
    if (!loc) return;
    setAdding(true);
    setActionError(null);
    try {
      const res = await requireClient().call("knowledge.add", { location: loc, ...(projectId ? { projectId } : {}) });
      if (res.source.status === "Failed") setActionError(res.source.error ?? t("knowledge.indexingFailed"));
      setLocation("");
      sources.reload();
    } catch (err) {
      setActionError(errorMessage(err));
    } finally {
      setAdding(false);
    }
  };

  const run = async (id: string, fn: () => Promise<unknown>) => {
    setBusyId(id);
    setActionError(null);
    try {
      await fn();
      sources.reload();
      hits.reload();
    } catch (err) {
      setActionError(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const list = sources.data ?? [];

  return (
    <div className="mem-view">
      <form className="mem-toolbar" onSubmit={(e) => void add(e)}>
        <Input
          className="mem-search mem-search--wide"
          placeholder={t("knowledge.pathPlaceholder")}
          value={location}
          onChange={(e) => setLocation(e.target.value)}
          leftIcon={<FolderPlus size={14} />}
          aria-label={t("knowledge.pathLabel")}
        />
        <Button variant="primary" size="sm" type="submit" loading={adding} disabled={!location.trim()}>
          {t("knowledge.add")}
        </Button>
      </form>
      {(actionError || sources.error) && (
        <div className="mem-error" role="alert">
          {actionError ?? sources.error}
        </div>
      )}
      <div className="mem-scroll">
        {sources.loading && sources.data === null ? (
          <div className="mem-loading">
            <Spinner size={18} /> {t("knowledge.loading")}
          </div>
        ) : list.length === 0 ? (
          <EmptyState
            icon={<BookOpen size={26} strokeWidth={1.6} />}
            title={t("knowledge.emptyTitle")}
            description={t("knowledge.emptyDescription")}
          />
        ) : (
          <ul className="kb-sources" aria-label={t("knowledge.sources")}>
            {list.map((s) => (
              <li key={s.id} className="kb-source">
                <div className="kb-source__main">
                  <div className="kb-source__title">
                    <strong>{s.name}</strong>
                    <Badge tone={INDEX_STATUS_TONE[s.status]}>{t(`knowledge.status.${s.status}`)}</Badge>
                    <Badge tone="neutral">{t(`knowledge.kind.${s.kind}`)}</Badge>
                    {s.projectId === null && <Badge tone="violet">{t("knowledge.global")}</Badge>}
                  </div>
                  <code className="kb-source__loc" title={s.location}>
                    {s.location}
                  </code>
                  <div className="mem-card__meta">
                    <span>
                      {t("knowledge.files", { count: s.fileCount })} · {t("knowledge.chunks", { count: s.chunkCount })}
                    </span>
                    {s.indexedAt && <span title={fmt.date(s.indexedAt)}>{t("knowledge.indexed", { time: fmt.relative(s.indexedAt) })}</span>}
                  </div>
                  {s.error && <p className={s.status === "Failed" ? "kb-source__error" : "kb-source__note"}>{s.error}</p>}
                </div>
                <div className="mem-card__actions">
                  <IconButton
                    size="sm"
                    label={t("knowledge.reindex")}
                    icon={busyId === s.id ? <Spinner size={13} /> : <RefreshCw size={14} />}
                    disabled={busyId !== null}
                    onClick={() => void run(s.id, () => requireClient().call("knowledge.reindex", { id: s.id }))}
                  />
                  <IconButton
                    size="sm"
                    label={t("knowledge.remove")}
                    variant="danger"
                    icon={<Trash2 size={14} />}
                    disabled={busyId !== null}
                    onClick={() => {
                      if (window.confirm(t("knowledge.confirmRemove", { name: s.name }))) {
                        void run(s.id, () => requireClient().call("knowledge.remove", { id: s.id }));
                      }
                    }}
                  />
                </div>
              </li>
            ))}
          </ul>
        )}
        {list.length > 0 && (
          <section className="kb-search" aria-label={t("knowledge.searchLabel")}>
            <Input
              placeholder={t("knowledge.searchPlaceholder")}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              leftIcon={<Search size={14} />}
              aria-label={t("knowledge.searchLabel")}
            />
            {debounced &&
              (hits.loading && hits.data === null ? (
                <div className="mem-loading">
                  <Spinner size={16} /> {t("knowledge.searching")}
                </div>
              ) : hits.error ? (
                <div className="mem-error" role="alert">
                  {hits.error}
                </div>
              ) : (hits.data ?? []).length === 0 ? (
                <p className="mem-muted">{t("knowledge.noMatches")}</p>
              ) : (
                <ul className="kb-hits">
                  {(hits.data ?? []).map((h) => (
                    <li key={`${h.sourceId}:${h.path}:${h.ordinal}`} className="kb-hit">
                      <div className="kb-hit__where">
                        <span>{h.sourceName}</span>
                        <code>{h.path}</code>
                        {h.headingPath && <span className="mem-muted">{h.headingPath}</span>}
                      </div>
                      <p className="kb-hit__snippet">
                        <Highlighted text={h.snippet} />
                      </p>
                    </li>
                  ))}
                </ul>
              ))}
          </section>
        )}
      </div>
    </div>
  );
}
