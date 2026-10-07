import type { CreateTaskParams, Task, TaskPriority } from "@orkestr/protocol";
import { Button, Input } from "@orkestr/ui";
import { ListPlus, X } from "lucide-react";
import { type FormEvent, useEffect, useRef, useState } from "react";

import { useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { errorMessage, taskActions } from "./data";
import { PRIORITIES } from "./model";
import { useTasksUi } from "./store";
import { Overlay } from "../../components/Overlay";

/** Modal to create a task via `tasks.create`. */
export function CreateTaskDialog({ projectId, tasks }: { projectId: string; tasks: Task[] }) {
  const { t } = useT(["tasks", "common"]);
  const open = useTasksUi((s) => s.createOpen);
  const setOpen = useTasksUi((s) => s.setCreateOpen);
  const select = useTasksUi((s) => s.select);
  const toast = useTasksUi((s) => s.toast);
  const connected = useAppStore((s) => s.connection.state === "open");
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [criteria, setCriteria] = useState("");
  const [priority, setPriority] = useState<TaskPriority>("Normal");
  const [parentId, setParentId] = useState("");
  const [deps, setDeps] = useState<string[]>([]);
  const [estimate, setEstimate] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const titleRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setTitle("");
      setDescription("");
      setCriteria("");
      setPriority("Normal");
      setParentId("");
      setDeps([]);
      setEstimate("");
      setError(null);
      setBusy(false);
      requestAnimationFrame(() => titleRef.current?.focus());
    }
  }, [open]);

  if (!open) return null;

  const close = () => {
    if (!busy) setOpen(false);
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const name = title.trim();
    if (!name) {
      setError(t("createDialog.needTitle"));
      return;
    }
    const minutes = estimate.trim() ? Number.parseInt(estimate, 10) : undefined;
    if (minutes !== undefined && (!Number.isFinite(minutes) || minutes <= 0)) {
      setError(t("createDialog.estimateInvalid"));
      return;
    }
    const params: CreateTaskParams = { projectId, title: name, priority };
    if (description.trim()) params.description = description.trim();
    const lines = criteria
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);
    if (lines.length > 0) params.acceptanceCriteria = lines;
    if (parentId) params.parentId = parentId;
    if (deps.length > 0) params.dependsOn = deps;
    if (minutes !== undefined) params.estimateMinutes = minutes;
    setBusy(true);
    setError(null);
    try {
      const task = await taskActions.create(params);
      setOpen(false);
      select(task.id);
      toast("success", t("createDialog.created", { title: task.title }));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Overlay onDismiss={close}>
      <form
        className="dialog tasks-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="create-task-title"
        onSubmit={(e) => void submit(e)}
        onKeyDown={(e) => e.key === "Escape" && close()}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <ListPlus size={18} />
          </div>
          <div>
            <h2 id="create-task-title" className="dialog__title">
              {t("new")}
            </h2>
            <p className="dialog__subtitle">{t("createDialog.subtitle")}</p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          <Input
            ref={titleRef}
            label={t("createDialog.title")}
            placeholder={t("createDialog.titlePlaceholder")}
            value={title}
            maxLength={200}
            onChange={(e) => setTitle(e.target.value)}
            error={error ?? undefined}
          />
          <label className="ok-field">
            <span className="ok-field__label">{t("createDialog.description")}</span>
            <textarea
              className="tasks-textarea"
              rows={3}
              value={description}
              placeholder={t("createDialog.descriptionPlaceholder")}
              onChange={(e) => setDescription(e.target.value)}
            />
          </label>
          <label className="ok-field">
            <span className="ok-field__label">{t("createDialog.criteria")}</span>
            <textarea
              className="tasks-textarea"
              rows={3}
              value={criteria}
              placeholder={t("createDialog.criteriaPlaceholder")}
              onChange={(e) => setCriteria(e.target.value)}
            />
          </label>
          <div className="tasks-dialog__row">
            <label className="ok-field">
              <span className="ok-field__label">{t("createDialog.priority")}</span>
              <select className="tasks-select" value={priority} onChange={(e) => setPriority(e.target.value as TaskPriority)}>
                {PRIORITIES.map((p) => (
                  <option key={p} value={p}>
                    {t(`priority.${p}`)}
                  </option>
                ))}
              </select>
            </label>
            <label className="ok-field">
              <span className="ok-field__label">{t("createDialog.estimate")}</span>
              <input
                className="tasks-input"
                inputMode="numeric"
                value={estimate}
                placeholder="—"
                onChange={(e) => setEstimate(e.target.value.replace(/[^0-9]/g, ""))}
              />
            </label>
            <label className="ok-field">
              <span className="ok-field__label">{t("createDialog.parent")}</span>
              <select className="tasks-select" value={parentId} onChange={(e) => setParentId(e.target.value)}>
                <option value="">{t("common:state.none")}</option>
                {tasks.map((x) => (
                  <option key={x.id} value={x.id}>
                    {x.title}
                  </option>
                ))}
              </select>
            </label>
          </div>
          {tasks.length > 0 && (
            <fieldset className="ok-field tasks-dialog__deps">
              <legend className="ok-field__label">{t("createDialog.dependsOn")}</legend>
              <div className="tasks-dialog__deplist">
                {tasks.map((x) => (
                  <label key={x.id} className="tasks-dialog__dep">
                    <input
                      type="checkbox"
                      checked={deps.includes(x.id)}
                      onChange={(e) => setDeps((d) => (e.target.checked ? [...d, x.id] : d.filter((id) => id !== x.id)))}
                    />
                    <span>{x.title}</span>
                    <span className="tasks-muted">{t(`common:status.task.${x.status}`)}</span>
                  </label>
                ))}
              </div>
            </fieldset>
          )}
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={close} disabled={busy}>
            {t("common:actions.cancel")}
          </Button>
          <Button variant="primary" type="submit" loading={busy} disabled={!connected}>
            {t("create")}
          </Button>
        </footer>
      </form>
    </Overlay>
  );
}
