/**
 * Product view of the Tasks panel (§9, §68.47): the AI Product Manager turns
 * an idea into an editable breakdown — epic → features → stories →
 * acceptance criteria → tasks with dependencies. The draft is edited here,
 * saved (`pm.update`, validated by the core too), then approved: the core
 * creates the task hierarchy and can hand it to a Manager run, optionally
 * delegated to an Agent Team.
 */
import "./product.css";

import type { AgentRole, PmDraft, PmEpic, PmTask, Task, TaskPriority } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { CheckCircle2, ClipboardList, Lightbulb, Plus, RefreshCw, Save, Trash2, TriangleAlert, X } from "lucide-react";
import { useMemo, useState } from "react";

import { useT } from "../../../i18n";
import { BUILTIN_ROLES, roleLabel } from "../../Agents/model";
import { useTeams } from "../../Agents/teamsData";
import { teamName } from "../../Agents/teamsModel";
import { errorMessage } from "../data";
import { PRIORITIES } from "../model";
import { StatusBadge } from "../parts";
import { useTasksUi } from "../store";
import { pmActions, usePmDrafts } from "./pmData";
import { PM_LIMITS, allTasks, counts, edit, progressPercent, validateEpic } from "./pmModel";

export function ProductView({ projectId, tasks }: { projectId: string; tasks: readonly Task[] }) {
  const { t } = useT(["product", "common"]);
  const { drafts, loading, error, reload } = usePmDrafts(projectId);
  const [selected, setSelected] = useState<string | null>(null);
  const current = drafts.find((d) => d.id === selected) ?? drafts[0] ?? null;

  let detail: JSX.Element;
  if (loading && drafts.length === 0) {
    detail = (
      <div className="tasks-loading">
        <Spinner size={18} /> {t("loading")}
      </div>
    );
  } else if (error) {
    detail = (
      <EmptyState
        icon={<TriangleAlert size={26} strokeWidth={1.6} />}
        title={t("loadFailed")}
        description={error}
        action={
          <Button leftIcon={<RefreshCw size={14} />} onClick={reload}>
            {t("common:actions.retry")}
          </Button>
        }
      />
    );
  } else if (!current) {
    detail = <EmptyState icon={<ClipboardList size={26} strokeWidth={1.6} />} title={t("empty.title")} description={t("empty.description")} />;
  } else {
    detail = <DraftEditor key={`${current.id}-${current.updatedAt}`} draft={current} projectId={projectId} tasks={tasks} />;
  }

  return (
    <div className="pm-view">
      <aside className="pm-side" aria-label={t("draftsLabel")}>
        <IdeaBox projectId={projectId} onDrafted={(d) => setSelected(d.id)} />
        <ul className="pm-drafts">
          {drafts.map((d) => (
            <li key={d.id}>
              <button
                type="button"
                className={clsx("pm-draft-item", current?.id === d.id && "pm-draft-item--on")}
                aria-current={current?.id === d.id}
                onClick={() => setSelected(d.id)}
              >
                <span className="pm-draft-item__title">{d.epic.title}</span>
                <span className="pm-draft-item__meta">
                  <Badge tone={d.status === "Approved" ? "success" : d.status === "Discarded" ? "neutral" : "accent"}>{t(`status.${d.status}`)}</Badge>
                  {d.status === "Approved" && <span>{t("progress", { percent: progressPercent(d) })}</span>}
                </span>
              </button>
            </li>
          ))}
        </ul>
      </aside>
      <section className="pm-main">{detail}</section>
    </div>
  );
}

function IdeaBox({ projectId, onDrafted }: { projectId: string; onDrafted: (d: PmDraft) => void }) {
  const { t } = useT("product");
  const toast = useTasksUi((s) => s.toast);
  const [idea, setIdea] = useState("");
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    if (!idea.trim() || busy) return;
    setBusy(true);
    try {
      const d = await pmActions.draft(projectId, idea.trim());
      setIdea("");
      onDrafted(d);
      toast("success", t("drafted", { title: d.epic.title }));
    } catch (e) {
      toast("danger", errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="pm-idea">
      <label className="ok-field">
        <span className="ok-field__label">{t("idea.label")}</span>
        <textarea
          className="pm-textarea"
          rows={3}
          maxLength={PM_LIMITS.idea}
          value={idea}
          placeholder={t("idea.placeholder")}
          disabled={busy}
          onChange={(e) => setIdea(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) void submit();
          }}
        />
      </label>
      <Button size="sm" variant="primary" leftIcon={<Lightbulb size={14} />} loading={busy} disabled={!idea.trim()} onClick={() => void submit()}>
        {busy ? t("idea.thinking") : t("idea.submit")}
      </Button>
    </div>
  );
}

