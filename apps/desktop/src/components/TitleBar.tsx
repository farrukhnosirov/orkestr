import { Kbd } from "@orkestr/ui";
import { Copy, Minus, Search, Square, X } from "lucide-react";
import { useEffect, useState } from "react";

import { isTauri, windowControls } from "../lib/tauri";
import { useActiveProject, useAppStore } from "../store/app";
import { Logo } from "./Logo";

/** Custom frameless title bar (§28): drag region, palette trigger, window controls. */
export function TitleBar() {
  const project = useActiveProject();
  const setPaletteOpen = useAppStore((s) => s.setPaletteOpen);
  const [maximized, setMaximized] = useState(false);
  const native = isTauri();

  useEffect(() => {
    if (!native) return;
    let unlisten: (() => void) | undefined;
    const sync = () => {
      windowControls.isMaximized().then(setMaximized, () => {});
    };
    sync();
    windowControls.onResized(sync).then((u) => {
      unlisten = u;
    }, () => {});
    return () => unlisten?.();
  }, [native]);

  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="titlebar__brand" data-tauri-drag-region>
        <Logo size={20} />
        <span className="titlebar__wordmark" data-tauri-drag-region>
          Orkestr
        </span>
        {project && (
          <>
            <span className="titlebar__sep" data-tauri-drag-region>
              /
            </span>
            <span className="titlebar__project" data-tauri-drag-region>
              <span className="titlebar__project-dot" style={{ background: project.color ?? undefined }} />
              {project.name}
            </span>
          </>
        )}
      </div>

      <button type="button" className="titlebar__search" onClick={() => setPaletteOpen(true)}>
        <Search size={14} />
        <span>Search or run a command…</span>
        <span className="titlebar__search-keys">
          <Kbd>Ctrl</Kbd>
          <Kbd>K</Kbd>
        </span>
      </button>

      <div className="titlebar__spacer" data-tauri-drag-region />

      {native && (
        <div className="titlebar__controls">
          <button type="button" className="titlebar__ctl" aria-label="Minimize" onClick={() => void windowControls.minimize()}>
            <Minus size={15} />
          </button>
          <button
            type="button"
            className="titlebar__ctl"
            aria-label={maximized ? "Restore" : "Maximize"}
            onClick={() => void windowControls.toggleMaximize()}
          >
            {maximized ? <Copy size={13} style={{ transform: "scaleX(-1)" }} /> : <Square size={12} />}
          </button>
          <button
            type="button"
            className="titlebar__ctl titlebar__ctl--close"
            aria-label="Close"
            onClick={() => void windowControls.close()}
          >
            <X size={16} />
          </button>
        </div>
      )}
    </header>
  );
}
