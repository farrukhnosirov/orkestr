/**
 * The orb's own context menu (right-click / Shift+F10 / the menu key on the
 * sphere): open the hub or Orkestr, animation level, badge, settings, hide.
 * Arrow keys move, Enter / Space activate, Esc closes.
 */
import type { OrbAnimation } from "@orkestr/protocol";
import { clsx } from "clsx";
import { Check, EyeOff, LayoutPanelLeft, ExternalLink, Settings2, Sparkles } from "lucide-react";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";

import { useT } from "../../i18n";

export type OrbMenuAction =
  | { kind: "hub" }
  | { kind: "main" }
  | { kind: "animation"; value: OrbAnimation }
  | { kind: "badge" }
  | { kind: "settings" }
  | { kind: "hide" };

interface Item {
  id: string;
  action: OrbMenuAction;
  label: string;
  icon: typeof Check | null;
  checked?: boolean;
  radio?: boolean;
  danger?: boolean;
  separatorBefore?: boolean;
}

export const ANIMATIONS: readonly OrbAnimation[] = ["Full", "Reduced", "Static"];

export function OrbMenu({
  animation,
  showBadge,
  onAction,
  onClose,
}: {
  animation: OrbAnimation;
  showBadge: boolean;
  onAction: (a: OrbMenuAction) => void;
  onClose: () => void;
}) {
  const { t } = useT("orb");
  const items: Item[] = [
    { id: "hub", action: { kind: "hub" }, label: t("menu.hub"), icon: LayoutPanelLeft },
    { id: "main", action: { kind: "main" }, label: t("menu.openMain"), icon: ExternalLink },
    ...ANIMATIONS.map(
      (value, i): Item => ({
        id: `anim-${value}`,
        action: { kind: "animation", value },
        label: t(`animation.${value}`),
        icon: Sparkles,
        checked: animation === value,
        radio: true,
        separatorBefore: i === 0,
      }),
    ),
    { id: "badge", action: { kind: "badge" }, label: t("menu.badge"), icon: null, checked: showBadge, separatorBefore: true },
    { id: "settings", action: { kind: "settings" }, label: t("menu.settings"), icon: Settings2 },
    { id: "hide", action: { kind: "hide" }, label: t("menu.hide"), icon: EyeOff, danger: true },
  ];
  const [active, setActive] = useState(0);
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const rootRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => refs.current[active]?.focus(), [active]);

  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (rootRef.current && e.target instanceof Node && !rootRef.current.contains(e.target)) onClose();
    };
    window.addEventListener("pointerdown", onDown, true);
    return () => window.removeEventListener("pointerdown", onDown, true);
  }, [onClose]);

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = items.length;
      setActive((i) => (i + (e.key === "ArrowDown" ? 1 : n - 1)) % n);
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      setActive(e.key === "Home" ? 0 : items.length - 1);
    }
  };

  return (
    <div ref={rootRef} className="orb-menu" role="menu" aria-label={t("menu.label")} onKeyDown={onKey}>
      {items.map((item, i) => {
        const Icon = item.icon;
        return (
          <div key={item.id} role="none">
            {item.separatorBefore && <div className="orb-menu__sep" role="separator" />}
            {item.radio && i === 2 && <div className="orb-menu__heading">{t("menu.animation")}</div>}
            <button
              ref={(el) => {
                refs.current[i] = el;
              }}
              type="button"
              role={item.radio ? "menuitemradio" : item.checked !== undefined ? "menuitemcheckbox" : "menuitem"}
              aria-checked={item.checked}
              tabIndex={i === active ? 0 : -1}
              className={clsx("orb-menu__item", item.danger && "orb-menu__item--danger")}
              onMouseEnter={() => setActive(i)}
              onClick={() => onAction(item.action)}
            >
              <span className="orb-menu__icon" aria-hidden="true">
                {item.checked !== undefined ? item.checked ? <Check size={13} /> : null : Icon ? <Icon size={13} /> : null}
              </span>
              <span className="orb-menu__label">{item.label}</span>
            </button>
          </div>
        );
      })}
    </div>
  );
}
