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
import { Toasts } from "./features/notifications/NotificationCenter";
import { OnboardingWizard } from "./features/onboarding/OnboardingWizard";
import { ResumeCard } from "./features/resume/ResumeCard";
import { SettingsView } from "./features/settings/SettingsView";
import { useShellBootstrap } from "./features/settings/useShellBootstrap";
import { registerThemeFeature } from "./features/theme/register";
import { ThemeEditor } from "./features/theme/ThemeEditor";
import { ThemeBackdrop, useThemeEngine } from "./features/theme/ThemeRuntime";
import { useAppStore } from "./store/app";

registerSecurityFeature();
registerThemeFeature();

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
  useThemeEngine(ready);
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
      <ThemeBackdrop />
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
      {ready && (
        <>
          <SettingsView />
          <OnboardingWizard />
          <ResumeCard />
          <ThemeEditor />
        </>
      )}
      <Toasts />
    </div>
  );
}
