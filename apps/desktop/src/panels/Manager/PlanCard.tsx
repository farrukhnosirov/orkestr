/**
 * Plan card (§1 steps 2–4, §9): goal, summary, task tree grouped by epic /
 * feature with roles, dependencies, priorities and estimated cost. A
 * proposed plan can be approved as is, edited inline (rename / remove
 * tasks) and then approved, or rejected with an optional reason.
 */
import type { ManagerPlan } from "@orkestr/protocol";
import { Badge, Button, IconButton } from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, ClipboardList, CornerDownRight, Pencil, RotateCcw, Trash2, TriangleAlert, X } from "lucide-react";
import { useState } from "react";

import { roleLabel } from "../Agents/model";
import { errorMessage, managerActions } from "./data";
import {
  EMPTY_DRAFT,
  PLAN_STATUS_TONE,
  type PlanDraft,
  dependencyTitles,
  dependentsOf,
  draftToEdits,
  formatUsd,
  planCost,
  planTree,
  validateDraft,
} from "./model";
import { useManagerUi } from "./store";

export function PlanCard({ plan }: { plan: ManagerPlan }) {
  const toast = useManagerUi((s) => s.toast);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<PlanDraft>(EMPTY_DRAFT);
  const [rejecting, setRejecting] = useState(false);
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState<"approve" | "reject" | null>(null);
  const proposed = plan.status === "Proposed";
  const groups = planTree(plan.tasks);
  const draftError = editing ? validateDraft(plan, draft) : null;
  const removedCount = draft.removed.length;

  const approve = async () => {
    if (draftError) return;
    setBusy("approve");
    try {
      const r = await managerActions.approvePlan(plan.id, editing ? draftToEdits(plan, draft) : undefined);
      setEditing(false);
      setDraft(EMPTY_DRAFT);
      toast("success", r.run ? "Plan approved — the run started." : "Plan approved — tasks created.");
    } catch (e) {
      toast("danger", errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const reject = async () => {
    setBusy("reject");
    try {
      await managerActions.rejectPlan(plan.id, reason);
      setRejecting(false);
      setReason("");
    } catch (e) {
      toast("danger", errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const toggleRemove = (key: string) =>
    setDraft((d) => ({ ...d, removed: d.removed.includes(key) ? d.removed.filter((k) => k !== key) : [...d.removed, key] }));

  return (
    <section className={clsx("mgr-plan", `mgr-plan--${plan.status.toLowerCase()}`)} aria-label={`Plan: ${plan.goal}`}>
      <header className="mgr-plan__head">
        <ClipboardList size={16} aria-hidden="true" />
        <div className="mgr-plan__title">
          <strong>{plan.goal}</strong>
          <span className="mgr-muted">
            {plan.tasks.length} task{plan.tasks.length === 1 ? "" : "s"} · up to {plan.maxParallelism} in parallel · {planCost(plan)}
            {plan.source === "Extracted" ? " · extracted from text" : ""}
          </span>
        </div>
        <Badge tone={PLAN_STATUS_TONE[plan.status]}>{plan.status}</Badge>
      </header>
      {plan.summary && <p className="mgr-plan__summary">{plan.summary}</p>}
      {(plan.warnings.length > 0 || plan.risks.length > 0) && (
        <ul className="mgr-plan__notes">
          {plan.risks.map((r, i) => (
            <li key={`r${i}`}>
              <TriangleAlert size={12} aria-hidden="true" /> {r}
            </li>
          ))}
          {plan.warnings.map((w, i) => (
            <li key={`w${i}`} className="mgr-muted">
              {w}
            </li>
          ))}
        </ul>
      )}
      <div className="mgr-plan__tree" role="tree" aria-label="Plan tasks">
        {groups.map((g, gi) => (
          <div key={`${g.epic ?? ""}/${g.feature ?? ""}/${gi}`} className="mgr-plan__group" role="group" aria-label={g.feature ?? g.epic ?? "Tasks"}>
            {(g.epic || g.feature) && (
              <div className="mgr-plan__group-head">
                {g.epic && <span className="mgr-plan__epic">{g.epic}</span>}
                {g.epic && g.feature && <span className="mgr-muted">›</span>}
                {g.feature && <span>{g.feature}</span>}
              </div>
            )}
            {g.nodes.map(({ task, level }) => {
              const removed = draft.removed.includes(task.key);
              const deps = dependencyTitles(task, plan.tasks);
              const title = draft.titles[task.key] ?? task.title;
              return (
                <div
                  key={task.key}
                  role="treeitem"
                  aria-level={level + 1}
                  aria-selected={false}
                  className={clsx("mgr-ptask", removed && "mgr-ptask--removed")}
                  style={{ paddingLeft: `calc(var(--ok-space-3) + ${Math.min(level, 6)} * var(--ok-space-4))` }}
                >
                  <div className="mgr-ptask__row">
                    {level > 0 && <CornerDownRight size={12} className="mgr-muted" aria-hidden="true" />}
                    <span className="mgr-ptask__key">{task.key}</span>
                    {editing && !removed ? (
                      <input
                        className="mgr-input mgr-ptask__title-input"
                        value={title}
                        aria-label={`Title of ${task.key}`}
                        maxLength={200}
                        onChange={(e) => setDraft((d) => ({ ...d, titles: { ...d.titles, [task.key]: e.target.value } }))}
                      />
                    ) : (
                      <span className="mgr-ptask__title">{title}</span>
                    )}
                    <Badge tone="violet">{roleLabel(task.role)}</Badge>
                    {task.priority !== "Normal" && <Badge tone={task.priority === "Critical" || task.priority === "High" ? "warning" : "neutral"}>{task.priority}</Badge>}
                    {task.estimatedCostUsd !== null && <span className="mgr-muted">{formatUsd(task.estimatedCostUsd)}</span>}
                    {editing && (
                      <IconButton
                        size="sm"
                        label={removed ? `Keep ${task.key}` : `Remove ${task.key}`}
                        icon={removed ? <RotateCcw size={13} /> : <Trash2 size={13} />}
                        onClick={() => toggleRemove(task.key)}
                      />
                    )}
                  </div>
                  {deps.length > 0 && <div className="mgr-ptask__deps">after {deps.join(", ")}</div>}
                  {editing && removed && dependentsOf(task.key, plan.tasks).length > 0 && (
                    <div className="mgr-ptask__deps">tasks that depended on it will no longer wait for it</div>
                  )}
                  {!editing && task.acceptanceCriteria.length > 0 && (
                    <details className="mgr-ptask__details">
                      <summary>Acceptance criteria{task.files.length > 0 ? " & files" : ""}</summary>
                      <ul>
                        {task.acceptanceCriteria.map((c, i) => (
                          <li key={i}>{c}</li>
                        ))}
                      </ul>
                      {task.files.length > 0 && (
                        <div className="mgr-ptask__files">
                          {task.files.map((f) => (
                            <code key={f}>{f}</code>
                          ))}
                        </div>
                      )}
                    </details>
                  )}
                </div>
              );
            })}
          </div>
        ))}
      </div>
      {plan.rejectionReason && <p className="mgr-muted">Rejected: {plan.rejectionReason}</p>}
      {proposed && (
        <footer className="mgr-plan__actions">
          {draftError && (
            <span className="mgr-error" role="alert">
              {draftError}
            </span>
          )}
          {rejecting ? (
            <>
              <input
                className="mgr-input"
                placeholder="Why? (optional — the Manager sees it)"
                aria-label="Rejection reason"
                value={reason}
                onChange={(e) => setReason(e.target.value)}
              />
              <Button size="sm" variant="danger" loading={busy === "reject"} onClick={() => void reject()}>
                Reject plan
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setRejecting(false)}>
                Back
              </Button>
            </>
          ) : (
            <>
              <Button
                size="sm"
                variant="primary"
                leftIcon={<Check size={14} />}
                loading={busy === "approve"}
                disabled={busy !== null || draftError !== null}
                onClick={() => void approve()}
              >
                {editing ? `Approve edited plan${removedCount ? ` (−${removedCount})` : ""}` : "Approve"}
              </Button>
              {editing ? (
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    setEditing(false);
                    setDraft(EMPTY_DRAFT);
                  }}
                >
                  Discard edits
                </Button>
              ) : (
                <Button size="sm" variant="ghost" leftIcon={<Pencil size={14} />} onClick={() => setEditing(true)}>
                  Edit
                </Button>
              )}
              <Button size="sm" variant="ghost" leftIcon={<X size={14} />} disabled={busy !== null} onClick={() => setRejecting(true)}>
                Reject
              </Button>
            </>
          )}
        </footer>
      )}
    </section>
  );
}
