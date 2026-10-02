import type { Task } from "@orkestr/protocol";
import { clsx } from "clsx";
import { ArrowDown, ArrowUp, GitFork } from "lucide-react";

import { type SortKey, formatRelative, shortId, sortTasks } from "./model";
import { PriorityChip, StatusBadge } from "./parts";
import { useTasksUi } from "./store";

const COLUMNS: { key: SortKey; label: string; className: string }[] = [
  { key: "title", label: "Task", className: "tasks-col--title" },
  { key: "status", label: "Status", className: "tasks-col--status" },
  { key: "priority", label: "Priority", className: "tasks-col--priority" },
  { key: "updatedAt", label: "Updated", className: "tasks-col--updated" },
];

export function ListView({ tasks, readyIds }: { tasks: Task[]; readyIds: ReadonlySet<string> }) {
  const sortKey = useTasksUi((s) => s.sortKey);
  const sortDir = useTasksUi((s) => s.sortDir);
  const setSort = useTasksUi((s) => s.setSort);
  const selected = useTasksUi((s) => s.selectedTaskId);
  const select = useTasksUi((s) => s.select);
  const sorted = sortTasks(tasks, sortKey, sortDir);

  return (
    <div className="tasks-list" role="region" aria-label="Task list">
      <table className="tasks-table">
        <thead>
          <tr>
            {COLUMNS.map((c) => {
              const active = c.key === sortKey;
              return (
                <th
                  key={c.key}
                  className={c.className}
                  aria-sort={active ? (sortDir === "asc" ? "ascending" : "descending") : "none"}
                  scope="col"
                >
                  <button type="button" className="tasks-th" onClick={() => setSort(c.key)}>
                    {c.label}
                    {active && (sortDir === "asc" ? <ArrowUp size={12} /> : <ArrowDown size={12} />)}
                  </button>
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {sorted.map((t) => (
            <tr
              key={t.id}
              className={clsx("tasks-row", selected === t.id && "tasks-row--selected")}
              tabIndex={0}
              aria-selected={selected === t.id}
              onClick={() => select(t.id)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  select(t.id);
                }
              }}
            >
              <td className="tasks-col--title">
                <div className="tasks-row__title">
                  <span className="tasks-row__name">{t.title}</span>
                  {readyIds.has(t.id) && t.status !== "Ready" && <span className="tasks-row__hint">deps met</span>}
                </div>
                <div className="tasks-row__meta">
                  <span className="tasks-mono">#{shortId(t.id)}</span>
                  {t.dependencies.length > 0 && (
                    <span title={`${t.dependencies.length} dependencies`}>
                      <GitFork size={11} /> {t.dependencies.length}
                    </span>
                  )}
                  {t.tags.slice(0, 3).map((tag) => (
                    <span key={tag} className="tasks-tag">
                      {tag}
                    </span>
                  ))}
                </div>
              </td>
              <td className="tasks-col--status">
                <StatusBadge status={t.status} />
              </td>
              <td className="tasks-col--priority">
                <PriorityChip priority={t.priority} />
              </td>
              <td className="tasks-col--updated" title={new Date(t.updatedAt).toLocaleString()}>
                {formatRelative(t.updatedAt)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
