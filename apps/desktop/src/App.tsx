import { clsx } from "clsx";
import { useCallback, useEffect, useState } from "react";

import { CommandPalette } from "./components/CommandPalette";
import { WindowLibrary } from "./features/windowLibrary";
import { LauncherConfirmDialog, LauncherToast } from "./features/vscode/ExtensionLauncher";
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
import { AssistantSurface } from "./features/assistant/AssistantSurface";
import { AssistantTitleBar } from "./features/assistant/AssistantTitleBar";
import { registerAssistantFeature, useAssistantMode } from "./features/assistant/register";
import { registerCursorFeature } from "./features/cursor/register";
import { useAssistantStore } from "./features/assistant/store";
import { VisualLayer } from "./features/performance/VisualLayer";
import { registerRemoteSyncFeature } from "./features/remoteSync/register";
import { registerSecurityFeature } from "./features/security/register";
import { SecurityLayer } from "./features/security/SecurityLayer";
import { CHORD_TIMEOUT_MS, chordCommand, isModifierKey, shortcutCommand } from "./features/commands/dispatch";
import { commands, matchesShortcut } from "./features/commands/registry";
import { Toasts } from "./features/notifications/NotificationCenter";
import { OnboardingWizard } from "./features/onboarding/OnboardingWizard";
import { ResumeCard } from "./features/resume/ResumeCard";
import { OrbHost, registerOrbFeature } from "./features/orb/register";
import { SettingsView } from "./features/settings/SettingsView";
import { useShellBootstrap } from "./features/settings/useShellBootstrap";
import { registerThemeFeature } from "./features/theme/register";
import { ThemeEditor } from "./features/theme/ThemeEditor";
import { ThemeBackdrop, useThemeEngine } from "./features/theme/ThemeRuntime";
import { MissionControlOverlay } from "./panels/MissionControl";
import { useVoiceBootstrap, VoiceHud } from "./features/voice";
import { WelcomePage } from "./features/workbench/WelcomePage";
import { WorkbenchDialogs } from "./features/workbench/WorkbenchDialogs";
import { useAppStore } from "./store/app";

registerSecurityFeature();
registerRemoteSyncFeature();
registerThemeFeature();
registerLayoutFeature();
registerAssistantFeature();
registerCursorFeature();
registerOrbFeature();


function useGlobalShortcuts() {
  const setPaletteOpen = useAppStore((s) => s.setPaletteOpen);
  useEffect(() => {
    // Ctrl+K opens the palette and also arms VS Code-style chords
    // (`Ctrl+K Ctrl+O`): a matching second key within CHORD_TIMEOUT_MS runs
    // the chord command instead (and closes the palette again).
    let chordAt = 0;
    const onKey = (e: KeyboardEvent) => {
      if (isModifierKey(e.key)) return;
      const ctx = { projectId: useAppStore.getState().activeProjectId };
      if (chordAt !== 0) {
        const armed = Date.now() - chordAt <= CHORD_TIMEOUT_MS;
        chordAt = 0;
        const chord = armed ? chordCommand("Ctrl+K", e, commands, ctx) : null;
        if (chord) {
          e.preventDefault();
          e.stopPropagation();
          setPaletteOpen(false);
          void chord.run(ctx);
          return;
        }
      }
      if (matchesShortcut(e, "Ctrl+K")) {
        e.preventDefault();
        // Keep Ctrl+K away from Monaco: there it starts a chord (Ctrl+K Ctrl+C …)
        // and the editor would swallow the next key — the palette's Enter.
        e.stopPropagation();
        setPaletteOpen(!useAppStore.getState().paletteOpen);
        chordAt = Date.now();
        return;
      }
      // Registry commands with shortcuts (§54).
      // In a terminal only `inTerminal` commands fire; other keys reach the shell.
      const cmd = shortcutCommand(e, commands, ctx);
      if (!cmd) return;
      e.preventDefault();
      e.stopPropagation();
      void cmd.run(ctx);
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
  }, [setPaletteOpen]);
}

export function App() {
  const [ready, setReady] = useState(false);
  // No project open (closed, or none yet): the Welcome page takes the main area.
  // The workspace stays mounted underneath so the layout is kept.
  const showWelcome = useAppStore((s) => s.projectsLoaded && s.activeProjectId === null);
  useThemeEngine(ready);
  useGlobalShortcuts();
  useShellBootstrap(ready);
  useVoiceBootstrap(ready); // voice-sound (Phase 3)
  useAssistantMode(ready); // computer assistant (ADR-0035)
  const assistantMode = useAssistantStore((s) => s.mode === "assistant");

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
    <div className={clsx("app", assistantMode && "app--assistant")}>
      <ThemeBackdrop />
      <VisualLayer connected={ready} />
      {/* Assistant mode (ADR-0035) is its own compact shell: slim bar + assistant card only. */}
      {assistantMode ? <AssistantTitleBar /> : <TitleBar />}
      {ready && assistantMode && (
        <main className="app__assistant">
          <AssistantSurface />
        </main>
      )}
      {ready ? (
        // Assistant mode keeps the workspace mounted (layout, editors, terminals) but hidden.
        <div className={clsx("app__body", assistantMode && "app__body--hidden")} aria-hidden={assistantMode || undefined}>
          <ProjectRail />
          <main className="app__main">
            <div className={clsx("app__workspace", showWelcome && "app__workspace--hidden")} aria-hidden={showWelcome || undefined}>
              <Workspace />
            </div>
            {showWelcome && <WelcomePage />}
          </main>
        </div>
      ) : (
        <ConnectionScreen onRetry={boot} />
      )}
      {!assistantMode && <StatusBar />}
      <CommandPalette />
      {ready && <LauncherConfirmDialog />}
      <CreateProjectDialog />
      <SecurityLayer connected={ready} />
      {ready && (
        <>
          <SettingsView />
          <OnboardingWizard />
          <ThemeEditor />
          <LayoutEditorPanel />
          <LayoutDialogs />
          <WorkbenchDialogs />
          <MissionControlOverlay />
          <WindowLibrary />
          <OrbHost />
          {!assistantMode && <VoiceHud />}
        </>
      )}
      {ready && <AiUiLayer connected={ready} />}
      <div className="app__corner">
        {ready && !assistantMode && <ResumeCard />}
        {ready && <LauncherToast />}
        <Toasts />
      </div>
    </div>
  );
}
