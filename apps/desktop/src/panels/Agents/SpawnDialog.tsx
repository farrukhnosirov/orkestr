import type { AgentTemplate, ModelRef, SpawnAgentParams, Task, WorktreePolicy } from "@orkestr/protocol";
import { Button, Input } from "@orkestr/ui";
import { Bot, X } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";

import { ModelPicker } from "../../features/providers/ModelPicker";
import { useProvidersLive } from "../../features/providers/store";
import { useAppStore } from "../../store/app";
import { agentActions, errorMessage } from "./data";
import { roleLabel } from "./model";
import { useAgentsUi } from "./store";

const POLICIES: { id: WorktreePolicy; label: string; hint: string }[] = [
  { id: "DedicatedWorktree", label: "Dedicated worktree", hint: "Own branch agent/<role>-<id> (needs a git repo with a commit)" },
  { id: "SharedWorktree", label: "Shared checkout", hint: "Works directly in the project folder" },
  { id: "ReadOnlyMain", label: "Read-only", hint: "Reads the project, cannot change files" },
  { id: "None", label: "No files", hint: "Chat-only, no filesystem access" },
];

const ASSIGNABLE = new Set(["Planned", "Ready", "Failed", "Interrupted"]);

/** Spawn Agent dialog: template, task (existing or new), model override, worktree policy. */
export function SpawnDialog({
  projectId,
  templates,
  tasks,
}: {
  projectId: string;
  templates: AgentTemplate[];
  tasks: Task[];
}) {
  const open = useAgentsUi((s) => s.spawnOpen);
  const preselected = useAgentsUi((s) => s.spawnTemplateId);
  const close = useAgentsUi((s) => s.closeSpawn);
  const select = useAgentsUi((s) => s.select);
  const toast = useAgentsUi((s) => s.toast);
  const connected = useAppStore((s) => s.connection.state === "open");
  const { providers } = useProvidersLive();
  const [templateId, setTemplateId] = useState("");
  const [taskMode, setTaskMode] = useState<"new" | "existing">("new");
  const [taskId, setTaskId] = useState("");
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [model, setModel] = useState<ModelRef | null>(null);
  const [policy, setPolicy] = useState<WorktreePolicy | "">("");
  const [maxTurns, setMaxTurns] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const assignable = tasks.filter((t) => ASSIGNABLE.has(t.status));

  useEffect(() => {
    if (!open) return;
    setTemplateId(preselected ?? templates[0]?.id ?? "");
    setTaskMode(assignable.length > 0 ? "existing" : "new");
    setTaskId(assignable[0]?.id ?? "");
    setTitle("");
    setDescription("");
    setModel(null);
    setPolicy("");
    setMaxTurns("");
    setError(null);
    setBusy(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- reset only when the dialog opens
  }, [open]);

  if (!open) return null;
  const template = templates.find((t) => t.id === templateId);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!template) {
      setError("Choose a template.");
      return;
    }
    const params: SpawnAgentParams = { projectId, templateId: template.id };
    if (taskMode === "existing") {
      if (!taskId) {
        setError("Choose a task, or create a new one.");
        return;
      }
      params.taskId = taskId;
    } else {
      if (!title.trim()) {
        setError("Give the new task a title.");
        return;
      }
      params.newTask = description.trim() ? { title: title.trim(), description: description.trim() } : { title: title.trim() };
    }
    if (model?.providerId && model.model.trim()) params.model = { providerId: model.providerId, model: model.model.trim() };
    if (policy) params.worktreePolicy = policy;
    if (maxTurns.trim()) {
      const n = Number.parseInt(maxTurns, 10);
      if (!Number.isInteger(n) || n < 1 || n > 500) {
        setError("Max turns must be 1–500.");
        return;
      }
      params.maxTurns = n;
    }
    setBusy(true);
    setError(null);
    try {
      const r = await agentActions.spawn(params);
      select(r.agent.id);
      close();
      toast("success", `${r.agent.name} is working on “${r.task.title}”.`);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const dismiss = () => {
    if (!busy) close();
  };

  return (
    <div className="overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && dismiss()}>
      <form
        className="dialog agents-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="spawn-agent-title"
        onSubmit={(e) => void submit(e)}
        onKeyDown={(e) => e.key === "Escape" && dismiss()}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <Bot size={18} />
          </div>
          <div>
            <h2 id="spawn-agent-title" className="dialog__title">
              Spawn agent
            </h2>
            <p className="dialog__subtitle">The agent runs in isolation with its own worktree, budget and stop button.</p>
          </div>
          <button type="button" className="dialog__close" aria-label="Close" onClick={dismiss}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          <label className="ok-field">
            <span className="ok-field__label">Template</span>
            <select className="agents-select" value={templateId} onChange={(e) => setTemplateId(e.target.value)} aria-label="Template">
              {templates.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name} — {roleLabel(t.role)}
                  {t.source === "Builtin" ? " (official)" : ""}
                </option>
              ))}
            </select>
          </label>
          {template && <p className="agents-muted">{template.description}</p>}

          <fieldset className="ok-field agents-fieldset">
            <legend className="ok-field__label">Task</legend>
            <div className="agents-radio-row">
              <label>
                <input type="radio" checked={taskMode === "existing"} disabled={assignable.length === 0} onChange={() => setTaskMode("existing")} />{" "}
                Existing task
              </label>
              <label>
                <input type="radio" checked={taskMode === "new"} onChange={() => setTaskMode("new")} /> New task
              </label>
            </div>
            {taskMode === "existing" ? (
              <select className="agents-select" value={taskId} onChange={(e) => setTaskId(e.target.value)} aria-label="Existing task">
                {assignable.map((t) => (
                  <option key={t.id} value={t.id}>
                    {t.title} ({t.status})
                  </option>
                ))}
              </select>
            ) : (
              <>
                <Input label="Title" placeholder="e.g. Add CSV export endpoint" value={title} maxLength={200} onChange={(e) => setTitle(e.target.value)} />
                <label className="ok-field">
                  <span className="ok-field__label">Description</span>
                  <textarea className="agents-textarea" rows={3} value={description} onChange={(e) => setDescription(e.target.value)} />
                </label>
              </>
            )}
          </fieldset>

          <ModelPicker label="Model override (optional)" value={model} onChange={setModel} providers={providers} allowNone />

          <div className="agents-dialog__row">
            <label className="ok-field">
              <span className="ok-field__label">Worktree</span>
              <select className="agents-select" value={policy} onChange={(e) => setPolicy(e.target.value as WorktreePolicy | "")} aria-label="Worktree policy">
                <option value="">Template default{template ? ` (${POLICIES.find((p) => p.id === template.worktreePolicy)?.label ?? ""})` : ""}</option>
                {POLICIES.map((p) => (
                  <option key={p.id} value={p.id} title={p.hint}>
                    {p.label}
                  </option>
                ))}
              </select>
            </label>
            <label className="ok-field">
              <span className="ok-field__label">Max turns</span>
              <input
                className="agents-input"
                inputMode="numeric"
                value={maxTurns}
                placeholder={template ? String(template.maxTurns) : "—"}
                onChange={(e) => setMaxTurns(e.target.value.replace(/[^0-9]/g, ""))}
              />
            </label>
          </div>
          {error && (
            <p className="agents-error" role="alert">
              {error}
            </p>
          )}
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={dismiss} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" loading={busy} disabled={!connected || templates.length === 0}>
            Spawn agent
          </Button>
        </footer>
      </form>
    </div>
  );
}
