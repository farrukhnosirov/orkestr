/**
 * History tab: paginated log; selecting a commit lists its files, selecting a
 * file shows the commit's change in the Monaco diff.
 */
import type { CommitInfo, DiffResult, FileDiff } from "@orkestr/protocol";
import { Badge, Button, EmptyState, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { History } from "lucide-react";
import { useEffect, useState } from "react";

import { getClient } from "../../core/client";
import { type DiffContent, DiffPane, Notice } from "./components";
import { relativeTime, statusBadge } from "./model";
import { errorMessage } from "./state";

const PAGE = 50;

interface Props {
  projectId: string;
  worktree: string | null;
  refresh: number;
}

export function HistoryTab({ projectId, worktree, refresh }: Props) {
  const base = worktree ? { projectId, worktree } : { projectId };
  const [commits, setCommits] = useState<CommitInfo[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<CommitInfo | null>(null);
  const [files, setFiles] = useState<DiffResult | null>(null);
  const [filesLoading, setFilesLoading] = useState(false);
  const [file, setFile] = useState<FileDiff | null>(null);
  const [diff, setDiff] = useState<DiffContent | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [diffError, setDiffError] = useState<string | null>(null);

  const load = async (skip: number) => {
    setLoading(true);
    setError(null);
    try {
      const res = await getClient().call("vcs.log", { ...base, skip, limit: PAGE });
      setCommits((prev) => (skip === 0 ? res.commits : [...prev, ...res.commits]));
      setHasMore(res.hasMore);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load(0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId, worktree, refresh]);

  useEffect(() => {
    if (!selected) return;
    let cancelled = false;
    setFilesLoading(true);
    setFiles(null);
    setFile(null);
    setDiff(null);
    getClient()
      .call("vcs.diff", { ...base, target: { kind: "Commit", commit: selected.id }, maxPatchBytes: 1 })
      .then((r) => !cancelled && setFiles(r))
      .catch((e: unknown) => !cancelled && setDiffError(errorMessage(e)))
      .finally(() => !cancelled && setFilesLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected]);

  useEffect(() => {
    if (!selected || !file) return;
    let cancelled = false;
    const parent = selected.parents[0];
    setDiffLoading(true);
    setDiffError(null);
    const c = getClient();
    Promise.all([
      parent && file.status !== "Added"
        ? c.call("vcs.show", { ...base, path: file.oldPath ?? file.path, rev: { kind: "Commit", rev: parent } })
        : Promise.resolve(null),
      file.status === "Deleted"
        ? Promise.resolve(null)
        : c.call("vcs.show", { ...base, path: file.path, rev: { kind: "Commit", rev: selected.id } }),
    ])
      .then(([o, m]) => {
        if (cancelled) return;
        setDiff({
          path: file.path,
          original: o?.content ?? "",
          modified: m?.content ?? "",
          binary: file.binary || (o?.binary ?? false) || (m?.binary ?? false),
          truncated: (o?.truncated ?? false) || (m?.truncated ?? false),
          originalLabel: parent ? parent.slice(0, 8) : "(root)",
          modifiedLabel: selected.shortId,
        });
      })
      .catch((e: unknown) => !cancelled && setDiffError(errorMessage(e)))
      .finally(() => !cancelled && setDiffLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [file]);

  return (
    <div className="git-split">
      <div className="git-split__list">
        {error && <Notice tone="danger">{error}</Notice>}
        {loading && commits.length === 0 ? (
          <div className="git-center">
            <Spinner size={18} /> Loading history…
          </div>
        ) : commits.length === 0 && !error ? (
          <EmptyState icon={<History size={24} strokeWidth={1.6} />} title="No commits yet" description="Commit your first changes from the Changes tab." />
        ) : (
          <ul className="git-list" aria-label="Commits">
            {commits.map((c) => (
              <li key={c.id}>
                <button
                  type="button"
                  className={clsx("git-commit-row", selected?.id === c.id && "git-commit-row--selected")}
                  onClick={() => setSelected(c)}
                  aria-pressed={selected?.id === c.id}
                >
                  <span className="git-commit-row__subject">{c.subject || "(no message)"}</span>
                  <span className="git-commit-row__meta">
                    <code>{c.shortId}</code> · {c.authorName} · {relativeTime(c.authorDate)}
                    {c.parents.length > 1 && <Badge tone="violet">merge</Badge>}
                    {c.refs.slice(0, 2).map((r) => (
                      <Badge key={r} tone="accent">
                        {r.replace("HEAD -> ", "")}
                      </Badge>
                    ))}
                  </span>
                </button>
                {selected?.id === c.id && (
                  <div className="git-commit-files">
                    {c.body && <pre className="git-commit-body">{c.body}</pre>}
                    {filesLoading ? (
                      <div className="git-center git-small">
                        <Spinner size={14} /> Loading files…
                      </div>
                    ) : (
                      files?.files.map((f) => {
                        const b = statusBadge(f.status);
                        return (
                          <button
                            key={f.path}
                            type="button"
                            className={clsx("git-file-row", file?.path === f.path && "git-file-row--selected")}
                            onClick={() => setFile(f)}
                          >
                            <span className={clsx("git-letter", `git-letter--${b.tone}`)} title={b.label}>
                              {b.letter}
                            </span>
                            <span className="git-row__name">{f.oldPath ? `${f.oldPath} → ${f.path}` : f.path}</span>
                            <span className="git-stat">
                              {f.binary ? "bin" : (
                                <>
                                  <span className="git-add">+{f.additions}</span> <span className="git-del">−{f.deletions}</span>
                                </>
                              )}
                            </span>
                          </button>
                        );
                      })
                    )}
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}
        {hasMore && (
          <div className="git-more">
            <Button size="sm" variant="ghost" onClick={() => void load(commits.length)} loading={loading}>
              Load more
            </Button>
          </div>
        )}
      </div>
      <DiffPane content={diff} loading={diffLoading} error={diffError} emptyText="Pick a commit, then a file, to see what changed." />
    </div>
  );
}
