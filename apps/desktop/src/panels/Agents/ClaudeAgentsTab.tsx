/**
 * «Claude Code agents»: Claude Code's own sub-agents (built in + the
 * user's and the project's `.claude/agents/*.md`) as Orkestr agents. Each is
 * mirrored as a template by the core (`claudeEngine.agents`), so it can be
 * spawned on a task like any agent — it then runs through the official CLI
 * (`--agent`) with every tool call going through Orkestr's approvals.
 * File agents can be created, edited and deleted (with confirmation).
 */
import type { ClaudeAgentDef, ClaudeAgentScope } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { Bot, Pencil, Play, Plus, RefreshCw, Trash2, TriangleAlert, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { Overlay } from "../../components/Overlay";
import { CLAUDE_MODELS, deleteClaudeAgent, isValidAgentName, listClaudeAgents, parseTools, saveClaudeAgent } from "../../features/claudeEngine/api";
import { ClaudeEngineCard } from "../../features/claudeEngine/ClaudeEngineCard";
import "../../features/claudeEngine/claudeEngine.css";
import { TextArea } from "../../features/providers/controls";
import { useT } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";
import { useAgentsUi } from "./store";

const SCOPES: readonly ClaudeAgentScope[] = ["project", "user", "builtIn"];

interface EditorState {
  original: ClaudeAgentDef | null;
  scope: ClaudeAgentScope;
  name: string;
  description: string;
  tools: string;
  model: string;
  prompt: string;
  confirming: boolean;
  error: string | null;
  busy: boolean;
}

function editorFor(def: ClaudeAgentDef | null): EditorState {
  return {
    original: def,
    scope: def?.scope ?? "project",
    name: def?.name ?? "",
    description: def?.description ?? "",
    tools: def?.tools.join(", ") ?? "",
    model: def?.model ?? "inherit",
    prompt: def?.prompt ?? "",
    confirming: false,
    error: null,
    busy: false,
  };
}

export function ClaudeAgentsTab({ projectId, onTemplatesChanged }: { projectId: string; onTemplatesChanged: () => void }) {
  const { t } = useT(["agents", "common"]);
  const connected = useAppStore((s) => s.connection.state === "open");
  const openSpawn = useAgentsUi((s) => s.openSpawn);
  const toast = useAgentsUi((s) => s.toast);
  const [agents, setAgents] = useState<ClaudeAgentDef[]>([]);
  const [builtinError, setBuiltinError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [deleting, setDeleting] = useState<ClaudeAgentDef | null>(null);

  const load = useCallback(
    async (refresh: boolean) => {
      setLoading(true);
      setError(null);
      try {
        const r = await listClaudeAgents(projectId, refresh);
        setAgents(r.agents);
        setBuiltinError(r.builtinError ?? null);
        onTemplatesChanged();
      } catch (e) {
        setError(describeError(e));
      } finally {
        setLoading(false);
      }
    },
    [projectId, onTemplatesChanged],
  );

  useEffect(() => {
    if (connected) void load(false);
  }, [connected, load]);

  const save = async () => {
    if (!editor) return;
    const name = editor.name.trim();
    if (!isValidAgentName(name)) {
      setEditor({ ...editor, error: t("claude.editor.invalidName"), confirming: false });
      return;
    }
    if (!editor.description.trim() || !editor.prompt.trim()) {
      setEditor({ ...editor, error: t("claude.editor.required"), confirming: false });
      return;
    }
    if (!editor.confirming) {
      setEditor({ ...editor, confirming: true, error: null });
      return;
    }
    setEditor({ ...editor, busy: true, error: null });
    try {
      await saveClaudeAgent({
        projectId: editor.scope === "project" ? projectId : null,
        scope: editor.scope,
        name,
        description: editor.description.trim(),
        tools: parseTools(editor.tools),
        model: editor.model === "inherit" ? "inherit" : editor.model,
        prompt: editor.prompt,
        confirmed: true,
      });
      setEditor(null);
      toast("success", t("claude.saved", { name }));
      await load(false);
    } catch (e) {
      setEditor({ ...editor, busy: false, confirming: false, error: describeError(e) });
    }
  };

  const remove = async (def: ClaudeAgentDef) => {
    try {
      await deleteClaudeAgent(def.scope === "project" ? projectId : null, def.scope, def.name);
      toast("success", t("claude.deleted", { name: def.name }));
      setDeleting(null);
      await load(false);
    } catch (e) {
      toast("danger", describeError(e));
    }
  };

  const spawn = (def: ClaudeAgentDef) => {
    if (!def.templateId) {
      toast("danger", t("claude.noTemplate"));
      return;
    }
    openSpawn(def.templateId);
  };

  const target = editor ? (editor.scope === "project" ? `.claude/agents/${editor.name.trim() || "…"}.md` : `~/.claude/agents/${editor.name.trim() || "…"}.md`) : "";

  return (
    <div className="cca">
      <ClaudeEngineCard onApplied={onTemplatesChanged} />
      <div className="cca__bar">
        <div className="cca__bar-text">
          <h3 className="cca__bar-title">{t("claude.title")}</h3>
          <p className="cca__bar-sub">{t("claude.subtitle")}</p>
        </div>
        <Button size="sm" variant="primary" leftIcon={<Plus size={14} />} onClick={() => setEditor(editorFor(null))}>
          {t("claude.create")}
        </Button>
        <IconButton label={t("claude.refresh")} icon={<RefreshCw size={14} />} onClick={() => void load(true)} disabled={loading} />
      </div>
      {loading && agents.length === 0 ? (
        <div className="agents-loading">
          <Spinner size={18} /> {t("claude.loading")}
        </div>
      ) : error ? (
        <EmptyState icon={<TriangleAlert size={24} />} title={t("claude.loadFailed")} description={error} />
      ) : agents.length === 0 ? (
        <EmptyState icon={<Bot size={24} />} title={t("claude.empty.title")} description={t("claude.empty.description")} />
      ) : (
        SCOPES.map((scope) => {
          const list = agents.filter((a) => a.scope === scope);
          if (list.length === 0) return null;
          return (
            <section key={scope} aria-label={t(`claude.scope.${scope}`)}>
              <h4 className="cca__group-title">{t(`claude.scope.${scope}`)}</h4>
              <div className="cca__grid">
                {list.map((def) => (
                  <article key={`${def.scope}:${def.name}`} className="cca-card">
                    <div className="cca-card__head">
                      <Bot size={15} aria-hidden="true" />
                      <span className="cca-card__name" title={def.name}>
                        {def.name}
                      </span>
                      {def.model && <Badge tone="violet">{def.model}</Badge>}
                    </div>
                    {def.description && (
                      <p className="cca-card__desc" title={def.description}>
                        {def.description}
                      </p>
                    )}
                    <div className="cca-card__tools" aria-label={t("claude.tools")}>
                      {def.tools.length === 0 ? (
                        <span className="cca-card__tool">{t("claude.allTools")}</span>
                      ) : (
                        def.tools.slice(0, 8).map((tool) => (
                          <span key={tool} className="cca-card__tool">
                            {tool}
                          </span>
                        ))
                      )}
                    </div>
                    <div className="cca-card__actions">
                      <Button size="sm" variant="primary" leftIcon={<Play size={13} />} onClick={() => spawn(def)}>
                        {t("claude.spawn")}
                      </Button>
                      {def.scope !== "builtIn" && (
                        <>
                          <IconButton label={t("claude.edit")} icon={<Pencil size={13} />} onClick={() => setEditor(editorFor(def))} />
                          <IconButton label={t("claude.delete")} icon={<Trash2 size={13} />} onClick={() => setDeleting(def)} />
                        </>
                      )}
                    </div>
                  </article>
                ))}
              </div>
            </section>
          );
        })
      )}
      {builtinError && <p className="cca__bar-sub">{t("claude.builtinUnavailable")}</p>}

      {editor && (
        <Overlay onDismiss={() => !editor.busy && setEditor(null)}>
          <div className="cca-editor" role="dialog" aria-modal="true" aria-label={t(editor.original ? "claude.editor.editTitle" : "claude.editor.createTitle")}>
            <div className="cca__bar">
              <h3 className="cca-editor__title cca__bar-text">{t(editor.original ? "claude.editor.editTitle" : "claude.editor.createTitle")}</h3>
              <IconButton label={t("common:actions.close")} icon={<X size={14} />} onClick={() => setEditor(null)} disabled={editor.busy} />
            </div>
            <div className="cca-editor__row">
              <label className="ce-card__model">
                <span>{t("claude.editor.scope")}</span>
                <select
                  className="pv-select"
                  value={editor.scope}
                  disabled={editor.original != null || editor.busy}
                  onChange={(e) => setEditor({ ...editor, scope: e.target.value as ClaudeAgentScope, confirming: false })}
                >
                  <option value="project">{t("claude.scope.project")}</option>
                  <option value="user">{t("claude.scope.user")}</option>
                </select>
              </label>
              <label className="ce-card__model">
                <span>{t("claude.editor.model")}</span>
                <select className="pv-select" value={editor.model} disabled={editor.busy} onChange={(e) => setEditor({ ...editor, model: e.target.value, confirming: false })}>
                  <option value="inherit">{t("claude.editor.inherit")}</option>
                  {CLAUDE_MODELS.map((m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            <Input
              label={t("claude.editor.name")}
              hint={t("claude.editor.nameHint")}
              value={editor.name}
              disabled={editor.original != null || editor.busy}
              onChange={(e) => setEditor({ ...editor, name: e.target.value.toLowerCase(), confirming: false })}
            />
            <Input
              label={t("claude.editor.description")}
              hint={t("claude.editor.descriptionHint")}
              value={editor.description}
              disabled={editor.busy}
              onChange={(e) => setEditor({ ...editor, description: e.target.value, confirming: false })}
            />
            <Input
              label={t("claude.editor.tools")}
              hint={t("claude.editor.toolsHint")}
              value={editor.tools}
              disabled={editor.busy}
              onChange={(e) => setEditor({ ...editor, tools: e.target.value, confirming: false })}
            />
            <TextArea
              label={t("claude.editor.prompt")}
              rows={8}
              value={editor.prompt}
              disabled={editor.busy}
              onChange={(e) => setEditor({ ...editor, prompt: e.target.value, confirming: false })}
            />
            {editor.confirming && (
              <div className="cca-editor__confirm" role="alert">
                {t("claude.editor.confirm")} <span className="cca-editor__path">{target}</span>
              </div>
            )}
            {editor.error && <p className="cca-editor__error">{editor.error}</p>}
            <div className="cca-editor__footer">
              <Button onClick={() => setEditor(null)} disabled={editor.busy}>
                {t("common:actions.cancel")}
              </Button>
              <Button variant="primary" loading={editor.busy} onClick={() => void save()}>
                {editor.confirming ? t("claude.editor.confirmSave") : t("claude.editor.save")}
              </Button>
            </div>
          </div>
        </Overlay>
      )}

      {deleting && (
        <Overlay onDismiss={() => setDeleting(null)}>
          <div className="cca-editor" role="alertdialog" aria-modal="true" aria-label={t("claude.deleteTitle")}>
            <h3 className="cca-editor__title">{t("claude.deleteTitle")}</h3>
            <p className="cca__bar-sub">
              {t("claude.deleteConfirm")} <span className="cca-editor__path">{deleting.path ?? deleting.name}</span>
            </p>
            <div className="cca-editor__footer">
              <Button onClick={() => setDeleting(null)}>{t("common:actions.cancel")}</Button>
              <Button variant="danger" leftIcon={<Trash2 size={14} />} onClick={() => void remove(deleting)}>
                {t("claude.delete")}
              </Button>
            </div>
          </div>
        </Overlay>
      )}
    </div>
  );
}
