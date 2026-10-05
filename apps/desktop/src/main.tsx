import "@orkestr/ui/styles.css";
import "dockview-react/dist/styles/dockview.css";
import "./styles/app.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { DetachedPanelApp } from "./features/layout/DetachedPanelApp";
import { parseDetachedRoute } from "./features/layout/route";
import { HudApp } from "./features/hud/HudApp";
import { isHudWindow } from "./features/hud/hudWindow";
import "./i18n";
import { installLanguageEffects } from "./lib/language";

installLanguageEffects();

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing");

// Detached panel windows load the same bundle at `#/panel/<id>` (layout-multimonitor, Phase 3).
const detached = parseDetachedRoute(window.location.hash);

createRoot(root).render(
  <StrictMode>
    {detached ? <DetachedPanelApp route={detached} /> : <App />}
    {isHudWindow() ? <HudApp /> : <App />}
  </StrictMode>,
);
