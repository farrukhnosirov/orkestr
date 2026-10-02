import type { AgentTemplate, ModelRef, SpawnAgentParams, Task, WorktreePolicy } from "@orkestr/protocol";
import { Button, Input } from "@orkestr/ui";
import { Bot, X } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";

import { ModelPicker } from "../../features/providers/ModelPicker";
import { useProvidersLive } from "../../features/providers/store";
import { useT } from "../../i18n";
import { useAppStore } from "../../store/app";
import { agentActions, errorMessage } from "./data";
import { roleLabel } from "./model";
import { useAgentsUi } from "./store";
import { agentName, templateDescription, templateName } from "../../lib/coreText";

const POLICIES: readonly WorktreePolicy[] = ["DedicatedWorktree", "SharedWorktree", "ReadOnlyMain", "None"];

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
  const { t } = useT(["agents", "common"]);
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
  const assignable = tasks.filter((x) => ASSIGNABLE.has(x.status));

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
  const template = templates.find((x) => x.id === templateId);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!template) {
      setError(t("spawnDialog.chooseTemplate"));
      return;
    }
    const params: SpawnAgentParams = { projectId, templateId: template.id };
    if (taskMode === "existing") {
      if (!taskId) {
        setError(t("spawnDialog.chooseTask"));
        return;
      }
      params.taskId = taskId;
    } else {
      if (!title.trim()) {
        setError(t("spawnDialog.needTitle"));
        return;
      }
      params.newTask = description.trim() ? { title: title.trim(), description: description.trim() } : { title: title.trim() };
    }
    if (model?.providerId && model.model.trim()) params.model = { providerId: model.providerId, model: model.model.trim() };
    if (policy) params.worktreePolicy = policy;
    if (maxTurns.trim()) {
      const n = Number.parseInt(maxTurns, 10);
      if (!Number.isInteger(n) || n < 1 || n > 500) {
        setError(t("spawnDialog.maxTurnsRange"));
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
      toast("success", t("spawnDialog.started", { name: agentName(r.agent.name), task: r.task.title }));
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
              {t("spawn")}
            </h2>
            <p className="dialog__subtitle">{t("spawnDialog.subtitle")}</p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={dismiss}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          <label className="ok-field">
            <span className="ok-field__label">{t("spawnDialog.template")}</span>
            <select className="agents-select" value={templateId} onChange={(e) => setTemplateId(e.target.value)} aria-label={t("spawnDialog.template")}>
              {templates.map((x) => (
                <option key={x.id} value={x.id}>
                  {t(x.source === "Builtin" ? "spawnDialog.templateOptionOfficial" : "spawnDialog.templateOption", {
                    name: templateName(x.name),
                    role: roleLabel(x.role),
                  })}
                </option>
              ))}
            </select>
          </label>
          {template && <p className="agents-muted">{templateDescription(template.name, template.description)}</p>}

          <fieldset className="ok-field agents-fieldset">
            <legend className="ok-field__label">{t("spawnDialog.task")}</legend>
            <div className="agents-radio-row">
              <label>
                <input type="radio" checked={taskMode === "existing"} disabled={assignable.length === 0} onChange={() => setTaskMode("existing")} />{" "}
                {t("spawnDialog.existingTask")}
              </label>
              <label>
                <input type="radio" checked={taskMode === "new"} onChange={() => setTaskMode("new")} /> {t("spawnDialog.newTask")}
              </label>
            </div>
            {taskMode === "existing" ? (
              <select className="agents-select" value={taskId} onChange={(e) => setTaskId(e.target.value)} aria-label={t("spawnDialog.existingTask")}>
                {assignable.map((x) => (
                  <option key={x.id} value={x.id}>
                    {t("spawnDialog.taskOption", { title: x.title, status: t(`common:status.task.${x.status}`) })}
                  </option>
                ))}
              </select>
            ) : (
              <>
                <Input
                  label={t("spawnDialog.title")}
                  placeholder={t("spawnDialog.titlePlaceholder")}
                  value={title}
                  maxLength={200}
                  onChange={(e) => setTitle(e.target.value)}
                />
                <label className="ok-field">
                  <span className="ok-field__label">{t("spawnDialog.description")}</span>
                  <textarea className="agents-textarea" rows={3} value={description} onChange={(e) => setDescription(e.target.value)} />
                </label>
              </>
            )}
          </fieldset>

          <ModelPicker label={t("spawnDialog.model")} value={model} onChange={setModel} providers={providers} allowNone />

          <div className="agents-dialog__row">
            <label className="ok-field">
              <span className="ok-field__label">{t("spawnDialog.worktree")}</span>
              <select
                className="agents-select"
                value={policy}
                onChange={(e) => setPolicy(e.target.value as WorktreePolicy | "")}
                aria-label={t("spawnDialog.worktreeLabel")}
              >
                <option value="">
                  {template
                    ? t("spawnDialog.templateDefaultWith", { policy: t(`worktreePolicy.${template.worktreePolicy}`) })
                    : t("spawnDialog.templateDefault")}
                </option>
                {POLICIES.map((p) => (
                  <option key={p} value={p} title={t(`worktreePolicyHint.${p}`)}>
                    {t(`worktreePolicy.${p}`)}
                  </option>
                ))}
              </select>
            </label>
            <label className="ok-field">
              <span className="ok-field__label">{t("spawnDialog.maxTurns")}</span>
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
            {t("common:actions.cancel")}
          </Button>
          <Button variant="primary" type="submit" loading={busy} disabled={!connected || templates.length === 0}>
            {t("spawn")}
          </Button>
        </footer>
      </form>
    </div>
  );
}
