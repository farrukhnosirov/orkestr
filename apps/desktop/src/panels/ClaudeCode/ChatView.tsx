/**
 * Message list of one chat session: user messages, streaming Markdown
 * answers (safe renderer: no HTML, no remote images), tool cards, inline
 * permission cards, notices, errors and the per-turn result footer (tokens,
 * duration, changed files). Sticks to the bottom while new items arrive
 * unless the user scrolled up.
 */
import type { ClaudePermissionDecision } from "@orkestr/protocol";
import { Button, Spinner } from "@orkestr/ui";
import clsx from "clsx";
import { AlertTriangle, CircleStop, FileDiff, LogIn, Terminal as TerminalIcon, User } from "lucide-react";
import { type ReactNode, useEffect, useLayoutEffect, useRef } from "react";

import { useFormat, useT } from "../../i18n";
import { SafeMarkdown } from "../../features/extensions/markdown";
import { openChangedFile } from "../../features/claudeCode/bridge";
import { inputTokens, relativeTo, type ChatItem, type ChatState, type ResultItem } from "../../features/claudeCode/model";
import { PermissionCard } from "./PermissionCard";
import { ToolCard } from "./ToolCard";

export interface ChatViewProps {
  chat: ChatState;
  root: string | null;
  cwd: string | null;
  onRespond: (requestId: string, decision: ClaudePermissionDecision, message?: string) => void;
  onLogin: () => void;
  empty?: ReactNode;
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

export function ChatView({ chat, root, cwd, onRespond, onLogin, empty }: ChatViewProps) {
  const { t } = useT("claudeCode");
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

  const render = (items: ChatItem[]): ReactNode =>
    items.map((it) => {
      switch (it.kind) {
        case "user":
          return (
            <div key={it.id} className="cc-msg cc-msg--user">
              <User size={14} className="cc-msg__icon" aria-hidden="true" />
              <div className="cc-msg__text">{it.text}</div>
            </div>
          );
        case "assistant":
          return (
            <div key={it.id} className={clsx("cc-msg cc-msg--assistant", it.synthetic && "cc-msg--synthetic")}>
              <SafeMarkdown source={it.text} />
              {it.streaming && <span className="cc-caret" aria-hidden="true" />}
            </div>
          );
        case "tool":
          return <ToolCard key={it.id} item={it} root={root} cwd={cwd} renderChildren={render} />;
        case "permission":
          return (
            <PermissionCard
              key={it.id}
              item={it}
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
