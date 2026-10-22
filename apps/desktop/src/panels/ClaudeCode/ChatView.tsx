/**
 * Message list of one chat session: user messages, streaming Markdown
 * answers (safe renderer: no HTML, no remote images), tool cards, inline
 * permission cards, notices, errors and the per-turn result footer (tokens,
 * duration, changed files). Sticks to the bottom while new items arrive
 * unless the user scrolled up.
 */
import type { ClaudePermissionDecision, ClaudeRisk } from "@orkestr/protocol";
import { Button, Spinner } from "@orkestr/ui";
import clsx from "clsx";
import { AlertTriangle, Check, CircleStop, Copy, FileDiff, LogIn, Pencil, RotateCcw, Sparkle, Terminal as TerminalIcon, User } from "lucide-react";
import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";

import { useFormat, useT } from "../../i18n";
import { SafeMarkdown } from "../../features/extensions/markdown";
import { openChangedFile } from "../../features/claudeCode/bridge";
import { inputTokens, relativeTo, type ChatItem, type ChatState, type ResultItem } from "../../features/claudeCode/model";
import { PermissionCard } from "./permissions";
import { ToolCard } from "./tools";

export interface ChatViewProps {
  chat: ChatState;
  root: string | null;
  cwd: string | null;
  onRespond: (requestId: string, decision: ClaudePermissionDecision, message?: string) => void;
  onLogin: () => void;
  empty?: ReactNode;
  /** Puts a sent message back into the composer (edit & resend). */
  onEdit?: (text: string) => void;
  /** Rewinds the chat to before a user message (`/rewind`); `restoreFiles` also restores Claude's file changes. */
  onRewind?: (itemId: string, restoreFiles: boolean) => void;
}

function RewindBar({ onRewind, onCancel }: { onRewind: (restoreFiles: boolean) => void; onCancel: () => void }) {
  const { t } = useT("claudeCodePanel");
  return (
    <div className="cc-msg__rewind" role="group" aria-label={t("message.rewindTitle")}>
      <Button size="sm" variant="secondary" onClick={() => onRewind(false)}>
        {t("message.rewindChat")}
      </Button>
      <Button size="sm" variant="secondary" title={t("message.rewindFilesHint")} onClick={() => onRewind(true)}>
        {t("message.rewindChatAndFiles")}
      </Button>
      <Button size="sm" variant="ghost" onClick={onCancel}>
        {t("message.rewindCancel")}
      </Button>
    </div>
  );
}

function MessageActions({ text, onEdit, onRewind }: { text: string; onEdit?: (text: string) => void; onRewind?: () => void }) {
  const { t } = useT("claudeCodePanel");
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  };
  return (
    <div className="cc-msg__actions">
      <button type="button" className="cc-msg__action" aria-label={copied ? t("message.copied") : t("message.copy")} title={t("message.copy")} onClick={() => void copy()}>
        {copied ? <Check size={12} /> : <Copy size={12} />}
      </button>
      {onEdit && (
        <button type="button" className="cc-msg__action" aria-label={t("message.edit")} title={t("message.edit")} onClick={() => onEdit(text)}>
          <Pencil size={12} />
        </button>
      )}
      {onRewind && (
        <button type="button" className="cc-msg__action" aria-label={t("message.rewind")} title={t("message.rewindTitle")} onClick={onRewind}>
          <RotateCcw size={12} />
        </button>
      )}
    </div>
  );
}

function ResultFooter({ item, root, cwd }: { item: ResultItem; root: string | null; cwd: string | null }) {
  const { t } = useT("claudeCode");
  const fmt = useFormat();
  const interrupted = item.isError && item.subtype === "error_during_execution";
  const tokens = t("result.tokens", {
    input: fmt.number(inputTokens(item.usage)),
    output: fmt.number(item.usage.outputTokens),
  });
  return (
    <div className={clsx("cc-result", item.isError && !interrupted && "cc-result--error")}>
      <div className="cc-result__meta">
        {item.isError && !interrupted && <AlertTriangle size={13} />}
        <span>{interrupted ? t("result.interrupted") : item.isError ? t("result.failed") : t("result.done")}</span>
        {item.durationMs != null && <span>{fmt.duration(item.durationMs)}</span>}
        <span title={t("result.cacheHint", { read: fmt.number(item.usage.cacheReadTokens), write: fmt.number(item.usage.cacheWriteTokens) })}>
          {tokens}
        </span>
        {item.costUsd != null && item.costUsd > 0 && (
          <span title={t("result.costHint")}>{t("result.cost", { cost: fmt.currency(item.costUsd) })}</span>
        )}
      </div>
      {item.isError && !interrupted && item.text && <div className="cc-result__error">{item.text}</div>}
      {item.files.length > 0 && (
        <div className="cc-result__files">
          <FileDiff size={13} />
          <span>{t("result.files", { count: item.files.length })}</span>
          {item.files.map((f) => {
            const rel = relativeTo(root, f, cwd);
            return rel ? (
              <button key={f} type="button" className="cc-chip" onClick={() => openChangedFile(rel)} title={t("tool.openFile")}>
                {rel}
              </button>
            ) : (
              <span key={f} className="cc-chip cc-chip--plain">
                {f}
              </span>
            );
          })}
        </div>
      )}
    </div>
  );
}

