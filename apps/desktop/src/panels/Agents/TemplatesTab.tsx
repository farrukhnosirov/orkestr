import type { AgentRole, AgentTemplate, MemoryAccess, ModelRef, QualityGate, TemplateDraft, WorktreePolicy } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { Bot, Copy, Download, FileUp, LayoutTemplate, Lock, Pencil, Plus, RefreshCw, Trash2, X } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";

import { ModelPicker } from "../../features/providers/ModelPicker";
import { useProvidersLive } from "../../features/providers/store";
import { downloadText, errorMessage, templateActions } from "./data";
import { ALL_TOOLS, BUILTIN_ROLES, blankDraft, draftOf, duplicateDraft, isBuiltin, roleLabel, validateDraft } from "./model";
import { RoleAvatar } from "./parts";
import { useAgentsUi } from "./store";

const GATES: QualityGate[] = ["Build", "Lint", "Typecheck", "Tests", "Security", "Review", "AcceptanceCriteria"];
const POLICIES: WorktreePolicy[] = ["DedicatedWorktree", "SharedWorktree", "ReadOnlyMain", "None"];
const MEMORY: MemoryAccess[] = ["None", "ProjectOnly", "ProjectAndGlobal", "Full"];

function TemplateCard({ t, onChanged }: { t: AgentTemplate; onChanged: () => void }) {
  const ui = useAgentsUi();
  const builtin = isBuiltin(t);
  const run = async (fn: () => Promise<void>) => {
    try {
      await fn();
    } catch (e) {
      ui.toast("danger", errorMessage(e));
    }
  };
  return (
    <li className="agents-tpl">
      <RoleAvatar role={t.role} />
      <div className="agents-tpl__main">
        <div className="agents-tpl__top">
          <strong>{t.name}</strong>
          <Badge tone={builtin ? "accent" : "neutral"} icon={builtin ? <Lock size={11} /> : undefined}>
            {builtin ? "Official" : t.source}
          </Badge>
          <span className="agents-muted">v{t.version}</span>
        </div>
        <p className="agents-tpl__desc">{t.description || "No description."}</p>
        <div className="agents-tpl__meta">
          <span>{roleLabel(t.role)}</span>
          <span>{t.tools.length} tools</span>
          <span>{t.maxTurns} turns</span>
          <span>{t.worktreePolicy}</span>
          {t.budget?.maxCostUsd != null && <span>${t.budget.maxCostUsd.toFixed(2)} budget</span>}
          {t.preferredModel && <span>{t.preferredModel.model}</span>}
        </div>
      </div>
      <div className="agents-tpl__actions">
        <IconButton label="Spawn from template" icon={<Bot size={15} />} onClick={() => ui.openSpawn(t.id)} tooltip="bottom" />
        {!builtin && (
          <IconButton label="Edit template" icon={<Pencil size={15} />} onClick={() => ui.setEditor({ mode: "edit", template: t })} tooltip="bottom" />
        )}
        <IconButton
          label="Duplicate template"
          icon={<Copy size={15} />}
          onClick={() => ui.setEditor({ mode: "create", draft: duplicateDraft(t) })}
          tooltip="bottom"
        />
        <IconButton
          label="Export template"
          icon={<Download size={15} />}
          onClick={() =>
            void run(async () => {
              const r = await templateActions.exportJson(t.id);
              downloadText(r.fileName, r.json);
              ui.toast("success", `Exported ${r.fileName}.`);
            })
          }
          tooltip="bottom"
        />
        {!builtin && (
          <IconButton
            label="Delete template"
            icon={<Trash2 size={15} />}
            onClick={() =>
              void run(async () => {
                if (!window.confirm(`Delete the template “${t.name}”? Running agents are not affected.`)) return;
                await templateActions.remove(t.id);
                ui.toast("info", `Template “${t.name}” deleted.`);
                onChanged();
              })
            }
            tooltip="bottom"
          />
        )}
      </div>
    </li>
  );
}

function toggle<T>(list: readonly T[], item: T): T[] {
  return list.includes(item) ? list.filter((x) => x !== item) : [...list, item];
}

