/**
 * Git/snapshot commands for the command registry (§54): "Create Snapshot",
 * "Rollback Workspace", "Commit". They act through RPC or open the Git panel
 * with an intent the panel picks up.
 */
import { Camera, GitCommitHorizontal, RotateCcw } from "lucide-react";

import { getClient } from "../../core/client";
import { workspaceActions } from "../../components/workspaceActions";
import type { AppCommand, CommandContext } from "../../features/commands/registry";
import { useAppStore } from "../../store/app";
import { useGitUi } from "./state";

function hasRepoProject(ctx: CommandContext): boolean {
  if (!ctx.projectId) return false;
  const p = useAppStore.getState().projects.find((x) => x.id === ctx.projectId);
  return Boolean(p?.rootPath) && useAppStore.getState().connection.state === "open";
}

function openGit(tab: "changes" | "snapshots", intent: "commit" | "rollback" | null): void {
  const ui = useGitUi.getState();
  ui.setTab(tab);
  ui.setIntent(intent);
  workspaceActions.openPanel("git");
}

export const GIT_COMMANDS: AppCommand[] = [
  {
    id: "snapshots.create",
    title: "Create Snapshot",
    group: "Git",
    icon: Camera,
    keywords: ["checkpoint", "backup", "save state", "снимок"],
    enabled: hasRepoProject,
    run: async (ctx) => {
      if (!ctx.projectId) return;
      const worktree = useGitUi.getState().worktrees[ctx.projectId] ?? null;
      const stamp = new Date().toLocaleString();
      await getClient().call("snapshots.create", {
        projectId: ctx.projectId,
        label: `Snapshot ${stamp}`,
        ...(worktree ? { workspacePath: worktree } : {}),
      });
      openGit("snapshots", null);
    },
  },
  {
    id: "snapshots.rollbackWorkspace",
    title: "Rollback Workspace…",
    group: "Git",
    icon: RotateCcw,
    keywords: ["restore", "undo", "revert", "откат"],
    enabled: hasRepoProject,
    run: () => openGit("snapshots", "rollback"),
  },
  {
    id: "vcs.commit",
    title: "Commit…",
    group: "Git",
    icon: GitCommitHorizontal,
    keywords: ["git", "save", "коммит"],
    enabled: hasRepoProject,
    run: () => openGit("changes", "commit"),
  },
];
