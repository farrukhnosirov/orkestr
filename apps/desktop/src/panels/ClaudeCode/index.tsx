/**
 * Claude Code panel: Orkestr's own chat UI over the official `claude` CLI
 * (`claudeCode.*`). Screens: no project → Claude Code not found (pin a
 * path) → not signed in («Войти в Claude» opens a terminal running
 * `claude auth login`) → chat workspace (`v2/Workspace.tsx`): the «Чаты»
 * sidebar, plan usage, «Возможности», commands as buttons, messages and
 * the composer.
 */
import { Button, EmptyState, Spinner } from "@orkestr/ui";
import { FolderOpen, LogIn, RefreshCw, Sparkle } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";

import { workspaceActions } from "../../components/workspaceActions";
import { useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { useClaudeStore } from "../../features/claudeCode/store";
import { useTerminalUi } from "../Terminal/store";
import type { PanelProps } from "../types";
import { ChatWorkspace } from "./v2/Workspace";
import "./claudeCode.css";

function NotFound() {
  const { t } = useT("claudeCode");
  const status = useClaudeStore((s) => s.status);
  const setBinaryPath = useClaudeStore((s) => s.setBinaryPath);
  const loadStatus = useClaudeStore((s) => s.loadStatus);
  const loading = useClaudeStore((s) => s.statusLoading);
  const error = useClaudeStore((s) => s.statusError);
  const [path, setPath] = useState(status?.binaryOverride ?? "");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    void setBinaryPath(path);
  };
  return (
    <div className="cc-center">
      <EmptyState
        icon={<Sparkle size={28} />}
        title={t("notFound.title")}
        description={t("notFound.description")}
        action={
          <form className="cc-path-form" onSubmit={submit}>
            <input
              className="cc-input"
              value={path}
              onChange={(e) => setPath(e.target.value)}
              placeholder={t("notFound.pathPlaceholder")}
              aria-label={t("notFound.pathLabel")}
            />
            <Button type="submit" variant="primary">
              {t("notFound.usePath")}
            </Button>
            <Button leftIcon={<RefreshCw size={14} />} loading={loading} onClick={() => void loadStatus(true)}>
              {t("actions.checkAgain")}
            </Button>
          </form>
        }
      />
      {(status?.overrideError || error) && <div className="cc-error-text">{status?.overrideError ?? error}</div>}
    </div>
  );
}

function LoginScreen({ projectId, onContinue }: { projectId: string; onContinue: () => void }) {
  const { t } = useT("claudeCode");
  const login = useClaudeStore((s) => s.login);
  const loadStatus = useClaudeStore((s) => s.loadStatus);
  const loading = useClaudeStore((s) => s.statusLoading);
  const error = useClaudeStore((s) => s.error);
  const [opened, setOpened] = useState(false);
  const signIn = async () => {
    const id = await login(projectId);
    if (id) {
      useTerminalUi.getState().setActive(projectId, id);
      workspaceActions.openPanel("terminal");
      setOpened(true);
    }
  };
  return (
    <div className="cc-center">
      <EmptyState
        icon={<LogIn size={28} />}
        title={t("login.title")}
        description={opened ? t("login.openedHint") : t("login.description")}
        action={
          <div className="cc-row">
            <Button variant="primary" leftIcon={<LogIn size={14} />} onClick={() => void signIn()}>
              {t("login.signIn")}
            </Button>
            <Button leftIcon={<RefreshCw size={14} />} loading={loading} onClick={() => void loadStatus(true)}>
              {t("actions.checkAgain")}
            </Button>
            <Button variant="ghost" onClick={onContinue}>
              {t("login.continue")}
            </Button>
          </div>
        }
      />
      {error && <div className="cc-error-text">{error}</div>}
    </div>
  );
}

export default function ClaudeCodePanel({ projectId }: PanelProps) {
  const { t } = useT("claudeCode");
  const project = useAppStore((s) => (projectId ? (s.projects.find((p) => p.id === projectId) ?? null) : null));
  const status = useClaudeStore((s) => s.status);
  const loading = useClaudeStore((s) => s.statusLoading);
  const statusError = useClaudeStore((s) => s.statusError);
  const loadStatus = useClaudeStore((s) => s.loadStatus);
  const [skipLogin, setSkipLogin] = useState(false);

  useEffect(() => {
    if (!useClaudeStore.getState().status) void loadStatus();
  }, [loadStatus]);

  if (!projectId || !project) {
    return (
      <div className="cc-center">
        <EmptyState icon={<Sparkle size={28} />} title={t("noProject.title")} description={t("noProject.description")} />
      </div>
    );
  }
  if (!project.rootPath) {
    return (
      <div className="cc-center">
        <EmptyState icon={<FolderOpen size={28} />} title={t("noFolder.title")} description={t("noFolder.description")} />
      </div>
    );
  }
  if (!status) {
    return (
      <div className="cc-center">
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
  if (!status.binary) return <NotFound />;
  if (status.auth && !status.auth.loggedIn && !skipLogin) {
    return <LoginScreen projectId={projectId} onContinue={() => setSkipLogin(true)} />;
  }
  return <ChatWorkspace projectId={projectId} root={project.rootPath} />;
}