function TemplateEditor({ onSaved }: { onSaved: () => void }) {
  const editor = useAgentsUi((s) => s.editor);
  const setEditor = useAgentsUi((s) => s.setEditor);
  const toast = useAgentsUi((s) => s.toast);
  const { providers } = useProvidersLive();
  const [d, setD] = useState<TemplateDraft>(blankDraft());
  const [customRole, setCustomRole] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!editor) return;
    const draft = editor.mode === "edit" ? draftOf(editor.template) : editor.draft;
    setD(draft);
    setCustomRole(typeof draft.role === "object" ? draft.role.Custom : "");
    setError(null);
    setBusy(false);
  }, [editor]);

  if (!editor) return null;
  const set = (patch: Partial<TemplateDraft>) => setD((cur) => ({ ...cur, ...patch }));
  const roleValue = typeof d.role === "object" ? "__custom" : d.role;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const draft: TemplateDraft = typeof d.role === "object" ? { ...d, role: { Custom: customRole.trim() } } : d;
    const problem = validateDraft(draft);
    if (problem) {
      setError(problem);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const t = editor.mode === "edit" ? await templateActions.update(editor.template.id, draft) : await templateActions.create(draft);
      toast("success", `Template “${t.name}” saved.`);
      setEditor(null);
      onSaved();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const close = () => {
    if (!busy) setEditor(null);
  };
  const budget = d.budget ?? { maxCostUsd: null, maxTokens: null, maxDurationSecs: null, period: "PerTask" as const, warnAt: 0.8 };

  return (
    <div className="overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <form
        className="dialog agents-dialog agents-dialog--wide"
        role="dialog"
        aria-modal="true"
        aria-labelledby="tpl-editor-title"
        onSubmit={(e) => void submit(e)}
        onKeyDown={(e) => e.key === "Escape" && close()}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <LayoutTemplate size={18} />
          </div>
          <div>
            <h2 id="tpl-editor-title" className="dialog__title">
              {editor.mode === "edit" ? "Edit template" : "New template"}
            </h2>
            <p className="dialog__subtitle">Requested permissions become agent-scoped rules when the agent is spawned.</p>
          </div>
          <button type="button" className="dialog__close" aria-label="Close" onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body agents-editor">
          <div className="agents-dialog__row">
            <Input label="Name" value={d.name} maxLength={80} onChange={(e) => set({ name: e.target.value })} />
            <label className="ok-field">
              <span className="ok-field__label">Role</span>
              <select
                className="agents-select"
                aria-label="Role"
                value={roleValue}
                onChange={(e) => set({ role: e.target.value === "__custom" ? { Custom: customRole || "Custom" } : (e.target.value as AgentRole) })}
              >
                {BUILTIN_ROLES.map((r) => (
                  <option key={r} value={r}>
                    {roleLabel(r)}
                  </option>
                ))}
                <option value="__custom">Custom…</option>
              </select>
            </label>
            {roleValue === "__custom" && <Input label="Custom role" value={customRole} maxLength={40} onChange={(e) => setCustomRole(e.target.value)} />}
            <Input label="Version" value={d.version} onChange={(e) => set({ version: e.target.value })} />
          </div>
          <Input label="Description" value={d.description} maxLength={500} onChange={(e) => set({ description: e.target.value })} />
          <label className="ok-field">
            <span className="ok-field__label">System prompt — variables: {"{{project.name}} {{task.title}} {{agent.role}}"}</span>
            <textarea className="agents-textarea agents-textarea--tall" rows={8} value={d.systemPrompt} onChange={(e) => set({ systemPrompt: e.target.value })} />
          </label>
          <fieldset className="ok-field agents-fieldset">
            <legend className="ok-field__label">Tools (allow-list)</legend>
            <div className="agents-checks">
              {ALL_TOOLS.map((t) => (
                <label key={t}>
                  <input type="checkbox" checked={d.tools.includes(t)} disabled={t === "submit_result"} onChange={() => set({ tools: toggle(d.tools, t) })} /> {t}
                </label>
              ))}
            </div>
          </fieldset>
          <div className="agents-dialog__row">
            <label className="ok-field">
              <span className="ok-field__label">Worktree policy</span>
              <select className="agents-select" aria-label="Worktree policy" value={d.worktreePolicy} onChange={(e) => set({ worktreePolicy: e.target.value as WorktreePolicy })}>
                {POLICIES.map((p) => (
                  <option key={p} value={p}>
                    {p}
                  </option>
                ))}
              </select>
            </label>
            <label className="ok-field">
              <span className="ok-field__label">Memory</span>
              <select className="agents-select" aria-label="Memory access" value={d.memoryAccess} onChange={(e) => set({ memoryAccess: e.target.value as MemoryAccess })}>
                {MEMORY.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            </label>
            <label className="ok-field">
              <span className="ok-field__label">Max turns</span>
              <input
                className="agents-input"
                inputMode="numeric"
                value={String(d.maxTurns)}
                onChange={(e) => set({ maxTurns: Number.parseInt(e.target.value.replace(/[^0-9]/g, "") || "0", 10) })}
              />
            </label>
            <label className="ok-field">
              <span className="ok-field__label">Budget (USD / run)</span>
              <input
                className="agents-input"
                inputMode="decimal"
                value={budget.maxCostUsd ?? ""}
                placeholder="unlimited"
                onChange={(e) => {
                  const v = e.target.value.trim();
                  const n = v ? Number.parseFloat(v) : null;
                  set({ budget: n === null && budget.maxDurationSecs === null ? null : { ...budget, maxCostUsd: n !== null && Number.isFinite(n) ? n : null } });
                }}
              />
            </label>
            <label className="ok-field">
              <span className="ok-field__label">Time limit (min)</span>
              <input
                className="agents-input"
                inputMode="numeric"
                value={budget.maxDurationSecs ? String(Math.round(budget.maxDurationSecs / 60)) : ""}
                placeholder="60"
                onChange={(e) => {
                  const v = Number.parseInt(e.target.value.replace(/[^0-9]/g, ""), 10);
                  const secs = Number.isFinite(v) && v > 0 ? v * 60 : null;
                  set({ budget: secs === null && budget.maxCostUsd === null ? null : { ...budget, maxDurationSecs: secs } });
                }}
              />
            </label>
          </div>
          <fieldset className="ok-field agents-fieldset">
            <legend className="ok-field__label">Quality gates</legend>
            <div className="agents-checks">
              {GATES.map((g) => (
                <label key={g}>
                  <input type="checkbox" checked={d.qualityGates.includes(g)} onChange={() => set({ qualityGates: toggle(d.qualityGates, g) })} /> {g}
                </label>
              ))}
            </div>
          </fieldset>
          <ModelPicker label="Preferred model" value={d.preferredModel} onChange={(m: ModelRef | null) => set({ preferredModel: m })} providers={providers} allowNone />
          <div className="agents-dialog__row">
            <Input
              label="Robot / avatar id"
              value={d.avatar?.robot ?? ""}
              placeholder="orbit-backend"
              onChange={(e) => {
                const robot = e.target.value;
                set({ avatar: robot.trim() ? { robot, accent: d.avatar?.accent ?? null, scale: d.avatar?.scale ?? 1, floating: d.avatar?.floating ?? true } : null });
              }}
            />
            <Input
              label="Voice (provider:voice)"
              value={d.voice ? `${d.voice.provider}:${d.voice.voice}` : ""}
              placeholder="system:default"
              onChange={(e) => {
                const [provider, voice] = e.target.value.split(":");
                set({ voice: provider && voice ? { provider, voice, language: d.voice?.language ?? "en-US", style: null } : null });
              }}
            />
          </div>
          {d.permissions.length > 0 && (
            <p className="agents-muted">
              Requested permissions: {d.permissions.map((p) => `${p.capability} ${p.effect}`).join(", ")}
            </p>
          )}
          {error && (
            <p className="agents-error" role="alert">
              {error}
            </p>
          )}
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={close} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" loading={busy}>
            Save template
          </Button>
        </footer>
      </form>
    </div>
  );
}

function ImportDialog({ onImported }: { onImported: () => void }) {
  const open = useAgentsUi((s) => s.importOpen);
  const setOpen = useAgentsUi((s) => s.setImportOpen);
  const toast = useAgentsUi((s) => s.toast);
  const [json, setJson] = useState("");
  const [accept, setAccept] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (open) {
      setJson("");
      setAccept(false);
      setError(null);
    }
  }, [open]);
  if (!open) return null;
  const close = () => !busy && setOpen(false);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!json.trim()) {
      setError("Paste a template file or choose one.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const r = await templateActions.importJson(json, accept);
      const dropped = r.droppedPermissions.length;
      toast("success", `Imported “${r.template.name}”${dropped ? ` (${dropped} requested permission(s) not accepted)` : ""}.`);
      setOpen(false);
      onImported();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <form className="dialog agents-dialog" role="dialog" aria-modal="true" aria-labelledby="tpl-import-title" onSubmit={(e) => void submit(e)}>
        <header className="dialog__header">
          <div className="dialog__icon">
            <FileUp size={18} />
          </div>
          <div>
            <h2 id="tpl-import-title" className="dialog__title">
              Import template
            </h2>
            <p className="dialog__subtitle">Orkestr agent template (.agent.json). It is added as a custom template.</p>
          </div>
          <button type="button" className="dialog__close" aria-label="Close" onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          <input
            type="file"
            accept=".json,application/json"
            aria-label="Template file"
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (f) void f.text().then(setJson);
            }}
          />
          <label className="ok-field">
            <span className="ok-field__label">Template JSON</span>
            <textarea className="agents-textarea agents-textarea--tall" rows={10} value={json} onChange={(e) => setJson(e.target.value)} />
          </label>
          <label className="agents-check">
            <input type="checkbox" checked={accept} onChange={(e) => setAccept(e.target.checked)} /> Accept the permissions requested by this template
          </label>
          {error && (
            <p className="agents-error" role="alert">
              {error}
            </p>
          )}
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={close} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" loading={busy}>
            Import
          </Button>
        </footer>
      </form>
    </div>
  );
}

