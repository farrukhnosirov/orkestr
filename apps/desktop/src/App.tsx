import { BUILTIN_THEMES, applyTheme } from "@orkestr/ui";
import { useCallback, useEffect, useState } from "react";

import { CommandPalette } from "./components/CommandPalette";
import { ConnectionScreen } from "./components/ConnectionScreen";
import { CreateProjectDialog } from "./components/CreateProjectDialog";
import { ProjectRail } from "./components/ProjectRail";
import { StatusBar } from "./components/StatusBar";
import { TitleBar } from "./components/TitleBar";
import { Workspace } from "./components/Workspace";
import { bootstrapCore } from "./core/client";
import { registerSecurityFeature } from "./features/security/register";
import { SecurityLayer } from "./features/security/SecurityLayer";
import { commands, matchesShortcut } from "./features/commands/registry";
import { useAppStore } from "./store/app";

registerSecurityFeature();

function useTheme() {
  const themeId = useAppStore((s) => s.themeId);
  useEffect(() => {
    const theme = BUILTIN_THEMES[themeId] ?? BUILTIN_THEMES["nebula"];
    if (!theme) return;
    try {
      applyTheme(theme);
    } catch (e) {
      console.error("failed to apply theme", e);
    }
  }, [themeId]);
}

function useGlobalShortcuts() {
  const setPaletteOpen = useAppStore((s) => s.setPaletteOpen);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen(!useAppStore.getState().paletteOpen);
        return;
      }
      // Registry commands with shortcuts (§54).
      for (const cmd of commands) {
        if (!cmd.shortcut || !matchesShortcut(e, cmd.shortcut)) continue;
        const ctx = { projectId: useAppStore.getState().activeProjectId };
        if (cmd.enabled && !cmd.enabled(ctx)) continue;
        e.preventDefault();
        e.stopPropagation();
        void cmd.run(ctx);
        return;
      }
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
  }, [setPaletteOpen]);
}

export function App() {
  const [ready, setReady] = useState(false);
  useTheme();
  useGlobalShortcuts();

  const boot = useCallback(() => {
    bootstrapCore().then(
      () => setReady(true),
      (e: unknown) => console.error("core bootstrap failed", e),
    );
  }, []);

  useEffect(() => {
    boot();
  }, [boot]);

  return (
    <div className="app">
      <div className="app__backdrop" aria-hidden="true" />
      <TitleBar />
      {ready ? (
        <div className="app__body">
          <ProjectRail />
          <main className="app__main">
            <Workspace />
          </main>
        </div>
      ) : (
        <ConnectionScreen onRetry={boot} />
      )}
      <StatusBar />
      <CommandPalette />
      <CreateProjectDialog />
      <SecurityLayer connected={ready} />
    </div>
  );
}
