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
import { Trans } from "react-i18next";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { ConfirmDialog, Notice } from "./components";
import { branchNameError, relativeTime, shortId } from "./model";
import { errorMessage, gitScope, useAsync } from "./state";

interface Props {
  projectId: string;
  active: string | null;
  refresh: number;
  onOpen: (path: string | null) => void;
}

export function WorktreesTab({ projectId, active, refresh, onOpen }: Props) {
  const { t } = useT("git");
  const { data, error, loading, reload } = useAsync(() => getClient().call("vcs.worktrees", { projectId }), [projectId, refresh], gitScope(projectId));
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
      setNotice(r.worktree.branch ? t("worktrees.created", { branch: r.worktree.branch }) : t("worktrees.createdNoBranch"));
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
          ? t("worktrees.removedSnapshot")
          : t("worktrees.removed"),
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
          placeholder={t("worktrees.branchPlaceholder")}
          aria-label={t("worktrees.branch")}
          value={branch}
          onChange={(e) => setBranch(e.target.value)}
          error={nameError ?? undefined}
          leftIcon={<FolderGit2 size={14} />}
        />
        <Button type="submit" size="sm" variant="primary" leftIcon={<Plus size={14} />} disabled={!branch.trim() || nameError !== null} loading={busy === "create"}>
          {t("worktrees.add")}
        </Button>
      </form>
      <p className="git-muted git-small git-hint">
        <Trans t={t} i18nKey="worktrees.hint" values={{ pattern: "agent/<role>-<id>" }} components={{ code: <code /> }} />
      </p>
      {actionError && <Notice tone="danger">{actionError}</Notice>}
      {notice && <Notice tone="success">{notice}</Notice>}
      {error && <Notice tone="danger">{error}</Notice>}
      {loading && !data ? (
        <div className="git-center">
          <Spinner size={18} /> {t("worktrees.loading")}
        </div>
      ) : list.length === 0 ? (
        <EmptyState icon={<FolderGit2 size={24} strokeWidth={1.6} />} title={t("worktrees.empty.title")} description={t("worktrees.empty.description")} />
      ) : (
        <ul className="git-list git-scroll">
          {list.map((w) => {
            const isActive = w.isMain ? active === null : active === w.path;
            return (
              <li key={w.path} className={clsx("git-branch", isActive && "git-branch--current")}>
                <div className="git-branch__main">
                  <span className="git-branch__name">
                    {w.agentId ? <Bot size={13} /> : <FolderGit2 size={13} />}
                    {w.branch ?? (w.detached ? t("panel.detached", { commit: shortId(w.head) }) : "—")}
                    {w.isMain && <Badge tone="accent">{t("worktrees.main")}</Badge>}
                    {w.agentId && <Badge tone="violet">{w.role ? t("worktrees.agentRole", { role: w.role }) : t("worktrees.agent")}</Badge>}
                    {w.dirty === true && <Badge tone="warning">{t("worktrees.dirty")}</Badge>}
                    {w.locked && <Badge>{t("worktrees.locked")}</Badge>}
                    {w.prunable && <Badge tone="danger">{t("worktrees.missing")}</Badge>}
                    {isActive && <Badge tone="success">{t("worktrees.open")}</Badge>}
                  </span>
                  <span className="git-branch__meta" title={w.path}>
                    {w.path}
                    {w.createdAt && <> · {t("worktrees.createdAt", { time: relativeTime(w.createdAt) })}</>}
                  </span>
                </div>
                <span className="git-row__actions git-row__actions--always">
                  <IconButton
                    size="sm"
                    label={t("worktrees.openInPanel")}
                    icon={<FolderOpen size={14} />}
                    onClick={() => onOpen(w.isMain ? null : w.path)}
                    disabled={isActive || w.prunable || w.bare}
                  />
                  {!w.isMain && (
                    <IconButton size="sm" variant="danger" label={t("worktrees.remove")} icon={<Trash2 size={14} />} onClick={() => setRemoving(w)} disabled={busy !== null} />
                  )}
                </span>
              </li>
            );
          })}
        </ul>
      )}
      {removing && (
        <ConfirmDialog
          title={removing.branch ? t("worktrees.removeTitle", { branch: removing.branch }) : t("worktrees.removeTitleNoBranch")}
          description={
            removing.dirty
              ? t("worktrees.dirtyDescription")
              : t("worktrees.cleanDescription")
          }
          confirmLabel={removing.dirty ? t("worktrees.snapshotRemove") : t("worktrees.removeConfirm")}
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
