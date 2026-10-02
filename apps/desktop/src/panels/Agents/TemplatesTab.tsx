import type { AgentRole, AgentTemplate, MemoryAccess, ModelRef, QualityGate, TemplateDraft, WorktreePolicy } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { Bot, Copy, Download, FileUp, LayoutTemplate, Lock, Pencil, Plus, RefreshCw, Trash2, X } from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";

import { ModelPicker } from "../../features/providers/ModelPicker";
import { useFormat, useT } from "../../i18n";
import { useProvidersLive } from "../../features/providers/store";
import { downloadText, errorMessage, templateActions } from "./data";
import { ALL_TOOLS, BUILTIN_ROLES, blankDraft, draftOf, duplicateDraft, isBuiltin, roleLabel, validateDraft } from "./model";
import { RoleAvatar } from "./parts";
import { useAgentsUi } from "./store";
import { templateDescription, templateName } from "../../lib/coreText";

const GATES: QualityGate[] = ["Build", "Lint", "Typecheck", "Tests", "Security", "Review", "AcceptanceCriteria"];
const POLICIES: WorktreePolicy[] = ["DedicatedWorktree", "SharedWorktree", "ReadOnlyMain", "None"];
const MEMORY: MemoryAccess[] = ["None", "ProjectOnly", "ProjectAndGlobal", "Full"];
/** Variables available in system prompts (shown as is in every language). */
const TEMPLATE_VARS = "{{project.name}} {{task.title}} {{agent.role}}";

