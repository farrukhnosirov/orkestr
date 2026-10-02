import { BUILTIN_THEMES, Kbd, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import {
  Clock,
  CornerDownLeft,
  FolderOpen,
  LayoutGrid,
  Palette,
  PanelsTopLeft,
  Search,
  type LucideIcon,
} from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";

import { openProject } from "../core/client";
import { type AppCommand, commands as registryCommands } from "../features/commands/registry";
import { fuzzyMatch, highlightSegments } from "../features/search/fuzzy";
import { SEARCH_PREFIX, usePaletteStore } from "../features/search/paletteStore";
import "../features/search/palette.css";
import { projectsProvider, searchAll, type SearchGroup, type SearchResult } from "../features/search/providers";
import { PANELS } from "../panels/registry";
import { useAppStore } from "../store/app";
import { workspaceActions } from "./workspaceActions";

export { fuzzyScore } from "../features/search/fuzzy";

/** A runnable palette entry (registry command, panel, theme or search result). */
export interface PaletteItem {
  id: string;
  title: string;
  group: string;
  icon?: LucideIcon;
  hint?: string;
  shortcut?: string;
  subtitle?: string;
  keywords?: string;
  disabled?: boolean;
  indices: number[];
  run: () => void | Promise<void>;
}

/** Dynamic entries derived from registries (panels, layout, themes). */
function builtinItems(setThemeId: (id: string) => void): PaletteItem[] {
  const list: PaletteItem[] = [];
  for (const panel of PANELS) {
    list.push({
      id: `panel.open.${panel.id}`,
      title: `Open Panel: ${panel.title}`,
      group: "Panels",
      icon: panel.icon,
      hint: panel.phase > 0 ? `Phase ${panel.phase}` : undefined,
      keywords: panel.description,
      indices: [],
      run: () => workspaceActions.openPanel(panel.id),
    });
  }
  list.push({
    id: "layout.reset",
    title: "Reset Layout (Coding preset)",
    group: "Layout",
    icon: LayoutGrid,
    indices: [],
    run: () => workspaceActions.resetLayout(),
  });
  for (const theme of Object.values(BUILTIN_THEMES)) {
    list.push({
      id: `theme.${theme.id}`,
      title: `Theme: ${theme.name}`,
      group: "Appearance",
      icon: Palette,
      keywords: theme.mode,
      indices: [],
      run: () => setThemeId(theme.id),
    });
  }
  return list;
}

function fromCommand(c: AppCommand, projectId: string | null): PaletteItem {
  const item: PaletteItem = {
    id: c.id,
    title: c.title,
    group: c.group,
    keywords: c.keywords?.join(" "),
    disabled: c.enabled ? !c.enabled({ projectId }) : false,
    indices: [],
    run: () => c.run({ projectId }),
  };
  if (c.icon) item.icon = c.icon;
  if (c.shortcut) item.shortcut = c.shortcut;
  return item;
}

/** Ranks `items` for `query`; empty query keeps registry order. */
export function rankItems(items: readonly PaletteItem[], query: string): PaletteItem[] {
  if (!query.trim()) return [...items];
  const scored: Array<{ item: PaletteItem; score: number }> = [];
  for (const item of items) {
    const byTitle = fuzzyMatch(query, item.title);
    const byExtra = fuzzyMatch(query, `${item.group} ${item.keywords ?? ""}`);
    if (!byTitle && !byExtra) continue;
    const score = byTitle ? byTitle.score : (byExtra?.score ?? 0) - 200;
    scored.push({ item: { ...item, indices: byTitle?.indices ?? [] }, score: item.disabled ? score - 500 : score });
  }
  return scored.sort((a, b) => b.score - a.score).map((x) => x.item);
}

/** The order rows are displayed in (grouped); selection indices refer to it. */
export function displayOrder(items: readonly PaletteItem[]): PaletteItem[] {
  return groupItems(items).flatMap((g) => g.items);
}

/** Groups items keeping first-appearance order of groups. */
export function groupItems(items: readonly PaletteItem[]): Array<{ group: string; items: PaletteItem[] }> {
  const groups = new Map<string, PaletteItem[]>();
  for (const it of items) {
    const g = groups.get(it.group);
    if (g) g.push(it);
    else groups.set(it.group, [it]);
  }
  return [...groups].map(([group, list]) => ({ group, items: list }));
}

function Highlight({ text, indices }: { text: string; indices: number[] }) {
  return (
    <>
      {highlightSegments(text, indices).map((s, i) =>
        s.match ? (
          <mark key={i} className="palette__mark">
            {s.text}
          </mark>
        ) : (
          <span key={i}>{s.text}</span>
        ),
      )}
    </>
  );
}

function Shortcut({ keys }: { keys: string }) {
  return (
    <span className="palette__shortcut">
      {keys.split("+").map((k) => (
        <Kbd key={k}>{k}</Kbd>
      ))}
    </span>
  );
}

/** Ctrl+K command palette (§54) with fuzzy search, recents and `/` global search. */
export function CommandPalette() {
  const open = useAppStore((s) => s.paletteOpen);
  const setOpen = useAppStore((s) => s.setPaletteOpen);
  const setThemeId = useAppStore((s) => s.setThemeId);
  const projectId = useAppStore((s) => s.activeProjectId);
  const projectsVersion = useAppStore((s) => s.projects);
  const initialQuery = usePaletteStore((s) => s.initialQuery);
  const recent = usePaletteStore((s) => s.recent);
  const noteRecent = usePaletteStore((s) => s.noteRecent);
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const [searchGroups, setSearchGroups] = useState<SearchGroup[]>([]);
  const [searching, setSearching] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (open) {
      setQuery(initialQuery);
      setIndex(0);
      usePaletteStore.setState({ initialQuery: "" });
      requestAnimationFrame(() => inputRef.current?.focus());
    }
    // Only when the palette opens.
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  const searchMode = query.startsWith(SEARCH_PREFIX);
  const searchQuery = searchMode ? query.slice(SEARCH_PREFIX.length) : "";

  // Global search mode: run providers (debounced, abortable).
  useEffect(() => {
    if (!open || !searchMode) return;
    const ctrl = new AbortController();
    setSearching(true);
    const t = setTimeout(() => {
      const q = searchQuery.trim();
      // With an empty query list every project (most recent first) so `/` alone is useful.
      const run = q ? searchAll(q, ctrl.signal) : Promise.resolve(allProjectsGroup());
      run.then(
        (groups) => {
          if (!ctrl.signal.aborted) {
            setSearchGroups(groups);
            setSearching(false);
          }
        },
        () => !ctrl.signal.aborted && setSearching(false),
      );
    }, 80);
    return () => {
      ctrl.abort();
      clearTimeout(t);
    };
  }, [open, searchMode, searchQuery, projectsVersion]);

  const allItems = useMemo<PaletteItem[]>(
    () => [...registryCommands.map((c) => fromCommand(c, projectId)), ...builtinItems(setThemeId)],
    [projectId, setThemeId],
  );

  const results = useMemo<PaletteItem[]>(() => {
    if (searchMode) {
      return searchGroups.flatMap((g) =>
        g.results.map((r) => {
          const item: PaletteItem = {
            id: r.id,
            title: r.title,
            group: g.provider.title,
            indices: r.indices,
            run: r.run,
          };
          if (r.icon) item.icon = r.icon;
          if (r.subtitle) item.subtitle = r.subtitle;
          return item;
        }),
      );
    }
    if (!query.trim()) {
      const byId = new Map(allItems.map((i) => [i.id, i]));
      const recentItems = recent
        .map((id) => byId.get(id))
        .filter((i): i is PaletteItem => i !== undefined)
        .map((i) => ({ ...i, group: "Recent" }));
      const recentIds = new Set(recentItems.map((i) => i.id));
      return [...recentItems, ...allItems.filter((i) => !recentIds.has(i.id))];
    }
    return rankItems(allItems, query);
  }, [searchMode, searchGroups, query, allItems, recent]);

  // Rows are displayed grouped; keyboard selection and Enter must address
  // the same (display) order, not the ranking order.
  const ordered = useMemo(() => displayOrder(results), [results]);

  useEffect(() => {
    setIndex((i) => Math.min(i, Math.max(0, results.length - 1)));
  }, [results.length]);

  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${index}"]`)?.scrollIntoView({ block: "nearest" });
  }, [index]);

  if (!open) return null;

  const run = (item: PaletteItem | undefined) => {
    if (!item || item.disabled) return;
    setOpen(false);
    if (!searchMode) noteRecent(item.id);
    Promise.resolve(item.run()).catch((e: unknown) => console.error(`command ${item.id} failed`, e));
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setIndex((i) => (ordered.length ? (i + 1) % ordered.length : 0));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setIndex((i) => (ordered.length ? (i - 1 + ordered.length) % ordered.length : 0));
    } else if (e.key === "Home" && e.ctrlKey) {
      e.preventDefault();
      setIndex(0);
    } else if (e.key === "End" && e.ctrlKey) {
      e.preventDefault();
      setIndex(Math.max(0, ordered.length - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      run(ordered[index]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      setOpen(false);
    } else if (e.key === "Backspace" && query === SEARCH_PREFIX) {
      // Leave search mode with one backspace.
      e.preventDefault();
      setQuery("");
    }
  };

  let flatIndex = -1;
  const empty: ReactNode = searchMode ? (
    searching ? (
      <>
        <Spinner size={14} /> Searching…
      </>
    ) : searchQuery.trim() ? (
      <>
        <Search size={18} /> Nothing found for “{searchQuery.trim()}”
      </>
    ) : (
      <>
        <PanelsTopLeft size={18} /> No projects yet — create one with “Create Project”
      </>
    )
  ) : (
    <>
      <PanelsTopLeft size={18} /> No matching commands
    </>
  );

  return (
    <div className="overlay overlay--top" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && setOpen(false)}>
      <div className="palette" role="dialog" aria-modal="true" aria-label="Command palette" onKeyDown={onKeyDown}>
        <div className="palette__search">
          {searchMode ? <span className="palette__mode">Search</span> : <Search size={16} />}
          <input
            ref={inputRef}
            className="palette__input"
            placeholder={searchMode ? "Search projects…" : "Type a command, or / to search…"}
            value={searchMode ? searchQuery : query}
            onChange={(e) => {
              setQuery(searchMode ? SEARCH_PREFIX + e.target.value : e.target.value);
              setIndex(0);
            }}
            role="combobox"
            aria-expanded="true"
            aria-controls="palette-list"
            aria-autocomplete="list"
            aria-activedescendant={ordered[index] ? `palette-${ordered[index]?.id}` : undefined}
          />
          {searching && <Spinner size={14} />}
          <Kbd>Esc</Kbd>
        </div>
        <div className="palette__list" id="palette-list" role="listbox" ref={listRef} aria-label="Results">
          {results.length === 0 && <div className="palette__empty">{empty}</div>}
          {groupItems(results).map(({ group, items }) => (
            <div key={group} role="group" aria-label={group}>
              <div className="palette__group" aria-hidden="true">
                {group === "Recent" && <Clock size={10} />} {group}
              </div>
              {items.map((item) => {
                flatIndex += 1;
                const i = flatIndex;
                const Icon = item.icon;
                return (
                  <div
                    key={`${group}-${item.id}`}
                    id={`palette-${item.id}`}
                    role="option"
                    aria-selected={i === index}
                    aria-disabled={item.disabled || undefined}
                    data-index={i}
                    className={clsx(
                      "palette__item",
                      i === index && "palette__item--active",
                      item.disabled && "palette__item--disabled",
                    )}
                    onMouseMove={() => setIndex(i)}
                    onClick={() => run(item)}
                  >
                    <span className="palette__icon">{Icon ? <Icon size={16} /> : <CornerDownLeft size={14} />}</span>
                    <span className="palette__title">
                      <Highlight text={item.title} indices={item.indices} />
                      {item.subtitle && <span className="palette__subtitle">{item.subtitle}</span>}
                    </span>
                    {item.hint && <span className="palette__hint">{item.hint}</span>}
                    {item.shortcut && <Shortcut keys={item.shortcut} />}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
        <div className="palette__footer">
          <span>
            <Kbd>↑</Kbd>
            <Kbd>↓</Kbd> navigate
          </span>
          <span>
            <Kbd>Enter</Kbd> run
          </span>
          <span>
            <Kbd>/</Kbd> search projects
          </span>
          <span className="palette__footer-panels">{searchMode ? "Global search" : `${allItems.length} commands`}</span>
        </div>
      </div>
    </div>
  );
}

/** `/` with an empty query: every project, most recently opened first. */
function allProjectsGroup(): SearchGroup[] {
  const { projects } = useAppStore.getState();
  if (projects.length === 0) return [];
  const sorted = [...projects].sort((a, b) => (b.lastOpenedAt ?? "").localeCompare(a.lastOpenedAt ?? ""));
  return [
    {
      provider: projectsProvider,
      results: sorted.map((p) => {
        const r: SearchResult = {
          id: `project:${p.id}`,
          title: p.name,
          icon: FolderOpen,
          score: 0,
          indices: [],
          run: () => openProject(p.id),
        };
        if (p.rootPath) r.subtitle = p.rootPath;
        return r;
      }),
    },
  ];
}
