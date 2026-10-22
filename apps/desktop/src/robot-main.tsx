/**
 * Entry of floating robot windows (`robot.html`, ADR-0016). A separate, small
 * bundle: no docking layout, panels or editors — only the robot renderer, the
 * RPC client and i18n. three.js stays lazy (3D robots, Ultra Visual only).
 */
import "@orkestr/ui/styles.css";
import "./features/robots/window/robot-window.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { ContextMenuProvider } from "./features/contextMenu";
import "./i18n";
import { RobotApp } from "./features/robots/window/RobotApp";

const root = document.getElementById("root");
if (!root) throw new Error("#root element missing");

createRoot(root).render(
  <StrictMode>
    {/* No browser menu on the robots either (items only where text is selected / editable). */}
    <ContextMenuProvider>
      <RobotApp />
    </ContextMenuProvider>
  </StrictMode>,
);
