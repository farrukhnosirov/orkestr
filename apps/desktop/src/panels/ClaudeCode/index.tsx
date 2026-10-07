/**
 * Claude Code panel: Orkestr's own chat UI over the official `claude` CLI
 * (`claudeCode.*`). Screens: no project → Claude Code not found (pin a
 * path) → not signed in («Войти в Claude» opens a terminal running
 * `claude auth login`) → chat: session tabs, history / resume, model and
 * mode, the message list and the composer.
 */
import type { ClaudeHistoryEntry } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Spinner } from "@orkestr/ui";
import clsx from "clsx";
import { FolderOpen, History, LogIn, MessageSquarePlus, RefreshCw, Sparkle, X } from "lucide-react";
import { type ComponentProps, type FormEvent, useCallback, useEffect, useState } from "react";

import { workspaceActions } from "../../components/workspaceActions";
import { useFormat, useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { useClaudeStore } from "../../features/claudeCode/store";
import { useEditorSelection } from "../../features/claudeCode/bridge";
import { useTerminalUi } from "../Terminal/store";
import type { PanelProps } from "../types";
import { ChatView } from "./ChatView";
import { Composer, ModelPicker } from "./composer";
import { ModeSwitcher } from "./permissions";
import "./claudeCode.css";

const EMPTY: string[] = [];

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

function HistoryList({ projectId, onResume }: { projectId: string; onResume: (h: ClaudeHistoryEntry) => void }) {
  const { t } = useT("claudeCode");
  const fmt = useFormat();
  const history = useClaudeStore((s) => s.history[projectId]);
  const loading = useClaudeStore((s) => s.historyLoading);
  const loadHistory = useClaudeStore((s) => s.loadHistory);
  useEffect(() => {
    void loadHistory(projectId);
  }, [projectId, loadHistory]);
  if (loading && !history) return <Spinner size={16} label={t("history.loading")} />;
  if (!history || history.length === 0) return <div className="cc-muted">{t("history.empty")}</div>;
  return (
    <ul className="cc-history" aria-label={t("history.title")}>
      {history.map((h) => (
        <li key={h.claudeSessionId}>
          <button type="button" className="cc-history__item" onClick={() => onResume(h)} title={t("history.resume")}>
            <span className="cc-history__title">{h.title ?? h.firstPrompt ?? h.claudeSessionId}</span>
            <span className="cc-history__meta">
              {fmt.relative(h.updatedAt)} · {t("history.messages", { count: h.messages })}
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}

function Welcome({ projectId, onNew, onResume }: { projectId: string; onNew: () => void; onResume: (h: ClaudeHistoryEntry) => void }) {
  const { t } = useT("claudeCode");
  const busy = useClaudeStore((s) => s.busy);
  return (
    <div className="cc-welcome">
      <div className="cc-welcome__hero">
        <Sparkle size={28} className="cc-accent" />
        <h2>{t("welcome.title")}</h2>
        <p className="cc-muted">{t("welcome.description")}</p>
        <Button variant="primary" leftIcon={<MessageSquarePlus size={14} />} loading={busy} onClick={onNew}>
          {t("actions.newChat")}
        </Button>
      </div>
      <h3 className="cc-section-title">{t("history.title")}</h3>
      <HistoryList projectId={projectId} onResume={onResume} />
    </div>
  );
}

function StatusLine({ sessionId }: { sessionId: string | null }) {
  const { t } = useT("claudeCode");
  const fmt = useFormat();
  const status = useClaudeStore((s) => s.status);
  const view = useClaudeStore((s) => (sessionId ? s.sessions[sessionId] : undefined));
  const auth = status?.auth;
  const rate = view?.chat.rateLimit;
  return (
    <div className="cc-statusline">
      {status?.binary && (
        <span title={status.binary.path}>
          {t("status.binary", { version: status.binary.version ?? "?" })} · {t(`status.source.${status.binary.source}`)}
        </span>
      )}
      {auth && (
        <span>
          {auth.loggedIn ? t("status.signedIn", { account: auth.email ?? auth.subscriptionType ?? auth.authMethod ?? "" }) : t("status.signedOut")}
        </span>
      )}
      {view && (
        <span className="cc-statusline__cwd" title={view.info.cwd}>
          <FolderOpen size={12} /> {view.info.cwd}
        </span>
      )}
      {rate?.utilization != null && <span>{t("status.limit", { used: fmt.percent(rate.utilization) })}</span>}
      {view?.info.state && <Badge tone={view.info.state === "failed" ? "danger" : view.info.state === "waitingPermission" ? "warning" : "neutral"}>{t(`state.${view.info.state}`)}</Badge>}
    </div>
  );
}

/** The composer, re-rendered (alone) when the editor selection changes. */
function LiveComposer(props: Omit<ComponentProps<typeof Composer>, "selection">) {
  const selection = useEditorSelection();
  return <Composer {...props} selection={selection} />;
}

function ChatWorkspace({ projectId, root }: { projectId: string; root: string }) {
  const { t } = useT("claudeCode");
  const tabs = useClaudeStore((s) => s.tabs[projectId] ?? EMPTY);
  const activeId = useClaudeStore((s) => s.active[projectId] ?? null);
  const sessions = useClaudeStore((s) => s.sessions);
  const prefs = useClaudeStore((s) => s.prefs);
  const insert = useClaudeStore((s) => s.insert);
  const storeError = useClaudeStore((s) => s.error);
  const { start, send, interrupt, respond, close, activate, setMode, setModel, setPrefs, dequeue, consumeInsert, login } = useClaudeStore.getState();
  const [showHistory, setShowHistory] = useState(false);
  const view = activeId ? sessions[activeId] : undefined;

  const onNew = useCallback(() => {
    setShowHistory(false);
    void start(projectId);
  }, [projectId, start]);
  const onResume = (h: ClaudeHistoryEntry) => {
    setShowHistory(false);
    void start(projectId, { resume: h.claudeSessionId });
  };
  const onLogin = async () => {
    const id = await login(projectId);
    if (id) {
      useTerminalUi.getState().setActive(projectId, id);
      workspaceActions.openPanel("terminal");
    }
  };

  // "Ask Claude about selection" with no chat open starts one.
  useEffect(() => {
    if (insert && tabs.length === 0 && !useClaudeStore.getState().busy) onNew();
  }, [insert, tabs.length, onNew]);

  const ended = view ? view.info.state === "exited" || view.info.state === "failed" : true;

  return (
    <div className="cc-panel">
      <div className="cc-header">
        <div className="cc-tabs" role="tablist" aria-label={t("tabs.label")}>
          {tabs.map((id) => {
            const s = sessions[id];
            const title = s?.info.title ?? t("tabs.newChat");
            return (
              <div key={id} className={clsx("cc-tab", id === activeId && "cc-tab--active")}>
                <button type="button" role="tab" aria-selected={id === activeId} className="cc-tab__label" title={title} onClick={() => activate(projectId, id)}>
                  {s?.chat.running && <Spinner size={10} />}
                  {title}
                </button>
                <button type="button" className="cc-tab__close" aria-label={t("tabs.close", { title })} onClick={() => void close(projectId, id)}>
                  <X size={12} />
                </button>
              </div>
            );
          })}
        </div>
        <IconButton label={t("actions.newChat")} icon={<MessageSquarePlus size={15} />} onClick={onNew} tooltip="bottom" />
        <IconButton label={t("history.title")} icon={<History size={15} />} active={showHistory} onClick={() => setShowHistory((v) => !v)} tooltip="bottom" />
        <ModelPicker
          value={view?.info.model ?? prefs.model}
          models={view?.chat.models ?? []}
          disabled={view ? ended : false}
          onChange={(m) => (view && !ended ? void setModel(view.info.sessionId, m) : setPrefs({ model: m }))}
        />
        <ModeSwitcher
          mode={view?.info.permissionMode ?? prefs.mode}
          disabled={view ? ended : false}
          onChange={(m, confirmBypass) => (view && !ended ? void setMode(view.info.sessionId, m, confirmBypass) : setPrefs({ mode: m }))}
        />
      </div>
      {showHistory && (
        <div className="cc-history-pop">
          <HistoryList projectId={projectId} onResume={onResume} />
        </div>
      )}
      {storeError && <div className="cc-error-text">{storeError}</div>}
      {view ? (
        <>
          <ChatView
            chat={view.chat}
            root={root}
            cwd={view.info.cwd}
            onRespond={(rid, decision, message) => void respond(view.info.sessionId, rid, decision, message)}
            onLogin={() => void onLogin()}
            empty={<div className="cc-empty-chat cc-muted">{t("chat.empty")}</div>}
          />
          {view.error && <div className="cc-error-text">{view.error}</div>}
          {view.info.state === "failed" && view.info.error && <div className="cc-error-text">{view.info.error}</div>}
          {ended ? (
            <div className="cc-ended">
              <span className="cc-muted">{t("chat.ended")}</span>
              {view.chat.claudeSessionId && (
                <Button size="sm" onClick={() => void start(projectId, { resume: view.chat.claudeSessionId ?? undefined })}>
                  {t("actions.resume")}
                </Button>
              )}
              <Button size="sm" variant="primary" onClick={onNew}>
                {t("actions.newChat")}
              </Button>
            </div>
          ) : (
            <LiveComposer
              projectId={projectId}
              disabled={ended || view.chat.authRequired}
              disabledReason={ended ? "ended" : view.chat.authRequired ? "notSignedIn" : null}
              running={view.chat.running}
              queue={view.queue}
              commands={view.chat.commands}
              insert={insert?.text ?? null}
              onInsertConsumed={() => consumeInsert()}
              onSend={(p) => send(view.info.sessionId, p.text)}
              onInterrupt={() => void interrupt(view.info.sessionId)}
              onDequeue={(i) => dequeue(view.info.sessionId, i)}
            />
          )}
        </>
      ) : (
        <Welcome projectId={projectId} onNew={onNew} onResume={onResume} />
      )}
      <StatusLine sessionId={activeId} />
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
