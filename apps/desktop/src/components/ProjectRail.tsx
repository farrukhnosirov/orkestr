import type { Project } from "@orkestr/protocol";
import { IconButton, Tooltip } from "@orkestr/ui";
import { clsx } from "clsx";
import { Blocks, Code2, Command, FolderOpen, FolderX, Plus, Sparkle, Trash2 } from "lucide-react";
import { useCallback, useState } from "react";

import { openProject } from "../core/client";
import { useSecurityStore } from "../features/security/store";
import { openExtensions, openVsCodePanel } from "../features/vscode/quickAccess";
import { openClaudePanel } from "../features/claudeCode/bridge";
import { useT } from "../i18n";
import { ContextMenu } from "../panels/Files/ContextMenu";
import { useAppStore } from "../store/app";
import { projectsWithUnsavedTabs, useEditorStore } from "../store/editor";
import { DeleteProjectDialog } from "./DeleteProjectDialog";

/** Up to two initials for a project tile. */
export function projectInitials(name: string): string {
  const words = name
    .trim()
    .split(/[\s\-_.]+/)
    .filter(Boolean);
  const first = words[0] ?? "";
  const second = words[1] ?? "";
  const initials = second ? `${first[0] ?? ""}${second[0] ?? ""}` : first.slice(0, 2);
  return initials.toUpperCase() || "?";
}

/** Deterministic hue for projects without an explicit color. */
export function projectHue(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return h % 360;
}

function tileStyle(p: Project): React.CSSProperties {
  if (p.color) {
    return { ["--tile-a" as string]: p.color, ["--tile-b" as string]: "var(--ok-color-violet-strong)" };
  }
  const hue = projectHue(p.id);
  return {
    ["--tile-a" as string]: `hsl(${hue} 85% 60%)`,
    ["--tile-b" as string]: `hsl(${(hue + 50) % 360} 80% 55%)`,
  };
}

/** Vertical project switcher (§13 instant switching). */
export function ProjectRail() {
  const projects = useAppStore((s) => s.projects);
  const activeId = useAppStore((s) => s.activeProjectId);
  const loaded = useAppStore((s) => s.projectsLoaded);
  const setCreateOpen = useAppStore((s) => s.setCreateProjectOpen);
  const setPaletteOpen = useAppStore((s) => s.setPaletteOpen);
  const setDeleteProjectId = useAppStore((s) => s.setDeleteProjectId);
  // Joined so the selector returns a stable primitive.
  const unsavedKey = useEditorStore((s) => projectsWithUnsavedTabs(s).sort().join("|"));
  const unsaved = new Set(unsavedKey ? unsavedKey.split("|") : []);
  const [menu, setMenu] = useState<{ project: Project; x: number; y: number } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  const { t } = useT("shell");

  return (
    <nav className="rail" aria-label={t("rail.label")}>
      <div className="rail__list" role="list">
        {projects.map((p) => {
          const active = p.id === activeId;
          const dirty = !active && unsaved.has(p.id);
          return (
            <div role="listitem" key={p.id} className="rail__item">
              <Tooltip content={dirty ? t("rail.unsaved", { name: p.name }) : p.name} side="right">
                <button
                  type="button"
                  className={clsx("rail__tile", active && "rail__tile--active")}
                  style={tileStyle(p)}
                  aria-label={
                    dirty
                      ? `${t("rail.openProject", { name: p.name })}. ${t("rail.unsaved", { name: p.name })}`
                      : t("rail.openProject", { name: p.name })
                  }
                  aria-haspopup="menu"
                  aria-current={active ? "true" : undefined}
                  data-robot-drop={p.id}
                  onClick={() => {
                    if (!active) void openProject(p.id);
                  }}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    setMenu({ project: p, x: e.clientX, y: e.clientY });
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
                      e.preventDefault();
                      const r = e.currentTarget.getBoundingClientRect();
                      setMenu({ project: p, x: r.right, y: r.top });
                    }
                  }}
                >
                  {projectInitials(p.name)}
                  {dirty && <span className="rail__tile-dirty" aria-hidden="true" />}
                </button>
              </Tooltip>
            </div>
          );
        })}
        {loaded && projects.length === 0 && <div className="rail__hint">{t("rail.empty")}</div>}
      </div>
      <div className="rail__footer">
        <Tooltip content={t("rail.newProject")} side="right">
          <button type="button" className="rail__add" aria-label={t("rail.createProject")} onClick={() => setCreateOpen(true)}>
            <Plus size={18} />
          </button>
        </Tooltip>
        <IconButton label={t("rail.vscode")} icon={<Code2 size={16} />} onClick={openVsCodePanel} tooltip="right" />
        <IconButton label={t("rail.extensions")} icon={<Blocks size={16} />} onClick={openExtensions} tooltip="right" />
        <IconButton label={t("rail.claudeCode")} icon={<Sparkle size={16} />} onClick={openClaudePanel} tooltip="right" />
        <IconButton label={t("rail.palette")} icon={<Command size={16} />} onClick={() => setPaletteOpen(true)} tooltip="right" />
      </div>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          label={t("rail.menu", { name: menu.project.name })}
          onClose={closeMenu}
          items={[
            {
              type: "item",
              id: "open",
              label: t("rail.open"),
              icon: <FolderOpen size={14} />,
              disabled: menu.project.id === activeId,
              onSelect: () => void openProject(menu.project.id),
            },
            {
              type: "item",
              id: "stop",
              label: t("rail.stopProject"),
              icon: <FolderX size={14} />,
              danger: true,
              onSelect: () => useSecurityStore.getState().openStopProject(menu.project.id),
            },
            { type: "separator", id: "sep" },
            {
              type: "item",
              id: "delete",
              label: t("rail.delete"),
              icon: <Trash2 size={14} />,
              danger: true,
              onSelect: () => setDeleteProjectId(menu.project.id),
            },
          ]}
        />
      )}
      <DeleteProjectDialog />
    </nav>
  );
}
