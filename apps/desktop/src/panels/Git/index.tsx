/**
 * Git panel (§16, §25): changes, history, branches, worktrees (incl. agent
 * worktrees) and snapshots/rollback. All data via `vcs.*` / `snapshots.*` RPC.
 */
import "./git.css";

import type { RepoStatus } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Spinner, Tabs } from "@orkestr/ui";
import { Camera, FolderGit2, GitBranch, GitCommitHorizontal, History, ListTree, RefreshCw, X } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";

import { getClient } from "../../core/client";
import { useAppStore } from "../../store/app";
import type { PanelProps } from "../types";
import { BranchesTab } from "./BranchesTab";
import { ChangesTab } from "./ChangesTab";
import { Notice } from "./components";
import { HistoryTab } from "./HistoryTab";
import { statusSummary } from "./model";
import { SnapshotsTab } from "./SnapshotsTab";
import { type GitTab, errorMessage, useGitUi, useProjectRefresh } from "./state";
import { WorktreesTab } from "./WorktreesTab";

const TABS: { id: GitTab; label: string; icon: ReactNode }[] = [
  { id: "changes", label: "Changes", icon: <ListTree size={14} /> },
  { id: "history", label: "History", icon: <History size={14} /> },
  { id: "branches", label: "Branches", icon: <GitBranch size={14} /> },
  { id: "worktrees", label: "Worktrees", icon: <FolderGit2 size={14} /> },
  { id: "snapshots", label: "Snapshots", icon: <Camera size={14} /> },
];

export default function GitPanel({ projectId }: PanelProps) {
  const project = useAppStore((s) => s.projects.find((p) => p.id === projectId) ?? null);
  const connected = useAppStore((s) => s.connection.state === "open");

  if (!projectId || !project) {
    return (
      <div className="git-panel">
        <EmptyState icon={<GitBranch size={26} strokeWidth={1.6} />} title="No project open" description="Open or create a project to see its repository." />
      </div>
    );
  }
  if (!project.rootPath) {
    return (
      <div className="git-panel">
        <EmptyState
          icon={<FolderGit2 size={26} strokeWidth={1.6} />}
          title="No project folder"
          description="This project is not linked to a folder on disk yet. Set its root folder to use Git and snapshots."
        />
      </div>
    );
  }
  if (!connected) {
    return (
      <div className="git-panel git-center">
        <Spinner size={18} /> Connecting to the core…
      </div>
    );
  }
  return <GitWorkspace projectId={projectId} rootPath={project.rootPath} />;
}

function GitWorkspace({ projectId, rootPath }: { projectId: string; rootPath: string }) {
  const tab = useGitUi((s) => s.tab);
  const setTab = useGitUi((s) => s.setTab);
  const worktree = useGitUi((s) => s.worktrees[projectId] ?? null);
  const setWorktree = useGitUi((s) => s.setWorktree);
  const refresh = useProjectRefresh(projectId);
  const [manual, setManual] = useState(0);
  const [status, setStatus] = useState<RepoStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [initBusy, setInitBusy] = useState(false);
  const version = refresh + manual;

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    getClient()
      .call("vcs.status", worktree ? { projectId, worktree } : { projectId })
      .then((st) => {
        if (cancelled) return;
        setStatus(st);
        setError(null);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(errorMessage(e));
        // A worktree that disappeared: fall back to the main checkout.
        if (worktree) setWorktree(projectId, null);
      })
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [projectId, worktree, version, setWorktree]);

  const reload = () => setManual((m) => m + 1);

  const init = async () => {
    setInitBusy(true);
    try {
      setStatus(await getClient().call("vcs.init", { projectId }));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setInitBusy(false);
    }
  };

  if (!status) {
    return (
      <div className="git-panel">
        {loading ? (
          <div className="git-center">
            <Spinner size={18} /> Reading repository…
          </div>
        ) : (
          <EmptyState
            icon={<GitBranch size={26} strokeWidth={1.6} />}
            title="Git is unavailable"
            description={error ?? "Could not read the repository."}
            action={
              <Button variant="secondary" leftIcon={<RefreshCw size={14} />} onClick={reload}>
                Retry
              </Button>
            }
          />
        )}
      </div>
    );
  }

  if (!status.isRepo) {
    return (
      <div className="git-panel">
        <EmptyState
          icon={<GitBranch size={26} strokeWidth={1.6} />}
          title="Not a git repository"
          description={`${rootPath} is not under version control yet. Initialise a repository to track changes, branches and agent worktrees.`}
          badge={status.gitVersion ? <Badge>git {status.gitVersion}</Badge> : undefined}
          action={
            <Button variant="primary" leftIcon={<GitCommitHorizontal size={14} />} onClick={() => void init()} loading={initBusy}>
              Initialise repository
            </Button>
          }
        />
        {error && <Notice tone="danger">{error}</Notice>}
      </div>
    );
  }

  return (
    <div className="git-panel">
      <header className="git-header">
        <div className="git-header__repo">
          <GitBranch size={15} className="git-accent" />
          <span className="git-header__branch">{status.branch ?? (status.detached ? `detached @ ${status.head?.slice(0, 8) ?? ""}` : "no commits yet")}</span>
          {status.upstream && (
            <Badge tone={status.ahead || status.behind ? "warning" : "neutral"} title={`Upstream ${status.upstream}`}>
              ↑{status.ahead} ↓{status.behind}
            </Badge>
          )}
          {worktree && (
            <Badge tone="violet" title={worktree}>
              worktree
              <button type="button" className="git-chip-x" aria-label="Back to the main checkout" onClick={() => setWorktree(projectId, null)}>
                <X size={11} />
              </button>
            </Badge>
          )}
          <span className="git-muted git-small git-header__summary">{statusSummary(status)}</span>
        </div>
        <div className="git-spacer" />
        {loading && <Spinner size={14} />}
        <IconButton label="Refresh" icon={<RefreshCw size={15} />} onClick={reload} tooltip="bottom" />
      </header>
      <Tabs aria-label="Git views" items={TABS} value={tab} onChange={setTab} className="git-tabs" />
      {error && <Notice tone="danger">{error}</Notice>}
      <div className="git-body" role="tabpanel">
        {tab === "changes" && <ChangesTab projectId={projectId} worktree={worktree} status={status} onStatus={setStatus} reload={reload} />}
        {tab === "history" && <HistoryTab projectId={projectId} worktree={worktree} refresh={version} />}
        {tab === "branches" && <BranchesTab projectId={projectId} worktree={worktree} refresh={version} onChanged={reload} />}
        {tab === "worktrees" && (
          <WorktreesTab
            projectId={projectId}
            active={worktree}
            refresh={version}
            onOpen={(p) => {
              setWorktree(projectId, p);
              setTab("changes");
            }}
          />
        )}
        {tab === "snapshots" && <SnapshotsTab projectId={projectId} worktree={worktree} refresh={version} />}
      </div>
    </div>
  );
}
