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
import { useT } from "../../i18n";
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

const TABS: { id: GitTab; icon: ReactNode }[] = [
  { id: "changes", icon: <ListTree size={14} /> },
  { id: "history", icon: <History size={14} /> },
  { id: "branches", icon: <GitBranch size={14} /> },
  { id: "worktrees", icon: <FolderGit2 size={14} /> },
  { id: "snapshots", icon: <Camera size={14} /> },
];

export default function GitPanel({ projectId }: PanelProps) {
  const { t } = useT("git");
  const project = useAppStore((s) => s.projects.find((p) => p.id === projectId) ?? null);
  const connected = useAppStore((s) => s.connection.state === "open");

  if (!projectId || !project) {
    return (
      <div className="git-panel">
        <EmptyState icon={<GitBranch size={26} strokeWidth={1.6} />} title={t("panel.noProject.title")} description={t("panel.noProject.description")} />
      </div>
    );
  }
  if (!project.rootPath) {
    return (
      <div className="git-panel">
        <EmptyState
          icon={<FolderGit2 size={26} strokeWidth={1.6} />}
          title={t("panel.noFolder.title")}
          description={t("panel.noFolder.description")}
        />
      </div>
    );
  }
  if (!connected) {
    return (
      <div className="git-panel git-center">
        <Spinner size={18} /> {t("panel.connecting")}
      </div>
    );
  }
  return <GitWorkspace projectId={projectId} rootPath={project.rootPath} />;
}

function GitWorkspace({ projectId, rootPath }: { projectId: string; rootPath: string }) {
  const { t } = useT(["git", "common"]);
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
            <Spinner size={18} /> {t("panel.reading")}
          </div>
        ) : (
          <EmptyState
            icon={<GitBranch size={26} strokeWidth={1.6} />}
            title={t("panel.unavailable")}
            description={error ?? t("panel.readFailed")}
            action={
              <Button variant="secondary" leftIcon={<RefreshCw size={14} />} onClick={reload}>
                {t("common:actions.retry")}
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
          title={t("panel.notRepo.title")}
          description={t("panel.notRepo.description", { path: rootPath })}
          badge={status.gitVersion ? <Badge>{t("panel.gitVersion", { version: status.gitVersion })}</Badge> : undefined}
          action={
            <Button variant="primary" leftIcon={<GitCommitHorizontal size={14} />} onClick={() => void init()} loading={initBusy}>
              {t("panel.notRepo.init")}
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
          <span className="git-header__branch">{status.branch ?? (status.detached ? t("panel.detached", { commit: status.head?.slice(0, 8) ?? "" }) : t("panel.noCommits"))}</span>
          {status.upstream && (
            <Badge tone={status.ahead || status.behind ? "warning" : "neutral"} title={t("panel.upstream", { name: status.upstream })}>
              ↑{status.ahead} ↓{status.behind}
            </Badge>
          )}
          {worktree && (
            <Badge tone="violet" title={worktree}>
              {t("panel.worktree")}
              <button type="button" className="git-chip-x" aria-label={t("panel.backToMain")} onClick={() => setWorktree(projectId, null)}>
                <X size={11} />
              </button>
            </Badge>
          )}
          <span className="git-muted git-small git-header__summary">{statusSummary(status)}</span>
        </div>
        <div className="git-spacer" />
        {loading && <Spinner size={14} />}
        <IconButton label={t("panel.refresh")} icon={<RefreshCw size={15} />} onClick={reload} tooltip="bottom" />
      </header>
      <Tabs aria-label={t("tabs.label")} items={TABS.map((x) => ({ ...x, label: t(`tabs.${x.id}`) }))} value={tab} onChange={setTab} className="git-tabs" />
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
