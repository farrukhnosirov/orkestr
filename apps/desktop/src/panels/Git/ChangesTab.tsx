/**
 * Changes tab: conflicts / staged / unstaged / untracked trees with
 * stage, unstage and discard (confirmed), a commit box and a Monaco diff.
 */
import type { FileRevision, RepoStatus, StatusEntry } from "@orkestr/protocol";
import { Badge, Button, IconButton } from "@orkestr/ui";
import { CheckCheck, GitCommitHorizontal, Minus, Plus, Undo2 } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { getClient } from "../../core/client";
import { ConfirmDialog, type DiffContent, DiffPane, FileTree, Notice, SectionHeader } from "./components";
import { type ChangeGroup, buildTree, entryStatus, groupStatus, leaves, statusBadge } from "./model";
import { errorMessage, useGitUi } from "./state";

interface Props {
  projectId: string;
  worktree: string | null;
  status: RepoStatus;
  onStatus: (st: RepoStatus) => void;
  reload: () => void;
}

interface Selection {
  path: string;
  group: ChangeGroup;
  entry: StatusEntry;
}

const GROUPS: { id: ChangeGroup; title: string; tone: "danger" | "success" | "warning" | "info" }[] = [
  { id: "conflicts", title: "Merge conflicts", tone: "danger" },
  { id: "staged", title: "Staged", tone: "success" },
  { id: "unstaged", title: "Changes", tone: "warning" },
  { id: "untracked", title: "Untracked", tone: "info" },
];

function sides(sel: Selection): [FileRevision | null, FileRevision, string, string] {
  switch (sel.group) {
    case "staged":
      return [sel.entry.index === "Added" ? null : { kind: "Head" }, { kind: "Index" }, "HEAD", "Staged"];
    case "untracked":
      return [null, { kind: "Worktree" }, "—", "Working tree"];
    case "conflicts":
      return [{ kind: "Head" }, { kind: "Worktree" }, "HEAD", "Working tree (markers)"];
    default:
      return [{ kind: "Index" }, { kind: "Worktree" }, "Staged", "Working tree"];
  }
}

