import { BUILTIN_THEMES, Kbd } from "@orkestr/ui";
import { clsx } from "clsx";
import { FolderOpen, FolderPlus, LayoutGrid, Palette, PanelsTopLeft, Search } from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";

import { openProject } from "../core/client";
import { commands as registeredCommands } from "../features/commands/registry";
import { getPanel, PANELS } from "../panels/registry";
import { useAppStore } from "../store/app";
import { workspaceActions } from "./workspaceActions";

export interface PaletteCommand {
  id: string;
  title: string;
  group: string;
  icon: ReactNode;
  hint?: string;
  keywords?: string;
  run: () => void;
}

/** Subsequence fuzzy score (higher is better); -1 when not matching. */
export function fuzzyScore(query: string, text: string): number {
  const q = query.toLowerCase().trim();
  if (!q) return 0;
  const t = text.toLowerCase();
  const idx = t.indexOf(q);
  if (idx >= 0) return 1000 - idx * 2 - (t.length - q.length) * 0.1;
  let ti = 0;
  let score = 0;
  let streak = 0;
  for (const ch of q) {
    const found = t.indexOf(ch, ti);
    if (found < 0) return -1;
    streak = found === ti ? streak + 1 : 0;
    score += 10 + streak * 5 - (found - ti);
    ti = found + 1;
  }
  return score;
}

/** Ctrl+K command palette (§54). */
export function CommandPalette() {
  const open = useAppStore((s) => s.paletteOpen);
  const setOpen = useAppStore((s) => s.setPaletteOpen);
  const setCreateOpen = useAppStore((s) => s.setCreateProjectOpen);
  const projects = useAppStore((s) => s.projects);
  const activeId = useAppStore((s) => s.activeProjectId);
  const setThemeId = useAppStore((s) => s.setThemeId);
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (open) {
      setQuery("");
      setIndex(0);
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  const commands = useMemo<PaletteCommand[]>(() => {
    const list: PaletteCommand[] = [
      {
        id: "project.create",
        title: "Create Project…",
        group: "Projects",
        icon: <FolderPlus size={16} />,
        keywords: "new add",
        run: () => setCreateOpen(true),
      },
    ];
    const recent = [...projects].sort((a, b) => (b.lastOpenedAt ?? "").localeCompare(a.lastOpenedAt ?? ""));
    for (const p of recent) {
      if (p.id === activeId) continue;
      list.push({
        id: `project.switch.${p.id}`,
        title: `Switch Project: ${p.name}`,
        group: "Projects",
        icon: <FolderOpen size={16} />,
        keywords: `open go ${p.slug}`,
        run: () => void openProject(p.id),
      });
    }
    for (const panel of PANELS) {
      const Icon = panel.icon;
      list.push({
        id: `panel.open.${panel.id}`,
        title: `Open Panel: ${panel.title}`,
        group: "Panels",
        icon: <Icon size={16} />,
        hint: panel.phase > 0 ? `Phase ${panel.phase}` : undefined,
        keywords: panel.description,
        run: () => workspaceActions.openPanel(panel.id),
      });
    }
    // Feature commands from the shared registry (§54).
    for (const cmd of registeredCommands) {
      const Icon = cmd.icon;
      if (cmd.enabled && !cmd.enabled({ projectId: activeId })) continue;
      list.push({
        id: cmd.id,
        title: cmd.title,
        group: cmd.group,
        icon: Icon ? <Icon size={16} /> : <PanelsTopLeft size={16} />,
        hint: cmd.shortcut,
        keywords: cmd.keywords?.join(" "),
        run: () => void Promise.resolve(cmd.run({ projectId: activeId })).catch((e: unknown) => console.error(cmd.id, e)),
      });
    }
    list.push({
      id: "layout.reset",
      title: "Reset Layout (Coding preset)",
      group: "Layout",
      icon: <LayoutGrid size={16} />,
      run: () => workspaceActions.resetLayout(),
    });
    for (const theme of Object.values(BUILTIN_THEMES)) {
      list.push({
        id: `theme.${theme.id}`,
        title: `Theme: ${theme.name}`,
        group: "Appearance",
        icon: <Palette size={16} />,
        keywords: theme.mode,
        run: () => setThemeId(theme.id),
      });
    }
    return list;
  }, [projects, activeId, setCreateOpen, setThemeId]);

  const results = useMemo(() => {
    if (!query.trim()) return commands;
    return commands
      .map((c) => ({ c, s: Math.max(fuzzyScore(query, c.title), fuzzyScore(query, `${c.group} ${c.keywords ?? ""}`) - 200) }))
      .filter((x) => x.s >= 0)
      .sort((a, b) => b.s - a.s)
      .map((x) => x.c);
  }, [commands, query]);

  useEffect(() => {
    setIndex((i) => Math.min(i, Math.max(0, results.length - 1)));
  }, [results.length]);

  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${index}"]`)?.scrollIntoView({ block: "nearest" });
  }, [index]);

  if (!open) return null;

  const run = (cmd: PaletteCommand | undefined) => {
    if (!cmd) return;
    setOpen(false);
    cmd.run();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setIndex((i) => (results.length ? (i + 1) % results.length : 0));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setIndex((i) => (results.length ? (i - 1 + results.length) % results.length : 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      run(results[index]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      setOpen(false);
    }
  };

  let lastGroup = "";
  return (
    <div className="overlay overlay--top" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && setOpen(false)}>
      <div className="palette" role="dialog" aria-modal="true" aria-label="Command palette" onKeyDown={onKeyDown}>
        <div className="palette__search">
          <Search size={16} />
          <input
            ref={inputRef}
            className="palette__input"
            placeholder="Type a command or project name…"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setIndex(0);
            }}
            role="combobox"
            aria-expanded="true"
            aria-controls="palette-list"
            aria-activedescendant={results[index] ? `palette-${results[index]?.id}` : undefined}
          />
          <Kbd>Esc</Kbd>
        </div>
        <div className="palette__list" id="palette-list" role="listbox" ref={listRef}>
          {results.length === 0 && (
            <div className="palette__empty">
              <PanelsTopLeft size={18} /> No matching commands
            </div>
          )}
          {results.map((cmd, i) => {
            const header = cmd.group !== lastGroup ? cmd.group : null;
            lastGroup = cmd.group;
            return (
              <div key={cmd.id}>
                {header && <div className="palette__group">{header}</div>}
                <div
                  id={`palette-${cmd.id}`}
                  role="option"
                  aria-selected={i === index}
                  data-index={i}
                  className={clsx("palette__item", i === index && "palette__item--active")}
                  onMouseMove={() => setIndex(i)}
                  onClick={() => run(cmd)}
                >
                  <span className="palette__icon">{cmd.icon}</span>
                  <span className="palette__title">{cmd.title}</span>
                  {cmd.hint && <span className="palette__hint">{cmd.hint}</span>}
                </div>
              </div>
            );
          })}
        </div>
        <div className="palette__footer">
          <span>
            <Kbd>↑</Kbd>
            <Kbd>↓</Kbd> navigate
          </span>
          <span>
            <Kbd>Enter</Kbd> run
          </span>
          <span className="palette__footer-panels">{getPanel("logs") ? `${PANELS.length} panels` : ""}</span>
        </div>
      </div>
    </div>
  );
}
