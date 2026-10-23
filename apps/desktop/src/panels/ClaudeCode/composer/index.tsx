import "./composer.css";

/**
 * Claude Code composer.
 *
 * - Enter sends, Shift+Enter inserts a newline; while Claude works Enter
 *   queues (one message by default) and Esc / «Остановить» interrupts.
 * - `@` completes project files through `files.find`; picked files come back
 *   as `attachments` (the `@path` tokens stay in the text the CLI reads).
 * - `/` at the start completes slash commands: the built-ins plus the ones
 *   the CLI reported in its init event.
 * - «Прикрепить выделенное» attaches the Code editor's selection as a chip;
 *   on send it is prepended as `@path:lines` + a fenced block.
 */
import { IconButton } from "@orkestr/ui";
import clsx from "clsx";
import { FileCode2, ImagePlus, Lock, SendHorizontal, Slash, Square, TextSelect, X } from "lucide-react";
import { type ClipboardEvent, type KeyboardEvent, type ReactNode, useEffect, useId, useRef, useState } from "react";

import {
  IMAGE_TYPES,
  MAX_IMAGE_BYTES,
  MAX_IMAGES,
  parseDataUrl,
  type ImageAttachment,
} from "../../../features/claudeCode/chatTools";
import { getClient } from "../../../core/client";
import { currentSelection, type EditorSelection } from "../../../features/claudeCode/bridge";
import { composerKey, composerPopup, insertMention } from "../../../features/claudeCode/model";
import { useT } from "../../../i18n";
import type { ComposerProps } from "../types";
import {
  buildPayload,
  isBuiltinSlashCommand,
  lineRange,
  slashSuggestions,
  type ComposerSendPayloadWithAttachments,
} from "./attachments";

export * from "./attachments";
export { ModelPicker } from "./ModelPicker";

const MAX_INPUT_PX = 240;
/** Longest message accepted (characters). */
export const MAX_MESSAGE_CHARS = 100_000;
const FIND_DEBOUNCE_MS = 120;
const MAX_FILE_SUGGESTIONS = 8;

/** Why the composer is disabled (a known reason is localized; other text is shown as is). */
export type ComposerDisabledReason = "notSignedIn" | "noBinary" | "ended";

export interface ComposerViewProps extends Omit<ComposerProps, "onSend"> {
  /** Shown instead of the hint while `disabled`: a `ComposerDisabledReason` (localized) or ready text. */
  disabledReason?: ComposerDisabledReason | string | null;
  /** Current editor selection; `undefined` → read it from the Code editor bridge. */
  selection?: EditorSelection | null;
  /** How many messages may wait while Claude works (default 1). */
  maxQueued?: number;
  /** Resolves true when sent or queued (the composer then clears). */
  onSend: (payload: ComposerSendPayloadWithAttachments) => Promise<boolean>;
  /** The inserted text replaces the draft (edit & resend) instead of being appended. */
  insertReplace?: boolean;
  /** Chips under the input (mode, model, …). */
  toolbar?: ReactNode;
  /** Images can be pasted / attached (default true). */
  allowImages?: boolean;
  /** Offer Claude Code's built-in slash commands (default true; the Codex panel passes its own list). */
  builtinSlashCommands?: boolean;
  /** Engine-specific texts (the Codex panel reuses this composer); defaults are Claude's. */
  texts?: ComposerTexts;
}

/** Overridable composer texts (already localized). */
export interface ComposerTexts {
  label?: string;
  placeholder?: string;
  placeholderBusy?: string;
  queueFull?: string;
}

let imageSeq = 0;

function readImage(file: File): Promise<ImageAttachment | null> {
  return new Promise((resolve) => {
    const reader = new FileReader();
    reader.onload = () => {
      const parsed = typeof reader.result === "string" ? parseDataUrl(reader.result) : null;
      imageSeq += 1;
      resolve(parsed ? { id: `img-${imageSeq}`, name: file.name || `image-${imageSeq}`, ...parsed } : null);
    };
    reader.onerror = () => resolve(null);
    reader.readAsDataURL(file);
  });
}

function focusTextarea(el: HTMLTextAreaElement | null, pos: number): void {
  requestAnimationFrame(() => {
    if (!el) return;
    el.focus();
    el.selectionStart = el.selectionEnd = pos;
  });
}