export function ChangesTab({ projectId, worktree, status, onStatus, reload }: Props) {
  const groups = useMemo(() => groupStatus(status.entries), [status]);
  const [open, setOpen] = useState<Record<ChangeGroup, boolean>>({ conflicts: true, staged: true, unstaged: true, untracked: true });
  const [selected, setSelected] = useState<Selection | null>(null);
  const [diff, setDiff] = useState<DiffContent | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [diffError, setDiffError] = useState<string | null>(null);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [discard, setDiscard] = useState<string[] | null>(null);
  const messageRef = useRef<HTMLTextAreaElement>(null);
  const intent = useGitUi((s) => s.intent);
  const setIntent = useGitUi((s) => s.setIntent);
  const base = worktree ? { projectId, worktree } : { projectId };

  useEffect(() => {
    if (intent === "commit") {
      messageRef.current?.focus();
      setIntent(null);
    }
  }, [intent, setIntent]);

  // Drop the selection when the file disappears from the status.
  useEffect(() => {
    if (!selected) return;
    const still = status.entries.find((e) => e.path === selected.path);
    if (!still) {
      setSelected(null);
      setDiff(null);
    }
  }, [status, selected]);

  // Load both sides of the selected file.
  useEffect(() => {
    if (!selected) return;
    let cancelled = false;
    const [orig, mod, oLabel, mLabel] = sides(selected);
    const origPath = selected.entry.origPath ?? selected.path;
    setDiffLoading(true);
    setDiffError(null);
    const c = getClient();
    Promise.all([
      orig ? c.call("vcs.show", { ...base, path: origPath, rev: orig }) : Promise.resolve(null),
      c.call("vcs.show", { ...base, path: selected.path, rev: mod }),
    ])
      .then(([o, m]) => {
        if (cancelled) return;
        setDiff({
          path: selected.path,
          original: o?.content ?? "",
          modified: m.content,
          binary: (o?.binary ?? false) || m.binary,
          truncated: (o?.truncated ?? false) || m.truncated,
          originalLabel: oLabel,
          modifiedLabel: mLabel,
        });
      })
      .catch((e: unknown) => !cancelled && setDiffError(errorMessage(e)))
      .finally(() => !cancelled && setDiffLoading(false));
    return () => {
      cancelled = true;
    };
    // `status` re-triggers so the diff follows edits on disk.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, status, projectId, worktree]);

  const run = async (label: string, fn: () => Promise<void>) => {
    setBusy(label);
    setError(null);
    setNotice(null);
    try {
      await fn();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const stage = (paths: string[], all = false) =>
    run("stage", async () => onStatus(await getClient().call("vcs.stage", { ...base, paths, all })));
  const unstage = (paths: string[], all = false) =>
    run("unstage", async () => onStatus(await getClient().call("vcs.unstage", { ...base, paths, all })));

  const doDiscard = async () => {
    if (!discard) return;
    const paths = discard;
    await run("discard", async () => {
      const res = await getClient().call("vcs.discard", { ...base, paths, confirm: true });
      onStatus(res.status);
      setNotice(
        `Discarded ${res.restored.length + res.deleted.length} file(s). A safety snapshot was taken — restore it from the Snapshots tab.`,
      );
    });
    setDiscard(null);
  };

  const commit = () =>
    run("commit", async () => {
      const res = await getClient().call("vcs.commit", { ...base, message });
      setMessage("");
      setNotice(`Committed ${res.commit.shortId} — ${res.commit.subject}`);
      reload();
    });

  const abortMerge = () =>
    run("abort", async () => {
      onStatus(await getClient().call("vcs.abortMerge", base));
    });

  const canCommit = groups.staged.length > 0 && message.trim().length > 0 && groups.conflicts.length === 0;

  const rowActions = (group: ChangeGroup, paths: string[]) => (
    <>
      {group === "staged" ? (
        <IconButton size="sm" label="Unstage" icon={<Minus size={14} />} onClick={() => void unstage(paths)} disabled={busy !== null} />
      ) : group !== "conflicts" ? (
        <>
          <IconButton size="sm" variant="danger" label="Discard changes…" icon={<Undo2 size={14} />} onClick={() => setDiscard(paths)} disabled={busy !== null} />
          <IconButton size="sm" label="Stage" icon={<Plus size={14} />} onClick={() => void stage(paths)} disabled={busy !== null} />
        </>
      ) : (
        <IconButton size="sm" label="Mark resolved (stage)" icon={<CheckCheck size={14} />} onClick={() => void stage(paths)} disabled={busy !== null} />
      )}
    </>
  );

  const total = status.entries.filter((e) => e.worktree !== "Ignored").length;

  return (
    <div className="git-split">
      <div className="git-split__list">
        {status.mergeInProgress && (
          <Notice
            tone="warning"
            action={
              <Button size="sm" variant="ghost" onClick={() => void abortMerge()} loading={busy === "abort"}>
                Abort merge
              </Button>
            }
          >
            A merge is in progress. Resolve conflicts and commit, or abort.
          </Notice>
        )}
        {error && <Notice tone="danger">{error}</Notice>}
        {notice && <Notice tone="success">{notice}</Notice>}

        <div className="git-commit">
          <textarea
            ref={messageRef}
            className="git-commit__input"
            placeholder={groups.staged.length ? "Commit message (Ctrl+Enter to commit)" : "Stage changes, then write a commit message"}
            value={message}
            rows={3}
            aria-label="Commit message"
            onChange={(e) => setMessage(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && canCommit) {
                e.preventDefault();
                void commit();
              }
            }}
          />
          <div className="git-commit__row">
            <span className="git-muted git-small">
              {groups.staged.length} staged · author from git config
            </span>
            <div className="git-spacer" />
            {groups.unstaged.length + groups.untracked.length > 0 && (
              <Button size="sm" variant="ghost" leftIcon={<Plus size={14} />} onClick={() => void stage([], true)} disabled={busy !== null}>
                Stage all
              </Button>
            )}
            <Button
              size="sm"
              variant="primary"
              leftIcon={<GitCommitHorizontal size={14} />}
              onClick={() => void commit()}
              disabled={!canCommit}
              loading={busy === "commit"}
            >
              Commit
            </Button>
          </div>
        </div>

        {total === 0 ? (
          <div className="git-clean">
            <Badge tone="success">Clean</Badge>
            <span className="git-muted">Nothing to commit — the working tree matches HEAD.</span>
          </div>
        ) : (
          GROUPS.map((g) => {
            const items = groups[g.id];
            if (items.length === 0) return null;
            const isOpen = open[g.id];
            const paths = items.map((e) => e.path);
            return (
              <section key={g.id} className="git-group" aria-label={g.title}>
                <SectionHeader
                  title={g.title}
                  count={items.length}
                  tone={g.tone}
                  open={isOpen}
                  onToggle={() => setOpen((o) => ({ ...o, [g.id]: !o[g.id] }))}
                  actions={
                    g.id === "staged" ? (
                      <IconButton size="sm" label="Unstage all" icon={<Minus size={14} />} onClick={() => void unstage([], true)} disabled={busy !== null} />
                    ) : (
                      rowActions(g.id, paths)
                    )
                  }
                />
                {isOpen && (
                  <FileTree
                    nodes={buildTree(items, (e) => e.path)}
                    selected={selected?.group === g.id ? selected.path : null}
                    onSelect={(e) => setSelected({ path: e.path, group: g.id, entry: e })}
                    renderLeaf={(e, node) => ({
                      label: node.name,
                      title: e.origPath ? `${e.origPath} → ${e.path}` : e.path,
                      badge: statusBadge(entryStatus(e, g.id)),
                      actions: rowActions(g.id, [e.path]),
                    })}
                    folderActions={(node) => rowActions(g.id, leaves(node).map((e) => e.path))}
                  />
                )}
              </section>
            );
          })
        )}
      </div>
      <DiffPane
        content={diff}
        loading={diffLoading}
        error={diffError}
        emptyText="Select a changed file to compare it side by side."
      />
      {discard && (
        <ConfirmDialog
          title={`Discard changes in ${discard.length === 1 ? discard[0] : `${discard.length} files`}?`}
          description="Unstaged edits are reverted and untracked files are deleted. Orkestr takes a safety snapshot first, so this can be undone from the Snapshots tab."
          confirmLabel="Discard"
          danger
          busy={busy === "discard"}
          error={error}
          onConfirm={() => void doDiscard()}
          onCancel={() => setDiscard(null)}
        />
      )}
    </div>
  );
}
