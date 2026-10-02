import type { Task, TaskAttempt, TaskHistoryEntry, TaskPriority, TaskStatus } from "@orkestr/protocol";
import { Button, IconButton, Spinner } from "@orkestr/ui";
import { clsx } from "clsx";
import { Check, Coins, FileCode2, GitFork, History, Plus, RotateCcw, Trash2, X } from "lucide-react";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";

import { errorMessage, taskActions } from "./data";
import { ACTION_LABEL, PRIORITIES, TRANSITIONS, formatCost, formatRelative, shortId, statusColor } from "./model";
import { PriorityChip, StatusBadge } from "./parts";
import { useTasksUi } from "./store";

const DANGER: ReadonlySet<TaskStatus> = new Set<TaskStatus>(["Cancelled", "Failed", "Interrupted"]);

function actorLabel(h: TaskHistoryEntry): string {
  switch (h.actor.type) {
    case "User":
      return "You";
    case "Agent":
      return `Agent #${shortId(h.actor.id)}`;
    case "Manager":
      return "AI Manager";
    case "System":
      return "Engine";
    case "SystemRecovery":
      return "Crash recovery";
  }
}

function useHistory(taskId: string, version: number) {
  const [entries, setEntries] = useState<TaskHistoryEntry[] | null>(null);
  const [executions, setExecutions] = useState<TaskAttempt[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    taskActions
      .history(taskId)
      .then((r) => {
        if (cancelled) return;
        setEntries(r.entries);
        setExecutions(r.executions);
        setError(null);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [taskId, version]);
  return { entries, executions, error };
}

export function TaskDrawer({ task, tasks }: { task: Task; tasks: Task[] }) {
  const select = useTasksUi((s) => s.select);
  const toast = useTasksUi((s) => s.toast);
  const [busy, setBusy] = useState<string | null>(null);
  const [title, setTitle] = useState(task.title);
  const [description, setDescription] = useState(task.description);
  const [newCriterion, setNewCriterion] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [depPick, setDepPick] = useState("");
  const history = useHistory(task.id, task.version);
  const closeRef = useRef<HTMLButtonElement>(null);

  // Reset local edits when another task is opened or the task changed remotely.
  useEffect(() => {
    setTitle(task.title);
    setDescription(task.description);
  }, [task.id, task.title, task.description]);
  useEffect(() => {
    setConfirmDelete(false);
    setDepPick("");
    closeRef.current?.focus();
  }, [task.id]);

  const run = async (key: string, fn: () => Promise<unknown>, success?: string) => {
    setBusy(key);
    try {
      await fn();
      if (success) toast("success", success);
    } catch (e) {
      toast("danger", errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const saveTitle = () => {
    const t = title.trim();
    if (!t || t === task.title) {
      setTitle(task.title);
      return;
    }
    void run("title", () => taskActions.update(task.id, { title: t }));
  };
  const saveDescription = () => {
    if (description !== task.description) void run("description", () => taskActions.update(task.id, { description }));
  };
  const setCriteria = (criteria: Task["acceptanceCriteria"]) =>
    void run("criteria", () => taskActions.update(task.id, { acceptanceCriteria: criteria }));
  const setPriority = (priority: TaskPriority) => void run("priority", () => taskActions.update(task.id, { priority }));

  const byId = new Map(tasks.map((t) => [t.id, t]));
  const prereqs = task.dependencies.map((id) => ({ id, task: byId.get(id) }));
  const dependents = tasks.filter((t) => t.dependencies.includes(task.id));
  const candidates = tasks.filter((t) => t.id !== task.id && !task.dependencies.includes(t.id));
  const actions = TRANSITIONS[task.status];
  const met = task.acceptanceCriteria.filter((c) => c.met).length;

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      select(null);
    }
  };

  return (
    <aside className="tasks-drawer" aria-label={`Task ${task.title}`} onKeyDown={onKeyDown}>
      <header className="tasks-drawer__header" style={{ ["--status-color" as string]: statusColor(task.status) }}>
        <div className="tasks-drawer__eyebrow">
          <StatusBadge status={task.status} />
          <span className="tasks-mono">#{shortId(task.id)}</span>
          <span className="tasks-drawer__spacer" />
          <IconButton ref={closeRef} label="Close task details" icon={<X size={15} />} onClick={() => select(null)} />
        </div>
        <input
          className="tasks-drawer__title"
          value={title}
          aria-label="Task title"
          maxLength={200}
          onChange={(e) => setTitle(e.target.value)}
          onBlur={saveTitle}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
            if (e.key === "Escape") setTitle(task.title);
          }}
        />
      </header>

      <div className="tasks-drawer__body">
        <section className="tasks-section" aria-label="Status actions">
          <h4 className="tasks-section__title">Status</h4>
          {actions.length === 0 ? (
            <p className="tasks-muted">{task.status} is final.</p>
          ) : (
            <div className="tasks-actions">
              {actions.map((to) => (
                <Button
                  key={to}
                  size="sm"
                  variant={to === "Completed" || to === "Running" || to === "Ready" ? "primary" : DANGER.has(to) ? "danger" : "secondary"}
                  loading={busy === `t:${to}`}
                  disabled={busy !== null}
                  onClick={() => void run(`t:${to}`, () => taskActions.transition(task.id, to))}
                >
                  {ACTION_LABEL[to]}
                </Button>
              ))}
            </div>
          )}
          <div className="tasks-actions">
            {(task.status === "Failed" || task.status === "Interrupted") && (
              <Button
                size="sm"
                leftIcon={<RotateCcw size={13} />}
                loading={busy === "retry"}
                disabled={busy !== null}
                onClick={() => void run("retry", () => taskActions.retry(task.id), "Task queued again.")}
              >
                Retry
              </Button>
            )}
            {(task.status === "Planned" || task.status === "Cancelled") &&
              (confirmDelete ? (
                <>
                  <Button
                    size="sm"
                    variant="danger"
                    loading={busy === "delete"}
                    onClick={() =>
                      void run("delete", async () => {
                        await taskActions.remove(task.id);
                        select(null);
                      }, "Task deleted.")
                    }
                  >
                    Confirm delete
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setConfirmDelete(false)}>
                    Keep
                  </Button>
                </>
              ) : (
                <Button size="sm" variant="ghost" leftIcon={<Trash2 size={13} />} onClick={() => setConfirmDelete(true)}>
                  Delete
                </Button>
              ))}
          </div>
        </section>

        <section className="tasks-section">
          <h4 className="tasks-section__title">Priority</h4>
          <div className="tasks-priority-picker" role="radiogroup" aria-label="Priority">
            {PRIORITIES.map((p) => (
              <button
                key={p}
                type="button"
                role="radio"
                aria-checked={task.priority === p}
                className={clsx("tasks-priority-option", task.priority === p && "tasks-priority-option--active")}
                disabled={busy !== null}
                onClick={() => task.priority !== p && setPriority(p)}
              >
                <PriorityChip priority={p} />
              </button>
            ))}
          </div>
        </section>

        <section className="tasks-section">
          <h4 className="tasks-section__title">Description</h4>
          <textarea
            className="tasks-textarea"
            value={description}
            placeholder="What needs to be done? Markdown supported."
            rows={4}
            aria-label="Description"
            onChange={(e) => setDescription(e.target.value)}
            onBlur={saveDescription}
          />
        </section>

        <section className="tasks-section">
          <h4 className="tasks-section__title">
            Acceptance criteria
            {task.acceptanceCriteria.length > 0 && (
              <span className="tasks-section__count">
                {met}/{task.acceptanceCriteria.length}
              </span>
            )}
          </h4>
          <ul className="tasks-checklist">
            {task.acceptanceCriteria.map((c, i) => (
              <li key={`${i}-${c.text}`} className={clsx("tasks-check", c.met && "tasks-check--met")}>
                <label>
                  <input
                    type="checkbox"
                    checked={c.met}
                    disabled={busy !== null}
                    onChange={() =>
                      setCriteria(task.acceptanceCriteria.map((x, j) => (j === i ? { ...x, met: !x.met } : x)))
                    }
                  />
                  <span className="tasks-check__box" aria-hidden="true">
                    {c.met && <Check size={11} />}
                  </span>
                  <span>{c.text}</span>
                </label>
                <IconButton
                  size="sm"
                  label={`Remove criterion ${c.text}`}
                  icon={<X size={12} />}
                  onClick={() => setCriteria(task.acceptanceCriteria.filter((_, j) => j !== i))}
                />
              </li>
            ))}
          </ul>
          <form
            className="tasks-inline-form"
            onSubmit={(e) => {
              e.preventDefault();
              const text = newCriterion.trim();
              if (!text) return;
              setNewCriterion("");
              setCriteria([...task.acceptanceCriteria, { text, met: false }]);
            }}
          >
            <input
              className="tasks-input"
              value={newCriterion}
              placeholder="Add a criterion…"
              aria-label="New acceptance criterion"
              maxLength={500}
              onChange={(e) => setNewCriterion(e.target.value)}
            />
            <IconButton type="submit" size="sm" label="Add criterion" icon={<Plus size={13} />} disabled={!newCriterion.trim()} />
          </form>
        </section>

        <section className="tasks-section">
          <h4 className="tasks-section__title">
            <GitFork size={13} /> Depends on
          </h4>
          {prereqs.length === 0 ? (
            <p className="tasks-muted">No dependencies.</p>
          ) : (
            <ul className="tasks-links">
              {prereqs.map(({ id, task: dep }) => (
                <li key={id}>
                  <button type="button" className="tasks-link" onClick={() => select(id)}>
                    {dep ? <StatusBadge status={dep.status} /> : null}
                    <span>{dep?.title ?? `#${shortId(id)}`}</span>
                  </button>
                  <IconButton
                    size="sm"
                    label="Remove dependency"
                    icon={<X size={12} />}
                    disabled={busy !== null}
                    onClick={() => void run(`rm:${id}`, () => taskActions.removeDependency(task.id, id))}
                  />
                </li>
              ))}
            </ul>
          )}
          {candidates.length > 0 && (
            <form
              className="tasks-inline-form"
              onSubmit={(e) => {
                e.preventDefault();
                if (!depPick) return;
                const pick = depPick;
                setDepPick("");
                void run("dep", () => taskActions.addDependency(task.id, pick), "Dependency added.");
              }}
            >
              <select
                className="tasks-select"
                value={depPick}
                aria-label="Add dependency"
                onChange={(e) => setDepPick(e.target.value)}
              >
                <option value="">Add dependency…</option>
                {candidates.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.title} ({c.status})
                  </option>
                ))}
              </select>
              <IconButton type="submit" size="sm" label="Add dependency" icon={<Plus size={13} />} disabled={!depPick || busy !== null} />
            </form>
          )}
          {dependents.length > 0 && (
            <>
              <h5 className="tasks-subtitle">Blocks</h5>
              <ul className="tasks-links">
                {dependents.map((d) => (
                  <li key={d.id}>
                    <button type="button" className="tasks-link" onClick={() => select(d.id)}>
                      <StatusBadge status={d.status} />
                      <span>{d.title}</span>
                    </button>
                  </li>
                ))}
              </ul>
            </>
          )}
        </section>

        <section className="tasks-section">
          <h4 className="tasks-section__title">
            <Coins size={13} /> Cost & attempts
          </h4>
          <dl className="tasks-stats">
            <div>
              <dt>Cost</dt>
              <dd>{formatCost(task.usage.costUsd)}</dd>
            </div>
            <div>
              <dt>Tokens</dt>
              <dd>{(task.usage.inputTokens + task.usage.outputTokens).toLocaleString()}</dd>
            </div>
            <div>
              <dt>Attempts</dt>
              <dd>{task.attempts}</dd>
            </div>
            <div>
              <dt>Estimate</dt>
              <dd>{task.estimateMinutes !== null ? `${task.estimateMinutes}m` : "—"}</dd>
            </div>
          </dl>
          {history.executions.length > 0 && (
            <ul className="tasks-executions">
              {history.executions.map((x) => (
                <li key={x.id}>
                  <span className="tasks-mono">#{x.attempt}</span>
                  <span className={clsx("tasks-exec", `tasks-exec--${x.status.toLowerCase()}`)}>{x.status}</span>
                  <span className="tasks-muted">{formatRelative(x.startedAt)}</span>
                  {x.summary && <span className="tasks-exec__summary">{x.summary}</span>}
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="tasks-section">
          <h4 className="tasks-section__title">
            <FileCode2 size={13} /> Files touched
          </h4>
          {task.filesTouched.length === 0 ? (
            <p className="tasks-muted">No files recorded yet.</p>
          ) : (
            <ul className="tasks-files">
              {task.filesTouched.map((f) => (
                <li key={f} className="tasks-mono">
                  {f}
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="tasks-section">
          <h4 className="tasks-section__title">
            <History size={13} /> History
          </h4>
          {history.error ? (
            <p className="tasks-error">{history.error}</p>
          ) : history.entries === null ? (
            <div className="tasks-muted">
              <Spinner size={14} /> Loading history…
            </div>
          ) : (
            <ol className="tasks-timeline">
              {[...history.entries].reverse().map((h) => (
                <li key={h.seq} className="tasks-timeline__item" style={{ ["--status-color" as string]: statusColor(h.to) }}>
                  <div className="tasks-timeline__line">
                    {h.from ? (
                      <>
                        <span>{h.from}</span> → <strong>{h.to}</strong>
                      </>
                    ) : (
                      <strong>Created</strong>
                    )}
                    {h.forced && <span className="tasks-forced">system</span>}
                  </div>
                  <div className="tasks-timeline__meta">
                    {actorLabel(h)} · <time dateTime={h.at}>{formatRelative(h.at)}</time>
                    {h.reason && h.reason !== "created" && <> · {h.reason}</>}
                  </div>
                </li>
              ))}
            </ol>
          )}
        </section>
      </div>
    </aside>
  );
}
