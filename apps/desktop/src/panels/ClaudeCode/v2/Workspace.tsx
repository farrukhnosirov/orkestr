/**
 * Chat workspace of the Claude Code panel (v2 layout):
 *
 * ```
 * ┌ Чаты ───────┐┌ title · state ············ Сеанс ▮▮ 14 % | Неделя ▮▮ 23 %  [Команды] ┐
 * │ search      ││ Возможности: План · Правки · Размышления · Сжать · Контекст · MCP …   │
 * │ Сегодня     ││ messages                                                             │
 * │  chat …     ││                                                                      │
 * │ Вчера …     ││ composer  (images, @ files, / commands)                              │
 * │ Лимиты card ││ mode · model · fallback · shortcuts                                  │
 * └─────────────┘└──────────────────────────────────────────────────────────────────────┘
 * ```
 */
import { Badge, Button, IconButton } from "@orkestr/ui";
import clsx from "clsx";
import { Command, FolderOpen, Keyboard, MessageSquarePlus, PanelLeftOpen, Sparkle } from "lucide-react";
import { type ComponentProps, type KeyboardEvent, useCallback, useEffect, useState } from "react";

import { workspaceActions } from "../../../components/workspaceActions";
import { useEditorSelection } from "../../../features/claudeCode/bridge";
import type { NativeAction } from "../../../features/claudeCode/catalog";
import { chatMarkdown, exportFileName } from "../../../features/claudeCode/chatTools";
import { useClaudeStore } from "../../../features/claudeCode/store";
import { downloadText } from "../../../features/theme/i18nText";
import { useT } from "../../../i18n";
import { useTerminalUi } from "../../Terminal/store";
import { ChatView } from "../ChatView";
import { Composer, ModelPicker } from "../composer";
import { ModeSwitcher } from "../permissions";
import { ChatSidebar } from "./ChatSidebar";
import { CommandsDrawer } from "./CommandsDrawer";
import { FeatureBar } from "./FeatureBar";
import { UsageMeter, useUsagePolling } from "./Usage";
import "./v2.css";

/** The composer, re-rendered (alone) when the editor selection changes. */
function LiveComposer(props: Omit<ComponentProps<typeof Composer>, "selection">) {
  const selection = useEditorSelection();
  return <Composer {...props} selection={selection} />;
}

function EmptyChat({ onPick }: { onPick: (text: string) => void }) {
  const { t } = useT("claudeCodePanel");
  const suggestions = [t("empty.s1"), t("empty.s2"), t("empty.s3"), t("empty.s4")];
  return (
    <div className="cc2-empty">
      <div className="cc2-empty__icon" aria-hidden="true">
        <Sparkle size={26} />
      </div>
      <h2 className="cc2-empty__title">{t("empty.title")}</h2>
      <p className="cc2-empty__desc">{t("empty.description")}</p>
      <div className="cc2-empty__chips" role="group" aria-label={t("empty.suggestions")}>
        {suggestions.map((s) => (
          <button key={s} type="button" className="cc2-suggest" onClick={() => onPick(s)}>
            {s}
          </button>
        ))}
      </div>
    </div>
  );
}

function Shortcuts() {
  const { t } = useT("claudeCodePanel");
  const keys = [t("shortcuts.send"), t("shortcuts.newline"), t("shortcuts.interrupt"), t("shortcuts.mention"), t("shortcuts.command"), t("shortcuts.palette"), t("shortcuts.newChat")];
  return (
    <span className="cc2-keys" title={keys.join("\n")} aria-label={t("shortcuts.title")} role="note">
      <Keyboard size={12} aria-hidden />
      {t("shortcuts.palette")}
    </span>
  );
}

