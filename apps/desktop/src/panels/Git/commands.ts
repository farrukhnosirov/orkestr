/**
 * Git/snapshot commands for the command registry (§54): "Create Snapshot",
 * "Rollback Workspace", "Commit". They act through RPC or open the Git panel
 * with an intent the panel picks up.
 */
import { Camera, GitCommitHorizontal, GitMerge, RotateCcw } from "lucide-react";

import { getClient } from "../../core/client";
import { workspaceActions } from "../../components/workspaceActions";
import type { AppCommand, CommandContext } from "../../features/commands/registry";
import { formatDate, t } from "../../i18n";
import { useAppStore } from "../../store/app";
import { useGitUi } from "./state";

function hasRepoProject(ctx: CommandContext): boolean {
  if (!ctx.projectId) return false;
  const p = useAppStore.getState().projects.find((x) => x.id === ctx.projectId);
  return Boolean(p?.rootPath) && useAppStore.getState().connection.state === "open";
}

function openGit(tab: "changes" | "snapshots" | "merge", intent: "commit" | "rollback" | null): void {
  const ui = useGitUi.getState();
  ui.setTab(tab);
  ui.setIntent(intent);
  workspaceActions.openPanel("git");
}

export const GIT_COMMANDS: AppCommand[] = [
  {
    id: "snapshots.create",
    get title() {
      return t("git:commands.createSnapshot");
    },
    group: "Git",
    icon: Camera,
    keywords: ["snapshot", "checkpoint", "backup", "save state", "снимок", "сделать снимок", "резервная копия"],
    enabled: hasRepoProject,
    run: async (ctx) => {
      if (!ctx.projectId) return;
      const worktree = useGitUi.getState().worktrees[ctx.projectId] ?? null;
      const stamp = formatDate(new Date(), "dateTime");
      await getClient().call("snapshots.create", {
        projectId: ctx.projectId,
        label: t("git:commands.snapshotLabel", { date: stamp }),
        ...(worktree ? { workspacePath: worktree } : {}),
      });
      openGit("snapshots", null);
    },
  },
  {
    id: "snapshots.rollbackWorkspace",
    get title() {
      return t("git:commands.rollback");
    },
    group: "Git",
    icon: RotateCcw,
    keywords: ["rollback", "roll back", "restore", "undo", "revert", "откат", "откатить", "восстановить"],
    enabled: hasRepoProject,
    run: () => openGit("snapshots", "rollback"),
  },
  {
    id: "vcs.commit",
    get title() {
      return t("git:commands.commit");
    },
    group: "Git",
    icon: GitCommitHorizontal,
    keywords: ["git", "commit", "save", "коммит", "закоммитить", "сделать коммит"],
    enabled: hasRepoProject,
    run: () => openGit("changes", "commit"),
  },
  {
    id: "merge.open",
    get title() {
      return t("merge:commands.open");
    },
    group: "Git",
    icon: GitMerge,
    keywords: ["merge", "merge manager", "integrate", "branches", "слияние", "слить", "ветки", "менеджер слияния"],
    enabled: hasRepoProject,
    run: () => openGit("merge", null),
  },
];
