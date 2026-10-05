/** Title-bar layout switcher (§29): presets, user layouts, save, editor, lock. */
import "./layout.css";

import { clsx } from "clsx";
import { Check, ChevronDown, LayoutTemplate, Lock, Save, SlidersHorizontal, Unlock } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useRef, useState } from "react";

import { useT } from "../../i18n";
import * as layout from "./controller";
import { useCalmMotion } from "./motion";
import { layoutName } from "./names";
import { getBuiltinPreset } from "./presets";
import { layoutEntries } from "./selection";
import { useLayoutStore } from "./store";

export function LayoutSwitcher() {
  const { t } = useT("layout");
  const [open, setOpen] = useState(false);
  const state = useLayoutStore((s) => s.state);
  const activeId = useLayoutStore((s) => s.activeId);
  const locked = useLayoutStore((s) => s.locked);
  const loaded = useLayoutStore((s) => s.loaded);
  const calm = useCalmMotion();
  const rootRef = useRef<HTMLDivElement>(null);
  const entries = useMemo(() => layoutEntries(state), [state]);
  const active = entries.find((e) => e.id === activeId);
  const activeName = layoutName(activeId, active?.name);
  const ActiveIcon = getBuiltinPreset(activeId)?.icon ?? LayoutTemplate;

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const pick = (id: string) => {
    setOpen(false);
    void layout.switchLayout(id);
  };

  const builtins = entries.filter((e) => e.builtin);
  const users = entries.filter((e) => !e.builtin);

  return (
    <div className="lyt-switcher" ref={rootRef}>
      <button
        type="button"
        className={clsx("lyt-switcher__btn", open && "lyt-switcher__btn--open")}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={t("switcher.aria", { name: activeName })}
        title={t("switcher.aria", { name: activeName })}
        onClick={() => setOpen((o) => !o)}
      >
        <ActiveIcon size={13} className="lyt-switcher__icon" />
        <span className="lyt-switcher__name">{activeName}</span>
        {locked && <Lock size={11} className="lyt-switcher__lock" />}
        <ChevronDown size={12} className="lyt-switcher__chevron" />
      </button>
      <AnimatePresence>
        {open && (
          <motion.div
            className="lyt-menu"
            role="menu"
            initial={calm ? false : { opacity: 0, y: -6, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={calm ? { opacity: 0, transition: { duration: 0 } } : { opacity: 0, y: -4, scale: 0.98 }}
            transition={{ duration: 0.16, ease: [0.2, 0.8, 0.2, 1] }}
          >
            <div className="lyt-menu__label">{t("switcher.presets")}</div>
            {builtins.map((e) => {
              const Icon = getBuiltinPreset(e.id)?.icon ?? LayoutTemplate;
              return (
                <button key={e.id} type="button" role="menuitemradio" aria-checked={e.id === activeId} className="lyt-menu__item" onClick={() => pick(e.id)}>
                  <Icon size={14} className="lyt-menu__icon" />
                  <span className="lyt-menu__text">{layoutName(e.id)}</span>
                  {e.customised && <span className="lyt-dot" title={t("switcher.customised")} />}
                  {e.shortcut && <kbd className="lyt-kbd">{e.shortcut}</kbd>}
                  <Check size={13} className={clsx("lyt-menu__check", e.id !== activeId && "lyt-menu__check--hidden")} />
                </button>
              );
            })}
            <div className="lyt-menu__label">{t("switcher.mine")}</div>
            {users.length === 0 && <div className="lyt-menu__empty">{t("switcher.empty")}</div>}
            {users.map((e) => (
              <button key={e.id} type="button" role="menuitemradio" aria-checked={e.id === activeId} className="lyt-menu__item" onClick={() => pick(e.id)}>
                <LayoutTemplate size={14} className="lyt-menu__icon" />
                <span className="lyt-menu__text">{e.name}</span>
                {e.popouts > 0 && <span className="lyt-menu__meta">{t("switcher.windows", { count: e.popouts })}</span>}
                <Check size={13} className={clsx("lyt-menu__check", e.id !== activeId && "lyt-menu__check--hidden")} />
              </button>
            ))}
            <div className="lyt-menu__sep" />
            <button
              type="button"
              role="menuitem"
              className="lyt-menu__item"
              disabled={!loaded}
              onClick={() => {
                setOpen(false);
                useLayoutStore.getState().patch({ dialog: { kind: "saveAs" } });
              }}
            >
              <Save size={14} className="lyt-menu__icon" />
              <span className="lyt-menu__text">{t("switcher.saveAs")}</span>
            </button>
            <button
              type="button"
              role="menuitem"
              className="lyt-menu__item"
              onClick={() => {
                setOpen(false);
                layout.setLocked(!locked);
              }}
            >
              {locked ? <Unlock size={14} className="lyt-menu__icon" /> : <Lock size={14} className="lyt-menu__icon" />}
              <span className="lyt-menu__text">{locked ? t("switcher.unlock") : t("switcher.lock")}</span>
            </button>
            <button
              type="button"
              role="menuitem"
              className="lyt-menu__item"
              onClick={() => {
                setOpen(false);
                layout.openEditor(true);
              }}
            >
              <SlidersHorizontal size={14} className="lyt-menu__icon" />
              <span className="lyt-menu__text">{t("switcher.editor")}</span>
            </button>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
