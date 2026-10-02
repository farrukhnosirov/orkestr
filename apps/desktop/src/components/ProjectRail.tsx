import type { Project } from "@orkestr/protocol";
import { IconButton, Tooltip } from "@orkestr/ui";
import { clsx } from "clsx";
import { Command, Plus } from "lucide-react";

import { openProject } from "../core/client";
import { useT } from "../i18n";
import { useAppStore } from "../store/app";

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
  const { t } = useT("shell");

  return (
    <nav className="rail" aria-label={t("rail.label")}>
      <div className="rail__list" role="list">
        {projects.map((p) => {
          const active = p.id === activeId;
          return (
            <div role="listitem" key={p.id} className="rail__item">
              <Tooltip content={p.name} side="right">
                <button
                  type="button"
                  className={clsx("rail__tile", active && "rail__tile--active")}
                  style={tileStyle(p)}
                  aria-label={t("rail.openProject", { name: p.name })}
                  aria-current={active ? "true" : undefined}
                  onClick={() => {
                    if (!active) void openProject(p.id);
                  }}
                >
                  {projectInitials(p.name)}
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
        <IconButton label={t("rail.palette")} icon={<Command size={16} />} onClick={() => setPaletteOpen(true)} tooltip="right" />
      </div>
    </nav>
  );
}