function TemplateCard({ tpl, onChanged }: { tpl: AgentTemplate; onChanged: () => void }) {
  const { t } = useT(["agents", "common"]);
  const fmt = useFormat();
  const ui = useAgentsUi();
  const builtin = isBuiltin(tpl);
  const run = async (fn: () => Promise<void>) => {
    try {
      await fn();
    } catch (e) {
      ui.toast("danger", errorMessage(e));
    }
  };
  return (
    <li className="agents-tpl">
      <RoleAvatar role={tpl.role} />
      <div className="agents-tpl__main">
        <div className="agents-tpl__top">
          <strong>{templateName(tpl.name)}</strong>
          <Badge tone={builtin ? "accent" : "neutral"} icon={builtin ? <Lock size={11} /> : undefined}>
            {t(`source.${tpl.source}`)}
          </Badge>
          <span className="agents-muted">{t("templates.version", { version: tpl.version })}</span>
        </div>
        <p className="agents-tpl__desc">{templateDescription(tpl.name, tpl.description) || t("templates.noDescription")}</p>
        <div className="agents-tpl__meta">
          <span>{roleLabel(tpl.role)}</span>
          <span>{t("templates.tools", { count: tpl.tools.length })}</span>
          <span>{t("common:count.turns", { count: tpl.maxTurns })}</span>
          <span>{t(`worktreePolicy.${tpl.worktreePolicy}`)}</span>
          {tpl.budget?.maxCostUsd != null && <span>{t("templates.budget", { amount: fmt.currency(tpl.budget.maxCostUsd) })}</span>}
          {tpl.preferredModel && <span>{tpl.preferredModel.model}</span>}
        </div>
      </div>
      <div className="agents-tpl__actions">
        <IconButton label={t("templates.spawnFrom")} icon={<Bot size={15} />} onClick={() => ui.openSpawn(tpl.id)} tooltip="bottom" />
        {!builtin && (
          <IconButton label={t("templates.edit")} icon={<Pencil size={15} />} onClick={() => ui.setEditor({ mode: "edit", template: tpl })} tooltip="bottom" />
        )}
        <IconButton
          label={t("templates.duplicate")}
          icon={<Copy size={15} />}
          onClick={() => ui.setEditor({ mode: "create", draft: duplicateDraft(tpl) })}
          tooltip="bottom"
        />
        <IconButton
          label={t("templates.export")}
          icon={<Download size={15} />}
          onClick={() =>
            void run(async () => {
              const r = await templateActions.exportJson(tpl.id);
              downloadText(r.fileName, r.json);
              ui.toast("success", t("templates.exported", { file: r.fileName }));
            })
          }
          tooltip="bottom"
        />
        {!builtin && (
          <IconButton
            label={t("templates.delete")}
            icon={<Trash2 size={15} />}
            onClick={() =>
              void run(async () => {
                if (!window.confirm(t("templates.confirmDelete", { name: templateName(tpl.name) }))) return;
                await templateActions.remove(tpl.id);
                ui.toast("info", t("templates.deleted", { name: templateName(tpl.name) }));
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
  const { t } = useT(["agents", "common"]);
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
      const saved = editor.mode === "edit" ? await templateActions.update(editor.template.id, draft) : await templateActions.create(draft);
      toast("success", t("templates.saved", { name: saved.name }));
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
              {editor.mode === "edit" ? t("editor.editTitle") : t("editor.newTitle")}
            </h2>
            <p className="dialog__subtitle">{t("editor.subtitle")}</p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body agents-editor">
          <div className="agents-dialog__row">
            <Input label={t("editor.name")} value={d.name} maxLength={80} onChange={(e) => set({ name: e.target.value })} />
            <label className="ok-field">
              <span className="ok-field__label">{t("editor.role")}</span>
              <select
                className="agents-select"
                aria-label={t("editor.role")}
                value={roleValue}
                onChange={(e) =>
                  set({ role: e.target.value === "__custom" ? { Custom: customRole || t("editor.customDefault") } : (e.target.value as AgentRole) })
                }
              >
                {BUILTIN_ROLES.map((r) => (
                  <option key={r} value={r}>
                    {roleLabel(r)}
                  </option>
                ))}
                <option value="__custom">{t("editor.customOption")}</option>
              </select>
            </label>
            {roleValue === "__custom" && (
              <Input label={t("editor.customRole")} value={customRole} maxLength={40} onChange={(e) => setCustomRole(e.target.value)} />
            )}
            <Input label={t("editor.version")} value={d.version} onChange={(e) => set({ version: e.target.value })} />
          </div>
          <Input label={t("editor.description")} value={d.description} maxLength={500} onChange={(e) => set({ description: e.target.value })} />
          <label className="ok-field">
            <span className="ok-field__label">{t("editor.systemPrompt", { vars: TEMPLATE_VARS })}</span>
            <textarea className="agents-textarea agents-textarea--tall" rows={8} value={d.systemPrompt} onChange={(e) => set({ systemPrompt: e.target.value })} />
          </label>
          <fieldset className="ok-field agents-fieldset">
            <legend className="ok-field__label">{t("editor.tools")}</legend>
            <div className="agents-checks">
              {ALL_TOOLS.map((tool) => (
                <label key={tool}>
                  <input
                    type="checkbox"
                    checked={d.tools.includes(tool)}
                    disabled={tool === "submit_result"}
                    onChange={() => set({ tools: toggle(d.tools, tool) })}
                  />{" "}
                  {tool}
                </label>
              ))}
            </div>
          </fieldset>
          <div className="agents-dialog__row">
            <label className="ok-field">
              <span className="ok-field__label">{t("editor.worktreePolicy")}</span>
              <select
                className="agents-select"
                aria-label={t("editor.worktreePolicy")}
                value={d.worktreePolicy}
                onChange={(e) => set({ worktreePolicy: e.target.value as WorktreePolicy })}
              >
                {POLICIES.map((p) => (
                  <option key={p} value={p} title={t(`worktreePolicyHint.${p}`)}>
                    {t(`worktreePolicy.${p}`)}
                  </option>
                ))}
              </select>
            </label>
            <label className="ok-field">
              <span className="ok-field__label">{t("editor.memory")}</span>
              <select
                className="agents-select"
                aria-label={t("editor.memoryLabel")}
                value={d.memoryAccess}
                onChange={(e) => set({ memoryAccess: e.target.value as MemoryAccess })}
              >
                {MEMORY.map((m) => (
                  <option key={m} value={m}>
                    {t(`memoryAccess.${m}`)}
                  </option>
                ))}
              </select>
            </label>
            <label className="ok-field">
              <span className="ok-field__label">{t("editor.maxTurns")}</span>
              <input
                className="agents-input"
                inputMode="numeric"
                value={String(d.maxTurns)}
                onChange={(e) => set({ maxTurns: Number.parseInt(e.target.value.replace(/[^0-9]/g, "") || "0", 10) })}
              />
            </label>
            <label className="ok-field">
              <span className="ok-field__label">{t("editor.budget")}</span>
              <input
                className="agents-input"
                inputMode="decimal"
                value={budget.maxCostUsd ?? ""}
                placeholder={t("editor.budgetPlaceholder")}
                onChange={(e) => {
                  const v = e.target.value.trim();
                  const n = v ? Number.parseFloat(v) : null;
                  set({ budget: n === null && budget.maxDurationSecs === null ? null : { ...budget, maxCostUsd: n !== null && Number.isFinite(n) ? n : null } });
                }}
              />
            </label>
            <label className="ok-field">
              <span className="ok-field__label">{t("editor.timeLimit")}</span>
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
            <legend className="ok-field__label">{t("editor.qualityGates")}</legend>
            <div className="agents-checks">
              {GATES.map((g) => (
                <label key={g}>
                  <input type="checkbox" checked={d.qualityGates.includes(g)} onChange={() => set({ qualityGates: toggle(d.qualityGates, g) })} />{" "}
                  {t(`qualityGate.${g}`)}
                </label>
              ))}
            </div>
          </fieldset>
          <ModelPicker label={t("editor.preferredModel")} value={d.preferredModel} onChange={(m: ModelRef | null) => set({ preferredModel: m })} providers={providers} allowNone />
          <div className="agents-dialog__row">
            <Input
              label={t("editor.avatar")}
              value={d.avatar?.robot ?? ""}
              placeholder={t("editor.avatarPlaceholder")}
              onChange={(e) => {
                const robot = e.target.value;
                set({ avatar: robot.trim() ? { robot, accent: d.avatar?.accent ?? null, scale: d.avatar?.scale ?? 1, floating: d.avatar?.floating ?? true } : null });
              }}
            />
            <Input
              label={t("editor.voice")}
              value={d.voice ? `${d.voice.provider}:${d.voice.voice}` : ""}
              placeholder={t("editor.voicePlaceholder")}
              onChange={(e) => {
                const [provider, voice] = e.target.value.split(":");
                set({ voice: provider && voice ? { provider, voice, language: d.voice?.language ?? "en-US", style: null } : null });
              }}
            />
          </div>
          {d.permissions.length > 0 && (
            <p className="agents-muted">{t("editor.permissions", { list: d.permissions.map((p) => `${p.capability} ${p.effect}`).join(", ") })}</p>
          )}
          {error && (
            <p className="agents-error" role="alert">
              {error}
            </p>
          )}
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={close} disabled={busy}>
            {t("common:actions.cancel")}
          </Button>
          <Button variant="primary" type="submit" loading={busy}>
            {t("editor.save")}
          </Button>
        </footer>
      </form>
    </div>
  );
}

function ImportDialog({ onImported }: { onImported: () => void }) {
  const { t } = useT(["agents", "common"]);
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
      setError(t("importDialog.empty"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const r = await templateActions.importJson(json, accept);
      const dropped = r.droppedPermissions.length;
      toast(
        "success",
        dropped ? t("importDialog.importedDropped", { name: r.template.name, count: dropped }) : t("importDialog.imported", { name: r.template.name }),
      );
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
              {t("importDialog.title")}
            </h2>
            <p className="dialog__subtitle">{t("importDialog.subtitle")}</p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body">
          <input
            type="file"
            accept=".json,application/json"
            aria-label={t("importDialog.file")}
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (f) void f.text().then(setJson);
            }}
          />
          <label className="ok-field">
            <span className="ok-field__label">{t("importDialog.json")}</span>
            <textarea className="agents-textarea agents-textarea--tall" rows={10} value={json} onChange={(e) => setJson(e.target.value)} />
          </label>
          <label className="agents-check">
            <input type="checkbox" checked={accept} onChange={(e) => setAccept(e.target.checked)} /> {t("importDialog.accept")}
          </label>
          {error && (
            <p className="agents-error" role="alert">
              {error}
            </p>
          )}
        </div>
        <footer className="dialog__footer">
          <Button variant="ghost" onClick={close} disabled={busy}>
            {t("common:actions.cancel")}
          </Button>
          <Button variant="primary" type="submit" loading={busy}>
            {t("importDialog.submit")}
          </Button>
        </footer>
      </form>
    </div>
  );
}

export function TemplatesTab({ templates, loading, error, reload }: { templates: AgentTemplate[]; loading: boolean; error: string | null; reload: () => void }) {
  const { t } = useT(["agents", "common"]);
  const setEditor = useAgentsUi((s) => s.setEditor);
  const setImportOpen = useAgentsUi((s) => s.setImportOpen);
  let body: JSX.Element;
  if (loading && templates.length === 0) {
    body = (
      <div className="agents-loading">
        <Spinner size={16} /> {t("templates.loading")}
      </div>
    );
  } else if (error) {
    body = (
      <EmptyState
        title={t("templates.loadFailed")}
        description={error}
        action={
          <Button leftIcon={<RefreshCw size={14} />} onClick={reload}>
            {t("common:actions.retry")}
          </Button>
        }
      />
    );
  } else if (templates.length === 0) {
    body = <EmptyState title={t("templates.empty.title")} description={t("templates.empty.description")} />;
  } else {
    body = (
      <ul className="agents-tpls" aria-label={t("templates.listLabel")}>
        {templates.map((tpl) => (
          <TemplateCard key={tpl.id} tpl={tpl} onChanged={reload} />
        ))}
      </ul>
    );
  }
  return (
    <div className="agents-templates">
      <div className="agents-toolbar">
        <Button size="sm" variant="primary" leftIcon={<Plus size={14} />} onClick={() => setEditor({ mode: "create", draft: blankDraft() })}>
          {t("templates.new")}
        </Button>
        <Button size="sm" leftIcon={<FileUp size={14} />} onClick={() => setImportOpen(true)}>
          {t("templates.import")}
        </Button>
        <div className="agents-toolbar__spacer" />
        <IconButton label={t("templates.reload")} icon={<RefreshCw size={14} />} onClick={reload} tooltip="bottom" />
      </div>
      <div className="agents-scroll">{body}</div>
      <TemplateEditor onSaved={reload} />
      <ImportDialog onImported={reload} />
    </div>
  );
}