interface Suggestion {
  key: string;
  label: string;
  detail?: string | null;
  apply: () => void;
}

export function Composer({
  projectId,
  disabled,
  disabledReason,
  running,
  queue,
  commands,
  insert,
  onInsertConsumed,
  selection,
  maxQueued = 1,
  onSend,
  onInterrupt,
  onDequeue,
  insertReplace = false,
  toolbar,
  allowImages = true,
  builtinSlashCommands = true,
  texts,
}: ComposerViewProps) {
  const { t } = useT("claudeCodeComposer");
  const listId = useId();
  const area = useRef<HTMLTextAreaElement>(null);
  const [text, setText] = useState("");
  const [caret, setCaret] = useState(0);
  const [files, setFiles] = useState<string[] | null>(null);
  const [active, setActive] = useState(0);
  const [dismissed, setDismissed] = useState(false);
  const [picked, setPicked] = useState<string[]>([]);
  const [attached, setAttached] = useState<EditorSelection | null>(null);
  const [sending, setSending] = useState(false);
  const [images, setImages] = useState<ImageAttachment[]>([]);
  const [imageError, setImageError] = useState<string | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const findSeq = useRef(0);

  const addImages = async (files: readonly File[]) => {
    setImageError(null);
    const accepted: File[] = [];
    for (const f of files) {
      if (!(IMAGE_TYPES as readonly string[]).includes(f.type)) {
        setImageError(t("composer.imageType"));
        continue;
      }
      if (f.size > MAX_IMAGE_BYTES) {
        setImageError(t("composer.imageTooLarge"));
        continue;
      }
      accepted.push(f);
    }
    const read = (await Promise.all(accepted.map(readImage))).filter((i): i is ImageAttachment => i !== null);
    setImages((prev) => {
      const next = [...prev, ...read];
      if (next.length > MAX_IMAGES) setImageError(t("composer.tooManyImages", { count: MAX_IMAGES }));
      return next.slice(0, MAX_IMAGES);
    });
  };

  const onPaste = (e: ClipboardEvent<HTMLTextAreaElement>) => {
    if (!allowImages) return;
    const files = Array.from(e.clipboardData?.files ?? []).filter((f) => f.type.startsWith("image/"));
    if (files.length === 0) return;
    e.preventDefault();
    void addImages(files);
  };

  // Auto-grow up to MAX_INPUT_PX, then scroll.
  useEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_INPUT_PX)}px`;
  }, [text]);

  const textRef = useRef(text);
  textRef.current = text;
  const focusAt = (pos: number) => focusTextarea(area.current, pos);

  // Text inserted from outside ("Ask Claude about selection").
  useEffect(() => {
    if (insert === null) return;
    const prev = textRef.current;
    const next = insertReplace || !prev.trim() ? insert : `${prev}\n${insert}`;
    setText(next);
    setCaret(next.length);
    focusTextarea(area.current, next.length);
    onInsertConsumed();
  }, [insert, insertReplace, onInsertConsumed]);

  const popup = dismissed || disabled ? null : composerPopup(text, caret);
  const mentionQuery = popup?.kind === "mention" ? popup.query : null;

  // File suggestions for `@` (debounced; stale answers dropped).
  useEffect(() => {
    if (mentionQuery === null) {
      setFiles(null);
      return;
    }
    const seq = ++findSeq.current;
    const timer = setTimeout(() => {
      let request: Promise<{ matches: { path: string }[] }>;
      try {
        request = getClient().call("files.find", { projectId, query: mentionQuery, maxResults: MAX_FILE_SUGGESTIONS });
      } catch {
        request = Promise.resolve({ matches: [] });
      }
      request
        .then((res) => {
          if (seq === findSeq.current) setFiles(res.matches.map((m) => m.path));
        })
        .catch(() => {
          if (seq === findSeq.current) setFiles([]);
        });
    }, FIND_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [mentionQuery, projectId]);

  const setValue = (next: string, nextCaret: number) => {
    setText(next);
    setCaret(nextCaret);
    setActive(0);
    focusAt(nextCaret);
  };

  const suggestions: Suggestion[] =
    popup?.kind === "slash"
      ? slashSuggestions(commands, popup.query, 8, builtinSlashCommands).map((c) => ({
          key: c.name,
          label: c.argumentHint ? `/${c.name} ${c.argumentHint}` : `/${c.name}`,
          detail: builtinSlashCommands && isBuiltinSlashCommand(c.name) ? t(`composer.builtin.${c.name}`) : c.description,
          apply: () => setValue(`/${c.name} `, c.name.length + 2),
        }))
      : popup?.kind === "mention" && files
        ? files.map((f) => ({
            key: f,
            label: f,
            apply: () => {
              const r = insertMention(text, popup.start, caret, f);
              setPicked((prev) => (prev.includes(f) ? prev : [...prev, f]));
              setValue(r.text, r.caret);
            },
          }))
        : [];
  const showEmptyFiles = popup?.kind === "mention" && files !== null && files.length === 0 && popup.query !== "";
  const activeIndex = Math.min(active, Math.max(suggestions.length - 1, 0));

  const liveSelection = selection === undefined ? currentSelection() : selection;
  const queueFull = running && queue.length >= maxQueued;
  const tooLong = text.length > MAX_MESSAGE_CHARS;
  const canSend = !disabled && !sending && !queueFull && !tooLong && (text.trim() !== "" || attached !== null);

  const send = async () => {
    if (!canSend) return;
    const payload = buildPayload(text.trim(), picked, attached);
    if (images.length > 0) payload.images = images;
    setSending(true);
    try {
      const ok = await onSend(payload);
      if (ok) {
        setText("");
        setCaret(0);
        setPicked([]);
        setAttached(null);
        setImages([]);
        setImageError(null);
        setDismissed(false);
      }
    } finally {
      setSending(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing) return;
    if (suggestions.length > 0) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setActive((activeIndex + 1) % suggestions.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setActive((activeIndex - 1 + suggestions.length) % suggestions.length);
        return;
      }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) {
        e.preventDefault();
        suggestions[activeIndex]?.apply();
        return;
      }
    }
    if (e.key === "Escape" && popup) {
      e.preventDefault();
      setDismissed(true);
      return;
    }
    const action = composerKey({ key: e.key, shiftKey: e.shiftKey });
    if (action === "send") {
      e.preventDefault();
      void send();
    } else if (action === "interrupt" && running) {
      e.preventDefault();
      onInterrupt();
    }
  };

  const disabledText = disabledReason
    ? disabledReason === "notSignedIn" || disabledReason === "noBinary" || disabledReason === "ended"
      ? t(`composer.disabled.${disabledReason}`)
      : disabledReason
    : t("composer.disabled.generic");

  const popupOpen = suggestions.length > 0 || showEmptyFiles;
  const optionId = (i: number) => `${listId}-opt-${i}`;

  return (
    <div className={clsx("ccx-composer", running && "ccx-composer--running", disabled && "ccx-composer--disabled")}>
      {queue.length > 0 && (
        <ul className="ccx-queue" aria-label={t("composer.queued")}>
          {queue.map((q, i) => (
            <li key={i} className="ccx-chip ccx-chip--queued">
              <span className="ccx-chip__tag">{t("composer.queued")}</span>
              <span className="ccx-chip__text">{q.length > 80 ? `${q.slice(0, 80)}…` : q}</span>
              <button type="button" className="ccx-chip__x" aria-label={t("composer.removeQueued")} onClick={() => onDequeue(i)}>
                <X size={12} />
              </button>
            </li>
          ))}
        </ul>
      )}

      {popupOpen && (
        <ul
          id={listId}
          className="ccx-suggest"
          role="listbox"
          aria-label={popup?.kind === "slash" ? t("composer.commands") : t("composer.files")}
        >
          {suggestions.map((s, i) => (
            <li
              key={s.key}
              id={optionId(i)}
              role="option"
              aria-selected={i === activeIndex}
              className={clsx("ccx-suggest__item", i === activeIndex && "ccx-suggest__item--active")}
              onMouseEnter={() => setActive(i)}
              onMouseDown={(e) => {
                e.preventDefault();
                s.apply();
              }}
            >
              {popup?.kind === "mention" ? <FileCode2 size={13} aria-hidden /> : <Slash size={13} aria-hidden />}
              <span className="ccx-suggest__label">{s.label}</span>
              {s.detail && <span className="ccx-suggest__detail">{s.detail}</span>}
            </li>
          ))}
          {showEmptyFiles && suggestions.length === 0 && (
            <li className="ccx-suggest__empty" role="presentation">
              {t("composer.filesEmpty")}
            </li>
          )}
        </ul>
      )}

      {images.length > 0 && (
        <ul className="ccx-images" aria-label={t("composer.images")}>
          {images.map((img) => (
            <li key={img.id} className="ccx-image">
              <img src={`data:${img.mediaType};base64,${img.data}`} alt={img.name} />
              <button
                type="button"
                className="ccx-image__x"
                aria-label={t("composer.removeImage", { name: img.name })}
                onClick={() => setImages((prev) => prev.filter((i) => i.id !== img.id))}
              >
                <X size={11} />
              </button>
            </li>
          ))}
        </ul>
      )}

      {(attached || (liveSelection && !disabled)) && (
        <div className="ccx-attachments">
          {attached ? (
            <span className="ccx-chip ccx-chip--selection">
              <TextSelect size={12} aria-hidden />
              <span className="ccx-chip__text">
                {t("composer.attachedSelection", { path: attached.path, range: lineRange(attached) })}
              </span>
              <button type="button" className="ccx-chip__x" aria-label={t("composer.removeSelection")} onClick={() => setAttached(null)}>
                <X size={12} />
              </button>
            </span>
          ) : (
            liveSelection && (
              <button type="button" className="ccx-chip ccx-chip--action" onClick={() => setAttached(liveSelection)}>
                <TextSelect size={12} aria-hidden />
                <span className="ccx-chip__text">{t("composer.attachSelection")}</span>
                <span className="ccx-chip__meta">
                  {liveSelection.path}:{lineRange(liveSelection)}
                </span>
              </button>
            )
          )}
        </div>
      )}

      <div className="ccx-composer__box">
        <textarea
          ref={area}
          className="ccx-composer__input"
          rows={1}
          value={text}
          disabled={disabled}
          placeholder={running ? (texts?.placeholderBusy ?? t("composer.placeholderBusy")) : (texts?.placeholder ?? t("composer.placeholder"))}
          aria-label={texts?.label ?? t("composer.label")}
          role="combobox"
          aria-autocomplete="list"
          aria-expanded={popupOpen}
          aria-controls={popupOpen ? listId : undefined}
          aria-activedescendant={suggestions.length > 0 ? optionId(activeIndex) : undefined}
          onChange={(e) => {
            setText(e.target.value);
            setCaret(e.target.selectionStart);
            setDismissed(false);
            setActive(0);
          }}
          onSelect={(e) => setCaret(e.currentTarget.selectionStart)}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
        />
        <div className="ccx-composer__buttons">
          {allowImages && (
            <>
              <input
                ref={fileInput}
                type="file"
                accept={IMAGE_TYPES.join(",")}
                multiple
                hidden
                onChange={(e) => {
                  const files = Array.from(e.target.files ?? []);
                  e.target.value = "";
                  void addImages(files);
                }}
              />
              <IconButton
                label={t("composer.attachImage")}
                icon={<ImagePlus size={15} />}
                onClick={() => fileInput.current?.click()}
                disabled={disabled || running}
                tooltip="top"
              />
            </>
          )}
          {running && (
            <IconButton
              label={t("composer.interrupt")}
              icon={<Square size={14} />}
              variant="danger"
              onClick={onInterrupt}
              tooltip="top"
            />
          )}
          <IconButton
            label={running ? t("composer.queue") : t("composer.send")}
            icon={<SendHorizontal size={16} />}
            variant="solid"
            onClick={() => void send()}
            disabled={!canSend}
            tooltip="top"
          />
        </div>
      </div>

      {toolbar && <div className="ccx-composer__toolbar">{toolbar}</div>}

      {imageError && (
        <div className="ccx-composer__hint ccx-composer__hint--warn" role="alert">
          {imageError}
        </div>
      )}

      <div className={clsx("ccx-composer__hint", (disabled || tooLong || queueFull) && "ccx-composer__hint--warn")} role="status">
        {disabled ? (
          <>
            <Lock size={12} aria-hidden /> {disabledText}
          </>
        ) : tooLong ? (
          t("composer.tooLong", { count: MAX_MESSAGE_CHARS })
        ) : queueFull ? (
          (texts?.queueFull ?? t("composer.queueFull"))
        ) : running ? (
          t("composer.hintBusy")
        ) : (
          t("composer.hint")
        )}
      </div>
    </div>
  );
}