export function ChatView({ chat, root, cwd, onRespond, onLogin, empty, onEdit, onRewind }: ChatViewProps) {
  const { t } = useT("claudeCode");
  const { t: tp } = useT("claudeCodePanel");
  const modeLabel = (mode: string): string => {
    switch (mode) {
      case "default":
        return t("mode.default");
      case "acceptEdits":
        return t("mode.acceptEdits");
      case "plan":
        return t("mode.plan");
      case "bypassPermissions":
        return t("mode.bypassPermissions");
      default:
        return mode;
    }
  };
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const [rewindId, setRewindId] = useState<string | null>(null);

  const onScroll = () => {
    const el = scroller.current;
    if (!el) return;
    stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
  };

  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [chat.items, chat.thinking]);

  useEffect(() => {
    stick.current = true;
  }, []);

  const risks = new Map<string, ClaudeRisk>();
  for (const it of chat.items) if (it.kind === "permission" && it.toolUseId && it.risk) risks.set(it.toolUseId, it.risk);

  const render = (items: ChatItem[]): ReactNode =>
    items.map((it) => {
      switch (it.kind) {
        case "user":
          return (
            <div key={it.id} className="cc-msg cc-msg--user">
              <div className="cc-msg__bubble">
                <div className="cc-msg__who">
                  <User size={12} className="cc-msg__icon" aria-hidden="true" />
                  {tp("message.you")}
                </div>
                <div className="cc-msg__text">{it.text}</div>
              </div>
              <MessageActions
                text={it.text}
                onEdit={onEdit}
                onRewind={onRewind && !chat.running ? () => setRewindId(rewindId === it.id ? null : it.id) : undefined}
              />
              {onRewind && rewindId === it.id && !chat.running && (
                <RewindBar
                  onRewind={(restoreFiles) => {
                    setRewindId(null);
                    onRewind(it.id, restoreFiles);
                  }}
                  onCancel={() => setRewindId(null)}
                />
              )}
            </div>
          );
        case "assistant":
          return (
            <div key={it.id} className={clsx("cc-msg cc-msg--assistant", it.synthetic && "cc-msg--synthetic")}>
              <div className="cc-msg__avatar" aria-hidden="true">
                <Sparkle size={13} />
              </div>
              <div className="cc-msg__body">
                <SafeMarkdown source={it.text} />
                {it.streaming && <span className="cc-caret" aria-hidden="true" />}
                {!it.streaming && !it.synthetic && it.text.trim() && <MessageActions text={it.text} />}
              </div>
            </div>
          );
        case "tool":
          return (
            <ToolCard
              key={it.id}
              call={{ id: it.id, name: it.name, input: it.input }}
              result={it.result}
              root={root}
              cwd={cwd}
              onOpenFile={openChangedFile}
              risk={risks.get(it.id) ?? null}
            >
              {it.children.length > 0 ? render(it.children) : undefined}
            </ToolCard>
          );
        case "permission":
          return (
            <PermissionCard
              key={it.id}
              request={it}
              root={root}
              cwd={cwd}
              onRespond={(decision, message) => onRespond(it.id, decision, message)}
            />
          );
        case "result":
          return <ResultFooter key={it.id} item={it} root={root} cwd={cwd} />;
        case "error":
          return (
            <div key={it.id} className="cc-error" role="alert">
              <AlertTriangle size={14} />
              <span>{it.authRequired ? t("errors.authRequired") : it.message}</span>
              {it.authRequired && (
                <Button size="sm" variant="primary" leftIcon={<LogIn size={14} />} onClick={onLogin}>
                  {t("login.signIn")}
                </Button>
              )}
            </div>
          );
        case "notice":
          return (
            <div key={it.id} className="cc-notice">
              {it.notice === "interrupted" && (
                <>
                  <CircleStop size={13} /> {t("notice.interrupted")}
                </>
              )}
              {it.notice === "modeChanged" && t("notice.modeChanged", { mode: modeLabel(it.text) })}
              {it.notice === "modelChanged" && t("notice.modelChanged", { model: it.text })}
              {it.notice === "exited" && (it.text ? t("notice.exitedCode", { code: it.text }) : t("notice.exited"))}
              {it.notice === "commandOutput" && (
                <>
                  <TerminalIcon size={13} />
                  <pre className="cc-command-output">{it.text}</pre>
                </>
              )}
            </div>
          );
      }
    });

  return (
    <div className="cc-chat" ref={scroller} onScroll={onScroll} role="log" aria-live="polite" aria-label={t("chat.label")}>
      {chat.items.length === 0 && !chat.running && empty}
      {render(chat.items)}
      {chat.running && (chat.thinking || chat.streamingId === null) && (
        <div className="cc-thinking">
          <Spinner size={13} />
          <span>{chat.thinking ? t("chat.thinking") : t("chat.working")}</span>
        </div>
      )}
    </div>
  );
}
