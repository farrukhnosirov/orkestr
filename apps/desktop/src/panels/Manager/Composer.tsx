/**
 * Message composer: Enter sends, Shift+Enter inserts a newline, Ctrl/⌘+Enter
 * always sends. Shows the attachment from "Ask Manager about selection" and,
 * in Expert mode (§53), per-request model / budget / parallelism / time
 * limit controls.
 */
import type { ChatAttachment, ManagerRequestOptions, ModelRef } from "@orkestr/protocol";
import { Button, IconButton, Kbd } from "@orkestr/ui";
import { FileCode, SendHorizontal, X } from "lucide-react";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";

import { ModelPicker, useProvidersLive } from "../../features/providers";
import { EMPTY_EXPERT, type ExpertFields, attachmentLabel, buildOptions, composerKeyAction } from "./model";

export interface ComposerProps {
  disabled: boolean;
  busy: boolean;
  expert: boolean;
  focusNonce: number;
  text: string;
  onText: (text: string) => void;
  attachment: ChatAttachment | null;
  onClearAttachment: () => void;
  onSend: (text: string, options: ManagerRequestOptions | undefined) => Promise<boolean>;
}

const MAX = 20_000;

export function Composer({ disabled, busy, expert, focusNonce, text, onText, attachment, onClearAttachment, onSend }: ComposerProps) {
  const area = useRef<HTMLTextAreaElement>(null);
  const [fields, setFields] = useState<ExpertFields>(EMPTY_EXPERT);
  const [model, setModel] = useState<ModelRef | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const { providers } = useProvidersLive();

  useEffect(() => {
    if (focusNonce > 0) area.current?.focus();
  }, [focusNonce]);

  // Auto-grow up to ~10 lines.
  useEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 220)}px`;
  }, [text]);

  const canSend = !disabled && !busy && !sending && text.trim().length > 0 && text.length <= MAX;

  const send = async () => {
    if (!canSend) return;
    let options: ManagerRequestOptions | undefined;
    if (expert) {
      const built = buildOptions(fields, model);
      if (built.error) {
        setError(built.error);
        return;
      }
      options = built.options;
    }
    setError(null);
    setSending(true);
    try {
      await onSend(text.trim(), options);
    } finally {
      setSending(false);
      area.current?.focus();
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    const action = composerKeyAction({ key: e.key, shiftKey: e.shiftKey, ctrlKey: e.ctrlKey, metaKey: e.metaKey, isComposing: e.nativeEvent.isComposing });
    if (action === "send") {
      e.preventDefault();
      void send();
    }
  };

  const field = (key: keyof ExpertFields, label: string, placeholder: string) => (
    <label className="mgr-expert__field">
      <span>{label}</span>
      <input
        className="mgr-input"
        inputMode="decimal"
        placeholder={placeholder}
        value={fields[key]}
        onChange={(e) => setFields((f) => ({ ...f, [key]: e.target.value }))}
      />
    </label>
  );

  return (
    <div className="mgr-composer">
      {expert && (
        <div className="mgr-expert" role="group" aria-label="Expert options for this request">
          <ModelPicker label="Model for this request" value={model} onChange={setModel} providers={providers} allowNone />
          {field("budgetUsd", "Budget $", "Manager default")}
          {field("maxParallelAgents", "Parallel agents", "default")}
          {field("timeLimitMins", "Time limit (min)", "none")}
        </div>
      )}
      {attachment && (
        <div className="mgr-attachment">
          <FileCode size={13} aria-hidden="true" />
          <span title={attachment.text.slice(0, 400)}>{attachmentLabel(attachment)}</span>
          <IconButton size="sm" label="Remove attachment" icon={<X size={12} />} onClick={onClearAttachment} />
        </div>
      )}
      <div className="mgr-composer__row">
        <textarea
          ref={area}
          className="mgr-composer__input"
          rows={1}
          value={text}
          maxLength={MAX}
          disabled={disabled}
          aria-label="Message to the AI Manager"
          placeholder={disabled ? "Open a project to talk to the Manager" : "Ask the Manager to build, fix or explain something…"}
          onChange={(e) => onText(e.target.value)}
          onKeyDown={onKeyDown}
        />
        <Button variant="primary" leftIcon={<SendHorizontal size={14} />} disabled={!canSend} loading={sending} onClick={() => void send()} aria-label="Send">
          Send
        </Button>
      </div>
      <div className="mgr-composer__hint">
        {error ? (
          <span className="mgr-error" role="alert">
            {error}
          </span>
        ) : busy ? (
          <span className="mgr-muted">The Manager is answering…</span>
        ) : (
          <span className="mgr-muted">
            <Kbd>Enter</Kbd> send · <Kbd>Shift</Kbd>+<Kbd>Enter</Kbd> new line · <Kbd>Ctrl</Kbd>+<Kbd>Enter</Kbd> send
          </span>
        )}
      </div>
    </div>
  );
}
