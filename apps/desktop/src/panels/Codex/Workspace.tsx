/**
 * Chat workspace of the Codex panel (same layout as the Claude Code panel):
 *
 * ```
 * ┌ Chats ──────┐┌ title · state ··········· 5 h ▮▮ 14 % | Week ▮▮ 23 % | Context  [Commands] ┐
 * │ New chat    ││ messages, tool cards, approval cards                                        │
 * │ search      ││                                                                             │
 * │ Open / days ││ composer (@ files, / commands)                                              │
 * │ Limits card ││ model · reasoning effort · approval mode                                    │
 * └─────────────┘└─────────────────────────────────────────────────────────────────────────────┘
 * ```
 *
 * Slash commands (`/new`, `/resume`, `/fork`, `/compact`, `/model`,
 * `/status`) are handled by Orkestr and never sent to the model.
 */
import type { CodexApprovalMode, CodexModelOption } from "@orkestr/protocol";
import { Badge, Button, IconButton } from "@orkestr/ui";
import clsx from "clsx";
import { Bot, Command, FolderOpen, GitFork, MessageSquarePlus, MessagesSquare } from "lucide-react";
import { type KeyboardEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { useEditorSelection } from "../../features/claudeCode/bridge";
import { codexSlashCommands, CODEX_SLASH, parseCodexSlash } from "../../features/codex/commands";
import { maxLimitPercent } from "../../features/codex/model";
import { isEnded, isRunning, useCodexStore, type CodexSessionView } from "../../features/codex/store";
import { useFormat, useT } from "../../i18n";
import { Composer } from "../ClaudeCode/composer";
import { Popover } from "../ClaudeCode/v2/Popover";
import { LimitsCard, LimitsMeter, useLimitsPolling } from "./Limits";
import { CodexSidebar } from "./Sidebar";
import { Transcript } from "./Transcript";

const MODES: readonly CodexApprovalMode[] = ["readOnly", "ask", "auto"];

/** Below this width the chat list slides over the chat. */
const NARROW_PX = 900;

function useNarrow(ref: React.RefObject<HTMLDivElement | null>): boolean {
  const [narrow, setNarrow] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(([entry]) => {
      if (entry) setNarrow(entry.contentRect.width > 0 && entry.contentRect.width < NARROW_PX);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return narrow;
}

/** The model a session / the preferences use (null = the CLI's default). */
function currentModel(models: CodexModelOption[], id: string): CodexModelOption | null {
  if (id) return models.find((m) => m.id === id) ?? null;
  return models.find((m) => m.isDefault) ?? null;
}

export interface OptionPickersProps {
  model: string;
  effort: string;
  mode: CodexApprovalMode;
  models: CodexModelOption[];
  disabled?: boolean;
  onChange: (patch: { model?: string; effort?: string; mode?: CodexApprovalMode }) => void;
}

/** Model (display names from `codex.models`), reasoning effort (per model) and approval mode. */
export function OptionPickers({ model, effort, mode, models, disabled, onChange }: OptionPickersProps) {
  const { t } = useT("codex");
  const cur = currentModel(models, model);
  const efforts = cur?.efforts ?? [];
  const def = models.find((m) => m.isDefault);
  const effortLabel = (e: string): string => ((KNOWN_EFFORTS as readonly string[]).includes(e) ? t(`options.efforts.${e as KnownEffort}`) : e);
  return (
    <>
      <label className="cx-pick" title={cur?.description || t("options.modelHint")}>
        <span>{t("options.model")}</span>
        <select value={model} disabled={disabled} aria-label={t("options.model")} onChange={(e) => onChange({ model: e.target.value, effort: "" })} data-testid="codex-model">
          <option value="">{def ? t("options.defaultModelNamed", { name: def.displayName }) : t("options.defaultModel")}</option>
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.displayName}
            </option>
          ))}
          {model && !models.some((m) => m.id === model) && <option value={model}>{model}</option>}
        </select>
      </label>
      <label className="cx-pick" title={t("options.effortHint")}>
        <span>{t("options.effort")}</span>
        <select value={effort} disabled={disabled || efforts.length === 0} aria-label={t("options.effort")} onChange={(e) => onChange({ effort: e.target.value })} data-testid="codex-effort">
          <option value="">{cur?.defaultEffort ? t("options.defaultEffortNamed", { effort: effortLabel(cur.defaultEffort) }) : t("options.defaultEffort")}</option>
          {efforts.map((e) => (
            <option key={e} value={e}>
              {effortLabel(e)}
            </option>
          ))}
        </select>
      </label>
      <label className="cx-pick" title={t(`options.modeHint.${mode}`)}>
        <span>{t("options.mode")}</span>
        <select value={mode} disabled={disabled} aria-label={t("options.mode")} onChange={(e) => onChange({ mode: e.target.value as CodexApprovalMode })} data-testid="codex-mode">
          {MODES.map((m) => (
            <option key={m} value={m} title={t(`options.modeHint.${m}`)}>
              {t(`options.modes.${m}`)}
            </option>
          ))}
        </select>
      </label>
    </>
  );
}

