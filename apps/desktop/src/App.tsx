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
import { Toasts } from "./features/notifications/NotificationCenter";
import { OnboardingWizard } from "./features/onboarding/OnboardingWizard";
import { ResumeCard } from "./features/resume/ResumeCard";
import { SettingsView } from "./features/settings/SettingsView";
import { useShellBootstrap } from "./features/settings/useShellBootstrap";
import { useAppStore } from "./store/app";

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
  useShellBootstrap(ready);

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
      {ready && (
        <>
          <SettingsView />
          <OnboardingWizard />
          <ResumeCard />
        </>
      )}
      <Toasts />
    </div>
  );
}
