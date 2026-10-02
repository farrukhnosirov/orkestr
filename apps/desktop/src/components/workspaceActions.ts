/**
 * Bridge between the command palette and the docking workspace. The
 * workspace registers its handlers on mount; callers stay decoupled from
 * dockview.
 */
export interface WorkspaceHandlers {
  openPanel: (panelId: string) => void;
  resetLayout: () => void;
  /** Maximizes the active panel's group, or restores the layout if one is maximized. */
  toggleMaximize: () => void;
}

let handlers: WorkspaceHandlers | null = null;

export function registerWorkspace(h: WorkspaceHandlers | null): void {
  handlers = h;
}

export const workspaceActions: WorkspaceHandlers = {
  openPanel: (panelId) => handlers?.openPanel(panelId),
  resetLayout: () => handlers?.resetLayout(),
  toggleMaximize: () => handlers?.toggleMaximize(),
};
