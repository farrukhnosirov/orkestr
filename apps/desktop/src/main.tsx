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
import { OrbApp } from "./features/orb/OrbApp";
import { isOrbWindow } from "./features/orb/shell";
import "./i18n";
import { installLanguageEffects } from "./lib/language";
import { installWidgetDocumentLoader } from "./lib/tauri";
import { ownActiveProject } from "./store/app";

installLanguageEffects();

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing");

// Detached panel windows load the same bundle at `#/panel/<id>` (layout-multimonitor, Phase 3).
const detached = parseDetachedRoute(window.location.hash);
const hud = !detached && isHudWindow();
// The floating orb (ADR-0045): transparent from the first paint.
const orb = !detached && !hud && isOrbWindow();
if (orb) document.documentElement.dataset["okWindow"] = "orb";
// Only the main shell persists the active project; the other windows follow it.
if (!detached && !hud && !orb) ownActiveProject();
// Plugin widgets (dashboards) render in the main shell and detached panels.
if (!hud && !orb) installWidgetDocumentLoader();

createRoot(root).render(
  <StrictMode>
    {/* Exactly one root per window: detached panel, HUD, orb, or the main shell. */}
    {detached ? <DetachedPanelApp route={detached} /> : hud ? <HudApp /> : orb ? <OrbApp /> : <App />}
  </StrictMode>,
);
