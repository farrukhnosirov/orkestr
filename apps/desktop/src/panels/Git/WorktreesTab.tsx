/**
 * Worktrees tab: main checkout + agent worktrees; open one as the panel's
 * context, create a worktree for a new branch, remove (force = confirmed,
 * with an automatic safety snapshot).
 */
import type { WorktreeInfo } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { Bot, FolderGit2, FolderOpen, Plus, Trash2 } from "lucide-react";
import { type FormEvent, useState } from "react";

import { getClient } from "../../core/client";
import { ConfirmDialog, Notice } from "./components";
import { branchNameError, relativeTime, shortId } from "./model";
import { errorMessage, useAsync } from "./state";

interface Props {
  projectId: string;
  active: string | null;
  refresh: number;
  onOpen: (path: string | null) => void;
}

export function WorktreesTab({ projectId, active, refresh, onOpen }: Props) {
  const { data, error, loading, reload } = useAsync(() => getClient().call("vcs.worktrees", { projectId }), [projectId, refresh]);
  const [branch, setBranch] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [removing, setRemoving] = useState<WorktreeInfo | null>(null);
  const nameError = branch ? branchNameError(branch) : null;

  const create = async (e: FormEvent) => {
    e.preventDefault();
    if (!branch.trim() || nameError) return;
    setBusy("create");
    setActionError(null);
    setNotice(null);
    try {
      const r = await getClient().call("vcs.addWorktree", { projectId, branch: branch.trim() });
      setBranch("");
      setNotice(`Created worktree for ${r.worktree.branch ?? "branch"}`);
      reload();
    } catch (err) {
      setActionError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  };

  const remove = async (w: WorktreeInfo) => {
    const force = w.dirty === true;
    setBusy("remove");
    setActionError(null);
    try {
      const r = await getClient().call("vcs.removeWorktree", { projectId, path: w.path, force, confirm: force });
      setNotice(
        r.safetySnapshotId
          ? "Worktree removed. Its uncommitted changes were saved in a safety snapshot."
          : "Worktree removed (its branch is kept).",
      );
      if (active && active === w.path) onOpen(null);
      setRemoving(null);
      reload();
    } catch (err) {
      setActionError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  };

  const list = data?.worktrees ?? [];

  return (
    <div className="git-pane">
      <form className="git-form" onSubmit={(e) => void create(e)}>
        <Input
          placeholder="branch for a new worktree"
          aria-label="Branch for a new worktree"
          value={branch}
          onChange={(e) => setBranch(e.target.value)}
          error={nameError ?? undefined}
          leftIcon={<FolderGit2 size={14} />}
        />
        <Button type="submit" size="sm" variant="primary" leftIcon={<Plus size={14} />} disabled={!branch.trim() || nameError !== null} loading={busy === "create"}>
          Add worktree
        </Button>
      </form>
      <p className="git-muted git-small git-hint">
        Agents get their own worktree on <code>agent/&lt;role&gt;-&lt;id&gt;</code> automatically; they never write into your checkout.
      </p>
      {actionError && <Notice tone="danger">{actionError}</Notice>}
      {notice && <Notice tone="success">{notice}</Notice>}
      {error && <Notice tone="danger">{error}</Notice>}
      {loading && !data ? (
        <div className="git-center">
          <Spinner size={18} /> Loading worktrees…
        </div>
      ) : list.length === 0 ? (
        <EmptyState icon={<FolderGit2 size={24} strokeWidth={1.6} />} title="No worktrees" description="Create one above." />
      ) : (
        <ul className="git-list git-scroll">
          {list.map((w) => {
            const isActive = w.isMain ? active === null : active === w.path;
            return (
              <li key={w.path} className={clsx("git-branch", isActive && "git-branch--current")}>
                <div className="git-branch__main">
                  <span className="git-branch__name">
                    {w.agentId ? <Bot size={13} /> : <FolderGit2 size={13} />}
                    {w.branch ?? (w.detached ? `detached @ ${shortId(w.head)}` : "—")}
                    {w.isMain && <Badge tone="accent">main checkout</Badge>}
                    {w.agentId && <Badge tone="violet">{w.role ? `agent · ${w.role}` : "agent"}</Badge>}
                    {w.dirty === true && <Badge tone="warning">uncommitted</Badge>}
                    {w.locked && <Badge>locked</Badge>}
                    {w.prunable && <Badge tone="danger">missing</Badge>}
                    {isActive && <Badge tone="success">open</Badge>}
                  </span>
                  <span className="git-branch__meta" title={w.path}>
                    {w.path}
                    {w.createdAt && <> · created {relativeTime(w.createdAt)}</>}
                  </span>
                </div>
                <span className="git-row__actions git-row__actions--always">
                  <IconButton
                    size="sm"
                    label="Open in Git panel"
                    icon={<FolderOpen size={14} />}
                    onClick={() => onOpen(w.isMain ? null : w.path)}
                    disabled={isActive || w.prunable || w.bare}
                  />
                  {!w.isMain && (
                    <IconButton size="sm" variant="danger" label="Remove worktree…" icon={<Trash2 size={14} />} onClick={() => setRemoving(w)} disabled={busy !== null} />
                  )}
                </span>
              </li>
            );
          })}
        </ul>
      )}
      {removing && (
        <ConfirmDialog
          title={`Remove worktree ${removing.branch ?? ""}?`}
          description={
            removing.dirty
              ? "This worktree has uncommitted changes. They will be saved in a safety snapshot, then the worktree is force-removed. The branch is kept."
              : "The directory is removed; the branch and its commits are kept."
          }
          confirmLabel={removing.dirty ? "Snapshot & remove" : "Remove"}
          danger
          busy={busy === "remove"}
          error={actionError}
          onConfirm={() => void remove(removing)}
          onCancel={() => setRemoving(null)}
        />
      )}
    </div>
  );
}
