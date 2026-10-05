/**
 * The robot's context menu (custom glass menu inside the robot window — the
 * window becomes fully interactive while it is open). Items and their
 * enabled state come from `model/menu.ts`; "Assign to project…" opens a
 * project list in place.
 */
import type { Project } from "@orkestr/protocol";
import { clsx } from "clsx";
import { ArrowLeft, Check, EyeOff, FolderInput, OctagonX, PanelRightOpen, Volume2 } from "lucide-react";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";

import { useT } from "../../../i18n";
import { nextEnabled, type RobotMenuItem, type RobotMenuItemId } from "../model/menu";

const ICONS: Record<RobotMenuItemId, typeof Check> = {
  open: PanelRightOpen,
  assign: FolderInput,
  voice: Volume2,
  hide: EyeOff,
  stop: OctagonX,
};

export interface RobotMenuProps {
  items: RobotMenuItem[];
  projects: readonly Project[];
  onAction: (id: RobotMenuItemId) => void;
  onAssign: (projectId: string) => void;
  onClose: () => void;
}

export function RobotMenu({ items, projects, onAction, onAssign, onClose }: RobotMenuProps) {
  const { t } = useT("robots");
  const [sub, setSub] = useState(false);
  const [active, setActive] = useState(() => nextEnabled(items, -1, 1));
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const rootRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!sub) refs.current[active]?.focus();
  }, [active, sub]);

  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (rootRef.current && e.target instanceof Node && !rootRef.current.contains(e.target)) onClose();
    };
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      if (sub) setSub(false);
      else onClose();
      return;
    }
    if (sub) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => nextEnabled(items, i, e.key === "ArrowDown" ? 1 : -1));
    }
  };

  return (
    <div ref={rootRef} className="robot-menu" role="menu" aria-label={t("menu.label")} onKeyDown={onKey}>
      {sub ? (
        <>
          <button type="button" role="menuitem" className="robot-menu__item robot-menu__item--back" onClick={() => setSub(false)}>
            <ArrowLeft size={14} /> {t("menu.assignTitle")}
          </button>
          <div className="robot-menu__list">
            {projects.length === 0 && <div className="robot-menu__empty">{t("menu.noProjects")}</div>}
            {projects.map((p) => (
              <button key={p.id} type="button" role="menuitem" className="robot-menu__item" onClick={() => onAssign(p.id)}>
                <span className="robot-menu__swatch" style={{ background: p.color ?? "var(--ok-color-accent)" }} aria-hidden="true" />
                <span className="robot-menu__label">{p.name}</span>
              </button>
            ))}
          </div>
        </>
      ) : (
        items.map((item, i) => {
          const Icon = ICONS[item.id];
          return (
            <button
              key={item.id}
              ref={(el) => {
                refs.current[i] = el;
              }}
              type="button"
              role={item.checked === undefined ? "menuitem" : "menuitemcheckbox"}
              aria-checked={item.checked}
              disabled={!item.enabled}
              tabIndex={i === active ? 0 : -1}
              className={clsx("robot-menu__item", item.danger && "robot-menu__item--danger")}
              onMouseEnter={() => item.enabled && setActive(i)}
              onClick={() => (item.submenu ? setSub(true) : onAction(item.id))}
            >
              <Icon size={14} aria-hidden="true" />
              <span className="robot-menu__label">{t(`menu.${item.id}`)}</span>
              {item.checked !== undefined && (
                <span className={clsx("robot-menu__check", item.checked && "robot-menu__check--on")} aria-hidden="true">
                  {item.checked && <Check size={12} />}
                </span>
              )}
              {item.submenu && <span className="robot-menu__more" aria-hidden="true">›</span>}
            </button>
          );
        })
      )}
    </div>
  );
}
