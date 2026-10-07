/**
 * Composer: Enter sends, Shift+Enter inserts a newline, Esc interrupts.
 * `@` completes project files (`files.find`), `/` at the start completes
 * the CLI's slash commands (sent through as typed). While Claude works a
 * message is queued and sent after the turn. "Selection" attaches the Code
 * editor's selection with `@path:lines`.
 */
import { IconButton } from "@orkestr/ui";
import clsx from "clsx";
import { FileCode2, SendHorizontal, Square, TextSelect, X } from "lucide-react";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";

import { getClient } from "../../core/client";
import { useT } from "../../i18n";
import { currentSelection } from "../../features/claudeCode/bridge";
import { composerKey, composerPopup, insertMention, selectionPrompt } from "../../features/claudeCode/model";
import type { ComposerProps } from "./types";

const MAX_INPUT_PX = 220;
const MAX_CHARS = 100_000;

interface Suggestion {
  key: string;
  label: string;
  detail?: string;
  apply: () => void;
}


export function Composer({
  projectId,
  disabled,
  running,
  queue,
  commands,
  insert,
  onInsertConsumed,
  onSend,
  onInterrupt,
  onDequeue,
}: ComposerProps) {
  const { t } = useT("claudeCode");
  const area = useRef<HTMLTextAreaElement>(null);
  const [text, setText] = useState("");
  const [caret, setCaret] = useState(0);
  const [files, setFiles] = useState<string[]>([]);
  const [selected, setSelected] = useState(0);
  const [dismissed, setDismissed] = useState(false);
  const findSeq = useRef(0);

  // Auto-grow.
  useEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_INPUT_PX)}px`;
  }, [text]);

  // Inserted selection ("Ask Claude about selection").
  useEffect(() => {
    if (insert === null) return;
    setText((prev) => (prev.trim() ? `${prev}\n${insert}` : insert));
    onInsertConsumed();
    requestAnimationFrame(() => {
      const el = area.current;
      if (el) {
        el.focus();
        el.selectionStart = el.selectionEnd = el.value.length;
        setCaret(el.value.length);
      }
    });
  }, [insert, onInsertConsumed]);

  const popup = dismissed ? null : composerPopup(text, caret);

  // File suggestions for `@`.
  useEffect(() => {
    if (popup?.kind !== "mention") return;
    const seq = ++findSeq.current;
    const timer = setTimeout(() => {
      getClient()
        .call("files.find", { projectId, query: popup.query, maxResults: 8 })
        .then((res) => {
          if (seq === findSeq.current) setFiles(res.matches.map((m) => m.path));
        })
        .catch(() => {
          if (seq === findSeq.current) setFiles([]);
        });
    }, 120);
    return () => clearTimeout(timer);
  }, [popup?.kind, popup?.kind === "mention" ? popup.query : null, projectId]); // eslint-disable-line react-hooks/exhaustive-deps

  const setValue = (next: string, nextCaret: number) => {
    setText(next);
    setCaret(nextCaret);
    setSelected(0);
    requestAnimationFrame(() => {
      const el = area.current;
      if (el) {
        el.focus();
        el.selectionStart = el.selectionEnd = nextCaret;
      }
    });
  };

  const suggestions: Suggestion[] =
    popup?.kind === "slash"
      ? commands
          .filter((c) => c.name.toLowerCase().startsWith(popup.query.toLowerCase()))
          .slice(0, 8)
          .map((c) => ({
            key: c.name,
            label: `/${c.name}`,
            detail: c.description,
            apply: () => setValue(`/${c.name} `, c.name.length + 2),
          }))
      : popup?.kind === "mention"
        ? files.map((f) => ({
            key: f,
            label: f,
            apply: () => {
              const r = insertMention(text, popup.start, caret, f);
              setValue(r.text, r.caret);
            },
          }))
        : [];

  const send = async () => {
    const body = text.trim();
    if (!body || disabled || body.length > MAX_CHARS) return;
    const ok = await onSend({ text: body });
    if (ok) {
      setText("");
      setCaret(0);
      setDismissed(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (suggestions.length > 0) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSelected((i) => (i + 1) % suggestions.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setSelected((i) => (i - 1 + suggestions.length) % suggestions.length);
        return;
      }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) {
        e.preventDefault();
        suggestions[Math.min(selected, suggestions.length - 1)]?.apply();
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setDismissed(true);
        return;
      }
    }
    const action = composerKey({ key: e.key, shiftKey: e.shiftKey, isComposing: e.nativeEvent.isComposing });
    if (action === "send") {
      e.preventDefault();
      void send();
    } else if (action === "interrupt" && running) {
      e.preventDefault();
      onInterrupt();
    }
  };

  const attachSelection = () => {
    const sel = currentSelection();
    if (!sel) return;
    const snippet = selectionPrompt(sel);
    setValue(text.trim() ? `${text}\n${snippet}` : snippet, (text.trim() ? text.length + 1 : 0) + snippet.length);
  };

  const hasSelection = currentSelection() !== null;

  return (
    <div className="cc-composer">
      {queue.length > 0 && (
        <div className="cc-queue" aria-label={t("composer.queued")}>
          {queue.map((q, i) => (
            <span key={i} className="cc-chip">
              {t("composer.queuedItem", { text: q.length > 60 ? `${q.slice(0, 60)}…` : q })}
              <button type="button" className="cc-chip__x" aria-label={t("composer.removeQueued")} onClick={() => onDequeue(i)}>
                <X size={12} />
              </button>
            </span>
          ))}
        </div>
      )}
      {suggestions.length > 0 && (
        <ul className="cc-suggest" role="listbox" aria-label={popup?.kind === "slash" ? t("composer.commands") : t("composer.files")}>
          {suggestions.map((s, i) => (
            <li
              key={s.key}
              role="option"
              aria-selected={i === selected}
              className={clsx("cc-suggest__item", i === selected && "cc-suggest__item--active")}
              onMouseDown={(e) => {
                e.preventDefault();
                s.apply();
              }}
            >
              {popup?.kind === "mention" && <FileCode2 size={13} />}
              <span className="cc-suggest__label">{s.label}</span>
              {s.detail && <span className="cc-suggest__detail">{s.detail}</span>}
            </li>
          ))}
        </ul>
      )}
      <div className="cc-composer__row">
        <textarea
          ref={area}
          className="cc-composer__input"
          rows={2}
          value={text}
          disabled={disabled}
          placeholder={running ? t("composer.placeholderBusy") : t("composer.placeholder")}
          aria-label={t("composer.label")}
          onChange={(e) => {
            setText(e.target.value);
            setCaret(e.target.selectionStart);
            setDismissed(false);
          }}
          onSelect={(e) => setCaret(e.currentTarget.selectionStart)}
          onKeyDown={onKeyDown}
        />
        <div className="cc-composer__buttons">
          <IconButton
            label={t("composer.attachSelection")}
            icon={<TextSelect size={16} />}
            onClick={attachSelection}
            disabled={disabled || !hasSelection}
            tooltip="top"
          />
          {running && (
            <IconButton label={t("composer.interrupt")} icon={<Square size={15} />} variant="danger" onClick={onInterrupt} tooltip="top" />
          )}
          <IconButton
            label={running ? t("composer.queue") : t("composer.send")}
            icon={<SendHorizontal size={16} />}
            variant="solid"
            onClick={() => void send()}
            disabled={disabled || !text.trim() || text.length > MAX_CHARS}
            tooltip="top"
          />
        </div>
      </div>
      <div className="cc-composer__hint">{running ? t("composer.hintBusy") : t("composer.hint")}</div>
    </div>
  );
}
