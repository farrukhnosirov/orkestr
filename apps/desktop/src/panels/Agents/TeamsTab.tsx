/**
 * Teams tab of the Agents panel (§6, §68.27): persistent Agent Teams as cards
 * (member avatars, lead, policies, project assignment with its permission
 * scope), a create / edit dialog and the hierarchy tree of the team's lead
 * runs (Manager → Team Lead → Executors → Reviewer). Data: `teams.*` RPC and
 * live team events.
 */
import "./teams.css";

import type { AgentRole, AgentTemplate, ApprovalMode, ModelRef, TeamMember, TeamView } from "@orkestr/protocol";
import { Badge, Button, EmptyState, IconButton, Input, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { Crown, GitFork, Lock, Network, Pencil, Plus, RefreshCw, Trash2, TriangleAlert, Users, X } from "lucide-react";
import { type FormEvent, useEffect, useMemo, useState } from "react";
import { create } from "zustand";

import { ModelPicker } from "../../features/providers/ModelPicker";
import { useProvidersLive } from "../../features/providers/store";
import { templateName } from "../../lib/coreText";
import { t as translate, useFormat, useT } from "../../i18n";
import { errorMessage } from "./data";
import { roleLabel } from "./model";
import { RoleAvatar } from "./parts";
import { useAgentsUi } from "./store";
import { teamActions, useTeams } from "./teamsData";
import {
  LEAD_STATUS_TONE,
  TEAM_LIMITS,
  type TeamDraft,
  type TreeNode,
  blankTeamDraft,
  draftOfTeam,
  headcount,
  hierarchyTree,
  isAssigned,
  isLeadActive,
  memberAvatars,
  policiesOf,
  teamDescription,
  teamName,
  validateTeamDraft,
} from "./teamsModel";

type EditorState = { mode: "create" } | { mode: "edit"; view: TeamView } | null;

/** View state of the tab (editor target, expanded hierarchy). */
const useTeamsUi = create<{
  editor: EditorState;
  expanded: string | null;
  setEditor: (e: EditorState) => void;
  toggle: (id: string) => void;
}>()((set, get) => ({
  editor: null,
  expanded: null,
  setEditor: (editor) => set({ editor }),
  toggle: (id) => set({ expanded: get().expanded === id ? null : id }),
}));

const APPROVAL_MODES: readonly ApprovalMode[] = ["Ask", "SemiAuto", "FullAuto"];

export function TeamsTab({ projectId, templates }: { projectId: string; templates: readonly AgentTemplate[] }) {
  const { t } = useT(["teams", "common"]);
  const { teams, loading, error, reload } = useTeams(projectId);
  const setEditor = useTeamsUi((s) => s.setEditor);

  let body: JSX.Element;
  if (loading && teams.length === 0) {
    body = (
      <div className="agents-loading">
        <Spinner size={18} /> {t("loading")}
      </div>
    );
  } else if (error) {
    body = (
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
  } else if (teams.length === 0) {
    body = (
      <EmptyState
        icon={<Users size={26} strokeWidth={1.6} />}
        title={t("empty.title")}
        description={t("empty.description")}
        action={
          <Button variant="primary" leftIcon={<Plus size={14} />} onClick={() => setEditor({ mode: "create" })}>
            {t("new")}
          </Button>
        }
      />
    );
  } else {
    body = (
      <ul className="teams-list" aria-label={t("listLabel")}>
        {teams.map((v) => (
          <TeamCard key={v.team.id} view={v} projectId={projectId} templates={templates} />
        ))}
      </ul>
    );
  }

  return (
    <div className="agents-scroll teams-tab">
      <div className="teams-tab__bar">
        <p className="agents-muted">{t("intro")}</p>
        <Button size="sm" leftIcon={<Plus size={14} />} onClick={() => setEditor({ mode: "create" })}>
          {t("new")}
        </Button>
      </div>
      {body}
      <TeamEditor projectId={projectId} templates={templates} />
    </div>
  );
}

function TeamCard({ view, projectId, templates }: { view: TeamView; projectId: string; templates: readonly AgentTemplate[] }) {
  const { t } = useT(["teams", "common"]);
  const fmt = useFormat();
  const toast = useAgentsUi((s) => s.toast);
  const setEditor = useTeamsUi((s) => s.setEditor);
  const expanded = useTeamsUi((s) => s.expanded === view.team.id);
  const toggle = useTeamsUi((s) => s.toggle);
  const [busy, setBusy] = useState(false);
  const { team } = view;
  const name = teamName(team);
  const avatars = memberAvatars(team.members, templates);
  const assigned = isAssigned(view, projectId);
  const assignment = view.assignments.find((a) => a.projectId === projectId);
  const active = view.leadRuns.filter((r) => isLeadActive(r.status)).length;
  const others = view.assignments.filter((a) => a.projectId !== projectId).length;

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      toast("danger", errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <li className="teams-card">
      <div className="teams-card__head">
        <div className="teams-card__title">
          <strong>{name}</strong>
          {team.builtin && (
            <Badge tone="accent" icon={<Lock size={11} />}>
              {t("builtinBadge")}
            </Badge>
          )}
          {team.policies.hierarchy && (
            <Badge tone="violet" icon={<GitFork size={11} />}>
              {t("hierarchyBadge")}
            </Badge>
          )}
          {assigned && <Badge tone="success">{t("assignedBadge")}</Badge>}
          {active > 0 && <Badge tone="info">{t("activeLeads", { count: active })}</Badge>}
        </div>
        <div className="teams-card__actions">
          <IconButton
            label={expanded ? t("hideHierarchy") : t("showHierarchy")}
            icon={<Network size={15} />}
            active={expanded}
            aria-expanded={expanded}
            onClick={() => toggle(team.id)}
            tooltip="bottom"
          />
          <IconButton label={t("edit")} icon={<Pencil size={15} />} onClick={() => setEditor({ mode: "edit", view })} tooltip="bottom" />
          <IconButton
            label={t("delete")}
            icon={<Trash2 size={15} />}
            disabled={busy}
            onClick={() =>
              void run(async () => {
                if (!window.confirm(t("confirmDelete", { name }))) return;
                await teamActions.remove(team.id);
                toast("info", t("deleted", { name }));
              })
            }
            tooltip="bottom"
          />
        </div>
      </div>
      <p className="teams-card__desc">{teamDescription(team) || t("noDescription")}</p>
      <div className="teams-avatars" aria-label={t("membersLabel", { count: headcount(team.members) })}>
        {avatars.map((a) => (
          <span key={a.key} className={clsx("teams-member", a.isLead && "teams-member--lead")} title={templateName(a.name)}>
            <RoleAvatar role={a.role} size={28} />
            {a.isLead && <Crown className="teams-member__crown" size={11} aria-label={t("leadMark")} />}
            {a.count > 1 && <span className="teams-member__count">×{a.count}</span>}
          </span>
        ))}
      </div>
      <div className="teams-card__meta">
        <span>{t("members", { count: headcount(team.members) })}</span>
        <span>{t("policy.fanOut", { count: team.policies.maxFanOut })}</span>
        <span>{t("policy.parallel", { count: team.policies.maxParallel })}</span>
        <span>{t("policy.attempts", { count: team.policies.maxAttempts })}</span>
        <span>{team.policies.requireReview ? t("policy.review") : t("policy.noReview")}</span>
        {team.policies.budgetUsd != null && <span>{t("policy.budget", { amount: fmt.currency(team.policies.budgetUsd) })}</span>}
        {team.policies.approvalMode && <span>{t("policy.approval", { mode: t(`approvalMode.${team.policies.approvalMode}`) })}</span>}
        {others > 0 && <span>{t("otherProjects", { count: others })}</span>}
      </div>
      <div className="teams-card__assign">
        <Button
          size="sm"
          variant={assigned ? "ghost" : "primary"}
          loading={busy}
          onClick={() =>
            void run(async () => {
              await teamActions.assign(assigned ? { teamId: team.id, projectId, unassign: true } : { teamId: team.id, projectId });
              toast("success", assigned ? t("unassigned", { name }) : t("assigned", { name }));
            })
          }
        >
          {assigned ? t("unassign") : t("assign")}
        </Button>
        {assignment && (
          <label className="teams-check">
            <input
              type="checkbox"
              checked={assignment.permissions.secrets}
              disabled={busy}
              onChange={(e) =>
                void run(async () => {
                  await teamActions.assign({
                    teamId: team.id,
                    projectId,
                    permissions: { ...assignment.permissions, secrets: e.target.checked },
                  });
                })
              }
            />{" "}
            {t("secretsAllowed")}
          </label>
        )}
        {assignment && <span className="agents-muted">{t("scopeHint")}</span>}
      </div>
      {expanded && <HierarchyTree view={view} templates={templates} />}
    </li>
  );
}

function HierarchyTree({ view, templates }: { view: TeamView; templates: readonly AgentTemplate[] }) {
  const { t } = useT(["teams", "common"]);
  const lead = view.team.members.find((m) => m.isLead);
  const leadRole = lead ? (templates.find((x) => x.id === lead.templateId)?.role ?? null) : null;
  const tree = useMemo(() => hierarchyTree(view, leadRole), [view, leadRole]);
  return (
    <div className="teams-tree" role="tree" aria-label={t("tree.label")}>
      <TreeRow node={tree} depth={0} />
      {tree.children.length === 0 && (
        <p className="agents-muted teams-tree__empty">{lead ? t("tree.empty") : t("tree.noLead")}</p>
      )}
    </div>
  );
}

function statusText(node: TreeNode): string | null {
  if (!node.status) return null;
  if (node.level === "lead") return translate(`teams:leadStatus.${node.status as "Planning"}`);
  return translate(`common:status.task.${node.status as "Running"}`);
}

function TreeRow({ node, depth }: { node: TreeNode; depth: number }) {
  const { t } = useT(["teams", "common"]);
  const status = statusText(node);
  const tone = node.level === "lead" && node.status ? LEAD_STATUS_TONE[node.status as keyof typeof LEAD_STATUS_TONE] : "neutral";
  return (
    <div role="treeitem" aria-level={depth + 1} aria-expanded={node.children.length > 0 ? true : undefined}>
      <div className={clsx("teams-tree__row", `teams-tree__row--${node.level}`)} style={{ paddingLeft: `calc(${depth} * var(--ok-space-5))` }}>
        {node.role && <RoleAvatar role={node.role as AgentRole} size={22} />}
        <span className="teams-tree__level">{t(`tree.level.${node.level}`)}</span>
        <span className="teams-tree__title">{node.title}</span>
        {status && <Badge tone={tone}>{status}</Badge>}
        {node.attempts > 1 && <span className="agents-muted">{t("tree.attempts", { count: node.attempts })}</span>}
        {node.note && (
          <span className="agents-muted teams-tree__note" title={node.note}>
            {node.note}
          </span>
        )}
      </div>
      {node.children.length > 0 && (
        <div role="group">
          {node.children.map((c) => (
            <TreeRow key={c.id} node={c} depth={depth + 1} />
          ))}
        </div>
      )}
    </div>
  );
}

function num(text: string): number {
  return Number.parseInt(text.replace(/[^0-9]/g, "") || "0", 10);
}

function TeamEditor({ projectId, templates }: { projectId: string; templates: readonly AgentTemplate[] }) {
  const { t } = useT(["teams", "common"]);
  const editor = useTeamsUi((s) => s.editor);
  const setEditor = useTeamsUi((s) => s.setEditor);
  const toast = useAgentsUi((s) => s.toast);
  const { providers } = useProvidersLive();
  const [d, setD] = useState<TeamDraft>(() => blankTeamDraft(templates));
  const [projectOnly, setProjectOnly] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!editor) return;
    setD(editor.mode === "edit" ? draftOfTeam(editor.view.team) : blankTeamDraft(templates));
    setProjectOnly(false);
    setError(null);
    setBusy(false);
  }, [editor, templates]);

  if (!editor) return null;
  const set = (patch: Partial<TeamDraft>) => setD((cur) => ({ ...cur, ...patch }));
  const setPolicy = (patch: Partial<TeamDraft["policies"]>) => setD((cur) => ({ ...cur, policies: { ...cur.policies, ...patch } }));
  const setMember = (i: number, patch: Partial<TeamMember>) =>
    setD((cur) => ({
      ...cur,
      members: cur.members.map((m, j) => {
        if (j === i) return { ...m, ...patch, ...(patch.isLead ? { count: 1 } : {}) };
        // Only one lead.
        return patch.isLead ? { ...m, isLead: false } : m;
      }),
    }));
  const roleOf = (id: string) => templates.find((x) => x.id === id)?.role ?? null;
  const close = () => {
    if (!busy) setEditor(null);
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const problem = validateTeamDraft(d, roleOf);
    if (problem) {
      setError(problem);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const common = { name: d.name.trim(), description: d.description.trim(), members: d.members, policies: policiesOf(d) };
      const saved =
        editor.mode === "edit"
          ? await teamActions.update({ teamId: editor.view.team.id, ...common })
          : await teamActions.create({ ...common, ...(projectOnly ? { projectId } : {}) });
      toast("success", t("saved", { name: teamName(saved.team) }));
      setEditor(null);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="overlay" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <form
        className="dialog agents-dialog agents-dialog--wide"
        role="dialog"
        aria-modal="true"
        aria-labelledby="team-editor-title"
        onSubmit={(e) => void submit(e)}
        onKeyDown={(e) => e.key === "Escape" && close()}
      >
        <header className="dialog__header">
          <div className="dialog__icon">
            <Users size={18} />
          </div>
          <div>
            <h2 id="team-editor-title" className="dialog__title">
              {editor.mode === "edit" ? t("editor.editTitle") : t("editor.newTitle")}
            </h2>
            <p className="dialog__subtitle">{t("editor.subtitle")}</p>
          </div>
          <button type="button" className="dialog__close" aria-label={t("common:actions.close")} onClick={close}>
            <X size={16} />
          </button>
        </header>
        <div className="dialog__body agents-editor">
          <Input label={t("editor.name")} value={d.name} maxLength={TEAM_LIMITS.nameLen} onChange={(e) => set({ name: e.target.value })} />
          <label className="ok-field">
            <span className="ok-field__label">{t("editor.description")}</span>
            <textarea
              className="agents-textarea"
              rows={2}
              maxLength={TEAM_LIMITS.descriptionLen}
              value={d.description}
              onChange={(e) => set({ description: e.target.value })}
            />
          </label>
          <fieldset className="ok-field agents-fieldset">
            <legend className="ok-field__label">{t("editor.members")}</legend>
            {d.members.length === 0 && <p className="agents-muted">{t("editor.noMembers")}</p>}
            <ul className="teams-editor__members">
              {d.members.map((m, i) => (
                <li key={i} className="teams-editor__member">
                  <select
                    className="agents-select"
                    aria-label={t("editor.template")}
                    value={m.templateId}
                    onChange={(e) => setMember(i, { templateId: e.target.value })}
                  >
                    {templates.map((tpl) => (
                      <option key={tpl.id} value={tpl.id}>
                        {templateName(tpl.name)} — {roleLabel(tpl.role)}
                      </option>
                    ))}
                  </select>
                  <label className="teams-editor__count">
                    <span>{t("editor.count")}</span>
                    <input
                      className="agents-input"
                      inputMode="numeric"
                      aria-label={t("editor.count")}
                      value={String(m.count)}
                      disabled={m.isLead}
                      onChange={(e) => setMember(i, { count: num(e.target.value) })}
                    />
                  </label>
                  <label className="teams-check">
                    <input type="radio" name="team-lead" checked={m.isLead} onChange={() => setMember(i, { isLead: true })} /> {t("editor.lead")}
                  </label>
                  <ModelPicker
                    label={t("editor.model")}
                    value={m.model}
                    onChange={(model: ModelRef | null) => setMember(i, { model })}
                    providers={providers}
                    allowNone
                  />
                  <IconButton
                    label={t("editor.removeMember")}
                    icon={<Trash2 size={14} />}
                    onClick={() => set({ members: d.members.filter((_, j) => j !== i) })}
                  />
                </li>
              ))}
            </ul>
            <div className="teams-editor__member-actions">
              <Button
                size="sm"
                variant="ghost"
                leftIcon={<Plus size={13} />}
                disabled={templates.length === 0 || d.members.length >= TEAM_LIMITS.members}
                onClick={() => {
                  const first = templates[0];
                  if (first) set({ members: [...d.members, { templateId: first.id, isLead: false, count: 1, model: null }] });
                }}
              >
                {t("editor.addMember")}
              </Button>
              {d.members.some((m) => m.isLead) && (
                <Button size="sm" variant="ghost" onClick={() => set({ members: d.members.map((m) => ({ ...m, isLead: false })) })}>
                  {t("editor.noLead")}
                </Button>
              )}
            </div>
          </fieldset>
          <fieldset className="ok-field agents-fieldset">
            <legend className="ok-field__label">{t("editor.policies")}</legend>
            <label className="teams-check">
              <input type="checkbox" checked={d.policies.hierarchy} onChange={(e) => setPolicy({ hierarchy: e.target.checked })} />{" "}
              {t("editor.hierarchy")}
            </label>
            <p className="agents-muted">{t("editor.hierarchyHint")}</p>
            <label className="teams-check">
              <input type="checkbox" checked={d.policies.requireReview} onChange={(e) => setPolicy({ requireReview: e.target.checked })} />{" "}
              {t("editor.requireReview")}
            </label>
            <div className="agents-dialog__row">
              <label className="ok-field">
                <span className="ok-field__label">{t("editor.fanOut")}</span>
                <input className="agents-input" inputMode="numeric" value={String(d.policies.maxFanOut)} onChange={(e) => setPolicy({ maxFanOut: num(e.target.value) })} />
              </label>
              <label className="ok-field">
                <span className="ok-field__label">{t("editor.parallel")}</span>
                <input className="agents-input" inputMode="numeric" value={String(d.policies.maxParallel)} onChange={(e) => setPolicy({ maxParallel: num(e.target.value) })} />
              </label>
              <label className="ok-field">
                <span className="ok-field__label">{t("editor.attempts")}</span>
                <input className="agents-input" inputMode="numeric" value={String(d.policies.maxAttempts)} onChange={(e) => setPolicy({ maxAttempts: num(e.target.value) })} />
              </label>
              <label className="ok-field">
                <span className="ok-field__label">{t("editor.timeLimit")}</span>
                <input
                  className="agents-input"
                  inputMode="numeric"
                  value={String(d.policies.timeLimitMins)}
                  onChange={(e) => setPolicy({ timeLimitMins: num(e.target.value) })}
                />
              </label>
              <label className="ok-field">
                <span className="ok-field__label">{t("editor.budget")}</span>
                <input
                  className="agents-input"
                  inputMode="decimal"
                  value={d.budgetText}
                  placeholder={t("editor.budgetPlaceholder")}
                  onChange={(e) => set({ budgetText: e.target.value })}
                />
              </label>
              <label className="ok-field">
                <span className="ok-field__label">{t("editor.approval")}</span>
                <select
                  className="agents-select"
                  aria-label={t("editor.approval")}
                  value={d.policies.approvalMode ?? ""}
                  onChange={(e) => setPolicy({ approvalMode: e.target.value ? (e.target.value as ApprovalMode) : null })}
                >
                  <option value="">{t("editor.approvalInherit")}</option>
                  {APPROVAL_MODES.map((m) => (
                    <option key={m} value={m}>
                      {t(`approvalMode.${m}`)}
                    </option>
                  ))}
                </select>
              </label>
            </div>
          </fieldset>
          {editor.mode === "create" && (
            <label className="teams-check">
              <input type="checkbox" checked={projectOnly} onChange={(e) => setProjectOnly(e.target.checked)} /> {t("editor.projectOnly")}
            </label>
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
