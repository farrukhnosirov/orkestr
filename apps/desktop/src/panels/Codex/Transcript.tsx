/**
 * Message list of one Codex session: user messages, streaming Markdown
 * answers (safe renderer), reasoning summaries and the plan (collapsible),
 * tool cards (the Claude Code cards — Codex reports tools in the same
 * vocabulary — and a patch card for file changes), inline approval cards,
 * notices, errors (sign-in needed, plan limit reached) and the per-turn
 * footer (status, duration, changed files). Sticks to the bottom while new
 * items arrive unless the user scrolled up.
 */
import type { CodexApprovalDecision } from "@orkestr/protocol";
import { Button, Spinner } from "@orkestr/ui";
import clsx from "clsx";
import { AlertTriangle, Bot, Brain, Check, CircleStop, Copy, FileDiff, Gauge, Info, ListChecks, LogIn, RefreshCw, User } from "lucide-react";
import { type ReactNode, useLayoutEffect, useRef, useState } from "react";

import { openChangedFile } from "../../features/claudeCode/bridge";
import { relativeTo } from "../../features/claudeCode/model";
import { patchFiles, type CodexChat, type CxItem, type CxTurnItem } from "../../features/codex/model";
import { SafeMarkdown } from "../../features/extensions/markdown";
import { useFormat, useT } from "../../i18n";
import { ToolCard } from "../ClaudeCode/tools";
import { ApprovalCard, PatchCard } from "./Approval";

export interface TranscriptProps {
  chat: CodexChat;
  root: string | null;
  cwd: string | null;
  onRespond: (requestId: string, decision: CodexApprovalDecision) => void;
  onSignIn: () => void;
  onShowLimits: () => void;
  empty?: ReactNode;
}

function CopyButton({ text }: { text: string }) {
  const { t } = useT("codex");
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
      <button type="button" className="cc-msg__action" aria-label={copied ? t("chat.copied") : t("chat.copy")} title={t("chat.copy")} onClick={() => void copy()}>
        {copied ? <Check size={12} /> : <Copy size={12} />}
      </button>
    </div>
  );
}

