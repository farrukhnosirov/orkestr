/**
 * VS Code-style "File" menu in the custom title bar. Keyboard accessible:
 * Alt+F / Enter / ↓ opens it, ↑ ↓ Home End move, → opens "Recent projects",
 * ← / Esc close the submenu, Esc / Tab close the menu (focus returns to the
 * trigger). Items run the registry commands (`commands.ts`), so the menu,
 * the palette and the shortcuts always do the same thing.
 */
import "./workbench.css";

import { clsx } from "clsx";
import { ChevronRight, Clock, FolderPlus, type LucideIcon } from "lucide-react";
import { type KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";

import { useT } from "../../i18n";
import { isTauri } from "../../lib/tauri";
import { useAppStore } from "../../store/app";
import { commands } from "../commands/registry";
import { openRecent } from "./actions";
import { recentProjects, useWorkbenchStore } from "./store";

type MenuLabelKey =
  | "newProject"
  | "openFolder"
  | "openFile"
  | "clone"
  | "newFile"
  | "newFolder"
  | "save"
  | "saveAll"
  | "reveal"
  | "closeProject"
  | "exit";

type Entry = { kind: "cmd"; command: string; label: MenuLabelKey } | { kind: "sep" } | { kind: "recent" };

/** Menu layout (command ids from the registry). */
export const FILE_MENU: readonly Entry[] = [
  { kind: "cmd", command: "project.create", label: "newProject" },
  { kind: "cmd", command: "file.openFolder", label: "openFolder" },
  { kind: "cmd", command: "file.openFile", label: "openFile" },
  { kind: "cmd", command: "file.clone", label: "clone" },
  { kind: "sep" },
  { kind: "cmd", command: "file.newFile", label: "newFile" },
  { kind: "cmd", command: "file.newFolder", label: "newFolder" },
  { kind: "sep" },
  { kind: "cmd", command: "file.save", label: "save" },
  { kind: "cmd", command: "file.saveAll", label: "saveAll" },
  { kind: "sep" },
  { kind: "recent" },
  { kind: "sep" },
  { kind: "cmd", command: "file.reveal", label: "reveal" },
  { kind: "cmd", command: "file.closeProject", label: "closeProject" },
  { kind: "sep" },
  { kind: "cmd", command: "app.exit", label: "exit" },
];

const ITEM = "[role='menuitem']:not(:disabled)";

function focusables(root: HTMLElement | null): HTMLElement[] {
  return root ? [...root.querySelectorAll<HTMLElement>(`:scope > ${ITEM}, :scope > .wb-menu__sub-host > ${ITEM}`)] : [];
}

/** Moves focus among the enabled items of `root` (↑ ↓ Home End). Returns whether the key was handled. */
function moveFocus(root: HTMLElement | null, key: string): boolean {
  const items = focusables(root);
  if (items.length === 0) return false;
  const i = items.indexOf(document.activeElement as HTMLElement);
  let next: HTMLElement | undefined;
  if (key === "ArrowDown") next = items[(i + 1) % items.length];
  else if (key === "ArrowUp") next = items[(i - 1 + items.length) % items.length];
  else if (key === "Home") next = items[0];
  else if (key === "End") next = items[items.length - 1];
  else return false;
  next?.focus();
  return true;
}

export function FileMenu() {
  const { t } = useT("workbench");
  const open = useWorkbenchStore((s) => s.menuOpen);
  const setOpen = useWorkbenchStore((s) => s.setMenuOpen);
  const hidden = useWorkbenchStore((s) => s.hiddenRecent);
  const projects = useAppStore((s) => s.projects);
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const [subOpen, setSubOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const subRef = useRef<HTMLDivElement>(null);
  const recentRef = useRef<HTMLButtonElement>(null);
  const recent = useMemo(() => recentProjects(projects, hidden, activeProjectId), [projects, hidden, activeProjectId]);
  const native = isTauri();
  const ctx = { projectId: activeProjectId };

  // Focus the first item when the menu opens (click, keyboard or Alt+F).
  useEffect(() => {
    if (!open) {
      setSubOpen(false);
      return;
    }
    const id = requestAnimationFrame(() => focusables(menuRef.current)[0]?.focus());
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => {
      cancelAnimationFrame(id);
      window.removeEventListener("mousedown", onDown);
    };
  }, [open, setOpen]);

  const close = (refocus: boolean) => {
    setOpen(false);
    if (refocus) triggerRef.current?.focus();
  };

  const run = (commandId: string) => {
    const cmd = commands.find((c) => c.id === commandId);
    close(false);
    if (cmd && (!cmd.enabled || cmd.enabled(ctx))) void cmd.run(ctx);
  };

  const openSub = (focusFirst: boolean) => {
    setSubOpen(true);
    if (focusFirst) requestAnimationFrame(() => focusables(subRef.current)[0]?.focus());
  };

  const onMenuKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close(true);
    } else if (e.key === "Tab") {
      close(false);
    } else if (e.key === "ArrowRight" && document.activeElement === recentRef.current) {
      e.preventDefault();
      openSub(true);
    } else if (moveFocus(menuRef.current, e.key)) {
      e.preventDefault();
      setSubOpen(false);
    }
  };

  const onSubKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape" || e.key === "ArrowLeft") {
      e.preventDefault();
      e.stopPropagation();
      setSubOpen(false);
      recentRef.current?.focus();
    } else if (moveFocus(subRef.current, e.key)) {
      e.preventDefault();
      e.stopPropagation();
    } else if (e.key === "Tab") {
      close(false);
    }
  };

  const onTriggerKey = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (e.key === "ArrowDown" && !open) {
      e.preventDefault();
      setOpen(true);
    }
  };

  return (
    <div className="wb-menubar" role="menubar" aria-label={t("menu.aria")} ref={rootRef}>
      <button
        ref={triggerRef}
        type="button"
        role="menuitem"
        className={clsx("wb-menubar__btn", open && "wb-menubar__btn--open")}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? "wb-file-menu" : undefined}
        title={t("menu.fileAria")}
        onClick={() => setOpen(!open)}
        onKeyDown={onTriggerKey}
      >
        {t("menu.file")}
      </button>
      {open && (
        <div id="wb-file-menu" className="wb-menu" role="menu" aria-label={t("menu.fileAria")} ref={menuRef} onKeyDown={onMenuKey}>
          {FILE_MENU.map((entry, i) => {
            if (entry.kind === "sep") return <div key={`sep-${i}`} className="wb-menu__sep" role="separator" />;
            if (entry.kind === "recent") {
              return (
                <div key="recent" className="wb-menu__sub-host" onMouseEnter={() => openSub(false)} onMouseLeave={() => setSubOpen(false)}>
                  <button
                    ref={recentRef}
                    type="button"
                    role="menuitem"
                    tabIndex={-1}
                    aria-haspopup="menu"
                    aria-expanded={subOpen}
                    className={clsx("wb-menu__item", subOpen && "wb-menu__item--active")}
                    onClick={() => openSub(true)}
                  >
                    <Clock size={14} className="wb-menu__icon" />
                    <span className="wb-menu__text">{t("menu.recent")}</span>
                    <ChevronRight size={13} className="wb-menu__chevron" />
                  </button>
                  {subOpen && (
                    <div className="wb-menu wb-menu--sub" role="menu" aria-label={t("menu.recent")} ref={subRef} onKeyDown={onSubKey}>
                      {recent.length === 0 ? (
                        <button type="button" role="menuitem" tabIndex={-1} disabled className="wb-menu__item">
                          <span className="wb-menu__text">{t("menu.recentEmpty")}</span>
                        </button>
                      ) : (
                        recent.map((p) => (
                          <button
                            key={p.id}
                            type="button"
                            role="menuitem"
                            tabIndex={-1}
                            className="wb-menu__item"
                            title={p.rootPath ?? p.name}
                            onClick={() => {
                              close(false);
                              openRecent(p.id);
                            }}
                          >
                            <span className="wb-menu__dot" style={{ background: p.color ?? undefined }} />
                            <span className="wb-menu__text">{p.name}</span>
                            {p.rootPath && <span className="wb-menu__meta">{p.rootPath}</span>}
                          </button>
                        ))
                      )}
                    </div>
                  )}
                </div>
              );
            }
            if (entry.command === "app.exit" && !native) return null;
            const cmd = commands.find((c) => c.id === entry.command);
            if (!cmd) return null;
            const Icon: LucideIcon = entry.command === "project.create" ? FolderPlus : (cmd.icon ?? FolderPlus);
            const enabled = !cmd.enabled || cmd.enabled(ctx);
            return (
              <button
                key={entry.command}
                type="button"
                role="menuitem"
                tabIndex={-1}
                disabled={!enabled}
                className="wb-menu__item"
                onMouseEnter={() => setSubOpen(false)}
                onClick={() => run(entry.command)}
              >
                <Icon size={14} className="wb-menu__icon" />
                <span className="wb-menu__text">{t(`menu.${entry.label}`)}</span>
                {cmd.shortcut && <kbd className="wb-menu__kbd">{cmd.shortcut}</kbd>}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
