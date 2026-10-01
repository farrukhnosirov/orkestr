import { OrkestrError } from "@orkestr/client";
import { Button, Input } from "@orkestr/ui";
import { clsx } from "clsx";
import { FolderPlus, X } from "lucide-react";
import { type FormEvent, useEffect, useRef, useState } from "react";

import { createProject } from "../core/client";
import { useAppStore } from "../store/app";

/** Accent swatches offered for new projects (stored as `#RRGGBB` on the project). */
export const PROJECT_COLORS = ["#22d3ee", "#a78bfa", "#f472b6", "#34d399", "#fbbf24", "#60a5fa", "#fb7185"] as const;

/** Modal to create a project via `projects.create`. */
export function CreateProjectDialog() {
  const open = useAppStore((s) => s.createProjectOpen);
  const setOpen = useAppStore((s) => s.setCreateProjectOpen);
  const connected = useAppStore((s) => s.connection.state === "open");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [color, setColor] = useState<string>(PROJECT_COLORS[0]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setName("");
      setDescription("");
      setError(null);
      setBusy(false);
      requestAnimationFrame(() => nameRef.current?.focus());
    }
  }, [open]);

  if (!open) return null;

  const close = () => {
    if (!busy) setOpen(false);
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      setError("Give the project a name.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await createProject({ name: trimmed, description: description.trim(), color });
      setOpen(false);
    } catch (err) {
      setError(err instanceof OrkestrError ? err.message : "Could not create the project.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <form
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="create-project-title"
        onSubmit={(e) => void submit(e)}
        onKeyDown={(e) => e.key === "Escape" && close()}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <FolderPlus size={18} />
          </div>
          <div>
            <h2 id="create-project-title" className="dialog__title">
              New project
            </h2>
            <p className="dialog__subtitle">Projects hold repositories, agents, tasks, memory and budgets.</p>
          </div>
          <button type="button" className="dialog__close" aria-label="Close" onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          <Input
            ref={nameRef}
            label="Name"
            placeholder="e.g. Payroll Platform"
            value={name}
            maxLength={120}
            onChange={(e) => setName(e.target.value)}
            error={error ?? undefined}
          />
          <Input
            label="Description"
            placeholder="Optional — what is this project about?"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
          <div className="ok-field">
            <span className="ok-field__label">Accent</span>
            <div className="swatches" role="radiogroup" aria-label="Project accent color">
              {PROJECT_COLORS.map((c) => (
                <button
                  key={c}
                  type="button"
                  role="radio"
                  aria-checked={c === color}
                  aria-label={c}
                  className={clsx("swatch", c === color && "swatch--active")}
                  style={{ ["--swatch" as string]: c }}
                  onClick={() => setColor(c)}
                />
              ))}
            </div>
          </div>
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={close} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" loading={busy} disabled={!connected}>
            Create project
          </Button>
        </footer>
      </form>
    </div>
  );
}
