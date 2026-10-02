import type { Task } from "@orkestr/protocol";
import { clsx } from "clsx";
import { ArrowDown, ArrowUp, GitFork } from "lucide-react";

import { useFormat, useT } from "../../i18n";
import { taskTag } from "../../lib/coreText";
import { type SortKey, shortId, sortTasks } from "./model";
import { PriorityChip, StatusBadge } from "./parts";
import { useTasksUi } from "./store";

const COLUMNS: { key: SortKey; label: "task" | "status" | "priority" | "updated"; className: string }[] = [
  { key: "title", label: "task", className: "tasks-col--title" },
  { key: "status", label: "status", className: "tasks-col--status" },
  { key: "priority", label: "priority", className: "tasks-col--priority" },
  { key: "updatedAt", label: "updated", className: "tasks-col--updated" },
];

export function ListView({ tasks, readyIds }: { tasks: Task[]; readyIds: ReadonlySet<string> }) {
  const { t } = useT("tasks");
  const fmt = useFormat();
  const sortKey = useTasksUi((s) => s.sortKey);
  const sortDir = useTasksUi((s) => s.sortDir);
  const setSort = useTasksUi((s) => s.setSort);
  const selected = useTasksUi((s) => s.selectedTaskId);
  const select = useTasksUi((s) => s.select);
  const sorted = sortTasks(tasks, sortKey, sortDir);

  return (
    <div className="tasks-list" role="region" aria-label={t("list.label")}>
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
                    {t(`list.${c.label}`)}
                    {active && (sortDir === "asc" ? <ArrowUp size={12} /> : <ArrowDown size={12} />)}
                  </button>
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {sorted.map((task) => (
            <tr
              key={task.id}
              className={clsx("tasks-row", selected === task.id && "tasks-row--selected")}
              tabIndex={0}
              aria-selected={selected === task.id}
              onClick={() => select(task.id)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  select(task.id);
                }
              }}
            >
              <td className="tasks-col--title">
                <div className="tasks-row__title">
                  <span className="tasks-row__name">{task.title}</span>
                  {readyIds.has(task.id) && task.status !== "Ready" && <span className="tasks-row__hint">{t("list.depsMet")}</span>}
                </div>
                <div className="tasks-row__meta">
                  <span className="tasks-mono">#{shortId(task.id)}</span>
                  {task.dependencies.length > 0 && (
                    <span title={t("list.dependencies", { count: task.dependencies.length })}>
                      <GitFork size={11} /> {task.dependencies.length}
                    </span>
                  )}
                  {task.tags.slice(0, 3).map((tag) => (
                    <span key={tag} className="tasks-tag">
                      {taskTag(tag)}
                    </span>
                  ))}
                </div>
              </td>
              <td className="tasks-col--status">
                <StatusBadge status={task.status} />
              </td>
              <td className="tasks-col--priority">
                <PriorityChip priority={task.priority} />
              </td>
              <td className="tasks-col--updated" title={fmt.date(task.updatedAt)}>
                {fmt.relative(task.updatedAt)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