function FallbackPicker({ value, models, onChange }: { value: string; models: { value: string; displayName: string }[]; onChange: (v: string) => void }) {
  const { t } = useT("claudeCodePanel");
  return (
    <label className="cc2-fallback" title={t("features.fallbackHint")}>
      <span>{t("features.fallback")}</span>
      <select value={value} aria-label={t("features.fallback")} onChange={(e) => onChange(e.target.value)}>
        <option value="">{t("features.fallbackNone")}</option>
        {models
          .filter((m) => m.value !== "default")
          .map((m) => (
            <option key={m.value} value={m.value}>
              {m.displayName}
            </option>
          ))}
      </select>
    </label>
  );
}

export function ChatWorkspace({ projectId, root }: { projectId: string; root: string }) {
  const { t } = useT("claudeCode");
  const { t: tp } = useT("claudeCodePanel");
  const tabs = useClaudeStore((s) => s.tabs[projectId]);
  const activeId = useClaudeStore((s) => s.active[projectId] ?? null);
  const view = useClaudeStore((s) => (activeId ? s.sessions[activeId] : undefined));
  const prefs = useClaudeStore((s) => s.prefs);
  const insert = useClaudeStore((s) => s.insert);
  const storeError = useClaudeStore((s) => s.error);
  const meta = useClaudeStore((s) => s.meta);
  const { start, send, interrupt, respond, setMode, setModel, setPrefs, dequeue, consumeInsert, login, loadHistory, requestInsert } =
    useClaudeStore.getState();
  const [drawer, setDrawer] = useState(false);
  const sidebar = prefs.sidebar;
  const tabCount = tabs?.length ?? 0;

  useUsagePolling(activeId);
  useEffect(() => {
    void loadHistory(projectId);
  }, [projectId, loadHistory]);

  const onNew = useCallback(() => {
    void start(projectId);
  }, [projectId, start]);

  const onLogin = async () => {
    const id = await login(projectId);
    if (id) {
      useTerminalUi.getState().setActive(projectId, id);
      workspaceActions.openPanel("terminal");
    }
  };

  // "Ask Claude about selection" with no chat open starts one.
  useEffect(() => {
    if (insert && tabCount === 0 && !useClaudeStore.getState().busy) onNew();
  }, [insert, tabCount, onNew]);

  const ended = view ? view.info.state === "exited" || view.info.state === "failed" : true;
  const claudeId = view?.chat.claudeSessionId ?? view?.info.claudeSessionId ?? null;
  const title = (claudeId ? meta[claudeId]?.title : null) ?? view?.info.title ?? tp("sidebar.untitled");

  const sendText = (text: string) => {
    if (view && !ended) void send(view.info.sessionId, text);
  };

  const onNative = (action: NativeAction) => {
    if (!view) {
      if (action === "login") void onLogin();
      return;
    }
    switch (action) {
      case "usage":
        void useClaudeStore.getState().loadUsage(view.info.sessionId, true);
        sendText("/usage");
        break;
      case "export": {
        const md = chatMarkdown(view.chat, title, { user: tp("features.export_labels.user"), claude: tp("features.export_labels.claude"), tool: tp("features.export_labels.tool") });
        downloadText(exportFileName(title, new Date()), md);
        break;
      }
      case "login":
        void onLogin();
        break;
      case "rename":
      case "context":
      case "mcp":
      case "addDir":
      case "model":
        // These live in the «Возможности» bar / sidebar; the command itself also works.
        sendText(`/${action === "addDir" ? "add-dir" : action}`);
        break;
    }
    setDrawer(false);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.ctrlKey && !e.altKey && !e.shiftKey && (e.key === "/" || e.code === "Slash")) {
      e.preventDefault();
      setDrawer((v) => !v);
    } else if (e.ctrlKey && e.altKey && !e.shiftKey && e.code === "KeyN") {
      e.preventDefault();
      onNew();
    }
  };

  return (
    <div className={clsx("cc2", !sidebar && "cc2--nosidebar")} onKeyDown={onKeyDown}>
      {sidebar && (
        <ChatSidebar projectId={projectId} activeSessionId={activeId} onNew={onNew} onCollapse={() => setPrefs({ sidebar: false })} />
      )}
      <section className="cc2-main" aria-label={t("chat.label")}>
        <header className="cc2-top">
          {!sidebar && (
            <IconButton label={tp("sidebar.toggle")} icon={<PanelLeftOpen size={15} />} onClick={() => setPrefs({ sidebar: true })} tooltip="bottom" />
          )}
          {!sidebar && (
            <IconButton label={tp("sidebar.newChat")} icon={<MessageSquarePlus size={15} />} onClick={onNew} tooltip="bottom" />
          )}
          <div className="cc2-top__title">
            <h1 title={title}>{view ? title : t("welcome.title")}</h1>
            {view && (
              <span className="cc2-top__cwd" title={view.info.cwd}>
                <FolderOpen size={11} aria-hidden /> {view.info.cwd}
              </span>
            )}
          </div>
          {view && (
            <Badge tone={view.info.state === "failed" ? "danger" : view.info.state === "waitingPermission" ? "warning" : view.chat.running ? "accent" : "neutral"}>
              {t(`state.${view.info.state}`)}
            </Badge>
          )}
          <div className="cc2-top__spacer" />
          <UsageMeter sessionId={activeId} chat={view?.chat ?? null} />
          <Button size="sm" leftIcon={<Command size={13} />} onClick={() => setDrawer((v) => !v)} aria-expanded={drawer} title={tp("commands.openHint")}>
            {tp("commands.open")}
          </Button>
        </header>

        {view && !ended && <FeatureBar projectId={projectId} view={view} title={title} onSend={sendText} />}

        {storeError && <div className="cc-error-text">{storeError}</div>}

        {view ? (
          <>
            <ChatView
              chat={view.chat}
              root={root}
              cwd={view.info.cwd}
              onRespond={(rid, decision, message) => void respond(view.info.sessionId, rid, decision, message)}
              onLogin={() => void onLogin()}
              onEdit={ended ? undefined : (text) => requestInsert(text, true)}
              empty={<EmptyChat onPick={(text) => requestInsert(text, true)} />}
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
                insertReplace={insert?.replace ?? false}
                onInsertConsumed={() => consumeInsert()}
                onSend={(p) => send(view.info.sessionId, p.text, p.images?.map((i) => ({ mediaType: i.mediaType, data: i.data })))}
                onInterrupt={() => void interrupt(view.info.sessionId)}
                onDequeue={(i) => dequeue(view.info.sessionId, i)}
                toolbar={
                  <>
                    <ModeSwitcher
                      mode={view.info.permissionMode}
                      disabled={ended}
                      onChange={(m, confirmBypass) => void setMode(view.info.sessionId, m, confirmBypass)}
                    />
                    <ModelPicker
                      value={view.info.model ?? prefs.model}
                      models={view.chat.models}
                      disabled={ended}
                      onChange={(m) => void setModel(view.info.sessionId, m)}
                    />
                    <FallbackPicker value={prefs.fallbackModel} models={view.chat.models} onChange={(v) => setPrefs({ fallbackModel: v })} />
                    <Shortcuts />
                  </>
                }
              />
            )}
          </>
        ) : (
          <div className="cc2-welcome">
            <EmptyChat onPick={(text) => {
              requestInsert(text, true);
            }} />
            <div className="cc2-welcome__actions">
              <Button variant="primary" leftIcon={<MessageSquarePlus size={14} />} onClick={onNew}>
                {t("actions.newChat")}
              </Button>
              <ModelPicker value={prefs.model} models={[]} onChange={(m) => setPrefs({ model: m })} />
              <ModeSwitcher mode={prefs.mode} onChange={(m) => setPrefs({ mode: m })} />
            </div>
          </div>
        )}
        {drawer && (
          <CommandsDrawer
            commands={view?.chat.commands ?? []}
            canRun={view !== undefined && !ended}
            onRun={(text) => {
              sendText(text);
              setDrawer(false);
            }}
            onNative={onNative}
            onClose={() => setDrawer(false)}
          />
        )}
      </section>
    </div>
  );
}
