/**
 * Conversation mode switch and transcript (ADR-0049), shown in the Assistant
 * surface. The input stays the surface's own field: everything typed there
 * goes through `jarvisSession.send` (commands still run in conversation mode).
 */
import { IconButton } from "@orkestr/ui";
import { clsx } from "clsx";
import { Bot, Eraser, MessageCircle, Square, Terminal } from "lucide-react";
import { useEffect, useRef } from "react";

import { useT } from "../../i18n";
import { cancelChat, forgetConversation, loadHistory } from "./api";
import { jarvisSay } from "./persona";
import { type ConversationMode, useJarvisStore } from "./store";
import "./jarvis.css";

/** Commands / conversation toggle. */
export function ConversationModeSwitch() {
  const { t } = useT("jarvis");
  const mode = useJarvisStore((s) => s.mode);
  const setMode = useJarvisStore((s) => s.setMode);
  const pick = (m: ConversationMode) => {
    if (m === mode) return;
    setMode(m);
    useJarvisStore.getState().patch({ lastReply: jarvisSay(m === "chat" ? "modeChat" : "modeCommands") });
  };
  return (
    <div className="jv-mode" role="radiogroup" aria-label={t("mode.label")}>
      {(["commands", "chat"] as const).map((m) => (
        <button
          key={m}
          type="button"
          role="radio"
          aria-checked={mode === m}
          className={clsx("jv-mode__item", mode === m && "jv-mode__item--on")}
          title={t(m === "chat" ? "mode.chatHint" : "mode.commandsHint")}
          onClick={() => pick(m)}
        >
          {m === "chat" ? <MessageCircle size={13} /> : <Terminal size={13} />}
          {t(`mode.${m}`)}
        </button>
      ))}
    </div>
  );
}

/** The conversation transcript (conversation mode). */
export function ChatTranscript({ limit = 30 }: { limit?: number }) {
  const { t } = useT("jarvis");
  const entries = useJarvisStore((s) => s.entries);
  const streamingId = useJarvisStore((s) => s.streamingId);
  const endRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (useJarvisStore.getState().entries.length === 0) void loadHistory();
  }, []);
  const last = entries[entries.length - 1];
  useEffect(() => {
    endRef.current?.scrollIntoView?.({ block: "end" });
  }, [entries.length, last?.text]);

  const shown = entries.slice(-limit);
  return (
    <section className="jv-chat" aria-label={t("chat.title")}>
      <header className="jv-chat__head">
        <span className="jv-chat__title">
          <Bot size={14} /> {t("chat.title")}
        </span>
        {streamingId && <IconButton label={t("chat.stop")} icon={<Square size={13} />} onClick={() => void cancelChat()} />}
        <IconButton label={t("chat.forget")} icon={<Eraser size={14} />} disabled={entries.length === 0} onClick={() => void forgetConversation()} />
      </header>
      <div className="jv-chat__list" role="log" aria-live="polite">
        {shown.length === 0 && <p className="jv-chat__empty">{t("chat.empty")}</p>}
        {shown.map((e) => (
          <div key={e.id} className={clsx("jv-msg", `jv-msg--${e.role}`, e.tone && `jv-msg--${e.tone}`)}>
            <span className="jv-msg__who">{e.role === "user" ? t("chat.you") : t("chat.jarvis")}</span>
            <span className="jv-msg__text">{e.text || (e.streaming ? t("chat.thinking") : "")}</span>
            {e.role === "assistant" && e.source !== "user" && !e.streaming && <span className="jv-msg__src">{t(`chat.source.${e.source}`)}</span>}
          </div>
        ))}
        <div ref={endRef} />
      </div>
    </section>
  );
}
