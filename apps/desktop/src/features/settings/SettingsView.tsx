import { IconButton } from "@orkestr/ui";
import { clsx } from "clsx";
import { Settings2, X } from "lucide-react";
import { type KeyboardEvent, useEffect, useMemo, useRef } from "react";

import { useAppStore } from "../../store/app";
import { orderedSections } from "./sections";
import { useSettingsStore } from "./settingsStore";
import "./settings.css";

/** Full-screen Settings overlay (TitleBar gear, `Ctrl+,`, command "Open Settings"). */
export function SettingsView() {
  const open = useSettingsStore((s) => s.open);
  const sectionId = useSettingsStore((s) => s.section);
  const setSection = useSettingsStore((s) => s.setSection);
  const close = useSettingsStore((s) => s.closeSettings);
  const projectId = useAppStore((s) => s.activeProjectId);
  const sections = useMemo(() => orderedSections(), []);
  const navRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const restoreFocus = useRef<HTMLElement | null>(null);

  const current = sections.find((s) => s.id === sectionId) ?? sections[0];

  useEffect(() => {
    if (!open) return;
    restoreFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const idx = Math.max(0, sections.findIndex((s) => s.id === current?.id));
    requestAnimationFrame(() => navRefs.current[idx]?.focus());
    if (!useSettingsStore.getState().settings) void useSettingsStore.getState().load();
    return () => restoreFocus.current?.focus();
    // Runs only when the overlay opens (section changes must not steal focus).
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!open || !current) return null;

  const onNavKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const pos = sections.findIndex((s) => s.id === current.id);
    let next: number | undefined;
    if (e.key === "ArrowDown") next = (pos + 1) % sections.length;
    else if (e.key === "ArrowUp") next = (pos - 1 + sections.length) % sections.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = sections.length - 1;
    if (next === undefined) return;
    e.preventDefault();
    const s = sections[next];
    if (s) {
      setSection(s.id);
      navRefs.current[next]?.focus();
    }
  };

  const Section = current.component;
  return (
    <div
      className="settings-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="settings-title"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          close();
        }
      }}
    >
      <aside className="settings-nav">
        <div className="settings-nav__head" id="settings-title">
          <Settings2 size={16} /> Settings
        </div>
        <div role="tablist" aria-orientation="vertical" aria-label="Settings sections" onKeyDown={onNavKey}>
          {sections.map((s, i) => {
            const Icon = s.icon;
            const active = s.id === current.id;
            return (
              <button
                key={s.id}
                ref={(el) => {
                  navRefs.current[i] = el;
                }}
                type="button"
                role="tab"
                id={`settings-tab-${s.id}`}
                aria-selected={active}
                aria-controls="settings-panel"
                tabIndex={active ? 0 : -1}
                className={clsx("settings-nav__item", active && "settings-nav__item--active")}
                onClick={() => setSection(s.id)}
              >
                <Icon size={15} />
                {s.title}
              </button>
            );
          })}
        </div>
      </aside>
      <main className="settings-main">
        <header className="settings-main__head">
          <h2 className="settings-main__title">{current.title}</h2>
          <IconButton label="Close settings (Esc)" icon={<X size={16} />} onClick={close} tooltip="left" />
        </header>
        <div
          className="settings-main__body"
          role="tabpanel"
          id="settings-panel"
          aria-labelledby={`settings-tab-${current.id}`}
        >
          <Section projectId={projectId} />
        </div>
      </main>
    </div>
  );
}