export function TemplatesTab({ templates, loading, error, reload }: { templates: AgentTemplate[]; loading: boolean; error: string | null; reload: () => void }) {
  const setEditor = useAgentsUi((s) => s.setEditor);
  const setImportOpen = useAgentsUi((s) => s.setImportOpen);
  let body: JSX.Element;
  if (loading && templates.length === 0) {
    body = (
      <div className="agents-loading">
        <Spinner size={16} /> Loading templates…
      </div>
    );
  } else if (error) {
    body = (
      <EmptyState
        title="Could not load templates"
        description={error}
        action={
          <Button leftIcon={<RefreshCw size={14} />} onClick={reload}>
            Retry
          </Button>
        }
      />
    );
  } else if (templates.length === 0) {
    body = <EmptyState title="No templates" description="Create or import an agent template." />;
  } else {
    body = (
      <ul className="agents-tpls" aria-label="Agent templates">
        {templates.map((t) => (
          <TemplateCard key={t.id} t={t} onChanged={reload} />
        ))}
      </ul>
    );
  }
  return (
    <div className="agents-templates">
      <div className="agents-toolbar">
        <Button size="sm" variant="primary" leftIcon={<Plus size={14} />} onClick={() => setEditor({ mode: "create", draft: blankDraft() })}>
          New template
        </Button>
        <Button size="sm" leftIcon={<FileUp size={14} />} onClick={() => setImportOpen(true)}>
          Import
        </Button>
        <div className="agents-toolbar__spacer" />
        <IconButton label="Reload templates" icon={<RefreshCw size={14} />} onClick={reload} tooltip="bottom" />
      </div>
      <div className="agents-scroll">{body}</div>
      <TemplateEditor onSaved={reload} />
      <ImportDialog onImported={reload} />
    </div>
  );
}