const KNOWN_EFFORTS = ["minimal", "low", "medium", "high", "xhigh"] as const;
type KnownEffort = (typeof KNOWN_EFFORTS)[number];

function CommandsMenu({ onRun, canRun, onClose }: { onRun: (cmd: string) => void; canRun: boolean; onClose: () => void }) {
  const { t } = useT("codex");
  return (
    <Popover open onClose={onClose} label={t("commandsMenu.title")} align="end" className="cx-cmds">
      <div className="cx-cmds__list" role="menu" aria-label={t("commandsMenu.title")} data-testid="codex-commands">
        {CODEX_SLASH.map((name) => {
          const needsChat = name === "fork" || name === "compact";
          return (
            <button key={name} type="button" role="menuitem" className="cx-cmds__item" disabled={needsChat && !canRun} onClick={() => onRun(`/${name}`)}>
              <code>/{name}</code>
              <span>{t(`slash.${name}`)}</span>
            </button>
          );
        })}
      </div>
    </Popover>
  );
}

function EmptyChat() {
  const { t } = useT("codex");
  return (
    <div className="cc2-empty">
      <div className="cc2-empty__icon cx-avatar" aria-hidden="true">
        <Bot size={26} />
      </div>
      <h2 className="cc2-empty__title">{t("empty.title")}</h2>
      <p className="cc2-empty__desc">{t("empty.description")}</p>
    </div>
  );
}

