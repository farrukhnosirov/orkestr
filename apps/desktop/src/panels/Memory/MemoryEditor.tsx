import type { MemoryItem, MemoryKind, MemoryScope } from "@orkestr/protocol";
import { Button, Input } from "@orkestr/ui";
import { BrainCircuit, X } from "lucide-react";
import { type FormEvent, useEffect, useRef, useState } from "react";

import { errorMessage, requireClient } from "../../lib/rpcQuery";
import { KIND_INFO } from "./kinds";
import { Markdown } from "./markdown";

export interface MemoryEditorProps {
  /** Item to edit, or null to create. */
  item: MemoryItem | null;
  scope: MemoryScope;
  kinds: MemoryKind[];
  onClose: () => void;
  onSaved: (item: MemoryItem) => void;
}

/** Create / edit dialog for a memory item (`memory.create` / `memory.update`). */
export function MemoryEditor({ item, scope, kinds, onClose, onSaved }: MemoryEditorProps) {
  const [kind, setKind] = useState<MemoryKind>(item?.kind ?? kinds[0] ?? "Note");
  const [title, setTitle] = useState(item?.title ?? "");
  const [content, setContent] = useState(item?.content ?? "");
  const [tags, setTags] = useState((item?.tags ?? []).join(", "));
  const [key, setKey] = useState(item?.key ?? "");
  const [pinned, setPinned] = useState(item?.pinned ?? false);
  const [preview, setPreview] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    requestAnimationFrame(() => titleRef.current?.focus());
  }, []);

  const kindOptions = kinds.includes(kind) ? kinds : [kind, ...kinds];

  const close = () => {
    if (!busy) onClose();
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!title.trim()) {
      setError("Give the memory a title.");
      return;
    }
    setBusy(true);
    setError(null);
    const tagList = tags
      .split(/[,\n]/)
      .map((t) => t.trim())
      .filter(Boolean);
    try {
      const client = requireClient();
      const res = item
        ? await client.call("memory.update", {
            id: item.id,
            patch: { kind, title: title.trim(), content, tags: tagList, key: key.trim(), pinned },
          })
        : await client.call("memory.create", {
            scope,
            kind,
            title: title.trim(),
            content,
            tags: tagList,
            pinned,
            ...(key.trim() ? { key: key.trim() } : {}),
          });
      onSaved(res.item);
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  };

  return (
    <div className="overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <form
        className="dialog mem-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="mem-editor-title"
        onSubmit={(e) => void submit(e)}
        onKeyDown={(e) => e.key === "Escape" && close()}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <BrainCircuit size={18} />
          </div>
          <div>
            <h2 id="mem-editor-title" className="dialog__title">
              {item ? "Edit memory" : scope.scope === "Global" ? "New global memory" : "New project memory"}
            </h2>
            <p className="dialog__subtitle">Markdown is supported. Pinned items are always included in agent context.</p>
          </div>
          <button type="button" className="dialog__close" aria-label="Close" onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          <div className="mem-form-row">
            <label className="ok-field mem-field--kind">
              <span className="ok-field__label">Kind</span>
              <select className="mem-select" value={kind} onChange={(e) => setKind(e.target.value as MemoryKind)}>
                {kindOptions.map((k) => (
                  <option key={k} value={k}>
                    {KIND_INFO[k].label}
                  </option>
                ))}
              </select>
            </label>
            <Input
              ref={titleRef}
              className="mem-field--grow"
              label="Title"
              placeholder="e.g. Money is stored in integer cents"
              value={title}
              maxLength={200}
              onChange={(e) => setTitle(e.target.value)}
            />
          </div>
          <div className="ok-field">
            <div className="mem-field-head">
              <label className="ok-field__label" htmlFor="mem-content">
                Content
              </label>
              <button type="button" className="mem-link" onClick={() => setPreview((p) => !p)} aria-pressed={preview}>
                {preview ? "Edit" : "Preview"}
              </button>
            </div>
            {preview ? (
              <div className="mem-preview">{content.trim() ? <Markdown source={content} /> : <span className="mem-muted">Nothing to preview.</span>}</div>
            ) : (
              <textarea
                id="mem-content"
                className="mem-textarea"
                rows={8}
                value={content}
                placeholder="Details, rationale, examples…"
                onChange={(e) => setContent(e.target.value)}
              />
            )}
          </div>
          <div className="mem-form-row">
            <Input className="mem-field--grow" label="Tags" placeholder="money, rust" value={tags} onChange={(e) => setTags(e.target.value)} />
            <Input
              className="mem-field--grow"
              label="Rule key"
              hint="Project items override global items with the same key."
              placeholder="optional, e.g. indent"
              value={key}
              maxLength={120}
              onChange={(e) => setKey(e.target.value)}
            />
          </div>
          <label className="mem-check">
            <input type="checkbox" checked={pinned} onChange={(e) => setPinned(e.target.checked)} />
            Pin (always include in agent context)
          </label>
          {error && (
            <div className="ok-field__error" role="alert">
              {error}
            </div>
          )}
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={close} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" loading={busy}>
            {item ? "Save" : "Create"}
          </Button>
        </footer>
      </form>
    </div>
  );
}