function TurnFooter({ item, root, cwd }: { item: CxTurnItem; root: string | null; cwd: string | null }) {
  const { t } = useT("codex");
  const fmt = useFormat();
  const failed = item.status === "failed";
  const interrupted = item.status === "interrupted";
  return (
    <div className={clsx("cc-result", failed && "cc-result--error")} data-testid="codex-turn">
      <div className="cc-result__meta">
        {failed && <AlertTriangle size={13} />}
        <span>{interrupted ? t("turn.interrupted") : failed ? t("turn.failed") : t("turn.done")}</span>
        {item.durationMs != null && <span>{fmt.duration(item.durationMs)}</span>}
      </div>
      {failed && item.error && <div className="cc-result__error">{item.error}</div>}
      {item.files.length > 0 && (
        <div className="cc-result__files">
          <FileDiff size={13} />
          <span>{t("turn.files", { count: item.files.length })}</span>
          {item.files.map((f) => {
            const rel = relativeTo(root, f, cwd);
            return rel ? (
              <button key={f} type="button" className="cc-chip" onClick={() => openChangedFile(rel)} title={t("patch.open")}>
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

export function Transcript({ chat, root, cwd, onRespond, onSignIn, onShowLimits, empty }: TranscriptProps) {
  const { t } = useT("codex");
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const firstPending = chat.items.find((it) => it.kind === "approval" && it.status === "pending")?.id ?? null;

  const onScroll = () => {
    const el = scroller.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
  };

  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [chat.items, chat.running]);

  const render = (it: CxItem): ReactNode => {
    switch (it.kind) {
      case "user":
        return (
          <div key={it.id} className="cc-msg cc-msg--user">
            <div className="cc-msg__bubble">
              <div className="cc-msg__who">
                <User size={12} className="cc-msg__icon" aria-hidden="true" />
                {t("chat.you")}
              </div>
              <div className="cc-msg__text">{it.text}</div>
            </div>
            <CopyButton text={it.text} />
          </div>
        );
      case "assistant":
        return (
          <div key={it.id} className="cc-msg cc-msg--assistant" data-testid="codex-answer">
            <div className="cc-msg__avatar cx-avatar" aria-hidden="true">
              <Bot size={13} />
            </div>
            <div className="cc-msg__body">
              <SafeMarkdown source={it.text} />
              {it.streaming && <span className="cc-caret" aria-hidden="true" />}
              {!it.streaming && it.text.trim() && <CopyButton text={it.text} />}
            </div>
          </div>
        );
      case "reasoning":
        return (
          <details key={it.id} className="cx-fold cx-fold--reasoning">
            <summary>
              <Brain size={13} aria-hidden /> {t("chat.reasoning")}
            </summary>
            <div className="cx-fold__body">
              <SafeMarkdown source={it.text} />
            </div>
          </details>
        );
      case "plan":
        return (
          <details key={it.id} className="cx-fold cx-fold--plan" open>
            <summary>
              <ListChecks size={13} aria-hidden /> {t("chat.plan")}
            </summary>
            <div className="cx-fold__body">
              <SafeMarkdown source={it.text} />
            </div>
          </details>
        );
      case "tool": {
        const files = it.name === "Edit" ? patchFiles(it.input) : [];
        if (files.length > 0) {
          return <PatchCard key={it.id} files={files} result={it.result} root={root} cwd={cwd} onOpen={(rel) => openChangedFile(rel)} />;
        }
        return (
          <ToolCard
            key={it.id}
            call={{ id: it.id, name: it.name, input: it.input }}
            result={it.result}
            root={root}
            cwd={cwd}
            onOpenFile={openChangedFile}
          />
        );
      }
      case "approval":
        return (
          <ApprovalCard
            key={it.id}
            request={it}
            root={root}
            cwd={cwd}
            autoFocus={it.id === firstPending}
            onOpenFile={(rel) => openChangedFile(rel)}
            onRespond={(decision) => onRespond(it.id, decision)}
          />
        );
      case "turn":
        return <TurnFooter key={it.id} item={it} root={root} cwd={cwd} />;
      case "error":
        return (
          <div key={it.id} className={clsx("cc-error", it.willRetry && "cx-error--retry")} role="alert" data-testid="codex-error">
            {it.willRetry ? <RefreshCw size={14} /> : it.usageLimit ? <Gauge size={14} /> : <AlertTriangle size={14} />}
            <span>
              {it.authRequired ? t("errors.authRequired") : it.usageLimit ? t("errors.usageLimit") : it.message}
              {it.willRetry && ` ${t("errors.retrying")}`}
            </span>
            {it.authRequired && (
              <Button size="sm" variant="primary" leftIcon={<LogIn size={14} />} onClick={onSignIn}>
                {t("signIn.button")}
              </Button>
            )}
            {it.usageLimit && (
              <Button size="sm" leftIcon={<Gauge size={14} />} onClick={onShowLimits}>
                {t("limits.show")}
              </Button>
            )}
          </div>
        );
      case "notice":
        return (
          <div key={it.id} className="cc-notice">
            {it.notice === "info" && (
              <>
                <Info size={13} /> <span className="cx-notice__text">{it.text}</span>
              </>
            )}
            {it.notice === "modelChanged" && t("notice.modelChanged", { model: it.text })}
            {it.notice === "exited" && (
              <>
                <CircleStop size={13} /> {it.text ? t("notice.exitedCode", { code: it.text }) : t("notice.exited")}
              </>
            )}
          </div>
        );
    }
  };

  const waiting = chat.running && !chat.items.some((it) => it.kind === "assistant" && it.streaming) && firstPending === null;

  return (
    <div className="cc-chat" ref={scroller} onScroll={onScroll} role="log" aria-live="polite" aria-label={t("chat.label")} data-testid="codex-transcript">
      {chat.items.length === 0 && !chat.running && empty}
      {chat.items.map(render)}
      {waiting && (
        <div className="cc-thinking">
          <Spinner size={13} />
          <span>{t("chat.working")}</span>
        </div>
      )}
    </div>
  );
}
