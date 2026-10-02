import type { KnowledgeHit, KnowledgeSource } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { BookOpen, FolderPlus, RefreshCw, Search, Trash2 } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";

import { errorMessage, requireClient, useRpcQuery } from "../../lib/rpcQuery";
import { INDEX_STATUS_TONE, relativeTime } from "./kinds";
import { Highlighted } from "./markdown";

/** Knowledge sources (folders/files ingested into FTS) and search over their chunks. */
export function KnowledgeBase({ projectId }: { projectId: string | null }) {
  const [location, setLocation] = useState("");
  const [adding, setAdding] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [debounced, setDebounced] = useState("");
  const scopeKey = projectId ?? "global";

  useEffect(() => {
    const t = setTimeout(() => setDebounced(query.trim()), 250);
    return () => clearTimeout(t);
  }, [query]);

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
      if (res.source.status === "Failed") setActionError(res.source.error ?? "Indexing failed.");
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
          placeholder="Absolute path of a docs folder or a .md / .txt / code file"
          value={location}
          onChange={(e) => setLocation(e.target.value)}
          leftIcon={<FolderPlus size={14} />}
          aria-label="Knowledge source path"
        />
        <Button variant="primary" size="sm" type="submit" loading={adding} disabled={!location.trim()}>
          Add source
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
            <Spinner size={18} /> Loading knowledge sources…
          </div>
        ) : list.length === 0 ? (
          <EmptyState
            icon={<BookOpen size={26} strokeWidth={1.6} />}
            title="No knowledge sources"
            description="Add a docs folder or Markdown / text / code files. They are split by headings into searchable chunks that agents can cite. PDF and URL sources are not supported yet."
          />
        ) : (
          <ul className="kb-sources" aria-label="Knowledge sources">
            {list.map((s) => (
              <li key={s.id} className="kb-source">
                <div className="kb-source__main">
                  <div className="kb-source__title">
                    <strong>{s.name}</strong>
                    <Badge tone={INDEX_STATUS_TONE[s.status]}>{s.status}</Badge>
                    <Badge tone="neutral">{s.kind}</Badge>
                    {s.projectId === null && <Badge tone="violet">Global</Badge>}
                  </div>
                  <code className="kb-source__loc" title={s.location}>
                    {s.location}
                  </code>
                  <div className="mem-card__meta">
                    <span>
                      {s.fileCount} file(s) · {s.chunkCount} chunk(s)
                    </span>
                    {s.indexedAt && <span>indexed {relativeTime(s.indexedAt)}</span>}
                  </div>
                  {s.error && <p className={s.status === "Failed" ? "kb-source__error" : "kb-source__note"}>{s.error}</p>}
                </div>
                <div className="mem-card__actions">
                  <IconButton
                    size="sm"
                    label="Reindex"
                    icon={busyId === s.id ? <Spinner size={13} /> : <RefreshCw size={14} />}
                    disabled={busyId !== null}
                    onClick={() => void run(s.id, () => requireClient().call("knowledge.reindex", { id: s.id }))}
                  />
                  <IconButton
                    size="sm"
                    label="Remove"
                    variant="danger"
                    icon={<Trash2 size={14} />}
                    disabled={busyId !== null}
                    onClick={() => {
                      if (window.confirm(`Remove "${s.name}" from the knowledge base? Files on disk are not touched.`)) {
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
          <section className="kb-search" aria-label="Search knowledge">
            <Input
              placeholder="Search the knowledge base…"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              leftIcon={<Search size={14} />}
              aria-label="Search knowledge"
            />
            {debounced &&
              (hits.loading && hits.data === null ? (
                <div className="mem-loading">
                  <Spinner size={16} /> Searching…
                </div>
              ) : hits.error ? (
                <div className="mem-error" role="alert">
                  {hits.error}
                </div>
              ) : (hits.data ?? []).length === 0 ? (
                <p className="mem-muted">No matches.</p>
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
