import { useCallback, useEffect, useState } from "react";

import { CommandPalette } from "./components/CommandPalette";
import { ConnectionScreen } from "./components/ConnectionScreen";
import { CreateProjectDialog } from "./components/CreateProjectDialog";
import { ProjectRail } from "./components/ProjectRail";
import { StatusBar } from "./components/StatusBar";
import { TitleBar } from "./components/TitleBar";
import { Workspace } from "./components/Workspace";
import { bootstrapCore } from "./core/client";
import { LayoutDialogs } from "./features/layout/LayoutDialogs";
import { LayoutEditorPanel } from "./features/layout/LayoutEditor";
import { registerLayoutFeature } from "./features/layout/register";
import { AiUiLayer } from "./features/aiui/AiUiLayer";
import { VisualLayer } from "./features/performance/VisualLayer";
import { registerSecurityFeature } from "./features/security/register";
import { SecurityLayer } from "./features/security/SecurityLayer";
import { keyMatches } from "./features/commands/keys";
import { commands, matchesShortcut } from "./features/commands/registry";
import { Toasts } from "./features/notifications/NotificationCenter";
import { OnboardingWizard } from "./features/onboarding/OnboardingWizard";
import { ResumeCard } from "./features/resume/ResumeCard";
import { RobotHost, registerRobotsFeature } from "./features/robots/register";
import { SettingsView } from "./features/settings/SettingsView";
import { useShellBootstrap } from "./features/settings/useShellBootstrap";
import { registerThemeFeature } from "./features/theme/register";
import { ThemeEditor } from "./features/theme/ThemeEditor";
import { ThemeBackdrop, useThemeEngine } from "./features/theme/ThemeRuntime";
import { MissionControlOverlay } from "./panels/MissionControl";
import { useVoiceBootstrap, VoiceHud } from "./features/voice";
import { useAppStore } from "./store/app";

registerSecurityFeature();
registerThemeFeature();
registerLayoutFeature();
registerRobotsFeature();


function useGlobalShortcuts() {
  const setPaletteOpen = useAppStore((s) => s.setPaletteOpen);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && keyMatches(e, "k")) {
        e.preventDefault();
        // Keep Ctrl+K away from Monaco: there it starts a chord (Ctrl+K Ctrl+C …)
        // and the editor would swallow the next key — the palette's Enter.
        e.stopPropagation();
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
  useVoiceBootstrap(ready); // voice-sound (Phase 3)

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
      <VisualLayer connected={ready} />
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
          <LayoutEditorPanel />
          <LayoutDialogs />
          <MissionControlOverlay />
          <RobotHost />
          <VoiceHud />
        </>
      )}
      {ready && <AiUiLayer connected={ready} />}
      <Toasts />
    </div>
  );
}
