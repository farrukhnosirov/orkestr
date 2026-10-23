/**
 * Window Library (Wave 8): a gallery of every panel and window with
 * descriptions and real previews. Opened from the title bar, the rail, the
 * palette and Ctrl+Shift+L. Keyboard: arrows move, Enter opens here,
 * Shift+Enter opens in a new window, Ctrl+Enter adds to the layout, Ctrl+D
 * toggles the favorite, PageUp/PageDown switch the category, Esc closes.
 */
import { Kbd } from "@orkestr/ui";
import { clsx } from "clsx";
import { AppWindow, Clock, LayoutGrid, LibraryBig, Pin, PinOff, Search, SquarePlus, Star, X, type LucideIcon } from "lucide-react";
import { type KeyboardEvent as ReactKeyboardEvent, useEffect, useMemo, useRef, useState } from "react";

import { Overlay } from "../../components/Overlay";
import { chordSteps } from "../commands/keys";
import { highlightSegments } from "../search/fuzzy";
import { useLanguage, useT } from "../../i18n";
import { useLayoutStore } from "../layout/store";
import { activeTheme, useThemeStore } from "../theme/themeStore";
import { addToLayout, openHere, openInNewWindow, togglePin } from "./actions";
import {
  categoryCounts,
  cycleFilter,
  entryShortcut,
  filterEntries,
  libraryEntries,
  type LibraryEntry,
  moveSelection,
  searchEntries,
  type SearchHit,
} from "./catalog";
import { previewUrl, type PreviewMode } from "./previews";
import { useWindowLibraryStore } from "./store";
import "./windowLibrary.css";

type OpenState = "dock" | "window" | null;

function useOpenState(): (id: string) => OpenState {
  const dock = useLayoutStore((s) => s.dockPanels);
  const popouts = useLayoutStore((s) => s.popouts);
  return (id) => (popouts.some((w) => w.panelId === id) ? "window" : dock.includes(id) ? "dock" : null);
}