const ROLES: readonly AgentRole[] = BUILTIN_ROLES;

function DraftEditor({ draft, projectId, tasks }: { draft: PmDraft; projectId: string; tasks: readonly Task[] }) {
  const { t } = useT(["product", "tasks", "common"]);
  const toast = useTasksUi((s) => s.toast);
  const { teams } = useTeams(projectId);
  const [epic, setEpic] = useState<PmEpic>(draft.epic);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState<"save" | "approve" | "discard" | null>(null);
  const [startRun, setStartRun] = useState(true);
  const [teamId, setTeamId] = useState<string>(draft.teamId ?? "");
  const editable = draft.status === "Draft";
  const problem = useMemo(() => (editable ? validateEpic(epic) : null), [epic, editable]);
  const c = counts(epic);
  const taskById = useMemo(() => new Map(tasks.map((x) => [x.id, x])), [tasks]);
  const keys = useMemo(() => allTasks(epic).map((x) => ({ key: x.key, title: x.title })), [epic]);


  const change = (next: PmEpic) => {
    setEpic(next);
    setDirty(true);
  };

  const run = async (kind: "save" | "approve" | "discard", fn: () => Promise<void>) => {
    setBusy(kind);
    try {
      await fn();
    } catch (e) {
      toast("danger", errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const save = () =>
    run("save", async () => {
      await pmActions.update(draft.id, epic);
      setDirty(false);
      toast("success", t("saved"));
    });
  const approve = () =>
    run("approve", async () => {
      if (dirty) await pmActions.update(draft.id, epic);
      const r = await pmActions.approve(draft.id, startRun, teamId || null);
      setDirty(false);
      toast("success", r.run ? t("approvedRun", { count: c.tasks }) : t("approved", { count: c.tasks }));
    });
  const discard = () =>
    run("discard", async () => {
      if (!window.confirm(t("confirmDiscard", { title: draft.epic.title }))) return;
      await pmActions.discard(draft.id);
    });

  const statusOf = (id: string | null | undefined) => (id ? taskById.get(id)?.status : undefined);

  return (
    <div className="pm-editor">
      <div className="pm-editor__bar">
        <Badge tone={draft.status === "Approved" ? "success" : draft.status === "Discarded" ? "neutral" : "accent"}>{t(`status.${draft.status}`)}</Badge>
        <span className="tasks-muted">{[t("count.features", { count: c.features }), t("count.stories", { count: c.stories }), t("count.tasks", { count: c.tasks })].join(" · ")}</span>
        {draft.status === "Approved" && (
          <span className="pm-progress" aria-label={t("progressLabel")}>
            <span className="pm-progress__bar" style={{ width: `${progressPercent(draft)}%` }} />
          </span>
        )}
        <div className="tasks-toolbar__spacer" />
        {editable && (
          <>
            <Button size="sm" variant="ghost" leftIcon={<Trash2 size={13} />} loading={busy === "discard"} disabled={busy !== null} onClick={() => void discard()}>
              {t("discard")}
            </Button>
            <Button size="sm" leftIcon={<Save size={13} />} loading={busy === "save"} disabled={!dirty || problem !== null || busy !== null} onClick={() => void save()}>
              {t("save")}
            </Button>
          </>
        )}
      </div>
      <p className="pm-idea-text">
        <strong>{t("ideaPrefix")}</strong> {draft.idea}
      </p>
      {draft.warnings.length > 0 && (
        <ul className="pm-warnings" aria-label={t("warningsLabel")}>
          {draft.warnings.map((w) => (
            <li key={w}>{w}</li>
          ))}
        </ul>
      )}
      <div className="pm-tree" role="tree" aria-label={t("treeLabel")}>
        <div role="treeitem" aria-level={1} aria-expanded="true" className="pm-node pm-node--epic">
          <div className="pm-node__head">
            <span className="pm-kind">{t("kind.epic")}</span>
            <TextField value={epic.title} label={t("field.epicTitle")} editable={editable} onChange={(v) => change(edit.epic(epic, { title: v }))} strong />
            {draft.epic.taskId && statusOf(draft.epic.taskId) && <StatusBadge status={statusOf(draft.epic.taskId) ?? "Planned"} />}
          </div>
          <TextArea value={epic.description} label={t("field.description")} editable={editable} onChange={(v) => change(edit.epic(epic, { description: v }))} />
          <div role="group">
            {epic.features.map((f, fi) => (
              <div key={f.key} role="treeitem" aria-level={2} aria-expanded="true" className="pm-node pm-node--feature">
                <div className="pm-node__head">
                  <span className="pm-kind">{t("kind.feature")}</span>
                  <TextField value={f.title} label={t("field.featureTitle")} editable={editable} onChange={(v) => change(edit.feature(epic, fi, { title: v }))} strong />
                  {statusOf(f.taskId) && <StatusBadge status={statusOf(f.taskId) ?? "Planned"} />}
                  {editable && (
                    <IconButton size="sm" label={t("remove.feature")} icon={<X size={13} />} disabled={epic.features.length <= 1} onClick={() => change(edit.removeFeature(epic, fi))} />
                  )}
                </div>
                <div role="group">
                  {f.stories.map((s, si) => (
                    <div key={s.key} role="treeitem" aria-level={3} aria-expanded="true" className="pm-node pm-node--story">
                      <div className="pm-node__head">
                        <span className="pm-kind">{t("kind.story")}</span>
                        <TextField value={s.title} label={t("field.storyTitle")} editable={editable} onChange={(v) => change(edit.story(epic, fi, si, { title: v }))} />
                        {statusOf(s.taskId) && <StatusBadge status={statusOf(s.taskId) ?? "Planned"} />}
                        {editable && (
                          <IconButton size="sm" label={t("remove.story")} icon={<X size={13} />} disabled={f.stories.length <= 1} onClick={() => change(edit.removeStory(epic, fi, si))} />
                        )}
                      </div>
                      <div className="pm-ac" role="group" aria-label={t("kind.criteria")}>
                        <span className="pm-kind pm-kind--small">{t("kind.criteria")}</span>
                        <ul>
                          {s.acceptanceCriteria.map((ac, ci) => (
                            <li key={ci} className="pm-ac__item">
                              <CheckCircle2 size={12} aria-hidden="true" />
                              <TextField value={ac} label={t("field.criterion")} editable={editable} onChange={(v) => change(edit.criterion(epic, fi, si, ci, v))} />
                              {editable && (
                                <IconButton
                                  size="sm"
                                  label={t("remove.criterion")}
                                  icon={<X size={12} />}
                                  disabled={s.acceptanceCriteria.length <= 1}
                                  onClick={() => change(edit.removeCriterion(epic, fi, si, ci))}
                                />
                              )}
                            </li>
                          ))}
                        </ul>
                        {editable && s.acceptanceCriteria.length < PM_LIMITS.criteria && (
                          <Button size="sm" variant="ghost" leftIcon={<Plus size={12} />} onClick={() => change(edit.addCriterion(epic, fi, si))}>
                            {t("add.criterion")}
                          </Button>
                        )}
                      </div>
                      <div role="group">
                        {s.tasks.map((task, ti) => (
                          <TaskRow
                            key={task.key}
                            task={task}
                            editable={editable}
                            keys={keys}
                            status={statusOf(task.taskId)}
                            canRemove={s.tasks.length > 1}
                            onChange={(patch) => change(edit.task(epic, fi, si, ti, patch))}
                            onToggleDep={(dep) => change(edit.toggleDependency(epic, task.key, dep))}
                            onRemove={() => change(edit.removeTask(epic, fi, si, ti))}
                          />
                        ))}
                        {editable && (
                          <Button size="sm" variant="ghost" leftIcon={<Plus size={12} />} onClick={() => change(edit.addTask(epic, fi, si))}>
                            {t("add.task")}
                          </Button>
                        )}
                      </div>
                    </div>
                  ))}
                  {editable && f.stories.length < PM_LIMITS.stories && (
                    <Button size="sm" variant="ghost" leftIcon={<Plus size={12} />} onClick={() => change(edit.addStory(epic, fi))}>
                      {t("add.story")}
                    </Button>
                  )}
                </div>
              </div>
            ))}
            {editable && epic.features.length < PM_LIMITS.features && (
              <Button size="sm" variant="ghost" leftIcon={<Plus size={12} />} onClick={() => change(edit.addFeature(epic))}>
                {t("add.feature")}
              </Button>
            )}
          </div>
        </div>
      </div>
      {editable && (
        <div className="pm-approve">
          {problem && (
            <p className="tasks-error" role="alert">
              {problem}
            </p>
          )}
          <label className="ok-field pm-approve__team">
            <span className="ok-field__label">{t("approve.team")}</span>
            <select className="pm-select" aria-label={t("approve.team")} value={teamId} onChange={(e) => setTeamId(e.target.value)}>
              <option value="">{t("approve.noTeam")}</option>
              {teams.map((v) => (
                <option key={v.team.id} value={v.team.id}>
                  {teamName(v.team)}
                </option>
              ))}
            </select>
          </label>
          <label className="pm-check">
            <input type="checkbox" checked={startRun} onChange={(e) => setStartRun(e.target.checked)} /> {t("approve.startRun")}
          </label>
          <Button variant="primary" leftIcon={<CheckCircle2 size={14} />} loading={busy === "approve"} disabled={problem !== null || busy !== null} onClick={() => void approve()}>
            {t("approve.button", { count: c.tasks })}
          </Button>
        </div>
      )}
    </div>
  );
}

function TaskRow({
  task,
  editable,
  keys,
  status,
  canRemove,
  onChange,
  onToggleDep,
  onRemove,
}: {
  task: PmTask;
  editable: boolean;
  keys: readonly { key: string; title: string }[];
  status: Task["status"] | undefined;
  canRemove: boolean;
  onChange: (patch: Partial<Omit<PmTask, "key" | "taskId">>) => void;
  onToggleDep: (dep: string) => void;
  onRemove: () => void;
}) {
  const { t } = useT(["product", "tasks"]);
  const roleValue = typeof task.role === "object" ? "" : task.role;
  const others = keys.filter((k) => k.key !== task.key);
  return (
    <div role="treeitem" aria-level={4} className="pm-node pm-node--task">
      <div className="pm-node__head">
        <span className="pm-kind pm-kind--task">{task.key}</span>
        <TextField value={task.title} label={t("field.taskTitle")} editable={editable} onChange={(v) => onChange({ title: v })} />
        {status && <StatusBadge status={status} />}
        {editable ? (
          <>
            <select className="pm-select" aria-label={t("field.role")} value={roleValue} onChange={(e) => onChange({ role: e.target.value as AgentRole })}>
              {typeof task.role === "object" && <option value="">{task.role.Custom}</option>}
              {ROLES.map((r) => (
                <option key={String(r)} value={String(r)}>
                  {roleLabel(r)}
                </option>
              ))}
            </select>
            <select className="pm-select" aria-label={t("field.priority")} value={task.priority} onChange={(e) => onChange({ priority: e.target.value as TaskPriority })}>
              {PRIORITIES.map((p) => (
                <option key={p} value={p}>
                  {t(`tasks:priority.${p}`)}
                </option>
              ))}
            </select>
            <IconButton size="sm" label={t("remove.task")} icon={<X size={13} />} disabled={!canRemove} onClick={onRemove} />
          </>
        ) : (
          <span className="tasks-muted">
            {roleLabel(task.role)} · {t(`tasks:priority.${task.priority}`)}
          </span>
        )}
      </div>
      {(editable ? others.length > 0 : task.dependsOn.length > 0) && (
        <div className="pm-deps" role="group" aria-label={t("field.dependsOn")}>
          <span className="pm-kind pm-kind--small">{t("field.dependsOn")}</span>
          {editable
            ? others.map((k) => (
                <button
                  key={k.key}
                  type="button"
                  className={clsx("pm-dep", task.dependsOn.includes(k.key) && "pm-dep--on")}
                  aria-pressed={task.dependsOn.includes(k.key)}
                  title={k.title}
                  onClick={() => onToggleDep(k.key)}
                >
                  {k.key}
                </button>
              ))
            : task.dependsOn.map((d) => (
                <span key={d} className="pm-dep pm-dep--on" title={keys.find((k) => k.key === d)?.title}>
                  {d}
                </span>
              ))}
        </div>
      )}
    </div>
  );
}

function TextField({ value, label, editable, onChange, strong = false }: { value: string; label: string; editable: boolean; onChange: (v: string) => void; strong?: boolean }) {
  if (!editable) return <span className={clsx("pm-text", strong && "pm-text--strong")}>{value}</span>;
  return <input className={clsx("pm-input", strong && "pm-input--strong")} aria-label={label} value={value} maxLength={PM_LIMITS.title} onChange={(e) => onChange(e.target.value)} />;
}

function TextArea({ value, label, editable, onChange }: { value: string; label: string; editable: boolean; onChange: (v: string) => void }) {
  if (!editable) return value ? <p className="pm-desc">{value}</p> : null;
  return <textarea className="pm-textarea pm-textarea--small" aria-label={label} rows={2} value={value} onChange={(e) => onChange(e.target.value)} />;
}
