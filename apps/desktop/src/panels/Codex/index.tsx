/**
 * Codex panel: Orkestr's chat UI over the official OpenAI Codex CLI — the
 * user's ChatGPT subscription (`codex.*`, ADR-0047), side by side with the
 * Claude Code panel. Screens: no project → Codex CLI not installed
 * («Install Codex CLI», npm) → not signed in («Sign in with ChatGPT»,
 * device code, terminal) → chat workspace (`Workspace.tsx`).
 */
import { Button, EmptyState, Spinner } from "@orkestr/ui";
import { Bot, FolderOpen, RefreshCw } from "lucide-react";
import { useEffect } from "react";

import { CodexAccountLine, CodexInstall, CodexSignIn } from "../../features/codex/CodexSetup";
import { useCodexStore } from "../../features/codex/store";
import { useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import type { PanelProps } from "../types";
import { CodexWorkspace } from "./Workspace";
import "../ClaudeCode/claudeCode.css";
import "../ClaudeCode/v2/v2.css";
import "../ClaudeCode/permissions/permissions.css";
import "./codex.css";

function SetupScreen({ projectId, step }: { projectId: string | null; step: "install" | "signIn" }) {
  const { t } = useT("codex");
  const status = useCodexStore((s) => s.status);
  return (
    <div className="cc-center" data-testid="codex-panel">
      <section className="cx-hero" aria-labelledby="cx-hero-title">
        <div className="cx-hero__glow" aria-hidden="true" />
        <div className="cx-hero__icon" aria-hidden="true">
          <Bot size={26} />
        </div>
        <h2 id="cx-hero-title" className="cx-hero__title">
          {step === "install" ? t("install.title") : t("signIn.title")}
        </h2>
        <p className="cx-hero__desc">{t("hero.description")}</p>
        {status?.binary && (
          <p className="cx-hero__meta">
            {t("hero.version", { version: status.binary.version ?? "—" })} · {t(`hero.source.${status.binary.source}`)}
          </p>
        )}
        {step === "install" ? <CodexInstall /> : (
          <>
            <CodexSignIn projectId={projectId} />
            <CodexInstall compact />
          </>
        )}
        {status?.error && <p className="cx-setup__warn">{status.error}</p>}
      </section>
    </div>
  );
}

export default function CodexPanel({ projectId }: PanelProps) {
  const { t } = useT("codex");
  const project = useAppStore((s) => (projectId ? (s.projects.find((p) => p.id === projectId) ?? null) : null));
  const status = useCodexStore((s) => s.status);
  const loading = useCodexStore((s) => s.statusLoading);
  const statusError = useCodexStore((s) => s.statusError);
  const loadStatus = useCodexStore((s) => s.loadStatus);

  useEffect(() => {
    if (!useCodexStore.getState().status) void loadStatus();
  }, [loadStatus]);

  if (!status) {
    return (
      <div className="cc-center" data-testid="codex-panel">
        {loading || !statusError ? (
          <Spinner size={18} label={t("status.checking")} />
        ) : (
          <EmptyState
            title={t("errors.status")}
            description={statusError}
            action={
              <Button leftIcon={<RefreshCw size={14} />} onClick={() => void loadStatus(true)}>
                {t("actions.checkAgain")}
              </Button>
            }
          />
        )}
      </div>
    );
  }
  if (!status.binary) return <SetupScreen projectId={projectId} step="install" />;
  if (!status.loggedIn) return <SetupScreen projectId={projectId} step="signIn" />;
  if (!projectId || !project) {
    return (
      <div className="cc-center" data-testid="codex-panel">
        <EmptyState icon={<Bot size={28} />} title={t("noProject.title")} description={t("noProject.description")} action={<CodexAccountLine />} />
      </div>
    );
  }
  if (!project.rootPath) {
    return (
      <div className="cc-center" data-testid="codex-panel">
        <EmptyState icon={<FolderOpen size={28} />} title={t("noFolder.title")} description={t("noFolder.description")} />
      </div>
    );
  }
  return (
    <div className="cx-root" data-testid="codex-panel">
      <div className="cx-accountbar">
        <CodexAccountLine />
        <CodexInstall compact />
      </div>
      <CodexWorkspace projectId={projectId} root={project.rootPath} />
    </div>
  );
}