function Highlight({ text, indices }: { text: string; indices: number[] }) {
  return (
    <>
      {highlightSegments(text, indices).map((s, i) =>
        s.match ? (
          <mark key={i} className="wl-mark">
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
    <span className="wl-shortcut">
      {chordSteps(keys).map((step, i) => (
        <span key={i} className="wl-shortcut__chord">
          {step.split("+").map((k, j) => (
            <Kbd key={j}>{k}</Kbd>
          ))}
        </span>
      ))}
    </span>
  );
}

function Preview({ entry, mode, large }: { entry: LibraryEntry; mode: PreviewMode; large?: boolean }) {
  const { t } = useT("windowLibrary");
  const lang = useLanguage();
  const url = previewUrl(entry.id, lang, mode);
  const [failed, setFailed] = useState(false);
  const Icon = entry.icon;
  return (
    <div className={clsx("wl-preview", large && "wl-preview--large", `wl-preview--${entry.category}`)}>
      {url && !failed ? (
        <img src={url} alt={t("previewAlt", { title: entry.title })} loading="lazy" decoding="async" draggable={false} onError={() => setFailed(true)} />
      ) : (
        <div className="wl-preview__empty" aria-label={t("noPreview")}>
          <Icon size={large ? 40 : 28} aria-hidden="true" />
        </div>
      )}
      <span className="wl-preview__icon" aria-hidden="true">
        <Icon size={large ? 18 : 15} />
      </span>
    </div>
  );
}

const ACTION_ICONS: Record<"openHere" | "openWindow" | "addToLayout", LucideIcon> = {
  openHere: LayoutGrid,
  openWindow: AppWindow,
  addToLayout: SquarePlus,
};

/** Footer key hints (key names are the same in every language). */
const FOOT_HINTS: ReadonlyArray<{ keys: readonly string[]; hint: "navigate" | "open" | "newWindow" | "favorite" | "category" | "close" }> = [
  { keys: ["↑", "↓", "←", "→"], hint: "navigate" },
  { keys: ["Enter"], hint: "open" },
  { keys: ["Shift", "Enter"], hint: "newWindow" },
  { keys: ["Ctrl", "D"], hint: "favorite" },
  { keys: ["PgUp", "PgDn"], hint: "category" },
  { keys: ["Esc"], hint: "close" },
];

/** The library overlay. Rendered once in App; visible while the store says so. */
export function WindowLibrary() {
  const open = useWindowLibraryStore((s) => s.open);
  if (!open) return null;
  return <LibraryDialog />;
}

function LibraryDialog() {
  const { t } = useT("windowLibrary");
  const lang = useLanguage();
  const setOpen = useWindowLibraryStore((s) => s.setOpen);
  const filter = useWindowLibraryStore((s) => s.filter);
  const setFilter = useWindowLibraryStore((s) => s.setFilter);
  const favorites = useWindowLibraryStore((s) => s.favorites);
  const recent = useWindowLibraryStore((s) => s.recent);
  const pins = useWindowLibraryStore((s) => s.pins);
  const toggleFavorite = useWindowLibraryStore((s) => s.toggleFavorite);
  const mode: PreviewMode = useThemeStore((s) => activeTheme(s).mode) === "Light" ? "light" : "dark";
  const openState = useOpenState();
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);

  // Rebuilt on language change so getters and search read the new language.
  const entries = useMemo(() => libraryEntries(), [lang]); // eslint-disable-line react-hooks/exhaustive-deps
  const categories = useMemo(() => categoryCounts(entries), [entries]);
  const filters = useMemo(() => ["all", "favorites", "recent", ...categories.map((c) => c.id)], [categories]);

  // A search covers the whole library; the category filter applies when browsing.
  const hits: SearchHit[] = useMemo(() => {
    if (query.trim()) return searchEntries(entries, query, lang);
    return filterEntries(entries, filter, { favorites, recent }).map((entry) => ({ entry, score: 0, indices: [] }));
  }, [entries, query, lang, filter, favorites, recent]);

  const selected = hits[Math.min(index, hits.length - 1)]?.entry ?? null;

  useEffect(() => {
    requestAnimationFrame(() => inputRef.current?.focus());
  }, []);
  useEffect(() => setIndex(0), [query, filter]);
  useEffect(() => {
    if (!selected) return;
    const el = gridRef.current?.querySelector<HTMLElement>(`[data-wl-entry="${selected.id}"]`);
    el?.scrollIntoView?.({ block: "nearest" });
  }, [selected]);

  const close = () => setOpen(false);

  const columns = (): number => {
    const grid = gridRef.current;
    if (!grid || typeof window.getComputedStyle !== "function") return 1;
    const tpl = window.getComputedStyle(grid).gridTemplateColumns;
    return tpl && tpl !== "none" ? tpl.trim().split(/\s+/).length : 1;
  };

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      if (query) setQuery("");
      else close();
      return;
    }
    if (e.key === "PageDown" || e.key === "PageUp") {
      e.preventDefault();
      setQuery("");
      setFilter(cycleFilter(filters, filter, e.key === "PageDown" ? 1 : -1));
      return;
    }
    // Text editing keys stay in the search box.
    const inInput = e.target === inputRef.current;
    const editingKey = e.key === "Home" || e.key === "End" || e.key === "ArrowLeft" || e.key === "ArrowRight";
    if (inInput && editingKey && query && !e.ctrlKey) return;
    const next = moveSelection(index, e.key, columns(), hits.length);
    if (next !== null) {
      e.preventDefault();
      setIndex(next);
      return;
    }
    if (!selected) return;
    if (e.key === "Enter") {
      e.preventDefault();
      if (e.shiftKey) openInNewWindow(selected);
      else if (e.ctrlKey || e.metaKey) addToLayout(selected);
      else openHere(selected);
    } else if ((e.ctrlKey || e.metaKey) && !e.shiftKey && (e.code === "KeyD" || e.key.toLowerCase() === "d")) {
      e.preventDefault();
      e.stopPropagation();
      toggleFavorite(selected.id);
    }
  };

  const filterLabel = (id: string) => t(`categories.${id}` as "categories.all");
  const filterIcon = (id: string): LucideIcon | null => (id === "favorites" ? Star : id === "recent" ? Clock : id === "all" ? LibraryBig : null);
  const countFor = (id: string) =>
    id === "all" ? entries.length : id === "favorites" ? favorites.length : id === "recent" ? recent.length : (categories.find((c) => c.id === id)?.count ?? 0);
  const activeId = selected ? `wl-entry-${selected.id}` : undefined;

  return (
    <Overlay className="wl-overlay" onDismiss={close}>
      <div className="wl" role="dialog" aria-modal="true" aria-labelledby="wl-title" onKeyDown={onKeyDown}>
        <header className="wl__head">
          <div className="wl__brand">
            <span className="wl__brand-icon" aria-hidden="true">
              <LibraryBig size={18} />
            </span>
            <div>
              <h2 id="wl-title" className="wl__title">
                {t("title")}
              </h2>
              <p className="wl__subtitle">{t("subtitle")}</p>
            </div>
          </div>
          <label className="wl__search">
            <Search size={15} aria-hidden="true" />
            <input
              ref={inputRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t("searchPlaceholder")}
              aria-label={t("searchLabel")}
              aria-controls="wl-grid"
              aria-activedescendant={activeId}
              spellCheck={false}
              autoComplete="off"
            />
            {query && (
              <button type="button" className="wl__search-clear" aria-label={t("clearSearch")} onClick={() => setQuery("")}>
                <X size={13} />
              </button>
            )}
          </label>
          <span className="wl__count" aria-live="polite">
            {t("count", { count: hits.length })}
          </span>
          <button type="button" className="wl__close" aria-label={t("close")} title={t("close")} onClick={close}>
            <X size={16} />
          </button>
        </header>

        <div className="wl__body">
          <nav className="wl__cats" aria-label={t("categoriesLabel")}>
            {filters.map((id) => {
              const Icon = filterIcon(id);
              const active = !query.trim() && filter === id;
              return (
                <button
                  key={id}
                  type="button"
                  className={clsx("wl-cat", active && "wl-cat--active", `wl-cat--${id}`)}
                  aria-pressed={active}
                  onClick={() => {
                    setQuery("");
                    setFilter(id);
                    inputRef.current?.focus();
                  }}
                >
                  {Icon ? <Icon size={14} aria-hidden="true" /> : <span className={clsx("wl-cat__dot", `wl-dot--${id}`)} aria-hidden="true" />}
                  <span className="wl-cat__label">{filterLabel(id)}</span>
                  <span className="wl-cat__count">{countFor(id)}</span>
                </button>
              );
            })}
          </nav>

          <div className="wl__grid-wrap">
            {hits.length === 0 ? (
              <div className="wl__empty">
                <Search size={28} aria-hidden="true" />
                <p>{query ? t("empty", { query }) : filterLabel(filter)}</p>
                <p className="wl__empty-hint">{t("emptyHint")}</p>
              </div>
            ) : (
              <div id="wl-grid" ref={gridRef} className="wl__grid" role="listbox" aria-label={t("gridLabel")}>
                {hits.map(({ entry, indices }, i) => {
                  const state = openState(entry.id);
                  const fav = favorites.includes(entry.id);
                  const shortcut = entryShortcut(entry);
                  return (
                    <div
                      key={entry.id}
                      id={`wl-entry-${entry.id}`}
                      data-wl-entry={entry.id}
                      role="option"
                      aria-selected={selected?.id === entry.id}
                      className={clsx("wl-card", selected?.id === entry.id && "wl-card--selected", entry.phase > 0 && "wl-card--planned")}
                      onMouseEnter={() => setIndex(i)}
                      onClick={() => setIndex(i)}
                      onDoubleClick={() => openHere(entry)}
                    >
                      <Preview entry={entry} mode={mode} />
                      {state && (
                        <span className="wl-card__badge" title={t("openNowTitle")}>
                          <span className="wl-card__badge-dot" aria-hidden="true" />
                          {t("openNow")}
                        </span>
                      )}
                      <button
                        type="button"
                        tabIndex={-1}
                        className={clsx("wl-card__fav", fav && "wl-card__fav--on")}
                        aria-label={fav ? t("actions.unfavorite") : t("actions.favorite")}
                        title={fav ? t("actions.unfavorite") : t("actions.favorite")}
                        aria-pressed={fav}
                        onClick={(e) => {
                          e.stopPropagation();
                          toggleFavorite(entry.id);
                        }}
                      >
                        <Star size={14} />
                      </button>
                      <div className="wl-card__body">
                        <div className="wl-card__title-row">
                          <span className="wl-card__title">
                            <Highlight text={entry.title} indices={indices} />
                          </span>
                          {shortcut && <Shortcut keys={shortcut} />}
                        </div>
                        <p className="wl-card__summary">{entry.summary}</p>
                        <div className="wl-card__foot">
                          <span className={clsx("wl-tag", "wl-tag--cat", `wl-dot--${entry.category}`)}>{filterLabel(entry.category)}</span>
                          {entry.phase > 0 && <span className="wl-tag wl-tag--phase">{t("comingInPhase", { phase: entry.phase })}</span>}
                          {entry.tags.slice(0, 2).map((tag) => (
                            <span key={tag} className="wl-tag">
                              {tag}
                            </span>
                          ))}
                        </div>
                      </div>
                      <div className="wl-card__actions">
                        <button
                          type="button"
                          tabIndex={-1}
                          data-wl-open={entry.id}
                          className="wl-card__action wl-card__action--primary"
                          onClick={(e) => {
                            e.stopPropagation();
                            openHere(entry);
                          }}
                        >
                          {t("actions.openHere")}
                        </button>
                        {entry.kind === "panel" && (
                          <button
                            type="button"
                            tabIndex={-1}
                            className="wl-card__action"
                            aria-label={t("actions.openWindow")}
                            title={t("actions.openWindow")}
                            onClick={(e) => {
                              e.stopPropagation();
                              openInNewWindow(entry);
                            }}
                          >
                            <AppWindow size={14} />
                          </button>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>

          {selected && (
            <aside className="wl__detail" aria-live="polite">
              <Preview key={selected.id} entry={selected} mode={mode} large />
              <div className="wl__detail-head">
                <h3 className="wl__detail-title">{selected.title}</h3>
                <button
                  type="button"
                  className={clsx("wl-card__fav", "wl-card__fav--inline", favorites.includes(selected.id) && "wl-card__fav--on")}
                  aria-pressed={favorites.includes(selected.id)}
                  aria-label={favorites.includes(selected.id) ? t("actions.unfavorite") : t("actions.favorite")}
                  title={favorites.includes(selected.id) ? t("actions.unfavorite") : t("actions.favorite")}
                  onClick={() => toggleFavorite(selected.id)}
                >
                  <Star size={15} />
                </button>
              </div>
              <p className="wl__detail-text">{selected.details}</p>
              <dl className="wl__facts">
                <dt>{t("detail.category")}</dt>
                <dd>
                  <span className={clsx("wl-tag", "wl-tag--cat", `wl-dot--${selected.category}`)}>{filterLabel(selected.category)}</span>
                </dd>
                <dt>{t("detail.kind")}</dt>
                <dd>{selected.kind === "panel" ? t("detail.kindPanel") : t("detail.kindWindow")}</dd>
                {selected.kind === "panel" && (
                  <>
                    <dt>{t("detail.status")}</dt>
                    <dd>
                      {openState(selected.id) === "window"
                        ? t("detail.statusWindow")
                        : openState(selected.id) === "dock"
                          ? t("detail.statusOpen")
                          : t("detail.statusClosed")}
                    </dd>
                  </>
                )}
                {entryShortcut(selected) && (
                  <>
                    <dt>{t("detail.shortcut")}</dt>
                    <dd>
                      <Shortcut keys={entryShortcut(selected) ?? ""} />
                    </dd>
                  </>
                )}
                <dt>{t("detail.tags")}</dt>
                <dd className="wl__tags">
                  {selected.tags.map((tag) => (
                    <span key={tag} className="wl-tag">
                      {tag}
                    </span>
                  ))}
                </dd>
              </dl>
              <div className="wl__detail-actions">
                <button type="button" className="wl-btn wl-btn--primary" onClick={() => openHere(selected)}>
                  <ACTION_ICONS.openHere size={15} aria-hidden="true" />
                  {t("actions.openHere")}
                  <Kbd>Enter</Kbd>
                </button>
                {selected.kind === "panel" && (
                  <>
                    <button type="button" className="wl-btn" onClick={() => openInNewWindow(selected)}>
                      <ACTION_ICONS.openWindow size={15} aria-hidden="true" />
                      {t("actions.openWindow")}
                    </button>
                    <button type="button" className="wl-btn" onClick={() => addToLayout(selected)}>
                      <ACTION_ICONS.addToLayout size={15} aria-hidden="true" />
                      {t("actions.addToLayout")}
                    </button>
                  </>
                )}
                <button type="button" className="wl-btn" aria-pressed={pins.includes(selected.id)} onClick={() => togglePin(selected)}>
                  {pins.includes(selected.id) ? <PinOff size={15} aria-hidden="true" /> : <Pin size={15} aria-hidden="true" />}
                  {pins.includes(selected.id) ? t("actions.unpin") : t("actions.pin")}
                </button>
              </div>
            </aside>
          )}
        </div>

        <footer className="wl__foot" aria-hidden="true">
          {FOOT_HINTS.map(({ keys, hint }) => (
            <span key={hint}>
              {keys.map((k) => (
                <Kbd key={k}>{k}</Kbd>
              ))}{" "}
              {t(`keys.${hint}`)}
            </span>
          ))}
        </footer>
      </div>
    </Overlay>
  );
}