export function CodexWorkspace({ projectId, root }: { projectId: string; root: string }) {
  const { t } = useT("codex");
  const fmt = useFormat();
  const order = useCodexStore((s) => s.order[projectId]);
  const activeId = useCodexStore((s) => s.active[projectId] ?? null);
  const view: CodexSessionView | undefined = useCodexStore((s) => (activeId ? s.sessions[activeId] : undefined));
  const prefs = useCodexStore((s) => s.prefs);
  const models = useCodexStore((s) => s.models);
  const limits = useCodexStore((s) => s.limits);
  const storeError = useCodexStore((s) => s.error);
  const insert = useCodexStore((s) => s.insert);
  const { start, send, interrupt, compact, respond, setOptions, setPrefs, dequeue, consumeInsert, loadThreads, loadModels, loadLimits, note } =
    useCodexStore.getState();
  const selection = useEditorSelection();
  const [drawer, setDrawer] = useState(false);
  const [limitsOpen, setLimitsOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const narrow = useNarrow(rootRef);
  const [overlay, setOverlay] = useState(false);
  const sidebar = narrow ? overlay : prefs.sidebar;
  const toggleSidebar = (open: boolean) => (narrow ? setOverlay(open) : setPrefs({ sidebar: open }));
  const slash = useMemo(() => codexSlashCommands(), []);

  useLimitsPolling();
  useEffect(() => {
    void loadThreads(projectId);
    if (useCodexStore.getState().models.length === 0) void loadModels();
  }, [projectId, loadThreads, loadModels]);
  useEffect(() => setOverlay(false), [activeId]);

  const onNew = useCallback(() => {
    setOverlay(false);
    void start(projectId);
  }, [projectId, start]);

  const ended = isEnded(view);
  const running = isRunning(view);
  const sessionId = view?.info.sessionId ?? null;
  const threadId = view ? (view.chat.threadId ?? view.info.threadId) : null;
  const title = view?.info.title ?? t("sidebar.untitled");
  const model = view ? (view.info.model ?? prefs.model) : prefs.model;
  const effort = view ? (view.info.effort ?? prefs.effort) : prefs.effort;
  const mode = view ? view.info.mode : prefs.mode;

  const statusText = (): string => {
    const lim = useCodexStore.getState().limits;
    const parts: string[] = [];
    parts.push(t("status.model", { model: (view?.chat.model ?? model) || t("options.defaultModel") }));
    if (lim?.available) {
      if (lim.primary) parts.push(t("status.primary", { percent: fmt.percent(lim.primary.percent / 100) }));
      if (lim.secondary) parts.push(t("status.secondary", { percent: fmt.percent(lim.secondary.percent / 100) }));
    } else {
      parts.push(t("status.limitsUnknown"));
    }
    const tk = view?.chat.tokens;
    if (tk) parts.push(t("status.tokens", { total: fmt.integer(tk.total.inputTokens + tk.total.outputTokens) }));
    return parts.join(" · ");
  };

  /** Runs a slash command; true when the text was one. */
  const runSlash = async (text: string): Promise<boolean> => {
    const cmd = parseCodexSlash(text);
    if (!cmd) return false;
    switch (cmd.command) {
      case "new":
        onNew();
        break;
      case "resume":
        if (cmd.arg) await start(projectId, { threadId: cmd.arg });
        else toggleSidebar(true);
        break;
      case "fork":
        if (threadId) await start(projectId, { threadId, fork: true });
        else if (sessionId) note(sessionId, t("slash.noThread"));
        break;
      case "compact":
        if (sessionId) await compact(sessionId);
        break;
      case "model": {
        if (!cmd.arg) {
          if (sessionId) note(sessionId, t("slash.modelList", { models: models.map((m) => `${m.displayName} (${m.id})`).join(", ") || "—" }));
          break;
        }
        const q = cmd.arg.toLowerCase();
        const found = models.find((m) => m.id.toLowerCase() === q || m.displayName.toLowerCase() === q);
        if (found) {
          await setOptions(sessionId, { model: found.id, effort: "" });
          if (sessionId) note(sessionId, t("slash.modelSet", { model: found.displayName }));
        } else if (sessionId) {
          note(sessionId, t("slash.modelUnknown", { model: cmd.arg }));
        }
        break;
      }
      case "status":
        await loadLimits(true);
        if (sessionId) note(sessionId, statusText());
        setLimitsOpen(true);
        break;
    }
    return true;
  };

  const onSend = async (text: string): Promise<boolean> => {
    if (await runSlash(text)) return true;
    if (!sessionId || ended) return false;
    return send(sessionId, text);
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

  const limitPct = maxLimitPercent(limits);
  const usageBanner = view?.chat.usageLimit || (limits?.usageAllowed === false && limits.available);

  return (
    <div ref={rootRef} className={clsx("cc2 cx", narrow && "cc2--narrow")} onKeyDown={onKeyDown} data-testid="codex-workspace">
      {sidebar && narrow && <button type="button" className="cc2-scrim" aria-label={t("sidebar.toggle")} tabIndex={-1} onClick={() => setOverlay(false)} />}
      {sidebar && <CodexSidebar projectId={projectId} activeSessionId={activeId} onNew={onNew} onCollapse={() => toggleSidebar(false)} />}
      <section className="cc2-main" aria-label={t("chat.label")}>
        <header className="cc2-top">
          {!sidebar && (
            <button type="button" className="cc2-chats-btn" title={t("sidebar.toggle")} onClick={() => toggleSidebar(true)}>
              <MessagesSquare size={14} aria-hidden />
              {t("sidebar.title")}
            </button>
          )}
          {!sidebar && <IconButton label={t("sidebar.newChat")} icon={<MessageSquarePlus size={15} />} onClick={onNew} tooltip="bottom" />}
          <div className="cc2-top__title">
            <h1 title={title}>{view ? title : t("welcome.title")}</h1>
            {view && (
              <span className="cc2-top__cwd" title={view.info.cwd}>
                <FolderOpen size={11} aria-hidden /> {view.info.cwd}
              </span>
            )}
          </div>
          {view && (
            <Badge
              tone={view.info.state === "failed" ? "danger" : view.info.state === "waitingApproval" ? "warning" : running ? "accent" : "neutral"}
              data-testid="codex-state"
            >
              {t(`state.${view.info.state}`)}
            </Badge>
          )}
          <div className="cc2-top__spacer" />
          <div className="cc2-anchor">
            <LimitsMeter chat={view?.chat ?? null} />
            <Popover open={limitsOpen} onClose={() => setLimitsOpen(false)} label={t("limits.label")} align="end" className="cc2-pop--usage">
              <LimitsCard chat={view?.chat ?? null} />
            </Popover>
          </div>
          {threadId && !ended && (
            <IconButton label={t("actions.fork")} icon={<GitFork size={14} />} onClick={() => void start(projectId, { threadId, fork: true })} tooltip="bottom" />
          )}
          <div className="cc2-anchor">
            <Button size="sm" leftIcon={<Command size={13} />} onClick={() => setDrawer((v) => !v)} aria-expanded={drawer} title={t("commandsMenu.hint")} data-testid="codex-commands-button">
              {t("commandsMenu.open")}
            </Button>
            {drawer && (
              <CommandsMenu
                canRun={view !== undefined && !ended}
                onClose={() => setDrawer(false)}
                onRun={(cmd) => {
                  setDrawer(false);
                  void runSlash(cmd);
                }}
              />
            )}
          </div>
        </header>

        {storeError && <div className="cc-error-text">{storeError}</div>}
        {usageBanner && (
          <div className="cx-banner" role="alert" data-testid="codex-usage-banner">
            {limitPct !== null ? t("errors.usageLimitPct", { percent: fmt.percent(limitPct / 100) }) : t("errors.usageLimit")}
            <Button size="sm" onClick={() => setLimitsOpen(true)}>
              {t("limits.show")}
            </Button>
          </div>
        )}

        {view ? (
          <>
            <Transcript
              chat={view.chat}
              root={root}
              cwd={view.info.cwd}
              onRespond={(rid, decision) => void respond(view.info.sessionId, rid, decision)}
              onSignIn={() => void useCodexStore.getState().startLogin("browser")}
              onShowLimits={() => setLimitsOpen(true)}
              empty={<EmptyChat />}
            />
            {view.error && <div className="cc-error-text">{view.error}</div>}
            {view.info.state === "failed" && view.info.error && <div className="cc-error-text">{view.info.error}</div>}
            {ended ? (
              <div className="cc-ended">
                <span className="cc-muted">{t("chat.ended")}</span>
                {threadId && (
                  <Button size="sm" onClick={() => void start(projectId, { threadId })}>
                    {t("actions.resume")}
                  </Button>
                )}
                <Button size="sm" variant="primary" onClick={onNew}>
                  {t("actions.newChat")}
                </Button>
              </div>
            ) : (
              <Composer
                projectId={projectId}
                disabled={view.chat.authRequired}
                disabledReason={view.chat.authRequired ? t("errors.authRequired") : null}
                running={running}
                queue={view.queue}
                commands={slash}
                builtinSlashCommands={false}
                allowImages={false}
                selection={selection}
                insert={insert?.text ?? null}
                onInsertConsumed={() => consumeInsert()}
                onSend={(p) => onSend(p.text)}
                onInterrupt={() => void interrupt(view.info.sessionId)}
                onDequeue={(i) => dequeue(view.info.sessionId, i)}
                texts={{
                  label: t("composer.label"),
                  placeholder: t("composer.placeholder"),
                  placeholderBusy: t("composer.placeholderBusy"),
                  queueFull: t("composer.queueFull"),
                }}
                toolbar={
                  <OptionPickers
                    model={model}
                    effort={effort}
                    mode={mode}
                    models={models}
                    onChange={(patch) => void setOptions(view.info.sessionId, patch)}
                  />
                }
              />
            )}
          </>
        ) : (
          <div className="cc2-welcome">
            <EmptyChat />
            <div className="cc2-welcome__actions">
              <Button variant="primary" leftIcon={<MessageSquarePlus size={14} />} onClick={onNew} data-testid="codex-start">
                {t("actions.newChat")}
              </Button>
            </div>
            <div className="cc2-welcome__actions cx-welcome__options">
              <OptionPickers model={prefs.model} effort={prefs.effort} mode={prefs.mode} models={models} onChange={(patch) => void setOptions(null, patch)} />
            </div>
            {(order?.length ?? 0) === 0 && <p className="cx-setup__muted">{t("welcome.hint")}</p>}
          </div>
        )}
      </section>
    </div>
  );
}
