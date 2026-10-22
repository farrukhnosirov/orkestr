import "@orkestr/ui/styles.css";
import "dockview-react/dist/styles/dockview.css";
import "./styles/app.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { ContextMenuProvider, setContextMenuEnvironment } from "./features/contextMenu";
import { askManager } from "./features/files/actions";
import { SEARCH_PREFIX, usePaletteStore } from "./features/search/paletteStore";
import { DetachedPanelApp } from "./features/layout/DetachedPanelApp";
import { parseDetachedRoute } from "./features/layout/route";
import { HudApp } from "./features/hud/HudApp";
import { isHudWindow } from "./features/hud/hudWindow";
import "./i18n";
import { installLanguageEffects } from "./lib/language";
import { installWidgetDocumentLoader } from "./lib/tauri";
import { ownActiveProject, useAppStore } from "./store/app";

installLanguageEffects();

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing");

// Detached panel windows load the same bundle at `#/panel/<id>` (layout-multimonitor, Phase 3).
const detached = parseDetachedRoute(window.location.hash);
const hud = !detached && isHudWindow();
// Only the main shell persists the active project; the other windows follow it.
if (!detached && !hud) ownActiveProject();
// Plugin widgets (dashboards) render in the main shell and detached panels.
if (!hud) installWidgetDocumentLoader();
// Orkestr's own context menu: window-specific default actions on selected text.
if (!hud) {
  setContextMenuEnvironment({
    askManager: (text) => askManager({ text, projectId: useAppStore.getState().activeProjectId }),
    // Global search lives in the main window's command palette.
    ...(detached ? {} : { searchInOrkestr: (text: string) => usePaletteStore.getState().openPalette(`${SEARCH_PREFIX}${text}`) }),
  });
}

createRoot(root).render(
  <StrictMode>
    <ContextMenuProvider>
      {/* Exactly one root per window: detached panel, HUD, or the main shell. */}
      {detached ? <DetachedPanelApp route={detached} /> : hud ? <HudApp /> : <App />}
    </ContextMenuProvider>
  </StrictMode>,
);
